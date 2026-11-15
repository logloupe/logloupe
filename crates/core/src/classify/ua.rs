//! User-agent classification. Each distinct UA string is classified once.
//!
//! Order matters: many crawlers send a full browser string with their own
//! token added (`... Chrome/119.0 Safari/537.36 (compatible; Amazonbot/0.1; ...)`),
//! so bot rules run before browser rules.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UaCategory {
    Browser,
    Bot,
    Tool,
    Library,
    Empty,
    Unknown,
}

impl UaCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            UaCategory::Browser => "browser",
            UaCategory::Bot => "bot",
            UaCategory::Tool => "tool",
            UaCategory::Library => "library",
            UaCategory::Empty => "empty",
            UaCategory::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "browser" => UaCategory::Browser,
            "bot" => UaCategory::Bot,
            "tool" => UaCategory::Tool,
            "library" => UaCategory::Library,
            "empty" => UaCategory::Empty,
            "unknown" => UaCategory::Unknown,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UaClass {
    pub category: UaCategory,
    /// `Chrome`, `Googlebot`, `curl`, ... `None` when nothing recognisable.
    pub family: Option<String>,
}

/// Known bots: lowercase needle, display name. More specific needles first.
const BOTS: &[(&str, &str)] = &[
    ("googlebot", "Googlebot"),
    ("adsbot-google", "AdsBot-Google"),
    ("mediapartners-google", "Mediapartners-Google"),
    ("google-inspectiontool", "Google-InspectionTool"),
    ("googleother", "GoogleOther"),
    ("google-extended", "Google-Extended"),
    ("storebot-google", "Storebot-Google"),
    ("feedfetcher-google", "FeedFetcher-Google"),
    ("bingbot", "Bingbot"),
    ("bingpreview", "BingPreview"),
    ("adidxbot", "AdIdxBot"),
    ("msnbot", "MSNBot"),
    ("oai-searchbot", "OAI-SearchBot"),
    ("oai-adsbot", "OAI-AdsBot"),
    ("chatgpt-user", "ChatGPT-User"),
    ("gptbot", "GPTBot"),
    ("claudebot", "ClaudeBot"),
    ("claude-searchbot", "Claude-SearchBot"),
    ("claude-user", "Claude-User"),
    ("perplexitybot", "PerplexityBot"),
    ("perplexity-user", "Perplexity-User"),
    ("applebot", "Applebot"),
    ("amazonbot", "Amazonbot"),
    ("meta-externalagent", "Meta-ExternalAgent"),
    ("meta-externalfetcher", "Meta-ExternalFetcher"),
    ("meta-webindexer", "Meta-WebIndexer"),
    ("facebookexternalhit", "FacebookExternalHit"),
    ("facebookbot", "FacebookBot"),
    ("duckduckbot", "DuckDuckBot"),
    ("duckassistbot", "DuckAssistBot"),
    ("yandex", "YandexBot"),
    ("baiduspider", "Baiduspider"),
    ("ccbot", "CCBot"),
    ("bytespider", "Bytespider"),
    ("youbot", "YouBot"),
    ("exasearchbot", "ExaSearchBot"),
    ("ahrefsbot", "AhrefsBot"),
    ("semrushbot", "SemrushBot"),
    ("mj12bot", "MJ12bot"),
    ("dotbot", "DotBot"),
    ("dataforseobot", "DataForSeoBot"),
    ("screaming frog", "Screaming Frog"),
    ("petalbot", "PetalBot"),
    ("sogou", "Sogou"),
    ("seznambot", "SeznamBot"),
    ("twitterbot", "Twitterbot"),
    ("linkedinbot", "LinkedInBot"),
    ("pinterestbot", "Pinterestbot"),
    ("slackbot", "Slackbot"),
    ("discordbot", "Discordbot"),
    ("telegrambot", "TelegramBot"),
    ("whatsapp", "WhatsApp"),
    ("uptimerobot", "UptimeRobot"),
    ("pingdom", "Pingdom"),
    ("statuscake", "StatusCake"),
    ("monit/", "Monit"),
    ("ia_archiver", "ia_archiver"),
    ("archive.org_bot", "archive.org_bot"),
];

/// Markers that make an otherwise unknown agent a bot.
const BOT_MARKERS: &[&str] = &["bot", "crawl", "spider", "slurp", "+http", "scrape"];

/// Command-line and automation tools: lowercase needle, display name.
const TOOLS: &[(&str, &str)] = &[
    ("curl/", "curl"),
    ("wget/", "Wget"),
    ("httpie/", "HTTPie"),
    ("postmanruntime", "Postman"),
    ("insomnia/", "Insomnia"),
    ("headlesschrome", "HeadlessChrome"),
    ("phantomjs", "PhantomJS"),
    ("puppeteer", "Puppeteer"),
    ("playwright", "Playwright"),
    ("selenium", "Selenium"),
    ("scrapy", "Scrapy"),
    ("nmap", "Nmap"),
    ("masscan", "masscan"),
    ("zgrab", "zgrab"),
    ("nikto", "Nikto"),
    ("sqlmap", "sqlmap"),
    ("wpscan", "WPScan"),
    ("nuclei", "Nuclei"),
];

/// HTTP client libraries: lowercase needle, display name.
const LIBRARIES: &[(&str, &str)] = &[
    ("python-requests", "python-requests"),
    ("python-urllib", "python-urllib"),
    ("python-httpx", "python-httpx"),
    ("aiohttp", "aiohttp"),
    ("go-http-client", "Go-http-client"),
    ("okhttp", "okhttp"),
    ("apache-httpclient", "Apache-HttpClient"),
    ("java/", "Java"),
    ("libwww-perl", "libwww-perl"),
    ("lwp::", "libwww-perl"),
    ("guzzlehttp", "Guzzle"),
    ("axios/", "axios"),
    ("node-fetch", "node-fetch"),
    ("undici", "undici"),
    ("got (", "got"),
    ("ruby", "Ruby"),
    ("faraday", "Faraday"),
    ("reqwest", "reqwest"),
    ("php/", "PHP"),
    ("wordpress/", "WordPress"),
    ("dart:io", "Dart"),
    ("cfnetwork", "CFNetwork"),
    ("libcurl", "libcurl"),
];

pub fn classify(ua: Option<&str>) -> UaClass {
    let Some(ua) = ua.map(str::trim).filter(|s| !s.is_empty() && *s != "-") else {
        return class(UaCategory::Empty, None);
    };
    let lower = ua.to_ascii_lowercase();

    if let Some(name) = lookup(BOTS, &lower) {
        return class(UaCategory::Bot, Some(name.to_owned()));
    }
    if let Some(name) = lookup(TOOLS, &lower) {
        return class(UaCategory::Tool, Some(name.to_owned()));
    }
    if BOT_MARKERS.iter().any(|m| lower.contains(m)) {
        return class(UaCategory::Bot, generic_bot_name(ua, &lower));
    }
    if let Some(name) = lookup(LIBRARIES, &lower) {
        return class(UaCategory::Library, Some(name.to_owned()));
    }
    if let Some(family) = browser_family(&lower) {
        return class(UaCategory::Browser, Some(family.to_owned()));
    }
    // `Mozilla/5.0 (compatible; Something)` with no browser engine behind it.
    if lower.contains("compatible;") {
        return class(UaCategory::Bot, compatible_name(ua));
    }
    class(UaCategory::Unknown, None)
}

fn class(category: UaCategory, family: Option<String>) -> UaClass {
    UaClass { category, family }
}

fn lookup(table: &[(&str, &'static str)], lower: &str) -> Option<&'static str> {
    table
        .iter()
        .find(|(needle, _)| lower.contains(needle))
        .map(|(_, name)| *name)
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')
}

/// The product token that carries a bot marker, e.g. `Acme-Catalogue-Crawler`
/// from `Acme-Catalogue-Crawler/1.0 (+https://...)`. Falls back to the
/// first product token when the marker is only a `+http` contact URL.
fn generic_bot_name(ua: &str, lower: &str) -> Option<String> {
    // Tokens that are part of a browser string, never a bot's own name.
    const BOILERPLATE: &[&str] = &[
        "mozilla",
        "compatible",
        "applewebkit",
        "khtml",
        "like",
        "gecko",
        "chrome",
        "safari",
        "version",
        "mobile",
        "windows",
        "nt",
        "win64",
        "x64",
        "macintosh",
        "intel",
        "mac",
        "os",
        "x",
        "linux",
        "x11",
        "x86_64",
        "android",
        "iphone",
        "cpu",
    ];
    let mut start = 0;
    let mut first_token: Option<&str> = None;
    // Inside a contact URL, where words like `bot.html` say nothing.
    let mut in_url = false;
    // `lower` is an ASCII-lowercased copy, so byte offsets match `ua`.
    for (i, c) in lower.char_indices().chain([(lower.len(), ' ')]) {
        if is_name_char(c) {
            continue;
        }
        let token = &lower[start..i];
        let original = &ua[start..i];
        start = i + c.len_utf8();
        let was_in_url = in_url;
        if token.starts_with("http") {
            in_url = true;
        }
        if c.is_whitespace() || c == ')' || c == ';' {
            in_url = false;
        }
        if was_in_url
            || token.is_empty()
            || token.starts_with("http")
            || BOILERPLATE.contains(&token)
        {
            continue;
        }
        if ["bot", "crawl", "spider", "slurp", "scrape"]
            .iter()
            .any(|m| token.contains(m))
        {
            return Some(original.to_owned());
        }
        if first_token.is_none() && token.chars().any(|c| c.is_ascii_alphabetic()) {
            first_token = Some(original);
        }
    }
    first_token.map(str::to_owned)
}

/// The name after `compatible;`.
fn compatible_name(ua: &str) -> Option<String> {
    let lower = ua.to_ascii_lowercase();
    let after = &ua[lower.find("compatible;")? + "compatible;".len()..];
    let name: String = after
        .trim_start()
        .chars()
        .take_while(|c| is_name_char(*c))
        .collect();
    Some(name).filter(|n| !n.is_empty())
}

fn browser_family(lower: &str) -> Option<&'static str> {
    if !lower.starts_with("mozilla/") && !lower.starts_with("opera/") {
        return None;
    }
    // Most specific first: nearly everything also says Chrome and Safari.
    const FAMILIES: &[(&str, &str)] = &[
        ("edg/", "Edge"),
        ("edga/", "Edge"),
        ("edgios/", "Edge"),
        ("opr/", "Opera"),
        ("opera", "Opera"),
        ("samsungbrowser/", "Samsung Internet"),
        ("yabrowser/", "Yandex Browser"),
        ("vivaldi/", "Vivaldi"),
        ("duckduckgo/", "DuckDuckGo"),
        ("fxios/", "Firefox"),
        ("firefox/", "Firefox"),
        ("crios/", "Chrome"),
        ("chrome/", "Chrome"),
        ("safari/", "Safari"),
        ("trident/", "Internet Explorer"),
        ("msie ", "Internet Explorer"),
    ];
    lookup(FAMILIES, lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(ua: &str, category: UaCategory, family: Option<&str>) {
        let got = classify(Some(ua));
        assert_eq!(
            (got.category, got.family.as_deref()),
            (category, family),
            "{ua}"
        );
    }

    #[test]
    fn empty() {
        assert_eq!(classify(None).category, UaCategory::Empty);
        assert_eq!(classify(Some("-")).category, UaCategory::Empty);
        assert_eq!(classify(Some("  ")).category, UaCategory::Empty);
    }

    #[test]
    fn browsers() {
        use UaCategory::Browser;
        check(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36",
            Browser,
            Some("Chrome"),
        );
        check(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36 Edg/157.0.0.0",
            Browser,
            Some("Edge"),
        );
        check(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:131.0) Gecko/20100101 Firefox/131.0",
            Browser,
            Some("Firefox"),
        );
        check(
            "Mozilla/5.0 (iPhone; CPU iPhone OS 18_7 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/27.2 Mobile/15E148 Safari/604.1",
            Browser,
            Some("Safari"),
        );
        check(
            "Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/150.0.0.0 Mobile/15E148 Safari/604.1",
            Browser,
            Some("Chrome"),
        );
        check(
            "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) SamsungBrowser/31.0 Chrome/148.0.0.0 Mobile Safari/537.36",
            Browser,
            Some("Samsung Internet"),
        );
    }

    #[test]
    fn bots_that_look_like_browsers() {
        use UaCategory::Bot;
        check(
            "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; Amazonbot/0.1; +https://developer.amazon.com/support/amazonbot) Chrome/119.0.6045.214 Safari/537.36",
            Bot,
            Some("Amazonbot"),
        );
        check(
            "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; bingbot/2.0; +http://www.bing.com/bingbot.htm) Chrome/116.0.1938.76 Safari/537.36",
            Bot,
            Some("Bingbot"),
        );
        check(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15 (Applebot/0.1; +http://www.apple.com/go/applebot)",
            Bot,
            Some("Applebot"),
        );
        check(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36 (compatible; meta-externalagent/1.1 (+https://developers.facebook.com/docs/sharing/webmasters/crawler))",
            Bot,
            Some("Meta-ExternalAgent"),
        );
        check(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36; compatible; OAI-SearchBot/1.4; +https://openai.com/searchbot",
            Bot,
            Some("OAI-SearchBot"),
        );
        check(
            "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)",
            Bot,
            Some("Googlebot"),
        );
        check(
            "Mozilla/5.0 (compatible; YandexBot/3.0; +http://yandex.com/bots)",
            Bot,
            Some("YandexBot"),
        );
    }

    #[test]
    fn unlisted_bots_get_a_name_from_the_string() {
        use UaCategory::Bot;
        check(
            "Acme-Catalogue-Crawler/1.0 (+https://www.example.com; ops@example.com)",
            Bot,
            Some("Acme-Catalogue-Crawler"),
        );
        check(
            "ExampleIntranetCrawler/1.0",
            Bot,
            Some("ExampleIntranetCrawler"),
        );
        check(
            "Mozilla/5.0 (compatible; SpiderLing; +https://www.sketchengine.eu/crawler/)",
            Bot,
            Some("SpiderLing"),
        );
        check(
            "Cache-Primer/0.3.0 (+https://example.org/cache-primer)",
            Bot,
            Some("Cache-Primer"),
        );
        check(
            "Example-Sitemap/1.0 (+https://example.net)",
            Bot,
            Some("Example-Sitemap"),
        );
        check(
            "Mozilla/5.0 (compatible; pixellm-research-pilot)",
            Bot,
            Some("pixellm-research-pilot"),
        );
    }

    #[test]
    fn tools_and_libraries() {
        check("curl/8.5.0", UaCategory::Tool, Some("curl"));
        check(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/120.0.0.0 Safari/537.36",
            UaCategory::Tool,
            Some("HeadlessChrome"),
        );
        check(
            "python-requests/2.32.3",
            UaCategory::Library,
            Some("python-requests"),
        );
        check(
            "Go-http-client/2.0",
            UaCategory::Library,
            Some("Go-http-client"),
        );
        check(
            "WordPress/7.1.4; https://www.example.com",
            UaCategory::Library,
            Some("WordPress"),
        );
        check("Monit/5.33.0", UaCategory::Bot, Some("Monit"));
    }

    #[test]
    fn unknown() {
        // Truncated browser string with no browser token.
        check(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            UaCategory::Unknown,
            None,
        );
        check("x", UaCategory::Unknown, None);
    }
}
