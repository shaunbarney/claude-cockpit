//! Claude model pricing: vendored defaults (offline) + optional LiteLLM refresh.
use std::collections::HashMap;

use serde_json::Value;

/// USD per token for one model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPrice {
    pub input: f64,
    pub output: f64,
    pub cache_write: f64,
    pub cache_read: f64,
}

#[derive(Debug, Clone, Default)]
pub struct PriceTable(pub HashMap<String, ModelPrice>);

impl PriceTable {
    pub fn get(&self, model: &str) -> ModelPrice {
        self.0.get(model).copied().unwrap_or(ModelPrice {
            input: 0.0,
            output: 0.0,
            cache_write: 0.0,
            cache_read: 0.0,
        })
    }
}

/// input/output USD per *million* tokens -> per-token ModelPrice.
/// Cache write = 1.25x input, cache read = 0.1x input (Anthropic 5-min ephemeral convention).
fn per_million(input_m: f64, output_m: f64) -> ModelPrice {
    let input = input_m / 1_000_000.0;
    let output = output_m / 1_000_000.0;
    ModelPrice {
        input,
        output,
        cache_write: input * 1.25,
        cache_read: input * 0.1,
    }
}

/// Vendored Claude prices (USD / 1M tokens). Source: Anthropic pricing, 2026-06.
pub fn vendored() -> PriceTable {
    let mut m = HashMap::new();
    m.insert("claude-fable-5".into(), per_million(10.0, 50.0));
    m.insert("claude-opus-4-8".into(), per_million(5.0, 25.0));
    m.insert("claude-opus-4-7".into(), per_million(5.0, 25.0));
    m.insert("claude-opus-4-6".into(), per_million(5.0, 25.0));
    m.insert("claude-opus-4-5".into(), per_million(5.0, 25.0));
    m.insert("claude-sonnet-4-6".into(), per_million(3.0, 15.0));
    m.insert("claude-haiku-4-5".into(), per_million(1.0, 5.0));
    m.insert("claude-haiku-4-5-20251001".into(), per_million(1.0, 5.0));
    PriceTable(m)
}

/// Parse a LiteLLM `model_prices_and_context_window.json` body into a PriceTable.
/// Only entries with `input_cost_per_token` are included. Cache fields default to
/// 1.25x / 0.1x input when absent.
pub fn parse_litellm(json: &str) -> PriceTable {
    let v: Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return PriceTable::default(),
    };
    let Some(obj) = v.as_object() else {
        return PriceTable::default();
    };
    let mut m = HashMap::new();
    for (model, entry) in obj {
        let Some(input) = entry.get("input_cost_per_token").and_then(|x| x.as_f64()) else {
            continue;
        };
        let output = entry
            .get("output_cost_per_token")
            .and_then(|x| x.as_f64())
            .unwrap_or(0.0);
        let cache_write = entry
            .get("cache_creation_input_token_cost")
            .and_then(|x| x.as_f64())
            .unwrap_or(input * 1.25);
        let cache_read = entry
            .get("cache_read_input_token_cost")
            .and_then(|x| x.as_f64())
            .unwrap_or(input * 0.1);
        m.insert(
            model.clone(),
            ModelPrice {
                input,
                output,
                cache_write,
                cache_read,
            },
        );
    }
    PriceTable(m)
}

/// Best-effort fetch of the live LiteLLM price table; caches to ~/.claude/.cockpit-prices.json.
/// Never blocks long (3s timeout); returns None on any failure.
pub fn fetch_litellm() -> Option<PriceTable> {
    const URL: &str = "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
    let body = ureq::get(URL)
        .timeout(std::time::Duration::from_secs(3))
        .call()
        .ok()?
        .into_string()
        .ok()?;
    if let Some(home) = crate::util::claude_home() {
        let _ = std::fs::write(home.join(".cockpit-prices.json"), &body);
    }
    let t = parse_litellm(&body);
    if t.0.is_empty() {
        None
    } else {
        Some(t)
    }
}

/// Load prices: vendored defaults overlaid with the cached LiteLLM file if present.
/// (The live refresh is triggered separately by the refresh thread; this never blocks.)
pub fn load() -> PriceTable {
    let mut table = vendored();
    if let Some(home) = crate::util::claude_home() {
        if let Ok(body) = std::fs::read_to_string(home.join(".cockpit-prices.json")) {
            for (k, v) in parse_litellm(&body).0 {
                table.0.entry(k).or_insert(v); // vendored wins for known Claude IDs
            }
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendored_has_claude_models() {
        let t = vendored();
        assert!(t.0.contains_key("claude-opus-4-8"));
        let p = t.0["claude-opus-4-8"];
        assert!(p.output > p.input);
        assert!((p.cache_read - p.input * 0.1).abs() < 1e-18);
    }

    #[test]
    fn parses_litellm_entry() {
        let j = r#"{"claude-x":{"input_cost_per_token":1e-6,"output_cost_per_token":5e-6,
            "cache_creation_input_token_cost":1.25e-6,"cache_read_input_token_cost":1e-7}}"#;
        let t = parse_litellm(j);
        assert!((t.0["claude-x"].output - 5e-6).abs() < 1e-12);
        assert!((t.0["claude-x"].cache_read - 1e-7).abs() < 1e-12);
    }
}
