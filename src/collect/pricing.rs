//! Claude model pricing: vendored defaults (offline) + optional LiteLLM refresh.
use std::collections::HashMap;

use serde_json::Value;

/// USD per token for one model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPrice {
    pub input: f64,
    pub output: f64,
    /// 5-minute ephemeral cache write (1.25x input on Anthropic).
    pub cache_write: f64,
    /// 1-hour ephemeral cache write (2x input on Anthropic).
    pub cache_write_1h: f64,
    pub cache_read: f64,
}

/// How a model name was matched to a price.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Match {
    /// The exact model ID is in the table.
    Exact,
    /// Matched after stripping a date / `[1m]` / provider decoration, or by
    /// trimming trailing version segments (e.g. `claude-opus-5-5-x` -> `claude-opus-5-5`).
    Alias,
}

#[derive(Debug, Clone, Default)]
pub struct PriceTable(pub HashMap<String, ModelPrice>);

impl PriceTable {
    /// Price for `model`, or `None` when no exact or alias match exists.
    pub fn resolve(&self, model: &str) -> Option<(ModelPrice, Match)> {
        if let Some(p) = self.0.get(model) {
            return Some((*p, Match::Exact));
        }
        candidates(model)
            .into_iter()
            .find_map(|c| self.0.get(&c).map(|p| (*p, Match::Alias)))
    }

    /// Price for `model`; zero when unknown (callers should flag it via
    /// [`PriceTable::resolve`] rather than silently reporting $0).
    pub fn get(&self, model: &str) -> ModelPrice {
        self.resolve(model).map(|(p, _)| p).unwrap_or(ModelPrice {
            input: 0.0,
            output: 0.0,
            cache_write: 0.0,
            cache_write_1h: 0.0,
            cache_read: 0.0,
        })
    }
}

/// Lookup keys to try for a model ID that has no exact entry, most specific
/// first: decoration stripped (provider prefix, `[1m]`, `@date`, `-YYYYMMDD`,
/// `-v1:0`), then trailing `-segment`s trimmed while a `claude-<family>-<n>`
/// remains — so a dated or suffixed variant resolves to its base model.
fn candidates(model: &str) -> Vec<String> {
    let mut m = model.trim().to_ascii_lowercase();
    if let Some(i) = m.find("claude-") {
        m = m[i..].to_string(); // drop "anthropic." / "us.anthropic." / "anthropic/"
    }
    if let Some(i) = m.find('[') {
        m.truncate(i); // "[1m]" context-window tag
    }
    if let Some(i) = m.find('@') {
        m.truncate(i); // Vertex "@20251001"
    }
    if let Some(i) = m.find(":") {
        m.truncate(i); // Bedrock ":0"
    }
    let mut segs: Vec<&str> = m.split('-').collect();
    while segs
        .last()
        .is_some_and(|s| (s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit())) || *s == "v1")
    {
        segs.pop();
    }
    let mut out = Vec::new();
    // Keep at least "claude-<family>-<major>".
    while segs.len() >= 3 {
        out.push(segs.join("-"));
        segs.pop();
    }
    out
}

/// input/output USD per *million* tokens -> per-token ModelPrice.
/// Cache write = 1.25x input (5-min) / 2x input (1-hour); cache read = 0.1x input.
fn per_million(input_m: f64, output_m: f64) -> ModelPrice {
    with_cache_read(input_m, output_m, input_m * 0.1)
}

/// Like [`per_million`] but with an explicit cache-read price per million —
/// newer models price cache reads below the old 0.1x convention.
fn with_cache_read(input_m: f64, output_m: f64, cache_read_m: f64) -> ModelPrice {
    let input = input_m / 1_000_000.0;
    ModelPrice {
        input,
        output: output_m / 1_000_000.0,
        cache_write: input * 1.25,
        cache_write_1h: input * 2.0,
        cache_read: cache_read_m / 1_000_000.0,
    }
}

/// Vendored Claude prices (USD / 1M tokens). Source: Anthropic first-party API
/// pricing as of 2026-10 (cross-checked against LiteLLM). Fast mode and the
/// Haiku 5.5 >100K-token prompt tier are not modelled.
pub fn vendored() -> PriceTable {
    let mut m = HashMap::new();
    m.insert("claude-fable-5-1".into(), with_cache_read(10.0, 50.0, 0.25));
    m.insert("claude-fable-5".into(), per_million(10.0, 50.0));
    m.insert("claude-opus-5-5".into(), with_cache_read(4.0, 20.0, 0.20));
    m.insert("claude-opus-5".into(), per_million(5.0, 25.0));
    m.insert("claude-opus-4-8".into(), per_million(5.0, 25.0));
    m.insert("claude-opus-4-7".into(), per_million(5.0, 25.0));
    m.insert("claude-opus-4-6".into(), per_million(5.0, 25.0));
    m.insert("claude-opus-4-5".into(), per_million(5.0, 25.0));
    m.insert("claude-sonnet-5-5".into(), with_cache_read(2.0, 10.0, 0.20));
    m.insert("claude-sonnet-5".into(), with_cache_read(2.0, 10.0, 0.20));
    m.insert("claude-sonnet-4-6".into(), per_million(3.0, 15.0));
    // Haiku 5.5 cache-read price is not published separately; 0.1x input assumed.
    m.insert("claude-haiku-5-5".into(), per_million(0.10, 0.50));
    m.insert("claude-haiku-4-5".into(), per_million(1.0, 5.0));
    PriceTable(m)
}

/// Parse a LiteLLM `model_prices_and_context_window.json` body into a PriceTable.
/// Only entries with `input_cost_per_token` are included. Cache fields default to
/// 1.25x (5-min write) / 2x (1-hour write) / 0.1x (read) input when absent.
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
        let f = |k: &str, default: f64| entry.get(k).and_then(|x| x.as_f64()).unwrap_or(default);
        m.insert(
            model.clone(),
            ModelPrice {
                input,
                output: f("output_cost_per_token", 0.0),
                cache_write: f("cache_creation_input_token_cost", input * 1.25),
                cache_write_1h: f("cache_creation_input_token_cost_above_1hr", input * 2.0),
                cache_read: f("cache_read_input_token_cost", input * 0.1),
            },
        );
    }
    PriceTable(m)
}

/// Best-effort fetch of the live LiteLLM price table; caches to ~/.claude/.cockpit-prices.json.
/// The body is ~3 MB, so allow 15 s — this runs once on the background cost
/// thread and never blocks the UI. Returns None on any failure.
pub fn fetch_litellm() -> Option<PriceTable> {
    const URL: &str = "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
    let body = ureq::get(URL)
        .timeout(std::time::Duration::from_secs(15))
        .call()
        .ok()?
        .into_string()
        .ok()?;
    let t = parse_litellm(&body);
    if t.0.is_empty() {
        return None;
    }
    if let Some(home) = crate::util::claude_home() {
        let _ = std::fs::write(home.join(".cockpit-prices.json"), &body);
    }
    Some(t)
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
        assert!((p.cache_write_1h - p.input * 2.0).abs() < 1e-18);
        for m in [
            "claude-opus-5",
            "claude-opus-5-5",
            "claude-sonnet-5",
            "claude-sonnet-5-5",
            "claude-haiku-5-5",
            "claude-fable-5-1",
        ] {
            assert!(t.0.contains_key(m), "missing {m}");
        }
        assert!((t.0["claude-opus-5-5"].input - 4e-6).abs() < 1e-15);
    }

    #[test]
    fn parses_litellm_entry() {
        let j = r#"{"claude-x":{"input_cost_per_token":1e-6,"output_cost_per_token":5e-6,
            "cache_creation_input_token_cost":1.25e-6,"cache_read_input_token_cost":1e-7,
            "cache_creation_input_token_cost_above_1hr":3e-6}}"#;
        let t = parse_litellm(j);
        assert!((t.0["claude-x"].output - 5e-6).abs() < 1e-12);
        assert!((t.0["claude-x"].cache_read - 1e-7).abs() < 1e-12);
        assert!((t.0["claude-x"].cache_write_1h - 3e-6).abs() < 1e-12);
        // Missing 1h field defaults to 2x input.
        let t2 = parse_litellm(r#"{"m":{"input_cost_per_token":1e-6}}"#);
        assert!((t2.0["m"].cache_write_1h - 2e-6).abs() < 1e-12);
    }

    #[test]
    fn alias_fallback_strips_decorations() {
        let t = vendored();
        let opus = t.0["claude-opus-5-5"];
        for name in [
            "claude-opus-5-5[1m]",
            "claude-opus-5-5-20260901",
            "us.anthropic.claude-opus-5-5-v1:0",
            "claude-opus-5-5@20260901",
        ] {
            assert_eq!(t.resolve(name), Some((opus, Match::Alias)), "{name}");
        }
        assert_eq!(
            t.resolve("claude-haiku-4-5-20251001").map(|(_, m)| m),
            Some(Match::Alias)
        );
        assert_eq!(
            t.resolve("claude-opus-5-5").map(|(_, m)| m),
            Some(Match::Exact)
        );
        // Unknown families stay unpriced instead of borrowing a random price.
        assert_eq!(t.resolve("claude-nova-1"), None);
        assert_eq!(t.resolve("gpt-x"), None);
        assert_eq!(t.get("gpt-x").input, 0.0);
    }
}
