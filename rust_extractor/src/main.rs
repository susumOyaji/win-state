use regex::Regex;
use serde_json::Value;
use std::error::Error;

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

            let price = data["mainStocksPriceBoard"]["priceBoard"]["price"].as_str();
            let price_change = data["mainStocksPriceBoard"]["priceBoard"]["priceChange"].as_str();
            let price_change_rate = data["mainStocksPriceBoard"]["priceBoard"]["priceChangeRate"].as_str();
            let update_time = data["mainStocksPriceBoard"]["priceBoard"]["priceDateTime"].as_str();

            if let Some(p) = price {
                println!("Stock Price: {}", p);
            } else {
                println!("Could not find stock price in the JSON data.");
            }

            if let Some(pc) = price_change {
                println!("Change from previous day: {}", pc);
            } else {
                println!("Could not find change from previous day in the JSON data.");
            }

            if let Some(pcr) = price_change_rate {
                println!("Change rate from previous day: {}%", pcr);
            } else {
                println!("Could not find change rate from previous day in the JSON data.");
            }

            if let Some(ut) = update_time {
                println!("Update Time: {}", ut);
            } else {
                println!("Could not find update time in the JSON data.");
            }
        }
    } else {
        println!("Could not find window.__PRELOADED_STATE__");
    }

    Ok(())
}