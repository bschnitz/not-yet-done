//! Fuzzy filter used by [`super::LeaderList`]'s optional search. Backed by the
//! same [`SkimMatcherV2`] the rest of the app filters with (select lists, the
//! file picker, the content tree), so typing behaves identically everywhere.
//! The query is whitespace-tokenised: every token must fuzzy-match the
//! haystack and their scores are summed, so `jira open` narrows a prefixed
//! `Jira › … › Open` row the way multi-word content filtering does.

use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;

/// Case-insensitive fuzzy match of `needle` in `haystack` using `matcher`.
///
/// Returns `None` when any whitespace-separated token of `needle` is not a
/// fuzzy match, otherwise the summed relevance score (higher is better). An
/// empty (or whitespace-only) `needle` matches everything with a neutral score.
pub(crate) fn fuzzy_score(matcher: &SkimMatcherV2, haystack: &str, needle: &str) -> Option<i32> {
    let mut total: i64 = 0;
    let mut any = false;
    for token in needle.split_whitespace() {
        any = true;
        total += matcher.fuzzy_match(haystack, token)?;
    }
    if !any {
        return Some(0);
    }
    Some(total.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matcher() -> SkimMatcherV2 {
        SkimMatcherV2::default()
    }

    #[test]
    fn empty_needle_matches_everything() {
        assert_eq!(fuzzy_score(&matcher(), "anything", ""), Some(0));
        assert_eq!(fuzzy_score(&matcher(), "anything", "   "), Some(0));
    }

    #[test]
    fn non_subsequence_is_none() {
        assert!(fuzzy_score(&matcher(), "abc", "xyz").is_none());
        assert!(fuzzy_score(&matcher(), "abc", "acb").is_none());
    }

    #[test]
    fn case_insensitive() {
        assert!(fuzzy_score(&matcher(), "Delete Comment", "dc").is_some());
    }

    #[test]
    fn contiguous_scores_higher_than_scattered() {
        let m = matcher();
        let contiguous = fuzzy_score(&m, "attach", "att").unwrap();
        let scattered = fuzzy_score(&m, "a-t-t-x", "att").unwrap();
        assert!(contiguous > scattered, "{contiguous} !> {scattered}");
    }

    #[test]
    fn every_token_must_match() {
        let m = matcher();
        // Both tokens hit the prefixed location + name.
        assert!(fuzzy_score(&m, "Jira \u{203a} Tickets \u{203a} Open", "jira open").is_some());
        // Second token is absent → no match.
        assert!(fuzzy_score(&m, "Jira \u{203a} Tickets \u{203a} Open", "jira close").is_none());
    }
}
