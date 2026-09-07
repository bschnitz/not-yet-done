//! What starting a tracking does to the trackings already running.
//!
//! Every path that starts a tracking — the `toggle-tracking` action, the
//! `tracking: true` field of the task editor, the tree editor's `-t` flag,
//! the `track start` command — used to carry its own copy of the rule "stop
//! the others unless parallel is allowed". The rule now lives here, once, and
//! [`TrackingService::start`](super::TrackingService::start) is the only
//! place that applies it.
//!
//! Tasks are told apart by their **label path**: the descriptions from the
//! forest root down to the task, joined by `/` with a leading `/`
//! (`/Work/Customer/Ticket`). A grouped policy partitions that space with
//! regular expressions.

use regex::Regex;

/// The exclusivity rule a start applies to the trackings already running.
#[derive(Clone, Debug)]
pub enum TrackingPolicy {
    /// Starting stops every other running tracking — at most one task tracks
    /// at a time. The default.
    Exclusive,
    /// Starting leaves the other trackings alone.
    Parallel,
    /// Starting stops only the running trackings whose task lies in the same
    /// group. A task's group is the index of the **first** pattern that
    /// matches its label path; tasks no pattern matches form one group of
    /// their own, so two of them still exclude each other.
    Grouped(Vec<Regex>),
}

impl Default for TrackingPolicy {
    fn default() -> Self {
        Self::Exclusive
    }
}

impl TrackingPolicy {
    /// The two-state policy behind an `allow_parallel` flag.
    pub fn from_parallel_flag(parallel: bool) -> Self {
        if parallel {
            Self::Parallel
        } else {
            Self::Exclusive
        }
    }

    /// A grouped policy from pattern strings. Every pattern must compile; the
    /// error names the offending one by position and text. An empty list is
    /// the same as [`Self::Exclusive`], because all tasks then share the
    /// rest group.
    pub fn grouped<S: AsRef<str>>(patterns: &[S]) -> Result<Self, String> {
        let mut compiled = Vec::with_capacity(patterns.len());
        for (i, p) in patterns.iter().enumerate() {
            let p = p.as_ref();
            let re = Regex::new(p)
                .map_err(|e| format!("group_paths[{i}] '{p}' is not a valid regex: {e}"))?;
            compiled.push(re);
        }
        Ok(Self::Grouped(compiled))
    }

    /// Whether a start under this policy leaves other trackings running
    /// (used by validations that want to refuse two starts up front).
    pub fn is_exclusive(&self) -> bool {
        matches!(self, Self::Exclusive)
    }

    /// The group a label path falls into: `Some(index of the first matching
    /// pattern)`, or `None` for the rest group. Always `None` for the two
    /// ungrouped policies.
    pub fn group_of(&self, label_path: &str) -> Option<usize> {
        match self {
            Self::Grouped(patterns) => patterns.iter().position(|re| re.is_match(label_path)),
            _ => None,
        }
    }

    /// Whether starting a tracking on the task at `starting` stops a running
    /// tracking on the task at `running`.
    pub fn stops(&self, starting: &str, running: &str) -> bool {
        match self {
            Self::Exclusive => true,
            Self::Parallel => false,
            Self::Grouped(_) => self.group_of(starting) == self.group_of(running),
        }
    }
}

/// Join task descriptions from the root down into the label path the
/// policies match against: `/` + descriptions joined by `/`.
pub fn label_path<'a>(descriptions_root_first: impl IntoIterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for d in descriptions_root_first {
        out.push('/');
        out.push_str(d);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_stops_everything_and_parallel_nothing() {
        assert!(TrackingPolicy::Exclusive.stops("/a", "/b"));
        assert!(!TrackingPolicy::Parallel.stops("/a", "/a"));
        assert!(TrackingPolicy::from_parallel_flag(false).is_exclusive());
        assert!(!TrackingPolicy::from_parallel_flag(true).is_exclusive());
    }

    #[test]
    fn grouped_stops_only_within_the_same_group() {
        let p = TrackingPolicy::grouped(&["^/Work", "^/Other/Group"]).unwrap();
        assert!(p.stops("/Work/a", "/Work/b/c"));
        assert!(!p.stops("/Work/a", "/Other/Group/x"));
        assert!(!p.stops("/Work/a", "/Private/x"));
        // The rest group is a real group.
        assert!(p.stops("/Private/x", "/Hobby/y"));
        assert_eq!(p.group_of("/Other/Group/x"), Some(1));
        assert_eq!(p.group_of("/Hobby"), None);
    }

    #[test]
    fn first_matching_pattern_wins() {
        let p = TrackingPolicy::grouped(&["^/Work", "^/Work/Special"]).unwrap();
        assert_eq!(p.group_of("/Work/Special/x"), Some(0));
    }

    #[test]
    fn an_invalid_pattern_is_named_by_position() {
        let err = TrackingPolicy::grouped(&["^/Work", "("]).unwrap_err();
        assert!(
            err.starts_with("group_paths[1] '(' is not a valid regex"),
            "{err}"
        );
    }

    #[test]
    fn label_path_joins_root_first() {
        assert_eq!(
            label_path(["Work", "Customer", "Ticket"]),
            "/Work/Customer/Ticket"
        );
        assert_eq!(label_path(std::iter::empty::<&str>()), "");
    }
}
