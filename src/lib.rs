//! Personal Groktor — local health-data insight pipeline.
//!
//! Ingest wearable exports → normalize → store → rule analysis → optional Grok narrative.

pub mod analyze;
pub mod error;
pub mod ingest;
pub mod llm;
pub mod normalize;
pub mod report;
pub mod schema;
pub mod store;

pub use error::{GroktorError, Result};
pub use store::Store;
