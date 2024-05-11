//! The `mojeek` module handles the scraping of results from the mojeek search engine
//! by querying the upstream mojeek search engine with user provided query and with a page
//! number if provided.

use std::collections::HashMap;

use reqwest::header::HeaderMap;
use reqwest::Client;
use scraper::Html;

use crate::models::aggregation_models::SearchResult;

use crate::models::engine_models::{EngineError, SearchEngine};

use error_stack::{Report, Result, ResultExt};

use super::search_result_parser::SearchResultParser;

/// Base URL for the upstream search engine
const BASE_URL: &str = "https://www.mojeek.com";

/// A new Mojeek engine type defined in-order to implement the `SearchEngine` trait which allows to
/// reduce code duplication as well as allows to create vector of different search engines easily.
pub struct Mojeek {
    /// The parser, used to interpret the search result.
    parser: SearchResultParser,
}

impl Mojeek {
    /// Creates the Mojeek parser.
    pub fn new() -> Result<Self, EngineError> {
        Ok(Self {
            parser: SearchResultParser::new(
                ".result-col",
                ".results-standard li",
                "a span.url",
                "h2 a.title",
                "p.s",
            )?,
        })
    }
}

#[async_trait::async_trait]
impl SearchEngine for Mojeek {
    async fn results(
        &self,
        query: &str,
        page: u32,
        user_agent: &str,
        client: &Client,
        safe_search: u8,
        accept_language: &str,
    ) -> Result<Vec<(String, SearchResult)>, EngineError> {
        // Mojeek uses `start results from this number` convention
        // So, for 10 results per page, page 0 starts at 1, page 1
        // starts at 11, and so on.
        let results_per_page = 10;
        let start_result = results_per_page * page + 1;

        let results_per_page = results_per_page.to_string();
        let start_result = start_result.to_string();

        let search_engines = vec![
            "Bing",
            "Brave",
            "DuckDuckGo",
            "Ecosia",
            "Google",
            "Lilo",
            "Metager",
            "Qwant",
            "Startpage",
            "Swisscows",
            "Yandex",
            "Yep",
            "You",
        ];

        let qss = search_engines.join("%2C");

        // A branchless condition to check whether the `safe_search` parameter has the
        // value 0 or not. If it is zero then it sets the value 0 otherwise it sets
        // the value to 1 for all other values of `safe_search`
        //
        // Moreover, the below branchless code is equivalent to the following code below:
        //
        // ```rust
        // let safe = if safe_search == 0 { 0 } else { 1 }.to_string();
        // ```
        //
        // For more information on branchless programming. See:
        //
        // * https://piped.video/watch?v=bVJ-mWWL7cE
        let safe = u8::from(safe_search != 0).to_string();

        // Mojeek detects automated requests, these are preferences that are
        // able to circumvent the countermeasure. Some of these are
        // not documented in their Search API
        let query_params: Vec<(&str, &str)> = vec![
            ("t", results_per_page.as_str()),
            ("theme", "dark"),
            ("arc", "none"),
            ("date", "1"),
            ("cdate", "1"),
            ("tlen", "100"),
            ("ref", "1"),
            ("hp", "minimal"),
            ("lb", "en"),
            ("qss", &qss),
            ("safe", &safe),
        ];

        let mut query_params_string = String::new();
        for (k, v) in &query_params {
            query_params_string.push_str(&format!("&{k}={v}"));
        }

        let url: String = match page {
            0 => {
                format!("{BASE_URL}/search?q={query}{query_params_string}")
            }
            _ => {
                format!("{BASE_URL}/search?q={query}&s={start_result}{query_params_string}")
            }
        };

        let header_map = HeaderMap::try_from(&HashMap::from([
            ("User-Agent".to_string(), user_agent.to_string()),
            ("Accept-Language".to_string(), accept_language.to_string()),
            ("Referer".to_string(), format!("{}/", BASE_URL)),
            (
                "Content-Type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ),
            ("Sec-GPC".to_string(), "1".to_string()),
        ]))
        .change_context(EngineError::UnexpectedError)?;

        let document: Html = Html::parse_document(
            &Mojeek::fetch_html_from_upstream(self, &url, header_map, client).await?,
        );

        if let Some(no_result_msg) = self.parser.parse_for_no_results(&document).nth(0) {
            if no_result_msg
                .inner_html()
                .contains("No pages found matching:")
            {
                return Err(Report::new(EngineError::EmptyResultSet));
            }
        }

        // scrape all the results from the html
        self.parser
            .parse_for_results(&document, |title, url, desc| {
                Some(SearchResult::new(
                    title.inner_html().trim(),
                    url.inner_html().trim(),
                    desc.inner_html().trim(),
                    &["mojeek"],
                ))
            })
    }
}
