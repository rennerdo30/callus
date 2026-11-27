//! Shared scrubber test helpers. Fake secrets are split with `concat!` so no
//! scanner mistakes this public source for a leaked credential.

// Each test binary uses its own subset.
#![allow(dead_code, unreachable_pub, clippy::unwrap_used)]

use callus_core::{RedactionCategory, Scrubber, ScrubberConfig};

pub const AWS: &str = concat!("AKIA", "IOSFODNN7EXAMPLE");
pub const AWS_TEMP: &str = concat!("ASIA", "Y34FZKBOKMUTVV7A");
pub const GITHUB: &str = concat!("ghp_", "1a2B3c4D5e6F7g8H9i0JkLmNoPqRsTuVwXyZ");
pub const GITHUB_FINE: &str = concat!("github_pat_", "11ABCDEFG0aBcDeFgHiJkL_mNoPqRsTuVwXyZ012345");
pub const GITLAB: &str = concat!("glpat-", "x9Yz8Wv7Ut6Sr5Qp4On3");
pub const OPENAI: &str = concat!("sk-", "abc123DEF456ghi789JKL012mno345PQR");
pub const OPENAI_PROJECT: &str = concat!("sk-", "proj-Zx_9yW-8vU7tS6rQ5pO4nM3lK2");
pub const ANTHROPIC: &str = concat!("sk-", "ant-api03-Qw3rTy-UiOp_AsDfGh12345678");
pub const GOOGLE: &str = concat!("AIza", "SyD-1234567890abcdefghijklmnopqrstu");
pub const SLACK: &str = concat!("xox", "b-123456789012-abcdefABCDEF");
pub const STRIPE: &str = concat!("sk_", "live_51HxYzAbCdEfGhIjKlMnOp");
pub const STRIPE_RESTRICTED: &str = concat!("rk_", "live_51HxYzAbCdEfGhIjKlMnOp");
pub const JWT: &str = concat!(
    "eyJ",
    "hbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U"
);
pub const RANDOM: &str = concat!("q8Zr2Lm9Xv4T", "b7Nc1Kd6Pw3Hs5Jf");

pub const REPO_ROOT: &str = r"C:\Users\alice\code\widget";
pub const HOME: &str = r"C:\Users\alice";

pub fn config() -> ScrubberConfig {
    ScrubberConfig {
        repo_root: Some(REPO_ROOT.to_owned()),
        home_dir: Some(HOME.to_owned()),
        extra_hostnames: vec!["buildbox".to_owned()],
        deny_regexes: vec![r"(?i)project[- ]falcon".to_owned()],
    }
}

pub fn scrubber() -> Scrubber {
    Scrubber::new(&config()).unwrap()
}

pub fn scrub(text: &str) -> String {
    scrubber().scrub_text(text).scrubbed
}

pub fn count(scrubber: &Scrubber, text: &str, category: RedactionCategory) -> usize {
    scrubber
        .scrub_text(text)
        .redactions
        .iter()
        .find(|hit| hit.category == category)
        .map_or(0, |hit| hit.count)
}

/// Every `(input, expected)` pair scrubs as stated, and stays so when repeated.
pub fn expect(cases: &[(&str, &str)]) {
    let scrubber = scrubber();
    for &(input, expected) in cases {
        let once = scrubber.scrub_text(input).scrubbed;
        assert_eq!(once, expected, "input: {input:?}");
        let twice = scrubber.scrub_text(&once);
        assert_eq!(twice.scrubbed, once, "not idempotent: {input:?}");
        assert!(twice.redactions.is_empty(), "recounted: {input:?}");
    }
}

/// None of these may be touched.
pub fn keep(cases: &[&str]) {
    let scrubber = scrubber();
    for &text in cases {
        let result = scrubber.scrub_text(text);
        assert_eq!(result.scrubbed, text);
        assert!(result.redactions.is_empty(), "{text:?}: {result:?}");
    }
}
