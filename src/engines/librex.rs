//! The `librex` module contains the implementation of a search engine for LibreX using the reqwest and scraper libraries.
//! It includes a `SearchEngine` trait implementation for interacting with the search engine and retrieving search results.

use std::collections::HashMap;

use reqwest::header::HeaderMap;
use reqwest::Client;
use scraper::Html;

use crate::models::aggregation_models::SearchResult;
use crate::models::engine_models::{EngineError, SearchEngine};

use error_stack::{Report, Result, ResultExt};

use super::search_result_parser::SearchResultParser;

/// Base URL for the upstream search engine
const BASE_URL: &str = "https://search.ahwx.org";

/// Represents the LibreX search engine.
pub struct LibreX {
    /// The parser used to extract search results from HTML documents.
    parser: SearchResultParser,
}

impl LibreX {
    /// Creates a new instance of LibreX with a default configuration.
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing `LibreX` if successful, otherwise an `EngineError`.
    pub fn new() -> Result<Self, EngineError> {
        Ok(Self {
            parser: SearchResultParser::new(
                ".text-result-container>p",
                ".text-result-container>.text-result-wrapper",
                "a>h2",
                "a",
                "span",
            )?,
        })
    }
}

#[async_trait::async_trait]
impl SearchEngine for LibreX {
    /// Retrieves search results from LibreX based on the provided query, page, user agent, and client.
    ///
    /// # Arguments
    ///
    /// * `query` - The search query.
    /// * `page` - The page number for pagination.
    /// * `user_agent` - The user agent string.
    /// * `client` - The reqwest client for making HTTP requests.
    /// * `_safe_search` - A parameter for safe search (not currently used).
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing a `HashMap` of search results if successful, otherwise an `EngineError`.
    /// The `Err` variant is explicit for better documentation.
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
        let url: String = format!("{BASE_URL}/search.php?q={query}&p={}&t=10", page * 10);

        let safe_search_level = match safe_search {
            0 => "off",
            _ => "on",
        };

        // Constructing the Cookie.
        let settings: Vec<(&str, &str)> = vec![
            ("theme", "amoled"),
            ("disable_special", "on"),
            ("disable_frontends", "on"),
            ("language", "en"),
            ("number_of_results", "20"),
            ("safe_search", safe_search_level),
            ("save", "1"),
        ];

        let joined_pairs: Vec<String> = settings
            .iter()
            .map(|&(key, value)| format!("{}={}", key, value))
            .collect();

        let cookie = format!("preferences={}", joined_pairs.join(", "));

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
            ("Cookie".to_string(), cookie),
        ]))
        .change_context(EngineError::UnexpectedError)?;

        let document: Html = Html::parse_document(
            &LibreX::fetch_html_from_upstream(self, &url, header_map, client).await?,
        );

        if self.parser.parse_for_no_results(&document).next().is_some() {
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
                        &["librex"],
                    )
                })
            })
    }
}
