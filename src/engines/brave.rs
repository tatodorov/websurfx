//! The `brave` module handles the scraping of results from the brave search engine
//! by querying the upstream brave search engine with user provided query and with a page
//! number if provided.

use std::collections::HashMap;

use reqwest::{header::HeaderMap, Client};
use scraper::Html;

use crate::models::aggregation_models::SearchResult;
use error_stack::{Report, Result, ResultExt};

use crate::models::engine_models::{EngineError, SearchEngine};

use super::search_result_parser::SearchResultParser;

/// Base URL for the upstream search engine
const BASE_URL: &str = "https://search.brave.com";

/// Scrapes the results from the Brave search engine.  
pub struct Brave {
    /// Utilises generic logic for parsing search results.
    parser: SearchResultParser,
}

impl Brave {
    /// Creates the Brave parser.
    pub fn new() -> Result<Brave, EngineError> {
        Ok(Self {
            parser: SearchResultParser::new(
                "#results h4",
                "#results [data-pos]",
                "a > .url",
                "a",
                ".snippet-description",
            )?,
        })
    }
}

#[async_trait::async_trait]
impl SearchEngine for Brave {
    async fn results(
        &self,
        query: &str,
        page: u32,
        user_agent: &str,
        client: &Client,
        safe_search: u8,
        accept_language: &str,
    ) -> Result<Vec<(String, SearchResult)>, EngineError> {
        let url = format!("{BASE_URL}/search?q={query}&offset={page}");

        let safe_search_level = match safe_search {
            0 => "off",
            1 => "moderate",
            _ => "strict",
        };

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
            (
                "Cookie".to_string(),
                format!("safe_search={safe_search_level}"),
            ),
        ]))
        .change_context(EngineError::UnexpectedError)?;

        let document: Html = Html::parse_document(
            &Brave::fetch_html_from_upstream(self, &url, header_map, client).await?,
        );

        if let Some(no_result_msg) = self.parser.parse_for_no_results(&document).nth(0) {
            if no_result_msg
                .inner_html()
                .contains("Not many great matches came back for your search")
            {
                return Err(Report::new(EngineError::EmptyResultSet));
            }
        }

        self.parser
            .parse_for_results(&document, |title, url, desc| {
                url.value().attr("href").map(|url| {
                    SearchResult::new(
                        title.text().collect::<Vec<_>>().join("").trim(),
                        url.trim(),
                        desc.inner_html().trim(),
                        &["brave"],
                    )
                })
            })
    }
}
