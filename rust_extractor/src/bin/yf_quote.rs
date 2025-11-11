use clap::Parser;
use regex::Regex;
use serde_json::{Map, Value};
use std::error::Error;
use scraper::{Html, Selector};

/// A struct to define a data source for financial data within the JSON.
struct DataSource {
    /// The path to the nested object containing the data.
    path: &'static [&'static str],
    /// The key for the symbol/code within the object.
    code_key: &'static str,
    /// The key for the name/description within the object.
    name_key: &'static str,
    /// The key for the price/value within the object.
    price_key: &'static str,
    /// The key for the price change.
    change_key: &'static str,
    /// The key for the price change rate.
    change_rate_key: &'static str,
    /// The key for the update time.
    time_key: &'static str,
    /// Whether to strip the market suffix (e.g., .T) before comparison.
    strip_suffix: bool,
}

/// A struct to hold the normalized data extracted from any data source.
struct NormalizedData {
    code: String,
    name: String,
    price: String,
    price_change: String,
    price_change_rate: String,
    update_time: String,
}

/// Command line arguments for the application.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// One or more stock/currency/index codes, separated by commas (e.g., "7203,^DJI,USDJPY=X").
    codes: String,
}

/// Finds a specific JSON object by traversing a path of keys.
fn find_object<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Map<String, Value>> {
    let mut current = value;
    for key in path {
        if let Some(nested) = current.get(key) {
            current = nested;
        } else {
            return None;
        }
    }
    current.as_object()
}

/// Extracts a string value from a JSON object, trimming quotes.
fn get_string_value<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    obj.get(key)?.as_str()
}

/// Recursively finds paths to objects that contain all the specified keys.
fn find_object_paths<'a>(
    value: &'a Value,
    keys_to_find: &[String],
    current_path: &mut Vec<&'a str>,
    found_paths: &mut Vec<Vec<&'a str>>,
) {
    if let Value::Object(map) = value {
        let has_all_keys = keys_to_find.iter().all(|key| map.contains_key(key));
        if has_all_keys {
            found_paths.push(current_path.clone());
        }

        for (key, nested_value) in map.iter() {
            current_path.push(key);
            find_object_paths(nested_value, keys_to_find, current_path, found_paths);
            current_path.pop(); // Backtrack
        }
    } else if let Value::Array(arr) = value {
        for nested_value in arr {
            find_object_paths(nested_value, keys_to_find, current_path, found_paths);
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let codes: Vec<&str> = args.codes.split(',').map(|s| s.trim()).collect();

    // Define the different data sources we know how to handle.
    let data_sources = vec![
        // For individual stocks
        DataSource {
            path: &["mainStocksPriceBoard", "priceBoard"],
            code_key: "code",
            name_key: "name",
            price_key: "price",
            change_key: "priceChange",
            change_rate_key: "priceChangeRate",
            time_key: "priceDateTime",
            strip_suffix: true,
        },
        // For currencies and indices
        DataSource {
            path: &["mainCurrencyPriceBoard", "currencyPrices"],
            code_key: "currencyPairCode",
            name_key: "currencyPairName",
            price_key: "bid",
            change_key: "priceChange", // Note: This key might not exist here, handled below
            change_rate_key: "priceChangeRate", // Note: This key might not exist here
            time_key: "priceUpdateTime",
            strip_suffix: false,
        },
        // For domestic indices like 998407.O (Nikkei 225)
        DataSource {
            path: &["mainDomesticIndexPriceBoard", "indexPrices"],
            code_key: "code",
            name_key: "name",
            price_key: "price",
            change_key: "changePrice",
            change_rate_key: "changePriceRate",
            time_key: "japanUpdateTime",
            strip_suffix: false,
        },
    ];

    for code in codes {
        if code.is_empty() {
            continue;
        }

        let url = if code.starts_with('^') || code.contains('=') || code.ends_with(".T") || code.ends_with(".O") {
            format!("https://finance.yahoo.co.jp/quote/{}/", code)
        } else {
            format!("https://finance.yahoo.co.jp/quote/{}.T/", code)
        };
        println!("Fetching data for code {} from: {}", code, url);

        let body = reqwest::get(&url).await?.text().await?;
        let re = Regex::new(r"(?s)window\.__PRELOADED_STATE__\s*=\s*(.*?)</script>")?;

        if let Some(caps) = re.captures(&body) {
            if let Some(json_match) = caps.get(1) {
                let mut json_str = json_match.as_str().trim();
                if json_str.ends_with(';') {
                    json_str = &json_str[..json_str.len() - 1];
                }

                let data: Value = match serde_json::from_str(json_str) {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("! Failed to parse JSON for code {}: {}", code, e);
                        continue;
                    }
                };

                let mut normalized_data: Option<NormalizedData> = None;

                // Try each data source to find the correct data
                for source in &data_sources {
                    if let Some(target_obj) = find_object(&data, source.path) {
                        if let Some(found_code) = get_string_value(target_obj, source.code_key) {
                                                            let code_to_compare = if source.strip_suffix {
                                                                code.split('.').next().unwrap_or(code)
                                                            } else {
                                                                code
                                                            };
                            
                                                            if found_code.trim() == code_to_compare {                                // Found the right object, now normalize it
                                normalized_data = Some(NormalizedData {
                                    code: found_code.to_string(),
                                    name: get_string_value(target_obj, source.name_key).unwrap_or("N/A").to_string(),
                                    price: target_obj.get(source.price_key).map_or("N/A".to_string(), |v| v.to_string().trim_matches('"').to_string()),
                                    price_change: target_obj.get(source.change_key).map_or("N/A".to_string(), |v| v.to_string().trim_matches('"').to_string()),
                                    price_change_rate: target_obj.get(source.change_rate_key).map_or("N/A".to_string(), |v| v.to_string().trim_matches('"').to_string()),
                                    update_time: get_string_value(target_obj, source.time_key).unwrap_or("N/A").to_string(),
                                });
                                break; // Stop searching once found
                            }
                        }
                    }
                }
                
                // If no data found with specific paths, fallback to generic key search
                if normalized_data.is_none() {
                    println!("-> Could not find data in known locations. Falling back to generic key search.");
                    let fallback_keys = vec!["code".to_string(), "name".to_string()];
                    let mut found_paths = Vec::new();
                    find_object_paths(&data, &fallback_keys, &mut Vec::new(), &mut found_paths);

                    for path in found_paths {
                        let mut target_obj = &data;
                        for &key in &path {
                            target_obj = &target_obj[key];
                        }
                        if let Some(obj_map) = target_obj.as_object() {
                            if let Some(found_code) = get_string_value(obj_map, "code") {
                                let code_to_compare = code.split('.').next().unwrap_or(code);
                                if found_code.trim() == code_to_compare {
                                    normalized_data = Some(NormalizedData {
                                        code: found_code.to_string(),
                                        name: get_string_value(obj_map, "name").unwrap_or("N/A").to_string(),
                                        price: obj_map.get("price").map_or("N/A".to_string(), |v| v.to_string().trim_matches('"').to_string()),
                                        price_change: obj_map.get("priceChange").map_or("N/A".to_string(), |v| v.to_string().trim_matches('"').to_string()),
                                        price_change_rate: obj_map.get("priceChangeRate").map_or("N/A".to_string(), |v| v.to_string().trim_matches('"').to_string()),
                                        update_time: obj_map.get("priceDateTime").map_or("N/A".to_string(), |v| v.to_string().trim_matches('"').to_string()),
                                    });
                                    break; // Found it, no need to check other paths
                                }
                            }
                        }
                    }
                }

                println!("----------------------------------------");
                if let Some(d) = normalized_data {
                    println!("{:<18}: {}", "Code", d.code);
                    println!("{:<18}: {}", "Name", d.name);
                    println!("{:<18}: {}", "Price", d.price);
                    println!("{:<18}: {}", "Change", d.price_change);
                    println!("{:<18}: {}", "Change (Rate)", d.price_change_rate);
                    println!("{:<18}: {}", "Time", d.update_time);
                } else {
                    println!("-> No data found matching code '{}' in known locations.", code);
                }
                println!("----------------------------------------");

            } else {
                println!("-> Could not find JSON data in the script tag for code {}.", code);
            }
        } else {
            // Fallback to DOM scraping if __PRELOADED_STATE__ is not found
            println!("-> __PRELOADED_STATE__ not found. Falling back to DOM scraping for {}.", code);
            let document = Html::parse_document(&body);

            // Selectors for index data (based on Yahoo Finance Japan's structure for indices)
            let name_selector = Selector::parse("h1").unwrap();
            let price_selector = Selector::parse("div[class*='_CommonPriceBoard__priceBlock'] span[class*='_StyledNumber__value']").unwrap();
            let change_selector = Selector::parse("span[class*='_PriceChangeLabel__primary'] span[class*='_StyledNumber__value']").unwrap();
            let change_rate_selector = Selector::parse("span[class*='_PriceChangeLabel__secondary'] span[class*='_StyledNumber__value']").unwrap();
            let time_selector = Selector::parse("span[class*='_Time'], time[class*='timestamp']").unwrap();

            let name = document.select(&name_selector).next().map(|el| el.text().collect::<String>().trim().to_string()).unwrap_or("N/A".to_string());
            let price = document.select(&price_selector).next().map(|el| el.text().collect::<String>().trim().to_string()).unwrap_or("N/A".to_string());
            let price_change = document.select(&change_selector).next().map(|el| el.text().collect::<String>().trim().to_string()).unwrap_or("N/A".to_string());
            let price_change_rate = document.select(&change_rate_selector).next().map(|el| el.text().collect::<String>().trim().to_string()).unwrap_or("N/A".to_string());
            let update_time = document.select(&time_selector).next().map(|el| el.text().collect::<String>().trim().to_string()).unwrap_or("N/A".to_string());

            println!("----------------------------------------");
            if name != "N/A" && price != "N/A" {
                println!("{:<18}: {}", "Code", code);
                println!("{:<18}: {}", "Name", name);
                println!("{:<18}: {}", "Price", price);
                println!("{:<18}: {}", "Change", price_change);
                println!("{:<18}: {}", "Change (Rate)", price_change_rate);
                println!("{:<18}: {}", "Time", update_time);
            } else {
                println!("-> Failed to scrape data from DOM for code '{}'.", code);
            }
            println!("----------------------------------------");
        }
    }

    Ok(())
}
