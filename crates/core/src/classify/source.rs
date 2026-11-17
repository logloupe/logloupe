//! Traffic source classification from the referrer and the query string, and
//! the page / asset split.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

/// The rules shipped with the binary. A copy in the data directory overrides them.
pub const DEFAULT_RULES: &str = include_str!("../../data/sources.toml");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    Direct,
    Internal,
    Search,
    Ai,
    Social,
    Email,
    Campaign,
    Other,
}

impl SourceType {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceType::Direct => "direct",
            SourceType::Internal => "internal",
            SourceType::Search => "search",
            SourceType::Ai => "ai",
            SourceType::Social => "social",
            SourceType::Email => "email",
            SourceType::Campaign => "campaign",
            SourceType::Other => "other",
        }
    }
}

#[derive(Debug, Deserialize)]
struct RulesFile {
    ai: HostList,
    email: HostList,
    social: HostList,
    search: HostList,
    campaign: CampaignRules,
    pages: PageRules,
}

#[derive(Debug, Deserialize)]
struct HostList {
    hosts: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CampaignRules {
    params: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PageRules {
    asset_extensions: Vec<String>,
    non_page_prefixes: Vec<String>,
}

#[derive(Debug, Clone)]
enum HostPattern {
    /// `bing.com`: the host itself or any subdomain.
    Suffix(String),
    /// `google.*`: the name as a label followed by one or two TLD labels.
    AnyTld(String),
}

impl HostPattern {
    fn parse(pattern: &str) -> Self {
        let pattern = pattern.trim().to_ascii_lowercase();
        match pattern.strip_suffix(".*") {
            Some(name) => HostPattern::AnyTld(name.to_owned()),
            None => HostPattern::Suffix(pattern),
        }
    }

    fn matches(&self, host: &str) -> bool {
        match self {
            HostPattern::Suffix(suffix) => ends_with_label(host, suffix),
            HostPattern::AnyTld(name) => {
                // Drop one or two trailing labels and see if `name` is what remains at the end.
                let mut rest = host;
                for _ in 0..2 {
                    let Some((head, _tld)) = rest.rsplit_once('.') else {
                        return false;
                    };
                    if ends_with_label(head, name) {
                        return true;
                    }
                    rest = head;
                }
                false
            }
        }
    }
}

/// `host` is `suffix` or ends with `.suffix`.
fn ends_with_label(host: &str, suffix: &str) -> bool {
    host.strip_suffix(suffix)
        .is_some_and(|head| head.is_empty() || head.ends_with('.'))
}

/// Compiled source rules plus the site's own hostnames.
#[derive(Debug, Clone)]
pub struct SourceRules {
    site_hosts: Vec<String>,
    ai: Vec<HostPattern>,
    email: Vec<HostPattern>,
    social: Vec<HostPattern>,
    search: Vec<HostPattern>,
    campaign_params: Vec<String>,
    asset_extensions: Vec<String>,
    non_page_prefixes: Vec<String>,
}

impl SourceRules {
    /// Builds rules from TOML text (see `data/sources.toml`) and the site's hostnames.
    pub fn from_toml(text: &str, site_hosts: &[String]) -> Result<Self, toml::de::Error> {
        let file: RulesFile = toml::from_str(text)?;
        let compile = |list: HostList| list.hosts.iter().map(|h| HostPattern::parse(h)).collect();
        let lower = |v: Vec<String>| v.into_iter().map(|s| s.to_ascii_lowercase()).collect();
        Ok(SourceRules {
            site_hosts: site_hosts.iter().map(|h| h.to_ascii_lowercase()).collect(),
            ai: compile(file.ai),
            email: compile(file.email),
            social: compile(file.social),
            search: compile(file.search),
            campaign_params: lower(file.campaign.params),
            asset_extensions: lower(file.pages.asset_extensions),
            non_page_prefixes: file.pages.non_page_prefixes,
        })
    }

    pub fn with_defaults(site_hosts: &[String]) -> Self {
        Self::from_toml(DEFAULT_RULES, site_hosts).expect("bundled sources.toml is valid")
    }

    /// Classifies one request. `referer_host` must already be lowercase (see
    /// [`referer_host`]).
    pub fn source_type(&self, referer_host: Option<&str>, query: Option<&str>) -> SourceType {
        let host = referer_host.filter(|h| !h.is_empty());
        if let Some(host) = host
            && self.site_hosts.iter().any(|own| own == host)
        {
            return SourceType::Internal;
        }

        // Assistants often strip the referrer and tag the link instead
        // (`utm_source=chatgpt.com`), so the tag is checked against the AI list.
        let mut campaign = false;
        if let Some(query) = query {
            for (key, value) in query_pairs(query) {
                if !self
                    .campaign_params
                    .iter()
                    .any(|p| p.eq_ignore_ascii_case(key))
                {
                    continue;
                }
                campaign = true;
                if key.eq_ignore_ascii_case("utm_source") {
                    let value = value.to_ascii_lowercase();
                    if self.ai.iter().any(|p| p.matches(&value)) {
                        return SourceType::Ai;
                    }
                }
            }
        }

        if let Some(host) = host
            && self.ai.iter().any(|p| p.matches(host))
        {
            return SourceType::Ai;
        }
        if campaign {
            return SourceType::Campaign;
        }
        let Some(host) = host else {
            return SourceType::Direct;
        };
        if self.email.iter().any(|p| p.matches(host)) {
            SourceType::Email
        } else if self.social.iter().any(|p| p.matches(host)) {
            SourceType::Social
        } else if self.search.iter().any(|p| p.matches(host)) {
            SourceType::Search
        } else {
            SourceType::Other
        }
    }

    /// Lowercase file extensions that mark a static asset.
    pub fn asset_extensions(&self) -> &[String] {
        &self.asset_extensions
    }

    /// Path prefixes that are never pages.
    pub fn non_page_prefixes(&self) -> &[String] {
        &self.non_page_prefixes
    }

    /// True when the path is a static asset or an internal endpoint rather
    /// than something a visitor would call a page.
    pub fn is_asset(&self, path: &str) -> bool {
        if self
            .non_page_prefixes
            .iter()
            .any(|p| path.starts_with(p.as_str()))
        {
            return true;
        }
        let file = path.rsplit('/').next().unwrap_or(path);
        match file.rsplit_once('.') {
            Some((_, ext)) => self
                .asset_extensions
                .iter()
                .any(|known| known.eq_ignore_ascii_case(ext)),
            None => false,
        }
    }
}

/// Iterates `key=value` pairs of a raw (not URL-decoded) query string.
pub fn query_pairs(query: &str) -> impl Iterator<Item = (&str, &str)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| pair.split_once('=').unwrap_or((pair, "")))
}

/// Extracts the lowercase host from a referrer. Handles `https://host/...`,
/// `android-app://package/`, and bare `host/path` values with no scheme.
pub fn referer_host(referer: &str) -> Option<Cow<'_, str>> {
    let rest = match referer.split_once("://") {
        Some((_scheme, rest)) => rest,
        None => referer,
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    // Drop `user:pass@`.
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if let Some(v6) = host_port.strip_prefix('[') {
        v6.split_once(']').map(|(h, _)| h)?
    } else {
        host_port.split(':').next()?
    };
    let host = host.trim_end_matches('.');
    if host.is_empty()
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._:".contains(&b))
    {
        return None;
    }
    Some(if host.bytes().any(|b| b.is_ascii_uppercase()) {
        Cow::Owned(host.to_ascii_lowercase())
    } else {
        Cow::Borrowed(host)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> SourceRules {
        SourceRules::with_defaults(&["www.example.com".to_owned(), "example.com".to_owned()])
    }

    #[test]
    fn hosts() {
        let host = |r| referer_host(r).map(Cow::into_owned);
        assert_eq!(
            host("https://www.Google.com/search?q=x"),
            Some("www.google.com".into())
        );
        assert_eq!(host("www.google.com/"), Some("www.google.com".into()));
        assert_eq!(
            host("http://user:pw@example.com:8080/a"),
            Some("example.com".into())
        );
        assert_eq!(
            host("android-app://com.google.android.gm/"),
            Some("com.google.android.gm".into())
        );
        assert_eq!(
            host("https://[2001:db8::1]:443/"),
            Some("2001:db8::1".into())
        );
        assert_eq!(host("https://example.com."), Some("example.com".into()));
        assert_eq!(host("https:///nohost"), None);
        assert_eq!(host("() { :; }; echo"), None);
    }

    #[test]
    fn source_types() {
        let r = rules();
        let st = |host: Option<&str>, query: Option<&str>| r.source_type(host, query);
        assert_eq!(st(None, None), SourceType::Direct);
        assert_eq!(st(Some("www.example.com"), None), SourceType::Internal);
        assert_eq!(
            st(Some("www.example.com"), Some("gclid=abc")),
            SourceType::Internal
        );
        assert_eq!(st(Some("www.google.com"), None), SourceType::Search);
        assert_eq!(st(Some("www.google.co.uk"), None), SourceType::Search);
        assert_eq!(st(Some("google.pl"), None), SourceType::Search);
        assert_eq!(st(Some("m.baidu.com"), None), SourceType::Search);
        assert_eq!(st(Some("yandex.ru"), None), SourceType::Search);
        assert_eq!(st(Some("gemini.google.com"), None), SourceType::Ai);
        assert_eq!(st(Some("chatgpt.com"), None), SourceType::Ai);
        assert_eq!(st(None, Some("utm_source=chatgpt.com")), SourceType::Ai);
        assert_eq!(st(Some("mail.google.com"), None), SourceType::Email);
        assert_eq!(st(Some("outlook.cloud.microsoft"), None), SourceType::Email);
        assert_eq!(st(Some("m.facebook.com"), None), SourceType::Social);
        assert_eq!(st(Some("www.youtube.com"), None), SourceType::Social);
        assert_eq!(
            st(
                Some("www.google.com"),
                Some("utm_source=adwords&utm_medium=ppc")
            ),
            SourceType::Campaign
        );
        assert_eq!(st(None, Some("page=2&FBCLID=xyz")), SourceType::Campaign);
        assert_eq!(st(None, Some("page=2")), SourceType::Direct);
        assert_eq!(st(Some("other-site.example"), None), SourceType::Other);
        // Not fooled by look-alikes.
        assert_eq!(st(Some("notgoogle.com"), None), SourceType::Other);
        assert_eq!(st(Some("google.evil.example.net"), None), SourceType::Other);
    }

    #[test]
    fn assets() {
        let r = rules();
        assert!(r.is_asset("/wp-content/uploads/2024/01/photo-500x332.jpeg"));
        assert!(r.is_asset("/style.CSS"));
        assert!(r.is_asset("/wp-admin/admin-ajax.php"));
        assert!(r.is_asset("/sitemap_index.xml"));
        assert!(!r.is_asset("/"));
        assert!(!r.is_asset("/products/blue-widget/"));
        assert!(!r.is_asset("/about.html"));
        assert!(!r.is_asset("/v1.2/overview"));
    }
}
