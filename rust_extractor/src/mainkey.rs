use regex::Regex;
use serde_json::Value;
use std::error::Error;

fn print_keys(value: &Value, prefix: &str) {
    match value {
        Value::Object(map) => {
            for (key, val) in map {
                let new_prefix = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", prefix, key)
                };
                println!("{}", new_prefix);
                print_keys(val, &new_prefix);
            }
        }
        Value::Array(arr) => {
            for (i, val) in arr.iter().enumerate() {
                let new_prefix = format!("{}[{}]", prefix, i);
                print_keys(val, &new_prefix);
            }
        }
        _ => {}
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let url = "https://finance.yahoo.co.jp/quote/5016.T";
    let body = reqwest::get(url).await?.text().await?;

    let re = Regex::new(r"(?s)window\.__PRELOADED_STATE__\s*=\s*(.*?)</script>")?;
    if let Some(captures) = re.captures(&body) {
        if let Some(json_str_match) = captures.get(1) {
            let mut json_str = json_str_match.as_str().trim();
            if json_str.ends_with(';') {
                json_str = &json_str[..json_str.len() - 1];
            }

            let data: Value = serde_json::from_str(json_str)?;
            print_keys(&data, "");
        }
    } else {
        println!("Could not find window.__PRELOADED_STATE__");
    }

    Ok(())
}
