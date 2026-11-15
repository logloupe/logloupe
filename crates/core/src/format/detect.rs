//! Format auto-detection from the first lines of a file.

use super::{Combined, LogFormat};

/// Lines sniffed per file.
pub const SNIFF_LINES: usize = 200;
/// Share of sniffed lines a format must parse to qualify. Matches the
/// tolerance ingest has for bad lines, so scanner garbage does not fail detection.
const MIN_SHARE: f64 = 0.9;

/// Every built-in format, most specific first (ties go to the earlier entry).
pub fn builtin() -> Vec<Box<dyn LogFormat>> {
    vec![Box::new(Combined)]
}

pub fn by_name(name: &str) -> Option<Box<dyn LogFormat>> {
    builtin().into_iter().find(|f| f.name() == name)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Score {
    pub format: String,
    pub parsed: usize,
    pub lines: usize,
}

/// Picks the format that parses the most of `lines` cleanly. A line only
/// counts when the format consumed all of it: leftover text is a mismatch, so
/// plain combined does not claim a log that has trailing fields.
pub fn detect<'a>(
    lines: impl IntoIterator<Item = &'a str>,
) -> Result<Box<dyn LogFormat>, Vec<Score>> {
    let lines: Vec<&str> = lines
        .into_iter()
        .filter(|l| !l.trim().is_empty())
        .take(SNIFF_LINES)
        .collect();

    let mut best: Option<(usize, Box<dyn LogFormat>)> = None;
    let mut scores = Vec::new();
    for format in builtin() {
        let parsed = lines
            .iter()
            .filter(|l| format.parse(l).is_ok_and(|p| p.rest.is_empty()))
            .count();
        scores.push(Score {
            format: format.name().to_owned(),
            parsed,
            lines: lines.len(),
        });
        let qualifies = !lines.is_empty() && parsed as f64 >= lines.len() as f64 * MIN_SHARE;
        if qualifies && best.as_ref().is_none_or(|(top, _)| parsed > *top) {
            best = Some((parsed, format));
        }
    }
    best.map(|(_, format)| format).ok_or(scores)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str =
        r#"203.0.113.7 - - [10/Oct/2026:13:55:36 +0000] "GET / HTTP/1.1" 200 10 "-" "curl/8.5.0""#;

    #[test]
    fn detects_combined_despite_some_garbage() {
        let mut lines = vec![GOOD; 19];
        lines.push("complete garbage");
        lines.push("");
        assert_eq!(detect(lines).ok().unwrap().name(), "combined");
    }

    #[test]
    fn trailing_fields_do_not_match_plain_combined() {
        let line = format!("{GOOD} 0.123");
        let scores = detect([line.as_str(); 10]).err().unwrap();
        assert_eq!(scores[0].parsed, 0);
    }

    #[test]
    fn too_much_garbage_fails() {
        let mut lines = vec![GOOD; 8];
        lines.extend(["garbage"; 2]);
        assert!(detect(lines).is_err());
        assert!(detect(Vec::<&str>::new()).is_err());
    }
}
