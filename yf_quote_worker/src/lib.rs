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
    code_key: &'static str,
    name_key: &'static str,
    price_key: &'static str,
    change_key: &'static str,
    change_rate_key: &'static str,
    time_key: &'static str,
    strip_suffix: bool,
}

/// Holds the normalized data extracted from any source.
#[derive(Serialize, Debug, Clone)]
struct NormalizedData {
    code: String,
    name: String,
    price: String,
    price_change: String,
    price_change_rate: String,
    update_time: String,
    status: String,
    source: String, // e.g., "json_predefined", "json_fallback", "dom"
}

/// Represents the final JSON response for a single code.
#[derive(Serialize, Debug)]
struct CodeResult {
    code: String,
    data: Option<NormalizedData>,
    error: Option<String>,
}

/// Main worker entry point.
#[event(fetch)]
pub async fn main(req: Request, _env: Env, _ctx: Context) -> Result<Response> {
    set_panic_hook();

    let url = req.url()?;
    let codes_query = url.query_pairs().find(|(key, _)| key == "code");

    if codes_query.is_none() {
        return Response::error("Query parameter 'code' is required. e.g., ?code=7203.T,^DJI", 400);
    }

    let codes: Vec<String> = codes_query
        .unwrap()
        .1
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.trim().to_string())
        .collect();

    if codes.is_empty() {
        return Response::error("Query parameter 'code' cannot be empty.", 400);
    }

    let futures = codes.iter().map(|code| fetch_single_code(code.clone()));
    let results = join_all(futures).await;

    Response::from_json(&results)
}

/// Fetches and processes data for a single stock code.
async fn fetch_single_code(code: String) -> CodeResult {
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

    let result_data = if let Some(caps) = re.captures(&body) {
        if let Some(json_match) = caps.get(1) {
            let mut json_str = json_match.as_str().trim();
            if json_str.ends_with(';') {
                json_str = &json_str[..json_str.len() - 1];
            }

            match serde_json::from_str(json_str) {
                Ok(data) => process_json_data(&code, &data),
                Err(e) => Err(worker::Error::from(format!("Failed to parse JSON: {}", e))),
            }
        } else {
            // JSON not found, fallback to DOM
            process_dom_data(&code, &body)
        }
    } else {
        // __PRELOADED_STATE__ script not found, fallback to DOM
        process_dom_data(&code, &body)
    };

    match result_data {
        Ok(data) => CodeResult { code, data: Some(data), error: None },
        Err(e) => CodeResult { code, data: None, error: Some(e.to_string()) },
    }
}

/// Processes the __PRELOADED_STATE__ JSON data to find financial info.
fn process_json_data(code: &str, data: &Value) -> Result<NormalizedData> {
    let data_sources = get_data_sources();

    // 1. Try predefined paths
    for source in &data_sources {
        if let Some(target_obj) = find_object(data, source.path) {
            if let Some(found_code) = get_string_value(target_obj, source.code_key) {
                let code_to_compare = if source.strip_suffix {
                    code.split('.').next().unwrap_or(code)
                } else {
                    code
                };

                if found_code.trim() == code_to_compare {
                    return Ok(NormalizedData {
                        code: code.to_string(),
                        name: get_string_value(target_obj, source.name_key).unwrap_or("N/A").to_string(),
                        price: target_obj.get(source.price_key).map_or("N/A".to_string(), |v| v.to_string().trim_matches('"').to_string()),
                        price_change: target_obj.get(source.change_key).map_or("N/A".to_string(), |v| v.to_string().trim_matches('"').to_string()),
                        price_change_rate: target_obj.get(source.change_rate_key).map_or("N/A".to_string(), |v| v.to_string().trim_matches('"').to_string()),
                        update_time: get_string_value(target_obj, source.time_key).unwrap_or("N/A").to_string(),
                        status: "OK".to_string(),
                        source: "json_predefined".to_string(),
                    });
                }
            }
        }
    }

    // 2. Fallback to generic key search
    let fallback_keys = vec!["code".to_string(), "name".to_string()];
    let mut found_paths = Vec::new();
    find_object_paths(data, &fallback_keys, &mut Vec::new(), &mut found_paths);

    for path in found_paths {
        let mut target_obj = data;
        for &key in &path {
            target_obj = &target_obj[key];
        }
        if let Some(obj_map) = target_obj.as_object() {
            if let Some(found_code) = get_string_value(obj_map, "code") {
                let code_to_compare = code.split('.').next().unwrap_or(code);
                if found_code.trim() == code_to_compare {
                    return Ok(NormalizedData {
                        code: code.to_string(),
                        name: get_string_value(obj_map, "name").unwrap_or("N/A").to_string(),
                        price: obj_map.get("price").map_or("N/A".to_string(), |v| v.to_string().trim_matches('"').to_string()),
                        price_change: obj_map.get("priceChange").map_or("N/A".to_string(), |v| v.to_string().trim_matches('"').to_string()),
                        price_change_rate: obj_map.get("priceChangeRate").map_or("N/A".to_string(), |v| v.to_string().trim_matches('"').to_string()),
                        update_time: obj_map.get("priceDateTime").map_or("N/A".to_string(), |v| v.to_string().trim_matches('"').to_string()),
                        status: "OK".to_string(),
                        source: "json_fallback".to_string(),
                    });
                }
            }
        }
    }

    Err(worker::Error::from("Could not find matching data in JSON."))
}

/// Processes the HTML body using CSS selectors as a fallback.
fn process_dom_data(code: &str, body: &str) -> Result<NormalizedData> {
    let document = Html::parse_document(body);

    let name_selector = Selector::parse("h1").unwrap();
    let price_selector = Selector::parse("div[class*='_CommonPriceBoard__priceBlock'] span[class*='_StyledNumber__value']").unwrap();
    let change_selector = Selector::parse("span[class*='_PriceChangeLabel__primary'] span[class*='_StyledNumber__value']").unwrap();
    let change_rate_selector = Selector::parse("span[class*='_PriceChangeLabel__secondary'] span[class*='_StyledNumber__value']").unwrap();
    let time_selector = Selector::parse("li[class*='_CommonPriceBoard__time'] time, span[class*='_Time']").unwrap();

    let name = document.select(&name_selector).next().map(|el| el.text().collect::<String>().trim().to_string());
    let price = document.select(&price_selector).next().map(|el| el.text().collect::<String>().trim().to_string());

    if name.is_none() || price.is_none() {
        return Err(worker::Error::from("Failed to scrape essential data (name/price) from DOM."));
    }

    let price_change = document.select(&change_selector).next().map(|el| el.text().collect::<String>().trim().to_string()).unwrap_or("N/A".to_string());
    let price_change_rate = document.select(&change_rate_selector).next().map(|el| el.text().collect::<String>().trim().to_string()).unwrap_or("N/A".to_string());
    let update_time = document.select(&time_selector).next().map(|el| el.text().collect::<String>().trim().to_string()).unwrap_or("N/A".to_string());

    Ok(NormalizedData {
        code: code.to_string(),
        name: name.unwrap(),
        price: price.unwrap(),
        price_change,
        price_change_rate,
        update_time,
        status: "OK".to_string(),
        source: "dom_fallback".to_string(),
    })
}

// --- Helper Functions (mostly unchanged) ---

fn get_data_sources() -> Vec<DataSource> {
    vec![
        DataSource {
            path: &["mainStocksPriceBoard", "priceBoard"],
            code_key: "code", name_key: "name", price_key: "price",
            change_key: "priceChange", change_rate_key: "priceChangeRate", time_key: "priceDateTime",
            strip_suffix: true,
        },
        DataSource {
            path: &["mainCurrencyPriceBoard", "currencyPrices"],
            code_key: "currencyPairCode", name_key: "currencyPairName", price_key: "bid",
            change_key: "priceChange", change_rate_key: "priceChangeRate", time_key: "priceUpdateTime",
            strip_suffix: false,
        },
        DataSource {
            path: &["mainDomesticIndexPriceBoard", "indexPrices"],
            code_key: "code", name_key: "name", price_key: "price",
            change_key: "changePrice", change_rate_key: "changePriceRate", time_key: "japanUpdateTime",
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