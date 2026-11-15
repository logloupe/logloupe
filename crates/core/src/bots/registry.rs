//! The bot registry: which user agents claim to be which bot, and which
//! operator's ranges vouch for them.

use std::path::Path;

use serde::Deserialize;

use crate::error::{Error, Result};

/// The registry shipped with the binary. `<data-dir>/bots.toml` overrides it.
pub const DEFAULT_REGISTRY: &str = include_str!("../../data/bots.toml");

#[derive(Debug, Clone, Deserialize)]
pub struct Bot {
    pub key: String,
    pub name: String,
    /// search | ai-training | ai-search | ai-user | seo-tool | monitor | social | other
    pub kind: String,
    /// Case-insensitive substrings of the user agent.
    pub ua_match: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Operator {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub min_prefixes: usize,
    #[serde(default)]
    pub range_urls: Vec<String>,
    /// Networks registered to the operator that its published lists leave
    /// out. Addresses inside them are verified.
    #[serde(default)]
    pub extra_ranges: Vec<String>,
    #[serde(default)]
    pub rdns: Vec<String>,
    pub bots: Vec<Bot>,
}

#[derive(Debug, Deserialize)]
struct RegistryFile {
    operator: Vec<Operator>,
}

/// A user agent's claim: which operator and which of its bots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BotId {
    pub operator: usize,
    pub bot: usize,
}

#[derive(Debug, Clone)]
pub struct Registry {
    operators: Vec<Operator>,
    /// (lowercase needle, id), in file order.
    needles: Vec<(String, BotId)>,
}

impl Registry {
    pub fn from_toml(text: &str) -> Result<Self> {
        let file: RegistryFile = toml::from_str(text)
            .map_err(|e| Error::invalid(format!("invalid bot registry: {e}")))?;
        let mut needles = Vec::new();
        let mut keys = std::collections::HashSet::new();
        for (o, operator) in file.operator.iter().enumerate() {
            for (b, bot) in operator.bots.iter().enumerate() {
                if !keys.insert(bot.key.clone()) {
                    return Err(Error::invalid(format!(
                        "invalid bot registry: bot key '{}' is used twice",
                        bot.key
                    )));
                }
                for needle in &bot.ua_match {
                    let needle = needle.to_ascii_lowercase();
                    if needle.is_empty() {
                        return Err(Error::invalid(format!(
                            "invalid bot registry: bot '{}' has an empty ua_match entry",
                            bot.key
                        )));
                    }
                    needles.push((
                        needle,
                        BotId {
                            operator: o,
                            bot: b,
                        },
                    ));
                }
            }
        }
        Ok(Registry {
            operators: file.operator,
            needles,
        })
    }

    pub fn embedded() -> Self {
        Self::from_toml(DEFAULT_REGISTRY).expect("bundled bots.toml is valid")
    }

    /// Loads `<data-dir>/bots.toml` when it exists, else the built-in registry.
    pub fn load(data_root: &Path) -> Result<Self> {
        match std::fs::read_to_string(data_root.join("bots.toml")) {
            Ok(text) => Self::from_toml(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::embedded()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn operators(&self) -> &[Operator] {
        &self.operators
    }

    pub fn operator(&self, id: BotId) -> &Operator {
        &self.operators[id.operator]
    }

    pub fn bot(&self, id: BotId) -> &Bot {
        &self.operators[id.operator].bots[id.bot]
    }

    /// Finds the bot a user agent claims to be. `lower_ua` must be lowercase.
    pub fn match_ua(&self, lower_ua: &str) -> Option<BotId> {
        self.needles
            .iter()
            .find(|(needle, _)| lower_ua.contains(needle.as_str()))
            .map(|(_, id)| *id)
    }

    pub fn find_key(&self, bot_key: &str) -> Option<BotId> {
        self.operators.iter().enumerate().find_map(|(o, operator)| {
            operator
                .bots
                .iter()
                .position(|b| b.key == bot_key)
                .map(|b| BotId {
                    operator: o,
                    bot: b,
                })
        })
    }

    /// Every bot with its operator.
    pub fn bots(&self) -> impl Iterator<Item = (&Operator, &Bot)> {
        self.operators
            .iter()
            .flat_map(|o| o.bots.iter().map(move |b| (o, b)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claimed(registry: &Registry, ua: &str) -> Option<(String, String)> {
        registry.match_ua(&ua.to_ascii_lowercase()).map(|id| {
            (
                registry.operator(id).key.clone(),
                registry.bot(id).key.clone(),
            )
        })
    }

    #[test]
    fn embedded_registry_matches_real_user_agents() {
        let r = Registry::embedded();
        let check = |ua: &str, operator: &str, bot: &str| {
            assert_eq!(
                claimed(&r, ua),
                Some((operator.to_owned(), bot.to_owned())),
                "{ua}"
            );
        };
        check(
            "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)",
            "google",
            "googlebot",
        );
        check("Googlebot-Image/1.0", "google", "googlebot");
        check(
            "AdsBot-Google (+http://www.google.com/adsbot.html)",
            "google-other",
            "adsbot-google",
        );
        check(
            "Mozilla/5.0 (compatible; Google-InspectionTool/1.0;)",
            "google-other",
            "google-inspectiontool",
        );
        check(
            "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; bingbot/2.0; +http://www.bing.com/bingbot.htm) Chrome/116.0.1938.76 Safari/537.36",
            "bing",
            "bingbot",
        );
        check(
            "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; GPTBot/1.2; +https://openai.com/gptbot)",
            "openai",
            "gptbot",
        );
        check(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36; compatible; OAI-SearchBot/1.0; +https://openai.com/searchbot",
            "openai",
            "oai-searchbot",
        );
        check(
            "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko); compatible; ChatGPT-User/1.0; +https://openai.com/bot",
            "openai",
            "chatgpt-user",
        );
        check(
            "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; ClaudeBot/1.0; +claudebot@anthropic.com)",
            "anthropic",
            "claudebot",
        );
        check(
            "Mozilla/5.0 (Linux; Android 7.0;) AppleWebKit/537.36 (HTML, like Gecko) Mobile Safari/537.36 (compatible; PetalBot;+https://webmaster.petalsearch.com/site/petalbot)",
            "huawei",
            "petalbot",
        );
        check("Monit/5.33.0", "monit", "monit");
        assert_eq!(
            claimed(
                &r,
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/157.0.0.0 Safari/537.36"
            ),
            None
        );
    }

    #[test]
    fn rejects_duplicate_keys() {
        let text = r#"
            [[operator]]
            key = "a"
            name = "A"
            bots = [{ key = "x", name = "X", kind = "other", ua_match = ["x"] }]
            [[operator]]
            key = "b"
            name = "B"
            bots = [{ key = "x", name = "X2", kind = "other", ua_match = ["y"] }]
        "#;
        assert!(Registry::from_toml(text).is_err());
    }
}
