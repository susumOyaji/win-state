use futures::future::join_all;
use regex::Regex;
use scraper::{Html, Selector};
use serde::{Serialize};
use serde_json::{Map, Value};
use worker::*;

// Set up a panic hook to log errors to the console
fn set_panic_hook() {
    console_error_panic_hook::set_once();
}

/// Defines a known location for financial data within the __PRELOADED_STATE__ JSON.
struct DataSource {
    path: &'static [&'static str],
    mappings: std::collections::HashMap<&'static str, &'static str>,
    strip_suffix: bool,
}

/// Represents the final JSON response for a single code.
#[derive(Serialize, Debug)]
struct CodeResult {
    code: String,
    data: Option<Map<String, Value>>,
    error: Option<String>,
}

/// Main worker entry point.
#[event(fetch)]
pub async fn main(req: Request, _env: Env, _ctx: Context) -> Result<Response> {
    set_panic_hook();

    let url = req.url()?;
    let query_params: std::collections::HashMap<String, String> = url.query_pairs().into_owned().collect();

    let codes_str = match query_params.get("code") {
        Some(c) => c,
        None => return Response::error("Query parameter 'code' is required. e.g., ?code=7203.T,^DJI", 400),
    };

    let codes: Vec<String> = codes_str
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.trim().to_string())
        .collect();

    if codes.is_empty() {
        return Response::error("Query parameter 'code' cannot be empty.", 400);
    }

    let keys: Option<Vec<String>> = query_params
        .get("keys")
        .map(|s| s.split(',').map(|k| k.trim().to_string()).collect());

    let futures = codes
        .iter()
        .map(|code| fetch_single_code(code.clone(), keys.clone()));
    let results = join_all(futures).await;

    Response::from_json(&results)
}

/// Fetches and processes data for a single stock code.
async fn fetch_single_code(code: String, keys: Option<Vec<String>>) -> CodeResult {
    let url = if code.starts_with('^') || code.contains('=') || code.ends_with(".T") || code.ends_with(".O") {
        format!("https://finance.yahoo.co.jp/quote/{}/", code)
    } else {
        format!("https://finance.yahoo.co.jp/quote/{}.T/", code)
    };

    let body = match Fetch::Url(url.parse().unwrap()).send().await {
        Ok(mut resp) => match resp.text().await {
            Ok(text) => text,
            Err(e) => return CodeResult { code, data: None, error: Some(format!("Failed to read response text: {}", e)) },
        },
        Err(e) => return CodeResult { code, data: None, error: Some(format!("Failed to fetch URL: {}", e)) },
    };

    let re = Regex::new(r"(?s)window\.__PRELOADED_STATE__\s*=\s*(.*?)</script>").unwrap();

    let result_data: Result<Map<String, Value>> = if let Some(caps) = re.captures(&body) {
        if let Some(json_match) = caps.get(1) {
            let mut json_str = json_match.as_str().trim();
            if json_str.ends_with(';') {
                json_str = &json_str[..json_str.len() - 1];
            }

            match serde_json::from_str(json_str) {
                Ok(data) => process_json_data(&code, &data, keys.as_ref()),
                Err(e) => Err(worker::Error::from(format!("Failed to parse JSON: {}", e))),
            }
        } else {
            // JSON not found, fallback to DOM
            process_dom_data(&code, &body, keys.as_ref())
        }
    } else {
        // __PRELOADED_STATE__ script not found, fallback to DOM
        process_dom_data(&code, &body, keys.as_ref())
    };

    match result_data {
        Ok(data) => CodeResult { code, data: Some(data), error: None },
        Err(e) => CodeResult { code, data: None, error: Some(e.to_string()) },
    }
}

/// Processes the __PRELOADED_STATE__ JSON data to find financial info.
fn process_json_data(code: &str, data: &Value, keys: Option<&Vec<String>>) -> Result<Map<String, Value>> {
    let data_sources = get_data_sources();

    // 1. Try predefined paths
    for source in &data_sources {
        if let Some(target_obj) = find_object(data, source.path) {
            if let Some(json_code_key) = source.mappings.get("code") {
                 if let Some(found_code) = get_string_value(target_obj, json_code_key) {
                    let code_to_compare = if source.strip_suffix {
                        code.split('.').next().unwrap_or(code)
                    } else {
                        code
                    };

                    if found_code.trim() == code_to_compare {
                        let mut results = Map::new();
                        if let Some(keys_vec) = keys {
                            for key in keys_vec {
                                if let Some(json_key) = source.mappings.get(key.as_str()) {
                                    if let Some(value) = target_obj.get(*json_key) {
                                        // Convert numbers/booleans to strings for consistency, then re-wrap as Value
                                        let str_val = value.to_string().trim_matches('"').to_string();
                                        results.insert(key.clone(), Value::String(str_val));
                                    }
                                } else if key == "code" {
                                    results.insert("code".to_string(), Value::String(code.to_string()));
                                }
                            }
                        } else {
                            // If keys is None, return the entire target_obj
                            results = target_obj.clone();
                            results.insert("code".to_string(), Value::String(code.to_string())); // Ensure code is present
                        }
                        results.insert("status".to_string(), Value::String("OK".to_string()));
                        results.insert("source".to_string(), Value::String("json_predefined".to_string()));
                        return Ok(results);
                    }
                }
            }
        }
    }

    // 2. Fallback to generic key search
    let fallback_keys_to_find = vec!["code".to_string()];
    let mut found_paths = Vec::new();
    find_object_paths(data, &fallback_keys_to_find, &mut Vec::new(), &mut found_paths);

    let fallback_mappings = HashMap::from([
        ("name", "name"),
        ("price", "price"),
        ("price_change", "priceChange"),
        ("price_change_rate", "priceChangeRate"),
        ("update_time", "priceDateTime"),
    ]);

    for path in found_paths {
        let mut target_obj = data;
        for &key in &path {
            target_obj = &target_obj[key];
        }
        if let Some(obj_map) = target_obj.as_object() {
            if let Some(found_code) = get_string_value(obj_map, "code") {
                let code_to_compare = code.split('.').next().unwrap_or(code);
                if found_code.trim() == code_to_compare {
                    let mut results = Map::new();
                    if let Some(keys_vec) = keys {
                        for key in keys_vec {
                            if let Some(json_key) = fallback_mappings.get(key.as_str()) {
                                if let Some(value) = obj_map.get(*json_key) {
                                    let str_val = value.to_string().trim_matches('"').to_string();
                                    results.insert(key.clone(), Value::String(str_val));
                                }
                            } else if key == "code" {
                                results.insert("code".to_string(), Value::String(code.to_string()));
                            }
                        }
                    } else {
                        // If keys is None, return the entire target_obj
                        results = obj_map.clone();
                        results.insert("code".to_string(), Value::String(code.to_string())); // Ensure code is present
                    }
                    results.insert("status".to_string(), Value::String("OK".to_string()));
                    results.insert("source".to_string(), Value::String("json_fallback".to_string()));
                    return Ok(results);
                }
            }
        }
    }

    Err(worker::Error::from("Could not find matching data in JSON."))
}

/// Processes the HTML body using CSS selectors as a fallback.
fn process_dom_data(code: &str, body: &str, keys: Option<&Vec<String>>) -> Result<Map<String, Value>> {
    let document = Html::parse_document(body);
    let mut results = Map::new();

    // Create a map of known keys to their selectors
    let mut selector_map = std::collections::HashMap::new();
    selector_map.insert("name", "h1");
    selector_map.insert("price", "div[class*='_CommonPriceBoard__priceBlock'] span[class*='_StyledNumber__value']");
    selector_map.insert("price_change", "span[class*='_PriceChangeLabel__primary'] span[class*='_StyledNumber__value']");
    selector_map.insert("price_change_rate", "span[class*='_PriceChangeLabel__secondary'] span[class*='_StyledNumber__value']");
    selector_map.insert("update_time", "li[class*='_CommonPriceBoard__time'] time, span[class*='_Time']");

    let keys_to_process = if let Some(k) = keys {
        k.clone()
    } else {
        // Default keys if not provided (for DOM, this means all known fields)
        vec![
            "code".to_string(),
            "name".to_string(),
            "price".to_string(),
            "price_change".to_string(),
            "price_change_rate".to_string(),
            "update_time".to_string(),
        ]
    };

    for key in &keys_to_process {
        let value = match key.as_str() {
            "code" => Some(code.to_string()),
            _ => {
                if let Some(selector_str) = selector_map.get(key.as_str()) {
                    let selector = Selector::parse(selector_str).unwrap();
                    document.select(&selector).next().map(|el| el.text().collect::<String>().trim().to_string())
                } else {
                    None
                }
            }
        };
        if let Some(val) = value {
            results.insert(key.clone(), Value::String(val));
        }
    }
    
    // Ensure essential keys are present if requested, or if no keys were requested (defaults used)
    if keys_to_process.contains(&"name".to_string()) && !results.contains_key("name") {
         return Err(worker::Error::from("Failed to scrape essential data (name) from DOM."));
    }
    if keys_to_process.contains(&"price".to_string()) && !results.contains_key("price") {
         return Err(worker::Error::from("Failed to scrape essential data (price) from DOM."));
    }

    results.insert("status".to_string(), Value::String("OK".to_string()));
    results.insert("source".to_string(), Value::String("dom_fallback".to_string()));

    Ok(results)
}

use std::collections::HashMap;
// --- Helper Functions ---

fn get_data_sources() -> Vec<DataSource> {
    vec![
        DataSource {
            path: &["mainStocksPriceBoard", "priceBoard"],
            mappings: HashMap::from([
                ("code", "code"),
                ("name", "name"),
                ("price", "price"),
                ("price_change", "priceChange"),
                ("price_change_rate", "priceChangeRate"),
                ("update_time", "priceDateTime"),
            ]),
            strip_suffix: true,
        },
        DataSource {
            path: &["mainCurrencyPriceBoard", "currencyPrices"],
            mappings: HashMap::from([
                ("code", "currencyPairCode"),
                ("name", "currencyPairName"),
                ("price", "bid"),
                ("price_change", "priceChange"),
                ("price_change_rate", "priceChangeRate"),
                ("update_time", "priceUpdateTime"),
            ]),
            strip_suffix: false,
        },
        DataSource {
            path: &["mainDomesticIndexPriceBoard", "indexPrices"],
            mappings: HashMap::from([
                ("code", "code"),
                ("name", "name"),
                ("price", "price"),
                ("price_change", "changePrice"),
                ("price_change_rate", "changePriceRate"),
                ("update_time", "japanUpdateTime"),
            ]),
            strip_suffix: false,
        },
    ]
}

fn find_object<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Map<String, Value>> {
    let mut current = value;
    for key in path {
        current = current.get(key)?;
    }
    current.as_object()
}

fn get_string_value<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    obj.get(key)?.as_str()
}

fn find_object_paths<'a>(
    value: &'a Value,
    keys_to_find: &[String],
    current_path: &mut Vec<&'a str>,
    found_paths: &mut Vec<Vec<&'a str>>,
) {
    if let Value::Object(map) = value {
        if keys_to_find.iter().all(|key| map.contains_key(key)) {
            found_paths.push(current_path.clone());
        }
        for (key, nested_value) in map {
            current_path.push(key);
            find_object_paths(nested_value, keys_to_find, current_path, found_paths);
            current_path.pop();
        }
    } else if let Value::Array(arr) = value {
        for nested_value in arr {
            find_object_paths(nested_value, keys_to_find, current_path, found_paths);
        }
    }
}