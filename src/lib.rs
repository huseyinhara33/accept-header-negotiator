//! Accept header parsing and content type negotiation.
//!
//! The parser implements the `Accept` header grammar from RFC 7231 section
//! 5.3.2, including quality factors and the `*/*` wildcard. Negotiation picks
//! the media type from a caller-supplied list that best matches the parsed
//! preferences.
//!
//! Design choices worth stating plainly:
//!
//! * Quality factors are parsed into `f64` and compared with a tolerance of
//!   `1e-9`. RFC 7231 allows up to three decimal places, so exact equality
//!   would work for well-formed input, but tolerating a tiny epsilon costs
//!   nothing and avoids surprising float behaviour for callers that build
//!   `f64` values themselves.
//! * A missing quality factor means `1.0`, not `0.0`.
//! * A quality factor of `0` means "not acceptable". Such entries are kept in
//!   the parsed list because they still participate in specificity ordering:
//!   an explicit `text/html;q=0` should suppress a later `*/*;q=1`.
//! * When two entries have the same quality, the more specific one wins. A
//!   concrete type beats a `type/*` wildcard, which beats `*/*`.
//! * When a caller-supplied candidate matches several entries, the entry with
//!   the highest quality wins; ties are broken by specificity, then by the
//!   order the entries appeared in the header.
//! * The returned value is a reference to the caller's own candidate string,
//!   so the caller keeps ownership and no allocation is required to report the
//!   winner.

use std::cmp::Ordering;

/// A single parsed entry from an `Accept` header.
#[derive(Debug, Clone, PartialEq)]
pub struct AcceptEntry {
    /// Lowercase media type, e.g. `text` or `*`.
    pub type_: String,
    /// Lowercase media subtype, e.g. `html` or `*`.
    pub subtype: String,
    /// Quality factor in the range `0.0..=1.0`.
    pub quality: f64,
    /// Zero-based position in the source header, used as a stable tie-break.
    pub position: usize,
}

impl AcceptEntry {
    /// Returns `true` if this entry is a `*/*` wildcard.
    pub fn is_full_wildcard(&self) -> bool {
        self.type_ == "*" && self.subtype == "*"
    }

    /// Returns `true` if this entry is a `type/*` wildcard.
    pub fn is_subtype_wildcard(&self) -> bool {
        self.type_ != "*" && self.subtype == "*"
    }

    /// Returns `true` if this entry has a concrete type and subtype.
    pub fn is_concrete(&self) -> bool {
        self.type_ != "*" && self.subtype != "*"
    }
}

/// Errors produced while parsing an `Accept` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// A media range was empty or contained no `/` separator.
    MalformedMediaRange(String),
    /// A quality factor was present but not a number in `0.0..=1.0`.
    InvalidQuality(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::MalformedMediaRange(s) => {
                write!(f, "malformed media range: {}", s)
            }
            ParseError::InvalidQuality(s) => write!(f, "invalid quality factor: {}", s),
        }
    }
}

impl std::error::Error for ParseError {}

/// Parses an `Accept` header value into a list of [`AcceptEntry`].
///
/// The input is split on commas, then each item is split on semicolons. The
/// first piece is the media range; remaining pieces are parameters, of which
/// only `q` is interpreted. Parameters other than `q` are ignored rather than
/// rejected, which matches the lenient behaviour real clients rely on.
///
/// A trailing comma or empty item is skipped. A media range with no `/` is an
/// error. A `q` value outside `0.0..=1.0` is an error.
///
/// # Examples
///
/// ```
/// use accept_header_negotiator::parse_accept;
///
/// let entries = parse_accept("text/html;q=0.9, application/json").unwrap();
/// assert_eq!(entries.len(), 2);
/// assert!((entries[0].quality - 0.9).abs() < 1e-9);
/// assert!((entries[1].quality - 1.0).abs() < 1e-9);
/// ```
pub fn parse_accept(header: &str) -> Result<Vec<AcceptEntry>, ParseError> {
    let mut entries = Vec::new();
    let mut position = 0usize;
    for raw_item in header.split(',') {
        let item = raw_item.trim();
        if item.is_empty() {
            continue;
        }
        let mut parts = item.split(';');
        let media_range = parts.next().unwrap_or("").trim();
        let (type_, subtype) = split_media_range(media_range)?;

        let mut quality = 1.0f64;
        for param in parts {
            let param = param.trim();
            if param.is_empty() {
                continue;
            }
            let (name, value) = match param.split_once('=') {
                Some(pair) => pair,
                None => continue,
            };
            if name.trim().eq_ignore_ascii_case("q") {
                let value = value.trim().trim_matches('"');
                let parsed: f64 = value
                    .parse()
                    .map_err(|_| ParseError::InvalidQuality(value.to_string()))?;
                if !(0.0..=1.0).contains(&parsed) {
                    return Err(ParseError::InvalidQuality(value.to_string()));
                }
                quality = parsed;
            }
        }

        entries.push(AcceptEntry {
            type_: type_.to_ascii_lowercase(),
            subtype: subtype.to_ascii_lowercase(),
            quality,
            position,
        });
        position += 1;
    }
    Ok(entries)
}

fn split_media_range(range: &str) -> Result<(&str, &str), ParseError> {
    match range.split_once('/') {
        Some((t, s)) => {
            let t = t.trim();
            let s = s.trim();
            if t.is_empty() || s.is_empty() {
                Err(ParseError::MalformedMediaRange(range.to_string()))
            } else {
                Ok((t, s))
            }
        }
        None => Err(ParseError::MalformedMediaRange(range.to_string())),
    }
}

/// How well an entry matches a concrete media type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchKind {
    /// `type/subtype` matched exactly.
    Exact,
    /// `type/*` matched the type.
    TypeWildcard,
    /// `*/*` matched anything.
    FullWildcard,
    /// No match at all.
    None,
}

fn match_kind(entry: &AcceptEntry, candidate_type: &str, candidate_subtype: &str) -> MatchKind {
    if entry.is_full_wildcard() {
        MatchKind::FullWildcard
    } else if entry.is_subtype_wildcard() {
        if entry.type_ == candidate_type {
            MatchKind::TypeWildcard
        } else {
            MatchKind::None
        }
    } else if entry.type_ == candidate_type && entry.subtype == candidate_subtype {
        MatchKind::Exact
    } else {
        MatchKind::None
    }
}

fn specificity(kind: MatchKind) -> u8 {
    match kind {
        MatchKind::Exact => 2,
        MatchKind::TypeWildcard => 1,
        MatchKind::FullWildcard => 0,
        MatchKind::None => 0,
    }
}

/// Selects the best matching candidate from `candidates`.
///
/// Each candidate is a media type string such as `text/html`. The header is
/// parsed with [`parse_accept`]; an empty or unparseable header yields `None`.
///
/// The winner is the candidate with the highest quality among the entries that
/// match it. Ties are broken by specificity (exact beats `type/*` beats
/// `*/*`), then by the entry's position in the header. A candidate that only
/// matches entries with quality `0` is not selected.
///
/// The returned reference borrows from `candidates`, so the caller keeps
/// ownership and no allocation is needed to report the winner.
///
/// # Examples
///
/// ```
/// use accept_header_negotiator::negotiate;
///
/// let candidates = ["text/html", "application/json"];
/// let chosen = negotiate("application/json;q=0.9, text/html;q=0.5", &candidates);
/// assert_eq!(chosen, Some("application/json"));
/// ```
pub fn negotiate<'a>(
    header: &str,
    candidates: &'a [&'a str],
) -> Option<&'a str> {
    let entries = parse_accept(header).ok()?;
    if entries.is_empty() {
        return None;
    }

    let mut best: Option<(&'a str, f64, u8, usize)> = None;
    for candidate in candidates.iter().copied() {
        let (cand_type, cand_subtype) = match split_candidate(candidate) {
            Some(pair) => pair,
            None => continue,
        };
        for entry in &entries {
            let kind = match_kind(entry, cand_type, cand_subtype);
            if kind == MatchKind::None {
                continue;
            }
            let spec = specificity(kind);
            let better = match best {
                None => true,
                Some((_, best_q, best_spec, best_pos)) => {
                    if entry.quality > best_q + 1e-9 {
                        true
                    } else if (entry.quality - best_q).abs() <= 1e-9 {
                        if spec > best_spec {
                            true
                        } else if spec == best_spec {
                            entry.position < best_pos
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                }
            };
            if better {
                best = Some((candidate, entry.quality, spec, entry.position));
            }
        }
    }

    match best {
        Some((candidate, quality, _, _)) if quality > 1e-9 => Some(candidate),
        _ => None,
    }
}

fn split_candidate(candidate: &str) -> Option<(&str, &str)> {
    let (t, s) = candidate.split_once('/')?;
    let t = t.trim();
    let s = s.trim();
    if t.is_empty() || s.is_empty() {
        None
    } else {
        Some((t, s))
    }
}

/// Compares two quality values with the tolerance used throughout the crate.
///
/// Exposed so callers writing their own selection logic do not have to guess
/// the epsilon this crate uses.
pub fn quality_eq(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9
}

/// Orders two entries by quality descending, then specificity descending,
/// then position ascending. Useful when a caller wants the same ordering the
/// negotiator applies.
pub fn compare_entries(a: &AcceptEntry, b: &AcceptEntry) -> Ordering {
    if a.quality > b.quality + 1e-9 {
        Ordering::Less
    } else if b.quality > a.quality + 1e-9 {
        Ordering::Greater
    } else {
        let sa = entry_specificity(a);
        let sb = entry_specificity(b);
        if sa != sb {
            sb.cmp(&sa)
        } else {
            a.position.cmp(&b.position)
        }
    }
}

fn entry_specificity(entry: &AcceptEntry) -> u8 {
    if entry.is_concrete() {
        2
    } else if entry.is_subtype_wildcard() {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn parses_single_entry_with_default_quality() {
        let entries = parse_accept("text/html").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].type_, "text");
        assert_eq!(entries[0].subtype, "html");
        assert!(approx(entries[0].quality, 1.0));
    }

    #[test]
    fn parses_multiple_entries_and_quality() {
        let entries = parse_accept("text/html;q=0.9, application/json;q=0.8").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].subtype, "html");
        assert!(approx(entries[0].quality, 0.9));
        assert_eq!(entries[1].subtype, "json");
        assert!(approx(entries[1].quality, 0.8));
    }

    #[test]
    fn lowercases_type_and_subtype() {
        let entries = parse_accept("TEXT/HTML").unwrap();
        assert_eq!(entries[0].type_, "text");
        assert_eq!(entries[0].subtype, "html");
    }

    #[test]
    fn ignores_unknown_parameters() {
        let entries = parse_accept("text/html;level=1;q=0.5").unwrap();
        assert!(approx(entries[0].quality, 0.5));
    }

    #[test]
    fn rejects_media_range_without_slash() {
        assert_eq!(
            parse_accept("texthtml"),
            Err(ParseError::MalformedMediaRange("texthtml".to_string()))
        );
    }

    #[test]
    fn rejects_quality_out_of_range() {
        assert_eq!(
            parse_accept("text/html;q=1.5"),
            Err(ParseError::InvalidQuality("1.5".to_string()))
        );
    }

    #[test]
    fn skips_empty_items() {
        let entries = parse_accept("text/html, , application/json,").unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn negotiate_picks_highest_quality() {
        let candidates = ["text/html", "application/json"];
        let chosen = negotiate("application/json;q=0.9, text/html;q=0.5", &candidates);
        assert_eq!(chosen, Some("application/json"));
    }

    #[test]
    fn negotiate_prefers_exact_over_wildcard() {
        let candidates = ["text/html", "application/json"];
        let chosen = negotiate("*/*;q=1.0, text/html;q=1.0", &candidates);
        assert_eq!(chosen, Some("text/html"));
    }

    #[test]
    fn negotiate_prefers_type_wildcard_over_full_wildcard() {
        let candidates = ["text/html", "application/json"];
        let chosen = negotiate("*/*;q=1.0, text/*;q=1.0", &candidates);
        assert_eq!(chosen, Some("text/html"));
    }

    #[test]
    fn negotiate_rejects_zero_quality_entries() {
        let candidates = ["text/html", "application/json"];
        let chosen = negotiate("text/html;q=0, application/json;q=0.1", &candidates);
        assert_eq!(chosen, Some("application/json"));
    }

    #[test]
    fn negotiate_returns_none_when_nothing_matches() {
        let candidates = ["application/json"];
        let chosen = negotiate("text/html", &candidates);
        assert_eq!(chosen, None);
    }

    #[test]
    fn negotiate_returns_none_on_empty_header() {
        let candidates = ["text/html"];
        assert_eq!(negotiate("", &candidates), None);
    }

    #[test]
    fn negotiate_returns_none_on_invalid_header() {
        let candidates = ["text/html"];
        assert_eq!(negotiate("not a media type", &candidates), None);
    }

    #[test]
    fn negotiate_uses_header_order_as_final_tiebreak() {
        // Two exact matches with equal quality: the earlier header entry wins.
        let candidates = ["text/html", "application/json"];
        let chosen = negotiate("text/html;q=0.5, application/json;q=0.5", &candidates);
        assert_eq!(chosen, Some("text/html"));
    }

    #[test]
    fn negotiate_skips_candidates_without_slash() {
        let candidates = ["bogus", "text/html"];
        let chosen = negotiate("text/html", &candidates);
        assert_eq!(chosen, Some("text/html"));
    }

    #[test]
    fn quality_eq_uses_tolerance() {
        assert!(quality_eq(0.9, 0.9 + 1e-12));
        assert!(!quality_eq(0.9, 0.91));
    }

    #[test]
    fn compare_entries_orders_by_quality_then_specificity_then_position() {
        let a = AcceptEntry {
            type_: "text".to_string(),
            subtype: "html".to_string(),
            quality: 0.9,
            position: 0,
        };
        let b = AcceptEntry {
            type_: "*".to_string(),
            subtype: "*".to_string(),
            quality: 0.9,
            position: 1,
        };
        assert_eq!(compare_entries(&a, &b), Ordering::Less);
        assert_eq!(compare_entries(&b, &a), Ordering::Greater);
    }
}
