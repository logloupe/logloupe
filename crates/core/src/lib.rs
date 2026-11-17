//! Logloupe engine.
//!
//! Everything lives here: parsing, enrichment, storage and reports. This crate
//! never prints, never reads CLI arguments and never assumes a GUI.

pub mod bots;
pub mod classify;
pub mod error;
pub mod format;
pub mod geo;
pub mod ingest;
pub mod maintain;
pub mod remote;
pub mod report;
pub mod source;
pub mod store;

pub use error::{Error, Result};
