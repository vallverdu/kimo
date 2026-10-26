//! What tokens would cost at API list prices, per model (US dollars per million tokens).
//! Subscriptions are not billed per token: this is what the same work costs on the API.

use super::usage::Tokens;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    /// Cache writes with a 5-minute lifetime (1-hour writes cost twice the input price).
    pub cache_write: f64,
    pub cache_read: f64,
}

const fn claude(input: f64, output: f64, cache_read: f64) -> Price {
    Price {
        input,
        output,
        cache_write: input * 1.25,
        cache_read,
    }
}

const fn openai(input: f64, output: f64, cache_read: f64) -> Price {
    Price {
        input,
        output,
        cache_write: input,
        cache_read,
    }
}

/// The list price of a model id as the agents record it (`claude-opus-5-5`,
/// `claude-sonnet-4-5-20250929`, `gpt-5-codex`). Unknown ids fall back on their family.
pub fn price_of(model: &str) -> Option<Price> {
    let m = model.to_ascii_lowercase();
    let has = |s: &str| m.contains(s);
    if has("fable") || has("mythos") {
        // Fable 5.1 and Mythos 5.1 read the cache at a quarter of Fable 5's price.
        return Some(if has("5-1") || has("5.1") {
            claude(10.0, 50.0, 0.25)
        } else {
            claude(10.0, 50.0, 1.0)
        });
    }
    if has("opus") {
        return Some(if has("opus-5-5") || has("opus-5.5") {
            claude(4.0, 20.0, 0.20)
        } else if has("opus-4-1") || has("opus-4-0") || has("opus-4-2") || has("3-opus") {
            // Opus 4.1, Opus 4 (claude-opus-4-20250514) and Opus 3.
            claude(15.0, 75.0, 1.5)
        } else {
            claude(5.0, 25.0, 0.5)
        });
    }
    if has("sonnet") {
        return Some(if has("sonnet-5") {
            claude(2.0, 10.0, 0.20)
        } else {
            claude(3.0, 15.0, 0.30)
        });
    }
    if has("haiku") {
        return Some(if has("haiku-5") {
            claude(0.10, 0.50, 0.01)
        } else if has("haiku-4") {
            claude(1.0, 5.0, 0.10)
        } else if has("3-5-haiku") {
            claude(0.80, 4.0, 0.08)
        } else {
            claude(0.25, 1.25, 0.03)
        });
    }
    if has("gpt") || has("codex") || m.starts_with('o') {
        return Some(if has("nano") {
            openai(0.05, 0.40, 0.005)
        } else if has("mini") {
            openai(0.25, 2.0, 0.025)
        } else {
            openai(1.25, 10.0, 0.125)
        });
    }
    None
}

/// Dollars for `tokens` on `model`; `cache_write_1h` of the cache writes lived an hour.
pub fn cost(model: &str, tokens: &Tokens, cache_write_1h: u64) -> f64 {
    let Some(p) = price_of(model) else {
        return 0.0;
    };
    let m = |n: u64| n as f64 / 1e6;
    let short = tokens.cache_write.saturating_sub(cache_write_1h);
    m(tokens.input) * p.input
        + m(tokens.output) * p.output
        + m(short) * p.cache_write
        + m(cache_write_1h) * p.input * 2.0
        + m(tokens.cache_read) * p.cache_read
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_by_family() {
        assert_eq!(price_of("claude-opus-5-5").unwrap().input, 4.0);
        assert_eq!(price_of("claude-opus-4-8").unwrap().output, 25.0);
        assert_eq!(price_of("claude-opus-4-1-20250805").unwrap().input, 15.0);
        assert_eq!(price_of("claude-opus-4-20250514").unwrap().input, 15.0);
        assert_eq!(price_of("claude-sonnet-5-5").unwrap().input, 2.0);
        assert_eq!(price_of("claude-sonnet-4-5-20250929").unwrap().input, 3.0);
        assert_eq!(price_of("claude-haiku-5-5").unwrap().output, 0.5);
        assert_eq!(price_of("claude-haiku-4-5").unwrap().input, 1.0);
        assert_eq!(price_of("claude-fable-5-1").unwrap().cache_read, 0.25);
        assert_eq!(price_of("claude-fable-5").unwrap().cache_read, 1.0);
        assert_eq!(price_of("gpt-5-codex").unwrap().output, 10.0);
        assert_eq!(price_of("gpt-5-mini").unwrap().input, 0.25);
        assert!(price_of("<synthetic>").is_none());
    }

    #[test]
    fn cost_of_a_message() {
        let t = Tokens {
            input: 1_000_000,
            output: 1_000_000,
            cache_write: 2_000_000,
            cache_read: 10_000_000,
        };
        // Opus 5.5: 4 + 20 + 1 M × 5 (5 min) + 1 M × 8 (1 h) + 10 M × 0.2
        let c = cost("claude-opus-5-5", &t, 1_000_000);
        assert!((c - (4.0 + 20.0 + 5.0 + 8.0 + 2.0)).abs() < 1e-9, "{c}");
        assert_eq!(cost("unknown", &t, 0), 0.0);
    }
}
