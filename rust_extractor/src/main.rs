use serde_json::Value;
use regex::Regex;
use std::collections::BTreeMap;
use std::error::Error;

/// 再帰的に構造体を生成
fn generate_structs(name: &str, value: &Value, defs: &mut BTreeMap<String, String>) {
    if let Value::Object(map) = value {
        let struct_name = to_pascal_case(name);
        let mut fields = vec![];

        for (k, v) in map {
            let field_name = k;
            let field_type = match v {
                Value::Object(_) => {
                    let nested_name = to_pascal_case(field_name);
                    generate_structs(field_name, v, defs);
                    format!("Option<{}>", nested_name)
                }
                Value::Array(arr) => {
                    if let Some(first) = arr.first() {
                        match first {
                            Value::Object(_) => {
                                let nested_name = to_pascal_case(field_name);
                                generate_structs(field_name, first, defs);
                                format!("Option<Vec<{}>>", nested_name)
                            }
                            _ => "Option<Vec<String>>".to_string(),
                        }
                    } else {
                        "Option<Vec<String>>".to_string()
                    }
                }
                Value::String(_) => "Option<String>".to_string(),
                Value::Number(_) => "Option<f64>".to_string(),
                Value::Bool(_) => "Option<bool>".to_string(),
                Value::Null => "Option<String>".to_string(),
            };
            fields.push(format!("    pub {}: {},", field_name, field_type));
        }

        let struct_def = format!(
            "#[derive(Debug, serde::Deserialize)]\n\
             pub struct {} {{\n{}\n}}",
            struct_name,
            fields.join("\n")
        );

        defs.insert(struct_name, struct_def);
    }
}

fn to_pascal_case(s: &str) -> String {
    s.split('_')
        .map(|part| {
            let mut c = part.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        })
        .collect::<String>()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let url = "https://finance.yahoo.co.jp/quote/5016.T";
    let body = reqwest::get(url).await?.text().await?;

    let re = Regex::new(r"(?s)window\.__PRELOADED_STATE__\s*=\s*(.*?)</script>")?;
    if let Some(caps) = re.captures(&body) {
        if let Some(json_match) = caps.get(1) {
            let mut json_str = json_match.as_str().trim();
            if json_str.ends_with(';') {
                json_str = &json_str[..json_str.len() - 1];
            }

            let data: Value = serde_json::from_str(json_str)?;

            let mut defs = BTreeMap::new();
            generate_structs("root", &data, &mut defs);

            println!("================ STRUCT DEFINITIONS ================");
            for def in defs.values() {
                println!("{}\n", def);
            }
        }
    }

    Ok(())
}
