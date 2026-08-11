//! Personal Groktor — local health-data insight pipeline.
//!
//! Ingest wearable exports → normalize → store → rule analysis → optional Grok narrative.
//! Brief + N=1 lab on top of the same health DB.

pub mod analyze;
pub mod brief;
pub mod error;
pub mod ingest;
pub mod lab;
pub mod llm;
pub mod normalize;
pub mod report;
pub mod schema;
pub mod store;

pub use error::{GroktorError, Result};
pub use store::Store;
