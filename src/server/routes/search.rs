//! This module handles the search route of the search engine website.

use crate::{
    cache::cacher::SharedCache,
    config::parser::Config,
    handler::{file_path, FileType},
    models::{
        aggregation_models::SearchResults,
        engine_models::EngineHandler,
        server_models::{self, SearchParams},
    },
    results::aggregator::aggregate,
};
use actix_web::{get, http::header::ContentType, web, HttpRequest, HttpResponse};
use regex::Regex;
use std::time::Instant;
use std::{borrow::Cow, collections::HashMap};
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, BufReader},
    join,
    time::Duration,
};

/// Handles the route of search page of the `websurfx` meta search engine website and it takes
/// two search url parameters `q` and `page` where `page` parameter is optional.
///
/// # Example
///
/// ```bash
/// curl "http://127.0.0.1:8080/search?q=sweden&page=1"
/// ```
///
/// Or
///
/// ```bash
/// curl "http://127.0.0.1:8080/search?q=sweden"
/// ```
#[get("/search")]
pub async fn search(
    req: HttpRequest,
    config: web::Data<&'static Config>,
    cache: web::Data<&'static SharedCache>,
) -> Result<HttpResponse, Box<dyn std::error::Error>> {
    let search_start_time = Instant::now();
    use std::sync::Arc;
    let params = web::Query::<SearchParams>::from_query(req.query_string())?;
    match &params.q {
        Some(query) => {
            if query.trim().is_empty() {
                return Ok(HttpResponse::TemporaryRedirect()
                    .insert_header(("location", "/"))
                    .finish());
            }

            let cookie = req.cookie("appCookie");

            // Get search settings using the user's cookie or from the server's config
            let mut search_settings: server_models::Cookie<'_> = cookie
                .and_then(|cookie_value| serde_json::from_str(cookie_value.value()).ok())
                .unwrap_or_else(|| {
                    server_models::Cookie::build(
                        &config.style,
                        config
                            .upstream_search_engines
                            .iter()
                            .filter_map(|(engine, enabled)| {
                                enabled.then_some(Cow::Borrowed(engine.as_str()))
                            })
                            .collect(),
                        config.safe_search,
                    )
                });

            search_settings.safe_search_level = get_safesearch_level(
                params.safesearch,
                search_settings.safe_search_level,
                config.safe_search,
            );

            // Closure wrapping the results function capturing local references
            let get_results = |page| results(&config, &cache, query, page, &search_settings);

            // .max(0) makes sure that the page >= 0.
            let page = params.page.unwrap_or(0).max(0);
            let previous_page = page.saturating_sub(1);
            let next_page = page + 1;

            let results: Arc<(SearchResults, String)>;
            let cached_results: HashMap<String, SearchResults>;
            if page != previous_page {
                let (previous_results, current_results) =
                    join!(get_results(previous_page), get_results(page),);
                let parsed_previous_results = previous_results?;
                results = Arc::new(current_results?);
                cached_results = [
                    (parsed_previous_results.1, parsed_previous_results.0),
                    (results.1.clone(), results.0.clone()),
                ]
                .iter()
                .cloned()
                .collect();
            } else {
                let current_results = get_results(page).await;
                results = Arc::new(current_results?);
                cached_results = [(results.1.clone(), results.0.clone())]
                    .iter()
                    .cloned()
                    .collect();
            }

            let config_clone = config.clone();
            let query_clone = query.clone();
            tokio::spawn(async move {
                _ = background_page_fetch(
                    &config_clone,
                    &cache,
                    &query_clone,
                    next_page,
                    &search_settings,
                    cached_results,
                )
                .await;
            });

            let search_duration = search_start_time.elapsed().as_secs_f32();

            Ok(HttpResponse::Ok().content_type(ContentType::html()).body(
                crate::templates::views::search::search(
                    &config.style.colorscheme,
                    &config.style.theme,
                    &config.style.animation,
                    query,
                    &results.0,
                    search_duration,
                )
                .0,
            ))
        }
        None => Ok(HttpResponse::TemporaryRedirect()
            .insert_header(("location", "/"))
            .finish()),
    }
}

/// Fetches the results for a query and page. It First checks the redis cache, if that
/// fails it gets proper results by requesting from the upstream search engines.
///
/// # Arguments
///
/// * `url` - It takes the url of the current page that requested the search results for a
/// particular search query.
/// * `config` - It takes a parsed config struct.
/// * `query` - It takes the page number as u32 value.
/// * `req` - It takes the `HttpRequest` struct as a value.
///
/// # Error
///
/// It returns the `SearchResults` struct if the search results could be successfully fetched from
/// the cache or from the upstream search engines otherwise it returns an appropriate error.
async fn results(
    config: &'static Config,
    cache: &'static SharedCache,
    query: &str,
    page: u32,
    search_settings: &server_models::Cookie<'_>,
) -> Result<(SearchResults, String), Box<dyn std::error::Error>> {
    // eagerly parse cookie value to evaluate safe search level
    let safe_search_level = search_settings.safe_search_level;

    let cache_key = format!(
        "http://{}:{}/search?q={}&page={}&safesearch={}&engines={}",
        config.binding_ip,
        config.port,
        query,
        page,
        safe_search_level,
        search_settings.engines.join(",")
    );

    // fetch the cached results json.
    let cached_results = cache.cached_results(&cache_key).await;
    // check if fetched cache results was indeed fetched or it was an error and if so
    // handle the data accordingly.
    match cached_results {
        Ok(results) => Ok((results, cache_key)),
        Err(_) => {
            if safe_search_level == 4 {
                let mut results: SearchResults = SearchResults::default();

                let flag: bool =
                    !is_match_from_filter_list(file_path(FileType::BlockList)?, query).await?;
                // Return early when query contains disallowed words,
                if flag {
                    results.set_disallowed();
                    cache
                        .cache_results(&[results.clone()], &[cache_key.clone()])
                        .await?;
                    results.set_safe_search_level(safe_search_level);
                    return Ok((results, cache_key));
                }
            }

            // check if the cookie value is empty or not if it is empty then use the
            // default selected upstream search engines from the config file otherwise
            // parse the non-empty cookie and grab the user selected engines from the
            // UI and use that.
            let mut results: SearchResults = match search_settings.engines.is_empty() {
                false => {
                    aggregate(
                        query,
                        page,
                        config,
                        &search_settings
                            .engines
                            .iter()
                            .filter_map(|engine| EngineHandler::new(engine).ok())
                            .collect::<Vec<EngineHandler>>(),
                        safe_search_level,
                    )
                    .await?
                }
                true => {
                    let mut search_results = SearchResults::default();
                    search_results.set_no_engines_selected();
                    search_results
                }
            };
            let (engine_errors_info, results_empty_check, no_engines_selected) = (
                results.engine_errors_info().is_empty(),
                results.results().is_empty(),
                results.no_engines_selected(),
            );
            results.set_filtered(engine_errors_info & results_empty_check & !no_engines_selected);
            cache
                .cache_results(&[results.clone()], &[cache_key.clone()])
                .await?;
            results.set_safe_search_level(safe_search_level);
            Ok((results, cache_key))
        }
    }
}

/// Asynchronously fetches search results for a specific page and updates the search cache.
///
/// This function is intended to be used in a background task to fetch search results for a given
/// page asynchronously and update the search cache with the results.
///
/// # Parameters
///
/// - `config`: A reference to the application configuration (`Config`) that specifies settings
///   and parameters for the search.
/// - `cache`: A reference to the shared cache (`SharedCache`) where search results will be stored.
/// - `query`: The search query string.
/// - `page`: The page number of the search results to fetch.
/// - `search_settings`: A reference to server-side settings (`Cookie`) used for the search.
/// - `cache_keys`: A mutable vector (`Vec<String>`) containing cache keys associated with the
///   search results.
/// - `results_list`: A mutable vector (`Vec<SearchResults>`) containing the list of search results
///   to be updated and stored in the cache.
///
async fn background_page_fetch(
    config: &'static Config,
    cache: &'static SharedCache,
    query: &str,
    page: u32,
    search_settings: &server_models::Cookie<'_>,
    cached_results: HashMap<String, SearchResults>,
) {
    if !config.aggregator.random_delay && page == 1 {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let results = results(config, cache, query, page, search_settings)
        .await
        .unwrap();
    let mut cache_keys: Vec<String> = cached_results.keys().cloned().collect();
    let mut results_list: Vec<SearchResults> = cached_results.values().cloned().collect();
    cache_keys.push(results.1);
    results_list.push(results.0);

    _ = cache.cache_results(&results_list, &cache_keys).await;
}

/// A helper function which checks whether the search query contains any keywords which should be
/// disallowed/allowed based on the regex based rules present in the blocklist and allowlist files.
///
/// # Arguments
///
/// * `file_path` - It takes the file path of the list as the argument.
/// * `query` - It takes the search query to be checked against the list as an argument.
///
/// # Error
///
/// Returns a bool indicating whether the results were found in the list or not on success
/// otherwise returns a standard error type on a failure.
async fn is_match_from_filter_list(
    file_path: &str,
    query: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    let reader = BufReader::new(File::open(file_path).await?);
    let mut lines = reader.lines();
    while let Some(line) = lines.next_line().await? {
        let re = Regex::new(&line)?;
        if re.is_match(query) {
            return Ok(true);
        }
    }

    Ok(false)
}

/// A helper function to choose the safe search level value based on the URL parameters,
/// cookie value and config value.
///
/// # Argurments
///
/// * `safe_search_level_from_url` - Safe search level from the URL parameters.
/// * `cookie_safe_search_level` - Safe search level value from the cookie.
/// * `config_safe_search_level` - Safe search level value from the config file.
///
/// # Returns
///
/// Returns an appropriate safe search level value based on the safe search level values
/// from the URL parameters, cookie and the config file.
fn get_safesearch_level(
    safe_search_level_from_url: Option<u8>,
    cookie_safe_search_level: u8,
    config_safe_search_level: u8,
) -> u8 {
    (u8::from(safe_search_level_from_url.is_some())
        * ((u8::from(config_safe_search_level >= 3) * config_safe_search_level)
            + (u8::from(config_safe_search_level < 3) * safe_search_level_from_url.unwrap_or(0))))
        + (u8::from(safe_search_level_from_url.is_none())
            * ((u8::from(config_safe_search_level >= 3) * config_safe_search_level)
                + (u8::from(config_safe_search_level < 3) * cookie_safe_search_level)))
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A helper function which creates a random mock safe search level value.
    ///
    /// # Returns
    ///
    /// Returns an optional u8 value.
    fn mock_safe_search_level_value() -> Option<u8> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as f32;
        let delay = ((nanos / 1_0000_0000 as f32).floor() as i8) - 1;

        match delay {
            -1 => None,
            some_num => Some(if some_num > 4 { some_num - 4 } else { some_num } as u8),
        }
    }

    #[test]
    /// A test function to test whether the output of the branchless and branched code
    /// for the code to choose the appropriate safe search level is same or not.
    fn get_safesearch_level_branched_branchless_code_test() {
        // Get mock values for the safe search level values for URL parameters, cookie
        // and config.
        let safe_search_level_from_url = mock_safe_search_level_value();
        let cookie_safe_search_level = mock_safe_search_level_value().unwrap_or(0);
        let config_safe_search_level = mock_safe_search_level_value().unwrap_or(0);

        // Branched code
        let safe_search_level_value_from_branched_code = match safe_search_level_from_url {
            Some(safe_search_level_from_url_parsed) => {
                if config_safe_search_level >= 3 {
                    config_safe_search_level
                } else {
                    safe_search_level_from_url_parsed
                }
            }
            None => {
                if config_safe_search_level >= 3 {
                    config_safe_search_level
                } else {
                    cookie_safe_search_level
                }
            }
        };

        // branchless code
        let safe_search_level_value_from_branchless_code =
            (u8::from(safe_search_level_from_url.is_some())
                * ((u8::from(config_safe_search_level >= 3) * config_safe_search_level)
                    + (u8::from(config_safe_search_level < 3)
                        * safe_search_level_from_url.unwrap_or(0))))
                + (u8::from(safe_search_level_from_url.is_none())
                    * ((u8::from(config_safe_search_level >= 3) * config_safe_search_level)
                        + (u8::from(config_safe_search_level < 3) * cookie_safe_search_level)));

        assert_eq!(
            safe_search_level_value_from_branched_code,
            safe_search_level_value_from_branchless_code
        );
    }
}
