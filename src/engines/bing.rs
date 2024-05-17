//! The `bing` module handles the scraping of results from the bing search engine
//! by querying the upstream bing search engine with user provided query and with a page
//! number if provided.

use std::collections::HashMap;

use regex::Regex;
use reqwest::header::HeaderMap;
use reqwest::Client;
use scraper::Html;

use crate::models::aggregation_models::SearchResult;

use crate::models::engine_models::{EngineError, SearchEngine};

use error_stack::{Report, Result, ResultExt};

use super::search_result_parser::SearchResultParser;

/// Base URL for the upstream search engine
const BASE_URL: &str = "https://www.bing.com";

/// A new Bing engine type defined in-order to implement the `SearchEngine` trait which allows to
/// reduce code duplication as well as allows to create vector of different search engines easily.
pub struct Bing {
    /// The parser, used to interpret the search result.
    parser: SearchResultParser,
}

impl Bing {
    /// Creates the Bing parser.
    pub fn new() -> Result<Self, EngineError> {
        Ok(Self {
            parser: SearchResultParser::new(
                "#b_results",
                "li.b_algo",
                "h2 > a",
                "div > a",
                "div > p",
            )?,
        })
    }
}

#[async_trait::async_trait]
impl SearchEngine for Bing {
    async fn results(
        &self,
        query: &str,
        page: u32,
        user_agent: &str,
        client: &Client,
        _safe_search: u8,
        accept_language: &str,
    ) -> Result<Vec<(String, SearchResult)>, EngineError> {
        // Bing uses `start results from this number` convention
        // So, for 10 results per page, page 0 starts at 1, page 1
        // starts at 11, and so on.
        let results_per_page = 10;
        let start_result = results_per_page * page + 1;

        let url: String = match page {
            0 => {
                format!("{BASE_URL}/search?q={query}&pq={query}")
            }
            1 => {
                format!("{BASE_URL}/search?q={query}&pq={query}&first={start_result}&FORM=PERE")
            }
            _ => {
                format!(
                    "{BASE_URL}/search?q={query}&pq={query}&first={start_result}&FORM=PERE{}",
                    page - 1
                )
            }
        };

        let query_params: Vec<(&str, &str)> = vec![
            ("_C_ETH", "1"),
            ("_EDGE_V", "1"),
            ("_Rwho", "u=d"),
            ("bngps=s", "0"),
            ("_UR", "QS=4"),
            ("ANIMIA", "FRE=1"),
            ("BCP", "AD=0&AL=0&SM=0"),
            ("bngps", "s=0"),
            ("SRCHD", "AF=NOFORM"),
        ];

        let mut cookie_string = String::new();
        for (k, v) in &query_params {
            cookie_string.push_str(&format!("{k}={v}; "));
        }

        let header_map = HeaderMap::try_from(&HashMap::from([
            ("User-Agent".to_string(), user_agent.to_string()),
            ("Accept-Language".to_string(), accept_language.to_string()),
            ("Referer".to_string(), format!("{}/", BASE_URL)),
            ("Origin".to_string(), BASE_URL.to_string()),
            ("Cookie".to_string(), cookie_string),
        ]))
        .change_context(EngineError::UnexpectedError)?;

        let document: Html = Html::parse_document(
            &Bing::fetch_html_from_upstream(self, &url, header_map, client).await?,
        );

        // Bing is very aggressive in finding matches
        // even with the most absurd of queries. ".b_algo" is the
        // class for the list item of results
        if let Some(no_result_msg) = self.parser.parse_for_no_results(&document).nth(0) {
            if no_result_msg
                .value()
                .attr("class")
                .map(|classes| classes.contains("b_algo"))
                .unwrap_or(false)
            {
                return Err(Report::new(EngineError::EmptyResultSet));
            }
        }

        let re_span = Regex::new(r#"<span.*?>.*?(?:</span>&nbsp;·|</span>)"#).unwrap();

        // scrape all the results from the html
        self.parser
            .parse_for_results(&document, |title, url, desc| {
                url.value().attr("href").map(|url| {
                    let obfuscated_url = url.starts_with("https://www.bing.com/ck/a?");
                    let url_decoded = if obfuscated_url {
                        decode_url(url)
                    } else {
                        url.to_string()
                    };
                    SearchResult::new(
                        title.inner_html().trim(),
                        url_decoded.as_str(),
                        &re_span.replace_all(desc.inner_html().trim(), ""),
                        &["bing"],
                    )
                })
            })
    }
}
/// Converts an obfuscated URL to a regilat one
fn decode_url(url: &str) -> String {
    use base64::Engine;
    let re = match Regex::new(r"&u=a1([^&]+)") {
        Ok(result) => result,
        Err(_) => {
            return url.to_string();
        }
    };
    if let Some(substr) = re.captures(url) {
        if let Some(matched) = substr.get(1) {
            let url_base64 = matched.as_str().to_string();
            let bytes = match base64::engine::general_purpose::STANDARD_NO_PAD.decode(url_base64) {
                Ok(b) => b,
                Err(_) => {
                    return url.to_string();
                }
            };

            return if let Ok(str) = String::from_utf8(bytes) {
                str
            } else {
                return url.to_string();
            };
        }
    }
    url.to_string()
}
