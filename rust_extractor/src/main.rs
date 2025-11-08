use clap::Parser;
use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::error::Error;

/// Command line arguments
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// The URL to fetch data from
    url: String,

    /// Keys that the target object must contain. Can be specified multiple times.
    #[arg(long = "key", required = true)]
    keys: Vec<String>,
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

/// Recursively generates struct definitions and returns them as a Vec of strings.
fn generate_structs(
    name: &str,
    value: &Value,
    all_defs: &mut BTreeMap<String, String>,
) -> Vec<String> {
    if let Value::Object(map) = value {
        let struct_name = to_pascal_case(name);
        if all_defs.contains_key(&struct_name) {
            return Vec::new(); // Already generated, return empty.
        }

        let mut fields = Vec::new();
        let mut nested_defs_to_print = Vec::new();

        for (k, v) in map {
            let field_name = k;
            let field_type = match v {
                Value::Object(_) => {
                    let nested_name = to_pascal_case(field_name);
                    nested_defs_to_print.extend(generate_structs(&nested_name, v, all_defs));
                    format!("Option<{}>", nested_name)
                }
                Value::Array(arr) => {
                    if let Some(first) = arr.first() {
                        match first {
                            Value::Object(_) => {
                                let singular_name = field_name.strip_suffix('s').unwrap_or(field_name);
                                let nested_name = to_pascal_case(singular_name);
                                nested_defs_to_print.extend(generate_structs(&nested_name, first, all_defs));
                                format!("Option<Vec<{}>>", nested_name)
                            }
                            _ => "Option<Vec<String>>".to_string(),
                        }
                    } else {
                        "Option<Vec<String>>".to_string()
                    }
                }
                Value::String(_) => "Option<String>".to_string(),
                Value::Number(n) => {
                    if n.is_i64() { "Option<i64>".to_string() }
                    else if n.is_u64() { "Option<u64>".to_string() }
                    else { "Option<f64>".to_string() }
                }
                Value::Bool(_) => "Option<bool>".to_string(),
                Value::Null => "Option<serde_json::Value>".to_string(),
            };
            fields.push(format!("    pub {}: {},", field_name, field_type));
        }

        let struct_def = format!(
            "#[derive(Debug, serde::Deserialize)]\n\
             pub struct {} {{\n{}\n}}",
            struct_name,
            fields.join("\n")
        );

        all_defs.insert(struct_name.clone(), struct_def.clone());
        
        let mut result = vec![struct_def];
        result.extend(nested_defs_to_print);
        result
    } else {
        Vec::new()
    }
}

fn to_pascal_case(s: &str) -> String {
    s.split(|c: char| c == '_' || !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
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
    let args = Args::parse();
    let body = reqwest::get(&args.url).await?.text().await?;
    let re = Regex::new(r"(?s)window\.__PRELOADED_STATE__\s*=\s*(.*?)</script>")?;

    if let Some(caps) = re.captures(&body) {
        if let Some(json_match) = caps.get(1) {
            let mut json_str = json_match.as_str().trim();
            if json_str.ends_with(';') {
                json_str = &json_str[..json_str.len() - 1];
            }

            let data: Value = serde_json::from_str(json_str)?;
            let mut found_paths = Vec::new();
            find_object_paths(&data, &args.keys, &mut Vec::new(), &mut found_paths);

            if found_paths.is_empty() {
                println!("No objects found with the specified keys: {:?}", args.keys);
                return Ok(());
            }

            println!("================ FOUND OBJECTS AND THEIR STRUCT DEFINITIONS ================");
            let mut all_defs = BTreeMap::new();
            let mut name_counts = HashMap::new();

            for path in found_paths.iter() {
                let mut target_obj = &data;
                for &key in path.iter() {
                    target_obj = &target_obj[key];
                }

                let object_key = *path.last().unwrap_or(&"Root");
                let base_name = to_pascal_case(object_key);
                
                let count = name_counts.entry(base_name.clone()).or_insert(0);
                *count += 1;
                
                let struct_name = if *count > 1 {
                    format!("{}{}", base_name, count)
                } else {
                    base_name
                };

                let indent_step = "  ";
                for (i, key) in path.iter().enumerate() {
                    println!("{}{:?}: {{", indent_step.repeat(i), key);
                }

                let defs_to_print = generate_structs(&struct_name, target_obj, &mut all_defs);
                let inner_indent = indent_step.repeat(path.len());
                for def in defs_to_print {
                    let indented_def = def.lines().map(|line| format!("{}{}", inner_indent, line)).collect::<Vec<_>>().join("\n");
                    println!("{}\n", indented_def);
                }

                for i in (0..path.len()).rev() {
                    println!("{}}}", indent_step.repeat(i));
                }
                println!();
            }
        } else {
             println!("Could not find the JSON data in the script tag.");
        }
    }
    else {
        println!("Could not find window.__PRELOADED_STATE__ script tag.");
    }

    Ok(())
}
