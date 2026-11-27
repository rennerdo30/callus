//! Core types of Callus: the domain model, deterministic error signatures and
//! the mandatory client-side privacy scrubber.

pub mod domain;
pub mod error;
pub mod scrubber;
pub mod signature;

pub use domain::*;
pub use error::CallusError;
pub use scrubber::{RedactionCategory, RedactionHit, ScrubResult, Scrubber, ScrubberConfig};
pub use signature::{compute_signature_hash, normalize_error_template};
