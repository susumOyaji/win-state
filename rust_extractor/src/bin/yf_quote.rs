use clap::Parser;
use regex::Regex;
use serde_json::{Map, Value};
use std::error::Error;

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
struct NormalizedData<'a> {
    code: &'a str,
    name: &'a str,
    price: String,
    price_change: String,
    price_change_rate: String,
    update_time: &'a str,
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
                                    code: found_code,
                                    name: get_string_value(target_obj, source.name_key).unwrap_or("N/A"),
                                    price: target_obj.get(source.price_key).map_or("N/A".to_string(), |v| v.to_string().trim_matches('"').to_string()),
                                    price_change: target_obj.get(source.change_key).map_or("N/A".to_string(), |v| v.to_string().trim_matches('"').to_string()),
                                    price_change_rate: target_obj.get(source.change_rate_key).map_or("N/A".to_string(), |v| v.to_string().trim_matches('"').to_string()),
                                    update_time: get_string_value(target_obj, source.time_key).unwrap_or("N/A"),
                                });
                                break; // Stop searching once found
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
            println!("-> Could not find __PRELOADED_STATE__ script tag for code {}.", code);
        }
    }

    Ok(())
}
