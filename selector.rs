use scraper::{ElementRef, Html, Selector};
use regex::Regex;
use worker::*;
use serde::Serialize;

pub mod selector_generator;
use selector_generator::generate_selector_candidates;

// --- セレクター検証API用のデータ構造 ---
#[derive(Serialize, Debug, Clone)]
struct VerificationResult {
    url: String,
    selector: String,
    is_valid_syntax: bool,
    error_message: Option<String>,
    match_count: usize,
    matches: Vec<ElementInfo>,
}

#[derive(Serialize, Debug, Clone)]
struct ElementInfo {
    tag: String,
    text: String,
    html: String,
}

// --- データ構造 ---
#[derive(Serialize, Debug, Clone)]
pub struct StockData {
    pub name: String,
    pub code: String,
    pub price: String,
    pub change_abs: String,
    pub change_pct: String,
    pub update_time: String,
}

#[derive(Serialize, Debug, Clone)]
struct RankedCandidate {
    text: String,
    score: u32,
    reason: String,
}

#[derive(Serialize, Debug)]
struct DiscoveredData {
    code: String,
    url: String,
    name_candidates: Vec<RankedCandidate>,
    price_candidates: Vec<RankedCandidate>,
    change_abs_candidates: Vec<RankedCandidate>,
    change_pct_candidates: Vec<RankedCandidate>,
    update_time_candidates: Vec<RankedCandidate>,
}

#[derive(Serialize, Debug)]
struct DynamicScrapeResult {
    data: StockData,
    used_selectors: std::collections::HashMap<String, String>,
}

fn deduplicate_and_sort_candidates(candidates: Vec<RankedCandidate>) -> Vec<RankedCandidate> {
    let mut map: std::collections::HashMap<String, RankedCandidate> = std::collections::HashMap::new();
    for candidate in candidates {
        map.entry(candidate.text.clone())
            .and_modify(|e| {
                if candidate.score > e.score {
                    *e = candidate.clone();
                }
            })
            .or_insert(candidate);
    }
    let mut final_candidates: Vec<RankedCandidate> = map.into_values().collect();
    final_candidates.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.text.cmp(&b.text)));
    final_candidates
}

// --- 改良版：セルフヒーリング付きスクレイピング本体 ---

async fn discover_data(code: &str) -> Result<DiscoveredData> {
    let url = format!("https://finance.yahoo.co.jp/quote/{}", code);
    let mut res = Fetch::Url(Url::parse(&url)?).send().await?;
    let html = res.text().await?;
    let document = Html::parse_document(&html);

    let mut name_candidates: Vec<RankedCandidate> = Vec::new();
    let mut price_candidates: Vec<RankedCandidate> = Vec::new();
    let mut change_abs_candidates: Vec<RankedCandidate> = Vec::new();
    let mut change_pct_candidates: Vec<RankedCandidate> = Vec::new();
    let mut update_time_candidates: Vec<RankedCandidate> = Vec::new();

    let mut base_name = String::new();
    if let Ok(title_selector) = Selector::parse("title") {
        if let Some(title_el) = document.select(&title_selector).next() {
            let title_text = title_el.text().collect::<String>();
            base_name = title_text.split('【').next().unwrap_or("")
                .split('(').next().unwrap_or("")
                .split('：').next().unwrap_or("")
                .trim().to_string();
            if !base_name.is_empty() {
                name_candidates.push(RankedCandidate { text: title_text.clone(), score: 50, reason: "Original <title> text".to_string() });
            }
        }
    }
    if !base_name.is_empty() {
        // Safely parse the heading selector; fall back to simpler selectors if parsing fails.
        let heading_selectors = match Selector::parse("h1, h2") {
            Ok(sel) => sel,
            Err(_) => {
                console_log!("[WARN] Failed to parse selector 'h1, h2', falling back to 'h1'");
                match Selector::parse("h1") {
                    Ok(s) => s,
                    Err(_) => {
                        console_log!("[WARN] Failed to parse fallback selector 'h1', using universal '*' selector");
                        // '*' should always be a valid selector; unwrap is safe here
                        Selector::parse("*").unwrap()
                    }
                }
            }
        };

        for element in document.select(&heading_selectors) {
            let text = element.text().collect::<String>().trim().to_string();
            if text.is_empty() { continue; }
            if text == base_name {
                name_candidates.push(RankedCandidate { text, score: 110, reason: format!("Exact match in <{}>", element.value().name()) });
            } else if text.contains(&base_name) {
                name_candidates.push(RankedCandidate { text, score: 100, reason: format!("Contains base name in <{}>", element.value().name()) });
            }
        }
    }

    // より広いセレクターパターンを試す
    for selector_str in &[
        "span[class*='PriceBoard__price'] span[class*='StyledNumber__value']", // Add this with high priority
        "[class*='price'], [class*='Price']",
        "span[class*='value'], div[class*='value']",
        "[class*='board'] span, [class*='Board'] span",
        "[data-field='regularMarketPrice']",
        "[class*='quote'], [class*='Quote']",
        "span[class*='last'], div[class*='last']",
        "[class*='current'], [class*='Current']"
    ] {
        if let Ok(sel) = Selector::parse(selector_str) {
            for element in document.select(&sel) {
                let text = element.text().collect::<String>().trim().to_string();
                // 数値っぽい文字列かどうかをチェック（より緩やかな判定）
                if text.chars().any(|c| c.is_ascii_digit()) {
                    let cleaned_text = text.replace(",", "");
                    if let Ok(parsed_price) = cleaned_text.parse::<f64>() {
                        if parsed_price >= 0.0 {
                            let mut score = 50;
                            let class_attr = element.value().attr("class").unwrap_or("");
                            // Assign higher score for the new specific selector
                            if *selector_str == "span[class*='PriceBoard__price'] span[class*='StyledNumber__value']" {
                                score += 100; // High score for current price
                            }
                            if text.contains(',') { score += 30; }
                            if class_attr.contains("value") { score += 20; }
                            if class_attr.contains("large") { score += 10; }
                            if class_attr.contains("code") || class_attr.contains("symbol") { score -= 40; }
                            price_candidates.push(RankedCandidate {
                                text: text.clone(),
                                score,
                                reason: format!("Found in element with class: {} (selector: {})", class_attr, selector_str)
                            });

                            // デバッグログ
                            console_log!(
                                "Found price candidate: {} (score: {}, selector: {})",
                                text, score, selector_str
                            );
                        }
                    }
                }
            }
        }
    }
    // 候補が見つからなかった場合のフォールバック
    if price_candidates.is_empty() {
        console_log!("No price candidates found, trying fallback selectors...");
        // フォールバック: より広いセレクターで数値を探す
        if let Ok(sel) = Selector::parse("span, div") {
            for element in document.select(&sel) {
                let text = element.text().collect::<String>().trim().to_string();
                if text.chars().any(|c| c.is_ascii_digit()) {
                    let cleaned_text = text.replace(",", "");
                    if let Ok(parsed_price) = cleaned_text.parse::<f64>() {
                        if parsed_price >= 0.0 {
                            price_candidates.push(RankedCandidate { 
                                text, 
                                score: 10, // フォールバックなので低いスコア
                                reason: format!("Fallback: found number in {}", element.value().name()) 
                            });
                        }
                    }
                }
            }
        }
    }

    if let Ok(sel) = Selector::parse("[class*='PriceChangeLabel__primary']") {
        for element in document.select(&sel) {
            let text = element.text().collect::<String>().trim().to_string();
            if (text.starts_with('+') || text.starts_with('-')) && text.chars().any(|c| c.is_digit(10)) {
                change_abs_candidates.push(RankedCandidate { text: text.clone(), score: 100, reason: "Found in primary change label".to_string() });
            }
            if text.contains('%') && text.contains('(') {
                change_pct_candidates.push(RankedCandidate { text, score: 100, reason: "Found in secondary change label".to_string() });
            }
        }
    }

    // Fallback for change_pct if not found by the primary selector
    if change_pct_candidates.is_empty() {
        console_log!("No change_pct candidates found with primary selector, trying broader fallback...");
        for selector_str in &[
            "[class*='change']", // Look for classes containing 'change'
            "[class*='percent']", // Look for classes containing 'percent'
            "span",               // General span elements
            "div",                // General div elements
        ] {
            if let Ok(sel) = Selector::parse(selector_str) {
                for element in document.select(&sel) {
                    let text = element.text().collect::<String>().trim().to_string();
                    if text.contains('%') && (text.starts_with('+') || text.starts_with('-') || text.chars().any(|c| c.is_ascii_digit())) {
                        change_pct_candidates.push(RankedCandidate {
                            text: text.clone(),
                            score: 50, // Lower score for broader fallback
                            reason: format!("Broader fallback: found '%' in element with selector: {}", selector_str)
                        });
                        //console_log!("[DEBUG] discover_data: Broader DOM Fallback Change Pct: {}", text);
                    }
                }
            }
        }
    }

    // Update Time _CommonPriceBoard__time_1g7gt_55
    for selector_str in &["ul[class*='PriceBoard__times'] time", "time[class*='timestamp']"] {
        if let Ok(sel) = Selector::parse(selector_str) {
            for element in document.select(&sel) {
                let text = element.text().collect::<String>().trim().to_string();
                if !text.is_empty() {
                    update_time_candidates.push(RankedCandidate { text, score: 100, reason: format!("Found in time element with selector: {}", selector_str) });
                }
            }
        }
    }

    let final_name_candidates = deduplicate_and_sort_candidates(name_candidates);
    let final_price_candidates = deduplicate_and_sort_candidates(price_candidates);
    let final_change_abs_candidates = deduplicate_and_sort_candidates(change_abs_candidates);
    let final_change_pct_candidates = deduplicate_and_sort_candidates(change_pct_candidates);
    let final_update_time_candidates = deduplicate_and_sort_candidates(update_time_candidates);

    Ok(DiscoveredData {
        code: code.to_string(),
        url,
        name_candidates: final_name_candidates,
        price_candidates: final_price_candidates,
        change_abs_candidates: final_change_abs_candidates,
        change_pct_candidates: final_change_pct_candidates,
        update_time_candidates: final_update_time_candidates,
    })
}

async fn discover_index_data(code: &str) -> Result<DiscoveredData> {
    let url = format!("https://finance.yahoo.co.jp/quote/{}", code);
    let mut res = Fetch::Url(Url::parse(&url)?).send().await?;
    let html = res.text().await?;
    let document = Html::parse_document(&html);

    let mut name_candidates: Vec<RankedCandidate> = Vec::new();
    let mut price_candidates: Vec<RankedCandidate> = Vec::new();
    let mut change_abs_candidates: Vec<RankedCandidate> = Vec::new();
    let mut change_pct_candidates: Vec<RankedCandidate> = Vec::new();
    let mut update_time_candidates: Vec<RankedCandidate> = Vec::new();

    // Try to extract data from window.__PRELOADED_STATE__ JSON
    let re_preloaded_state = Regex::new(r"window\.__PRELOADED_STATE__ = (\{.*?\});")
        .map_err(|e| Error::from(format!("Regex compilation failed: {}", e)))?;
    if let Some(caps) = re_preloaded_state.captures(&html) {
        if let Some(json_str) = caps.get(1).map(|m| m.as_str()) {
            if let Ok(parsed_json) = serde_json::from_str::<serde_json::Value>(json_str) {
                // Extract Name
                if let Some(name_val) = parsed_json["pageInfo"]["title"].as_str() {
                    let cleaned_name = name_val.split(" - ").next().unwrap_or("").trim().to_string();
                    if !cleaned_name.is_empty() {
                        name_candidates.push(RankedCandidate { text: cleaned_name.clone(), score: 100, reason: "Found in __PRELOADED_STATE__ (title)".to_string() });
                        console_log!("[DEBUG] discover_index_data: JSON Name: {}", cleaned_name);
                    }
                }

                // Extract Price, Change, and Pct from priceBoard
                if let Some(price_board) = parsed_json.get("priceBoard") {
                    // Price
                    if let Some(price_val) = price_board.get("price").and_then(|v| v.as_str()) {
                        price_candidates.push(RankedCandidate { text: price_val.to_string(), score: 100, reason: "Found in __PRELOADED_STATE__ (price)".to_string() });
                        console_log!("[DEBUG] discover_index_data: JSON Price: {}", price_val);
                    }
                    // Change Absolute
                    if let Some(change_val) = price_board.get("change").and_then(|v| v.as_str()) {
                        if !change_val.is_empty() {
                            change_abs_candidates.push(RankedCandidate {
                                text: change_val.to_string(),
                                score: 100,
                                reason: "Found in __PRELOADED_STATE__ (change)".to_string(),
                            });
                            console_log!("[DEBUG] discover_index_data: JSON Change Abs: {}", change_val);
                        }
                    }
                    // Change Percentage
                    if let Some(change_pct_val) = price_board.get("changePercent").and_then(|v| v.as_str()) {
                        if !change_pct_val.is_empty() {
                            let cleaned_pct = change_pct_val.trim_matches(|c| c == '(' || c == ')').to_string();
                            change_pct_candidates.push(RankedCandidate {
                                text: cleaned_pct.clone(),
                                score: 100,
                                reason: "Found in __PRELOADED_STATE__ (changePercent)".to_string(),
                            });
                            console_log!("[DEBUG] discover_index_data: JSON Change Pct: {}", cleaned_pct);
                        }
                    }
                    // Update Time
                    if let Some(time_val) = price_board.get("marketTime").or(price_board.get("tradeTime")).and_then(|v| v.as_str()) {
                        if !time_val.is_empty() {
                            update_time_candidates.push(RankedCandidate { text: time_val.to_string(), score: 100, reason: "Found in __PRELOADED_STATE__ (marketTime/tradeTime)".to_string() });
                            console_log!("[DEBUG] discover_index_data: JSON Update Time: {}", time_val);
                        }
                    }
                }
            }
        }
    }

    // Fallback for Name if JSON extraction fails
    if name_candidates.is_empty() {
        console_log!("[DEBUG] discover_index_data: JSON name extraction failed, falling back to DOM scraping.");
        // Use title tag as a primary fallback
        if let Ok(sel) = Selector::parse("title") {
            if let Some(el) = document.select(&sel).next() {
                let title_text = el.text().collect::<String>();
                let cleaned_name = title_text.split(" - ").next().unwrap_or("").trim().to_string();
                 if !cleaned_name.is_empty() {
                    name_candidates.push(RankedCandidate { text: cleaned_name, score: 80, reason: "Found in <title> tag (fallback)".to_string() });
                }
            }
        }
        // Use h1 tag as a secondary fallback
        if name_candidates.is_empty() {
             if let Ok(sel) = Selector::parse("h1") {
                if let Some(el) = document.select(&sel).next() {
                    let h1_text = el.text().collect::<String>().trim().to_string();
                    if !h1_text.is_empty() {
                        name_candidates.push(RankedCandidate { text: h1_text, score: 70, reason: "Found in <h1> tag (fallback)".to_string() });
                    }
                }
            }
        }
    }

    // Fallback to DOM scraping for price, change_abs, change_pct if JSON extraction fails or is incomplete
    if price_candidates.is_empty() || change_abs_candidates.is_empty() || change_pct_candidates.is_empty() {
        console_log!("[DEBUG] discover_index_data: JSON price/change extraction failed or incomplete, falling back to DOM scraping.");
        // Price
        if price_candidates.is_empty() {
            if let Ok(sel) = Selector::parse("div[class*='_CommonPriceBoard__priceBlock'] span[class*='_StyledNumber__value']") {
                for element in document.select(&sel) {
                    let text = element.text().collect::<String>().trim().to_string();
                    if !text.starts_with('+') && !text.starts_with('-') {
                        if let Ok(parsed_price) = text.replace(",", "").parse::<f64>() {
                            if parsed_price >= 0.0 {
                                price_candidates.push(RankedCandidate { text: text.clone(), score: 90, reason: "Found in _CommonPriceBoard__priceBlock (fallback)".to_string() });
                                console_log!("[DEBUG] discover_index_data: DOM Fallback Price: {}", text);
                            }
                        }
                    }
                }
            }
        }

        // Broader fallback for price within the main price information block
        if price_candidates.is_empty() {
            if let Ok(sel) = Selector::parse("div[class*='_BasePriceBoard__priceInformation'] span, div[class*='_BasePriceBoard__priceInformation'] div") {
                for element in document.select(&sel) {
                    let text = element.text().collect::<String>().trim().to_string();
                    // Heuristic to distinguish price from change values
                    if text.chars().any(|c| c.is_ascii_digit()) && !text.starts_with('+') && !text.starts_with('-') && !text.contains('%') {
                        let cleaned_text = text.replace(",", "");
                        if let Ok(parsed_price) = cleaned_text.parse::<f64>() {
                            if parsed_price >= 0.0 {
                                price_candidates.push(RankedCandidate { 
                                    text: text.clone(), 
                                    score: 70, // Lower score for broader fallback
                                    reason: format!("Broader fallback in _BasePriceBoard__priceInformation: {}", element.value().name()) 
                                });
                                console_log!("[DEBUG] discover_index_data: Broader DOM Fallback Price: {}", text);
                            }
                        }
                    }
                }
            }
        }

        // Change Absolute
        if change_abs_candidates.is_empty() {
            if let Ok(sel) = Selector::parse("span[class*='_PriceChangeLabel__primary'] span[class*='_StyledNumber__value']") {
                for element in document.select(&sel) {
                    let text = element.text().collect::<String>().trim().to_string();
                    if text.starts_with('+') || text.starts_with('-') {
                        change_abs_candidates.push(RankedCandidate { text: text.clone(), score: 90, reason: "Found in _PriceChangeLabel__primary (fallback)".to_string() });
                        console_log!("[DEBUG] discover_index_data: DOM Fallback Change Abs: {}", text);
                    }
                }
            }
        }

        // Change Percentage
        if change_pct_candidates.is_empty() {
            if let Ok(sel) = Selector::parse("span[class*='_PriceChangeLabel__secondary'] span[class*='_StyledNumber__value']") {
                for element in document.select(&sel) {
                    let text = element.text().collect::<String>().trim().to_string();
                    if !text.is_empty() {
                        change_pct_candidates.push(RankedCandidate { text: text.clone(), score: 90, reason: "Found in _PriceChangeLabel__secondary (fallback)".to_string() });
                        console_log!("[DEBUG] discover_index_data: DOM Fallback Change Pct: {}", text);
                    }
                }
            }
        }

        // Update Time
        if update_time_candidates.is_empty() {
            if let Ok(sel) = Selector::parse("span[class*='_Time'], time[class*='timestamp']") {
                for element in document.select(&sel) {
                    let text = element.text().collect::<String>().trim().to_string();
                    if !text.is_empty() {
                        update_time_candidates.push(RankedCandidate { text: text.clone(), score: 90, reason: "Found in DOM (fallback)".to_string() });
                        console_log!("[DEBUG] discover_index_data: DOM Fallback Update Time: {}", text);
                    }
                }
            }
        }
    }

    let final_name_candidates = deduplicate_and_sort_candidates(name_candidates);
    let final_price_candidates = deduplicate_and_sort_candidates(price_candidates);
    let final_change_abs_candidates = deduplicate_and_sort_candidates(change_abs_candidates);
    let final_change_pct_candidates = deduplicate_and_sort_candidates(change_pct_candidates);
    let final_update_time_candidates = deduplicate_and_sort_candidates(update_time_candidates);

    Ok(DiscoveredData {
        code: code.to_string(),
        url,
        name_candidates: final_name_candidates,
        price_candidates: final_price_candidates,
        change_abs_candidates: final_change_abs_candidates,
        change_pct_candidates: final_change_pct_candidates,
        update_time_candidates: final_update_time_candidates
    })
}

async fn discover_currency_x_data(code: &str) -> Result<DiscoveredData> {
    let url = format!("https://finance.yahoo.co.jp/quote/{}", code);
    let mut res = Fetch::Url(Url::parse(&url)?).send().await?;
    let html = res.text().await?;
    let document = Html::parse_document(&html);

    let mut name_candidates: Vec<RankedCandidate> = Vec::new();
    let mut price_candidates: Vec<RankedCandidate> = Vec::new();
    let mut update_time_candidates: Vec<RankedCandidate> = Vec::new();

    // --- DOM Fallback Logic --- 

    // Name
    if let Ok(sel) = Selector::parse("h1") {
        if let Some(el) = document.select(&sel).next() {
            let text = el.text().collect::<String>();
            let cleaned_name = text.split(" - ").next().unwrap_or("").trim().to_string();
            if !cleaned_name.is_empty() {
                name_candidates.push(RankedCandidate { text: cleaned_name, score: 100, reason: "Found in <h1>".to_string() });
            }
        }
    }

    // Price (Selector Guess)
    if let Ok(sel) = Selector::parse("div[class*='rate'] span, span[class*='price']") {
        for element in document.select(&sel) {
            let text = element.text().collect::<String>().trim().to_string();
            let cleaned_text = text.replace(",", "");
            if cleaned_text.parse::<f64>().is_ok() && !text.is_empty() {
                 let score = 90; // Define score here
                 price_candidates.push(RankedCandidate { text: text.clone(), score, reason: "Guessed DOM selector for price".to_string() });
                 console_log!(
                    "Found price candidate: {} (score: {}, selector: {})",
                    text, score, "div[class*='rate'] span, span[class*='price']" // Hardcode selector for log
                 );
            }
        }
    }

    // Update Time (Selector Guess)
    if let Ok(sel) = Selector::parse("span[class*='time'], time") {
        for element in document.select(&sel) {
            let text = element.text().collect::<String>().trim().to_string();
            if !text.is_empty() {
                update_time_candidates.push(RankedCandidate { text, score: 90, reason: "Guessed DOM selector for time".to_string() });
            }
        }
    }

    let final_name_candidates = deduplicate_and_sort_candidates(name_candidates);
    let final_price_candidates = deduplicate_and_sort_candidates(price_candidates);
    let final_update_time_candidates = deduplicate_and_sort_candidates(update_time_candidates);

    Ok(DiscoveredData {
        code: code.to_string(),
        url,
        name_candidates: final_name_candidates,
        price_candidates: final_price_candidates,
        change_abs_candidates: vec![], // Not searched in this function
        change_pct_candidates: vec![], // Not searched in this function
        update_time_candidates: final_update_time_candidates,
    })
}

async fn discover_currency_fx_data(code: &str) -> Result<DiscoveredData> {
    let url = format!("https://finance.yahoo.co.jp/quote/{}", code);
    let mut res = Fetch::Url(Url::parse(&url)?).send().await?;
    let html = res.text().await?;
    let document = Html::parse_document(&html);

    let mut name_candidates: Vec<RankedCandidate> = Vec::new();
    let mut price_candidates: Vec<RankedCandidate> = Vec::new();
    let mut change_abs_candidates: Vec<RankedCandidate> = Vec::new();
    let mut change_pct_candidates: Vec<RankedCandidate> = Vec::new();

    // Name
    if let Ok(sel) = Selector::parse("h1") {
        if let Some(el) = document.select(&sel).next() {
            let text = el.text().collect::<String>();
            let cleaned_name = text.split(" - ").next().unwrap_or("").trim().to_string();
            if !cleaned_name.is_empty() {
                name_candidates.push(RankedCandidate { text: cleaned_name, score: 100, reason: "Found in <h1>".to_string() });
            }
        }
    }

    // Price
    if let Ok(sel) = Selector::parse("div[class*='rate'] span, span[class*='price']") {
        for element in document.select(&sel) {
            let text = element.text().collect::<String>().trim().to_string();
            let cleaned_text = text.replace(",", "");
            if cleaned_text.parse::<f64>().is_ok() && !text.is_empty() {
                 price_candidates.push(RankedCandidate { text, score: 90, reason: "Guessed DOM selector for price".to_string() });
            }
        }
    }

    // Change
    if let Ok(sel) = Selector::parse("[class*='change'], [class*='diff'], [class*='gain'], [class*='loss'], [class*='up'], [class*='down']") {
        for element in document.select(&sel) {
            let text = element.text().collect::<String>().trim().to_string();
            if text.starts_with('+') || text.starts_with('-') {
                let score = 90; // Define score here
                let selector_str = "[class*='change'], [class*='diff'], [class*='gain'], [class*='loss'], [class*='up'], [class*='down']"; // Define selector for log
                if text.contains('%') {
                    change_pct_candidates.push(RankedCandidate { text: text.clone(), score, reason: "Guessed DOM selector for change pct".to_string() });
                    console_log!(
                        "Found change_pct candidate: {} (score: {}, selector: {})",
                        text, score, selector_str
                    );
                } else {
                    change_abs_candidates.push(RankedCandidate { text: text.clone(), score, reason: "Guessed DOM selector for change abs".to_string() });
                    console_log!(
                        "Found change_abs candidate: {} (score: {}, selector: {})",
                        text, score, selector_str
                    );
                }
            }
        }
    }

    let final_name_candidates = deduplicate_and_sort_candidates(name_candidates);
    let final_price_candidates = deduplicate_and_sort_candidates(price_candidates);
    let final_change_abs_candidates = deduplicate_and_sort_candidates(change_abs_candidates);
    let final_change_pct_candidates = deduplicate_and_sort_candidates(change_pct_candidates);

    Ok(DiscoveredData {
        code: code.to_string(),
        url,
        name_candidates: final_name_candidates,
        price_candidates: final_price_candidates,
        change_abs_candidates: final_change_abs_candidates,
        change_pct_candidates: final_change_pct_candidates,
        update_time_candidates: vec![], // Not searched in this function
    })
}

async fn discover_currency_data(code: &str) -> Result<DiscoveredData> {
    // Ensure we start with the =X code
    let code_x = if code.ends_with("=FX") {
        code.replace("=FX", "=X").to_string()
    } else {
        code.to_string()
    };
    let code_fx = code_x.replace("=X", "=FX");

    // 1. Get base data and update_time from =X page
    let data_x = discover_currency_x_data(&code_x).await?;

    // 2. Get change data from =FX page
    let data_fx = discover_currency_fx_data(&code_fx).await?;

    // 3. Merge the results
    Ok(DiscoveredData {
        code: code.to_string(),
        url: data_x.url, // Use the URL from the primary (=X) page
        name_candidates: data_x.name_candidates,
        price_candidates: data_x.price_candidates,
        change_abs_candidates: data_fx.change_abs_candidates,
        change_pct_candidates: data_fx.change_pct_candidates,
        update_time_candidates: data_x.update_time_candidates,
    })
}



async fn scrape_dynamically(code: &str) -> Result<DynamicScrapeResult> {
    let url = format!("https://finance.yahoo.co.jp/quote/{}", code);
    let mut res = Fetch::Url(Url::parse(&url)?).send().await?;
        let html = res.text().await?;
        let discovered = if code.starts_with('^') {
        discover_index_data(code).await?
    } else if code.ends_with("=X") || code.ends_with("=FX") {
        discover_currency_data(code).await?
    } else {
        discover_data(code).await?
    };
    
    let name = discovered.name_candidates.get(0).map_or(String::new(), |c| c.text.clone());
    let price = discovered.price_candidates.get(0).map_or(String::new(), |c| c.text.clone());
    let change_abs = discovered.change_abs_candidates.get(0).map_or(String::new(), |c| c.text.clone());
    let change_pct = discovered.change_pct_candidates.get(0).map_or(String::new(), |c| c.text.clone());
    let update_time = discovered.update_time_candidates.get(0).map_or(String::new(), |c| c.text.clone());

    // Selector generation is also optional
    let best_name_selector = discovered.name_candidates.get(0)
        .map(|c| generate_selector_candidates(&html, &c.text).get(0).cloned().unwrap_or_default())
        .unwrap_or_default();
    let best_price_selector = discovered.price_candidates.get(0)
        .map(|c| generate_selector_candidates(&html, &c.text).get(0).cloned().unwrap_or_default())
        .unwrap_or_default();
    let best_change_abs_selector = discovered.change_abs_candidates.get(0)
        .map(|c| generate_selector_candidates(&html, &c.text).get(0).cloned().unwrap_or_default())
        .unwrap_or_default();
    let best_change_pct_selector = discovered.change_pct_candidates.get(0)
        .map(|c| generate_selector_candidates(&html, &c.text).get(0).cloned().unwrap_or_default())
        .unwrap_or_default();
    let best_update_time_selector = discovered.update_time_candidates.get(0)
        .map(|c| generate_selector_candidates(&html, &c.text).get(0).cloned().unwrap_or_default())
        .unwrap_or_default();

    let stock_data = StockData { name, code: code.to_string(), price, change_abs, change_pct, update_time };

    let mut used_selectors = std::collections::HashMap::new();
    used_selectors.insert("name".to_string(), best_name_selector.clone());
    used_selectors.insert("price".to_string(), best_price_selector.clone());
    used_selectors.insert("change_abs".to_string(), best_change_abs_selector.clone());
    used_selectors.insert("change_pct".to_string(), best_change_pct_selector.clone());
    used_selectors.insert("update_time".to_string(), best_update_time_selector.clone());

    Ok(DynamicScrapeResult { data: stock_data, used_selectors })
}


#[event(fetch)]
pub async fn main(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();
    let router = Router::new();
    router
        .get("/health", |_, _| Response::ok("OK"))
        .get_async("/quote", |req, _ctx| async move {
            let url = req.url()?;
            let mut codes: Vec<String> = Vec::new();
            for (key, value) in url.query_pairs() {
                if key == "code" {
                    for part in value.split(',') {
                        let trimmed_part = part.trim();
                        if !trimmed_part.is_empty() {
                            codes.push(trimmed_part.to_string());
                        }
                    }
                }
            }
            if codes.is_empty() {
                return Response::error("Missing stock code query parameter", 400);
            }
            let futures = codes.iter().map(|code| scrape_dynamically(code));
            let results = futures::future::join_all(futures).await;

            let mut response_data = Vec::new();
            for result in results {
                match result {
                    Ok(data) => match serde_json::to_value(data) {
                        Ok(v) => response_data.push(v),
                        Err(e) => response_data.push(serde_json::json!({ "error": format!("serialization error: {}", e) })),
                    },
                    Err(e) => response_data.push(serde_json::json!({ "error": e.to_string() })),
                }
            }
            Response::from_json(&response_data)
        })
        .get_async("/discover-data", |req, _ctx| async move {
            let url = req.url()?;
            let code = match url.query_pairs().find(|(key, _)| key == "code") {
                Some((_, value)) => value.to_string(),
                None => return Response::error("Missing 'code' query parameter", 400),
            };

            // The original logic used discover_data, but the other endpoints now use a more advanced
            // routing. To be consistent, we'll use the same advanced routing here.
            let discovered = if code.starts_with('^') {
                discover_index_data(&code).await
            } else if code.ends_with("=X") {
                discover_currency_data(&code).await
            } else {
                discover_data(&code).await
            };

            match discovered {
                Ok(results) => Response::from_json(&results),
                Err(e) => Response::error(format!("Failed to discover data: {}", e), 500),
            }
        })
            .get_async("/generate-selectors", |req, _ctx| async move {
            let url = req.url()?;
            let mut target_url = None;
            let mut target_text = None;
            for (key, value) in url.query_pairs() {
                match key.as_ref() {
                    "url" => target_url = Some(value.to_string()),
                    "text" => target_text = Some(value.to_string()),
                    _ => {}
                }
            }
            let (target_url, target_text) = match (target_url, target_text) {
                (Some(u), Some(t)) => (u, t),
                _ => return Response::error("Missing 'url' and 'text' query parameters", 400),
            };
            let mut res = match Fetch::Url(Url::parse(&target_url)?).send().await {
                Ok(res) => res,
                Err(e) => return Response::error(format!("Failed to fetch URL: {}", e), 500),
            };
            let html = match res.text().await {
                Ok(html) => html,
                Err(e) => return Response::error(format!("Failed to read response text: {}", e), 500),
            };

            let selectors = generate_selector_candidates(&html, &target_text);
            Response::from_json(&selectors)
        })
        .get_async("/verify-selector", |req, _ctx| async move {
            let url = req.url()?;
            let mut target_url = None;
            let mut selector_str = None;
            for (key, value) in url.query_pairs() {
                match key.as_ref() {
                    "url" => target_url = Some(value.to_string()),
                    "selector" => selector_str = Some(value.to_string()),
                    _ => {}
                }
            }
            let (target_url, selector_str) = match (target_url, selector_str) {
                (Some(u), Some(s)) => (u, s),
                _ => return Response::error("Missing 'url' and 'selector' query parameters", 400),
            };
            let mut res = match Fetch::Url(Url::parse(&target_url)?).send().await {
                Ok(res) => res,
                Err(e) => return Response::error(format!("Failed to fetch URL: {}", e), 500),
            };
            let html = match res.text().await {
                Ok(html) => html,
                Err(e) => return Response::error(format!("Failed to read response text: {}", e), 500),
            };

            let document = Html::parse_document(&html);
            let mut result = VerificationResult {
                url: target_url,
                selector: selector_str.clone(),
                is_valid_syntax: false,
                error_message: None,
                match_count: 0,
                matches: vec![],
            };

            match Selector::parse(&selector_str) {
                Ok(selector) => {
                    result.is_valid_syntax = true;
                    let matches: Vec<ElementRef> = document.select(&selector).collect();
                    result.match_count = matches.len();
                    for element in matches.iter().take(5) {
                        result.matches.push(ElementInfo {
                            tag: element.value().name().to_string(),
                            text: element.text().collect::<String>().trim().to_string(),
                            html: element.html(),
                        });
                    }
                }
                Err(e) => {
                    result.is_valid_syntax = false;
                    result.error_message = Some(format!("{:?}", e));
                }
            };

            Response::from_json(&result)
        })
        .run(req, env)
        .await
}
