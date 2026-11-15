//! Classification done once at ingest: user agents and traffic sources.

pub mod source;
pub mod ua;

pub use source::{SourceRules, SourceType};
pub use ua::{UaCategory, UaClass};
