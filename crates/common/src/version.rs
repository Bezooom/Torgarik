/// Semantic version of the trading bot
pub const VERSION: &str = env!("CARGO_PKG_VERSION", "0.1.0");

/// Build timestamp (approximate at compile time)
pub const BUILD_TIMESTAMP: &str = "2026-08-06";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_not_empty() {
        assert!(!VERSION.is_empty());
    }
}
