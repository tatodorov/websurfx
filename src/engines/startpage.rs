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
        let url: String = format!("{BASE_URL}/sp/search?q={query}&num=20&start={}", page * 20,);

        let safe_search_level = match safe_search {
            0 => "1",
            _ => "0",
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
            ("Cookie".to_string(), format!("preferences=date_timeEEEworldN1Ndisable_family_filterEEE{safe_search_level}N1Ndisable_open_in_new_windowEEE1N1Nenable_post_methodEEE0N1Nenable_proxy_safety_suggestEEE0N1Nenable_stay_controlEEE0N1Ninstant_answersEEE0N1Nlang_homepageEEEs%2Fdevice%2FenN1NlanguageEEEenglishN1Nlanguage_uiEEEenglishN1Nnum_of_resultsEEE20N1Nsearch_results_regionEEEallN1NsuggestionsEEE0N1Nwt_unitEEEcelsius")),
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
