//! Bot registry and verification against published IP ranges (spec 5.1).

pub mod ranges;
pub mod registry;

pub use ranges::{Assessment, Ranges, UpdateOutcome, Verdict};
pub use registry::{Bot, Operator, Registry};

use crate::classify::ua::{self, UaCategory, UaClass};
use registry::BotId;

/// Classifies a user agent: the bot registry first, then the general rules.
/// Returns the registered bot it claims to be, if any.
pub fn classify(registry: &Registry, ua: &str) -> (UaClass, Option<BotId>) {
    let lower = ua.to_ascii_lowercase();
    match registry.match_ua(&lower) {
        Some(id) => (
            UaClass {
                category: UaCategory::Bot,
                family: Some(registry.bot(id).name.clone()),
            },
            Some(id),
        ),
        None => (ua::classify(Some(ua)), None),
    }
}
