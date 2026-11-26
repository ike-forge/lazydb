//! Fuzzy matching for the history picker, after fzf's v1: every term must
//! appear in order, not necessarily together, and tighter hits rank higher.
//! Linear in the haystack, since it runs over every entry on every keystroke.

use std::ops::Range;

const MATCH: i64 = 16;
const CONSECUTIVE: i64 = 8;
const BOUNDARY: i64 = 8;
const GAP_START: i64 = 3;
const GAP_EXTENSION: i64 = 1;

/// Whitespace-separated terms, each matched on its own, all required.
/// Case-insensitive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fuzzy {
    terms: Vec<Vec<char>>,
}

/// Higher `score` is better; `ranges` are byte ranges into the haystack,
/// sorted and disjoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub score: i64,
    pub ranges: Vec<Range<usize>>,
}

impl Fuzzy {
    /// `None` for a query with no terms, so `Option<Fuzzy>` means "no filter".
    #[must_use]
    pub fn new(query: &str) -> Option<Self> {
        let terms: Vec<Vec<char>> = query
            .split_whitespace()
            .map(|term| term.chars().map(lower).collect())
            .collect();

        (!terms.is_empty()).then_some(Self { terms })
    }

    #[must_use]
    pub fn find(&self, haystack: &str) -> Option<Hit> {
        let chars: Vec<(usize, char)> = haystack
            .char_indices()
            .map(|(at, character)| (at, lower(character)))
            .collect();
        let mut score = 0;
        let mut positions = Vec::new();

        for term in &self.terms {
            let (term_score, term_positions) = best(&chars, term)?;
            score += term_score;
            positions.extend(term_positions);
        }

        Some(Hit {
            score,
            ranges: ranges(haystack, &chars, positions),
        })
    }
}

/// One char: `İ` lowercases to two, and positions must stay one-to-one.
fn lower(character: char) -> char {
    character.to_lowercase().next().unwrap_or(character)
}

/// The term as a substring when it is one, else the shortest subsequence
/// window ending at its first full match.
fn best(chars: &[(usize, char)], term: &[char]) -> Option<(i64, Vec<usize>)> {
    if let Some(start) = substring(chars, term) {
        let positions: Vec<usize> = (start..start + term.len()).collect();
        return Some((score(chars, &positions), positions));
    }

    let positions = subsequence(chars, term)?;
    Some((score(chars, &positions), positions))
}

/// The occurrence at a word start if any, else the first.
fn substring(chars: &[(usize, char)], term: &[char]) -> Option<usize> {
    if term.len() > chars.len() {
        return None;
    }

    let mut first = None;

    for start in 0..=chars.len() - term.len() {
        let matches = chars[start..start + term.len()]
            .iter()
            .zip(term)
            .all(|((_, character), wanted)| character == wanted);

        if matches {
            if boundary(chars, start) {
                return Some(start);
            }
            first.get_or_insert(start);
        }
    }

    first
}

/// Forward to the first full match, then back from its end: the backward
/// pass finds the latest start, so the window is as tight as greed allows.
fn subsequence(chars: &[(usize, char)], term: &[char]) -> Option<Vec<usize>> {
    let mut wanted = term.iter().peekable();
    let mut end = None;

    for (at, (_, character)) in chars.iter().enumerate() {
        if wanted.peek() == Some(&character) {
            wanted.next();
            if wanted.peek().is_none() {
                end = Some(at);
                break;
            }
        }
    }

    let end = end?;
    let mut positions = Vec::with_capacity(term.len());
    let mut wanted = term.iter().rev().peekable();

    for at in (0..=end).rev() {
        if wanted.peek() == Some(&&chars[at].1) {
            wanted.next();
            positions.push(at);
            if wanted.peek().is_none() {
                break;
            }
        }
    }

    positions.reverse();
    Some(positions)
}

fn score(chars: &[(usize, char)], positions: &[usize]) -> i64 {
    let mut total = 0;
    let mut previous: Option<usize> = None;

    for &at in positions {
        total += MATCH;

        if boundary(chars, at) {
            total += BOUNDARY;
        }

        match previous {
            Some(before) if before + 1 == at => total += CONSECUTIVE,
            Some(before) => {
                let gap = i64::try_from(at - before - 1).unwrap_or(i64::MAX);
                total -= GAP_START + GAP_EXTENSION * (gap - 1);
            }
            None => {}
        }

        previous = Some(at);
    }

    total
}

/// At the start, or after anything that is not part of a word.
fn boundary(chars: &[(usize, char)], at: usize) -> bool {
    at.checked_sub(1)
        .and_then(|before| chars.get(before))
        .is_none_or(|(_, before)| !before.is_alphanumeric() && *before != '_')
}

fn ranges(haystack: &str, chars: &[(usize, char)], mut positions: Vec<usize>) -> Vec<Range<usize>> {
    positions.sort_unstable();
    positions.dedup();

    let end_of = |at: usize| {
        chars
            .get(at + 1)
            .map_or(haystack.len(), |(offset, _)| *offset)
    };
    let mut ranges: Vec<Range<usize>> = Vec::new();

    for at in positions {
        let range = chars[at].0..end_of(at);

        match ranges.last_mut() {
            Some(last) if last.end == range.start => last.end = range.end,
            _ => ranges.push(range),
        }
    }

    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fuzzy(query: &str) -> Fuzzy {
        Fuzzy::new(query).expect("a term")
    }

    fn matched<'a>(query: &str, haystack: &'a str) -> Vec<&'a str> {
        fuzzy(query)
            .find(haystack)
            .expect("a match")
            .ranges
            .into_iter()
            .map(|range| &haystack[range])
            .collect()
    }

    fn score(query: &str, haystack: &str) -> i64 {
        fuzzy(query).find(haystack).expect("a match").score
    }

    #[test]
    fn a_blank_query_is_no_filter() {
        assert_eq!(Fuzzy::new(""), None);
        assert_eq!(Fuzzy::new(" \t "), None);
    }

    #[test]
    fn letters_in_order_match_with_gaps() {
        assert_eq!(matched("slord", "SELECT * FROM orders"), ["S", "L", "ord"]);
    }

    #[test]
    fn letters_out_of_order_do_not_match() {
        assert_eq!(fuzzy("drs").find("SELECT 1"), None);
        assert_eq!(fuzzy("tceles").find("SELECT"), None);
    }

    #[test]
    fn matching_ignores_case() {
        assert_eq!(matched("ORDERS", "select * from orders"), ["orders"]);
    }

    #[test]
    fn every_term_must_match_in_any_order() {
        assert_eq!(
            matched("users select", "SELECT * FROM users"),
            ["SELECT", "users"]
        );
        assert_eq!(fuzzy("users orders").find("SELECT * FROM users"), None);
    }

    #[test]
    fn a_substring_is_found_even_where_a_scattered_match_comes_first() {
        assert_eq!(
            matched("order", "SELECT o, r, d, e, r FROM orders"),
            ["order"]
        );
    }

    #[test]
    fn a_substring_at_a_word_start_beats_one_inside_a_word() {
        let hit = fuzzy("id")
            .find("SELECT valid, id FROM t")
            .expect("a match");

        assert_eq!(hit.ranges.first(), Some(&(14..16)));
    }

    #[test]
    fn a_contiguous_hit_outranks_a_scattered_one() {
        assert!(score("orders", "FROM orders") > score("orders", "FROM o r d e r s"));
    }

    #[test]
    fn a_hit_at_a_word_start_outranks_one_inside_a_word() {
        assert!(score("id", "WHERE id = 1") > score("id", "WHERE valid = 1"));
    }

    #[test]
    fn a_tighter_window_outranks_a_looser_one() {
        assert!(score("abc", "a_b_c") > score("abc", "a____b____c"));
    }

    #[test]
    fn ranges_are_byte_ranges_around_multibyte_characters() {
        assert_eq!(matched("é", "SELECT 'café'"), ["é"]);
        assert_eq!(matched("ÉT", "SELECT 'été'"), ["ét"]);
    }

    #[test]
    fn a_character_matched_by_two_terms_is_one_range() {
        assert_eq!(matched("sel lect", "SELECT"), ["SELECT"]);
    }
}
