use serde_json::Value;

/// Try to parse a vector from stdin. Accepts either:
/// - A raw JSON array of numbers: [0.1, -0.2, ...]
/// - An OpenAI-compatible response: {"data": [{"embedding": [...]}]}
pub fn read_vector_from_stdin() -> Vec<f64> {
    use std::io::Read;
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .unwrap_or_else(|e| {
            eprintln!("error reading stdin: {e}");
            std::process::exit(1);
        });

    let input = input.trim();
    if input.is_empty() {
        eprintln!("error: no vector provided on stdin");
        std::process::exit(1);
    }

    let parsed: Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error parsing vector from stdin: {e}");
            std::process::exit(1);
        }
    };

    extract_vector(&parsed)
}

/// Extract a float vector from either a raw JSON array or an OpenAI embedding response.
fn extract_vector(value: &Value) -> Vec<f64> {
    // Raw array: [0.1, -0.2, ...]
    if let Some(arr) = value.as_array() {
        return parse_float_array(arr);
    }

    // OpenAI response: {"data": [{"embedding": [...]}]}
    if let Some(embedding) = value
        .get("data")
        .and_then(|d| d.get(0))
        .and_then(|d| d.get("embedding"))
        .and_then(|e| e.as_array())
    {
        return parse_float_array(embedding);
    }

    eprintln!("error: stdin must be a JSON array of numbers or an OpenAI embedding response");
    std::process::exit(1);
}

fn parse_float_array(arr: &[Value]) -> Vec<f64> {
    arr.iter()
        .enumerate()
        .map(|(i, v)| {
            v.as_f64().unwrap_or_else(|| {
                eprintln!("error: vector element {i} is not a number: {v}");
                std::process::exit(1);
            })
        })
        .collect()
}

/// Call the OpenAI embeddings API to generate a vector from text.
pub fn openai_embed(text: &str, model: &str) -> Vec<f64> {
    let api_key = match std::env::var("OPENAI_API_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => {
            eprintln!("error: OPENAI_API_KEY environment variable is not set");
            std::process::exit(1);
        }
    };

    let body = serde_json::json!({
        "input": text,
        "model": model,
    });

    let client = reqwest::blocking::Client::new();
    let req = client
        .post("https://api.openai.com/v1/embeddings")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body);
    let (status, resp_body) = match crate::util::send_debug(&client, req, Some(&body)) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("error connecting to OpenAI API: {e}");
            std::process::exit(1);
        }
    };

    if !status.is_success() {
        let message = serde_json::from_str::<Value>(&resp_body)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
            .unwrap_or(resp_body);
        eprintln!("error from OpenAI API ({status}): {message}");
        std::process::exit(1);
    }

    let parsed: Value = match serde_json::from_str(&resp_body) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error parsing OpenAI response: {e}");
            std::process::exit(1);
        }
    };

    extract_vector(&parsed)
}

/// Format a vector as a SQL ARRAY literal: ARRAY[0.1,-0.2,...]
pub fn vector_to_sql(vec: &[f64]) -> String {
    format!(
        "ARRAY[{}]",
        vec.iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(",")
    )
}
