//! Per-model Anthropic API pricing, for estimating a session's dollar cost from
//! its token usage.
//!
//! Rates are keyed by model family (the same Opus/Sonnet/Haiku/Fable buckets the
//! rest of this crate uses) and are **approximate, as of 2026-07** — Anthropic
//! list pricing changes over time, so these are an estimate and the numbers live
//! in one place here to keep them easy to update. An unrecognized model
//! contributes zero cost rather than a guessed rate.

/// Per-million-token USD rates for one model family.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_write_per_mtok: f64,
    pub cache_read_per_mtok: f64,
}

impl ModelPricing {
    /// USD cost of one message's usage components at these rates.
    pub fn cost(&self, input: i64, output: i64, cache_read: i64, cache_write: i64) -> f64 {
        (input as f64 * self.input_per_mtok
            + output as f64 * self.output_per_mtok
            + cache_read as f64 * self.cache_read_per_mtok
            + cache_write as f64 * self.cache_write_per_mtok)
            / 1_000_000.0
    }
}

/// Pricing for a model string, or `None` when the family isn't recognized.
/// Cache rates follow Anthropic's standard 5-minute cache: writes cost 1.25× the
/// input rate, reads 0.1×.
pub fn pricing_for(model: &str) -> Option<ModelPricing> {
    let (input, output) = if model.contains("opus") {
        (5.0, 25.0)
    } else if model.contains("sonnet") {
        (3.0, 15.0)
    } else if model.contains("haiku") {
        (1.0, 5.0)
    } else if model.contains("fable") {
        (10.0, 50.0)
    } else {
        return None;
    };
    Some(ModelPricing {
        input_per_mtok: input,
        output_per_mtok: output,
        cache_write_per_mtok: input * 1.25,
        cache_read_per_mtok: input * 0.1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_model_has_no_price() {
        assert_eq!(pricing_for("gpt-4-turbo"), None);
        assert_eq!(pricing_for(""), None);
    }

    #[test]
    fn cache_rates_derive_from_input() {
        let p = pricing_for("claude-opus-4").unwrap();
        assert_eq!(p.cache_write_per_mtok, 6.25);
        assert_eq!(p.cache_read_per_mtok, 0.5);
    }

    #[test]
    fn opus_cost_input_and_output() {
        let p = pricing_for("claude-opus-4-20250514").unwrap();
        // (1000 * 5 + 500 * 25) / 1e6
        assert!((p.cost(1000, 500, 0, 0) - 0.0175).abs() < 1e-9);
    }

    #[test]
    fn sonnet_cost_with_cache() {
        let p = pricing_for("claude-sonnet-4").unwrap();
        // (1000*3 + 500*15 + 800*0.3 + 200*3.75) / 1e6
        assert!((p.cost(1000, 500, 800, 200) - 0.01149).abs() < 1e-9);
    }

    #[test]
    fn haiku_cost_input_and_output() {
        let p = pricing_for("claude-3-haiku").unwrap();
        // (1000 * 1 + 500 * 5) / 1e6
        assert!((p.cost(1000, 500, 0, 0) - 0.0035).abs() < 1e-9);
    }
}
