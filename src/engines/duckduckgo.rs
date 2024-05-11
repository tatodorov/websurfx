//! The `duckduckgo` module handles the scraping of results from the duckduckgo search engine
//! by querying the upstream duckduckgo search engine with user provided query and with a page
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
const BASE_URL: &str = "https://html.duckduckgo.com";

/// A new DuckDuckGo engine type defined in-order to implement the `SearchEngine` trait which allows to
/// reduce code duplication as well as allows to create vector of different search engines easily.
pub struct DuckDuckGo {
    /// The parser, used to interpret the search result.
    parser: SearchResultParser,
}

impl DuckDuckGo {
    /// Creates the DuckDuckGo parser.
    pub fn new() -> Result<Self, EngineError> {
        Ok(Self {
            parser: SearchResultParser::new(
                ".no-results",
                ".results>.result",
                ".result__title>.result__a",
                ".result__url",
                ".result__snippet",
            )?,
        })
    }
}

#[async_trait::async_trait]
impl SearchEngine for DuckDuckGo {
    async fn results(
        &self,
        query: &str,
        page: u32,
        user_agent: &str,
        client: &Client,
        _safe_search: u8,
        accept_language: &str,
    ) -> Result<Vec<(String, SearchResult)>, EngineError> {
        // Page number can be missing or empty string and so appropriate handling is required
        // so that upstream server recieves valid page number.
        let url: String = match page {
            0 => {
                format!("{BASE_URL}/html/?q={query}&s=&dc=&v=1&o=json&api=/d.js")
            }
            _ => {
                format!(
                    "{BASE_URL}/html/?q={query}&s={}&dc={}&v=1&o=json&api=/d.js",
                    page * 30,
                    page * 30 + 1
                )
            }
        };

        // initializing HeaderMap and adding appropriate headers.
        let header_map = HeaderMap::try_from(&HashMap::from([
            ("User-Agent".to_string(), user_agent.to_string()),
            ("Accept-Language".to_string(), accept_language.to_string()),
            ("Referer".to_string(), format!("{}/", BASE_URL)),
            ("Origin".to_string(), BASE_URL.to_string()),
            (
                "Content-Type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ),
            ("Sec-GPC".to_string(), "1".to_string()),
        ]))
        .change_context(EngineError::UnexpectedError)?;

        let document: Html = Html::parse_document(
            &DuckDuckGo::fetch_html_from_upstream(self, &url, header_map, client).await?,
        );

        if self.parser.parse_for_no_results(&document).next().is_some() {
            return Err(Report::new(EngineError::EmptyResultSet));
        }

        // scrape all the results from the html
        self.parser
            .parse_for_results(&document, |title, url, desc| {
                Some(SearchResult::new(
                    title.inner_html().trim(),
                    &format!("https://{}", url.inner_html().trim()),
                    desc.inner_html().trim(),
                    &["duckduckgo"],
                ))
            })
    }
}
