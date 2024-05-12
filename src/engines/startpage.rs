//! The `startpage` module handles the scraping of results from the startpage search engine
//! by querying the upstream startpage search engine with user provided query and with a page
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
const BASE_URL: &str = "https://www.startpage.com";

/// A new Startpage engine type defined in-order to implement the `SearchEngine` trait which allows to
/// reduce code duplication as well as allows to create vector of different search engines easily.
pub struct Startpage {
    /// The parser, used to interpret the search result.
    parser: SearchResultParser,
}

impl Startpage {
    /// Creates the Startpage parser.
    pub fn new() -> Result<Self, EngineError> {
        Ok(Self {
            parser: SearchResultParser::new(
                ".no-results",
                ".w-gl>.result",
                ".result-title>h2",
                ".result-title",
                ".description",
            )?,
        })
    }
}

#[async_trait::async_trait]
impl SearchEngine for Startpage {
    async fn results(
        &self,
        query: &str,
        page: u32,
        user_agent: &str,
        client: &Client,
        safe_search: u8,
        accept_language: &str,
    ) -> Result<Vec<(String, SearchResult)>, EngineError> {
        // Page number can be missing or empty string and so appropriate handling is required
        // so that upstream server recieves valid page number.
        let url: String = match page {
            0 => format!("{BASE_URL}/sp/search?query={query}&abp=1&t=device&lui=english&cat=web"),
            _ => format!("{BASE_URL}/sp/search?lui=english&language=english&query={query}&cat=web&t=device&segment=startpage.udog&page={}", page+1),
        };

        let safe_search_level = match safe_search {
            0 => "1",
            _ => "0",
        };

        // Constructing the Cookie.
        let settings: Vec<(&str, &str)> = vec![
            ("date_time", "world"),
            ("disable_family_filter", safe_search_level),
            ("disable_open_in_new_window", "1"),
            ("enable_post_method", "0"),
            ("enable_proxy_safety_suggest", "0"),
            ("enable_stay_control", "0"),
            ("instant_answers", "0"),
            ("lang_homepage", "s%2Fdevice%2Fen"),
            ("language", "english"),
            ("language_ui", "english"),
            ("num_of_results", "20"),
            ("search_results_region", "all"),
            ("suggestions", "0"),
            ("wt_unit", "celsius"),
        ];

        let joined_pairs: Vec<String> = settings
            .iter()
            .map(|&(key, value)| format!("{}EEE{}", key, value))
            .collect();

        let cookie = format!("preferences={}", joined_pairs.join("N1N"));

        // initializing HeaderMap and adding appropriate headers.
        let header_map = HeaderMap::try_from(&HashMap::from([
            ("User-Agent".to_string(), user_agent.to_string()),
            ("Accept-Language".to_string(), accept_language.to_string()),
            ("Referer".to_string(), format!("{}/", BASE_URL)),
            ("Origin".to_string(), BASE_URL.to_string()),
            ("Sec-GPC".to_string(), "1".to_string()),
            ("Cookie".to_string(), cookie),
        ]))
        .change_context(EngineError::UnexpectedError)?;

        let document: Html = Html::parse_document(
            &Startpage::fetch_html_from_upstream(self, &url, header_map, client).await?,
        );

        if self.parser.parse_for_no_results(&document).next().is_some() {
            println!("parse_for_no_results");
            return Err(Report::new(EngineError::EmptyResultSet));
        }

        // scrape all the results from the html
        self.parser
            .parse_for_results(&document, |title, url, desc| {
                url.value().attr("href").map(|url| {
                    SearchResult::new(
                        title.inner_html().trim(),
                        url,
                        desc.inner_html().trim(),
                        &["startpage"],
                    )
                })
            })
    }
}
