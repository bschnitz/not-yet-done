//! App-wide stable reference to a node.
//!
//! Used by the link feature so that every node of every content adapter
//! can be addressed uniformly. Refs built by the host follow
//! `<adapter_type>/<instance_id>/<node_id>`, where the node id keeps its
//! own separators and stays opaque above the adapter — which alone knows
//! its internal ID scheme.
//!
//! Examples:
//! - `tasks/local/a1b2c3d4-e5f6-7890-abcd-ef0123456789`
//! - `jira/prod/PROJ-123`
//! - `jira/prod/PROJ-123/comment/abc`
//! - `taiga/main/task:123`
//!
//! Parsing is structural only: no segment is interpreted here. Each
//! consumer validates its own segments on the way down the path.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum NodeRefParseError {
    #[error("node ref must not be empty")]
    Empty,
    #[error("node ref must not start or end with `/`")]
    EdgeSlash,
    #[error("node ref must not contain empty segments (`//`)")]
    EmptySegment,
    #[error("segment must not contain `/`")]
    SeparatorInSegment,
}

/// Globally-unique reference to a node. The string form is canonical:
/// `NodeRef::parse(r.as_str()).unwrap() == r`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NodeRef {
    raw: String,
}

impl NodeRef {
    pub fn parse(s: &str) -> Result<Self, NodeRefParseError> {
        if s.is_empty() {
            return Err(NodeRefParseError::Empty);
        }
        if s.starts_with('/') || s.ends_with('/') {
            return Err(NodeRefParseError::EdgeSlash);
        }
        if s.contains("//") {
            return Err(NodeRefParseError::EmptySegment);
        }
        Ok(Self { raw: s.to_string() })
    }

    /// Compose a ref from individual segments. Use when an adapter
    /// builds a ref from typed parts that have not been concatenated.
    pub fn from_segments<I, S>(segments: I) -> Result<Self, NodeRefParseError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut raw = String::new();
        for seg in segments {
            let seg = seg.as_ref();
            if seg.is_empty() {
                return Err(NodeRefParseError::EmptySegment);
            }
            if seg.contains('/') {
                return Err(NodeRefParseError::SeparatorInSegment);
            }
            if !raw.is_empty() {
                raw.push('/');
            }
            raw.push_str(seg);
        }
        if raw.is_empty() {
            return Err(NodeRefParseError::Empty);
        }
        Ok(Self { raw })
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// First segment — the tab key (`tasks`, `tracking`, `jira`, …).
    /// Always non-empty because `parse`/`from_segments` reject empty refs.
    pub fn head(&self) -> &str {
        match self.raw.find('/') {
            Some(i) => &self.raw[..i],
            None => &self.raw,
        }
    }

    /// Everything after the first `/`. `None` for single-segment refs
    /// (a bare tab root, with no item below it).
    pub fn tail(&self) -> Option<&str> {
        self.raw.find('/').map(|i| &self.raw[i + 1..])
    }

    /// Split into (head, tail) for top-down routing. Each level
    /// consumes `head`, dispatches on it, and re-parses `tail` as the
    /// path for the next level.
    pub fn split_head(&self) -> (&str, Option<&str>) {
        (self.head(), self.tail())
    }

    pub fn segments(&self) -> std::str::Split<'_, char> {
        self.raw.split('/')
    }
}

impl fmt::Display for NodeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

impl FromStr for NodeRef {
    type Err = NodeRefParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple() {
        let r = NodeRef::parse("tasks/a1b2c3d4").unwrap();
        assert_eq!(r.as_str(), "tasks/a1b2c3d4");
        assert_eq!(r.head(), "tasks");
        assert_eq!(r.tail(), Some("a1b2c3d4"));
    }

    #[test]
    fn parse_deep() {
        let r = NodeRef::parse("jira/prod/PROJ-123/comment/abc").unwrap();
        assert_eq!(r.head(), "jira");
        assert_eq!(r.tail(), Some("prod/PROJ-123/comment/abc"));
        let segments: Vec<&str> = r.segments().collect();
        assert_eq!(segments, vec!["jira", "prod", "PROJ-123", "comment", "abc"]);
    }

    #[test]
    fn parse_segments_with_colon() {
        // Taiga uses `task:123` etc. as a segment — colon is allowed.
        let r = NodeRef::parse("taiga/main/task:123").unwrap();
        assert_eq!(r.head(), "taiga");
        let segments: Vec<&str> = r.segments().collect();
        assert_eq!(segments, vec!["taiga", "main", "task:123"]);
    }

    #[test]
    fn parse_single_segment_has_no_tail() {
        let r = NodeRef::parse("tasks").unwrap();
        assert_eq!(r.head(), "tasks");
        assert_eq!(r.tail(), None);
        let (h, t) = r.split_head();
        assert_eq!(h, "tasks");
        assert_eq!(t, None);
    }

    #[test]
    fn parse_rejects_empty() {
        assert_eq!(NodeRef::parse(""), Err(NodeRefParseError::Empty));
    }

    #[test]
    fn parse_rejects_leading_slash() {
        assert_eq!(
            NodeRef::parse("/tasks/x"),
            Err(NodeRefParseError::EdgeSlash)
        );
    }

    #[test]
    fn parse_rejects_trailing_slash() {
        assert_eq!(
            NodeRef::parse("tasks/x/"),
            Err(NodeRefParseError::EdgeSlash)
        );
    }

    #[test]
    fn parse_rejects_empty_segment() {
        assert_eq!(
            NodeRef::parse("jira//prod"),
            Err(NodeRefParseError::EmptySegment)
        );
    }

    #[test]
    fn from_segments_round_trip() {
        let r = NodeRef::from_segments(["jira", "prod", "PROJ-123"]).unwrap();
        assert_eq!(r.as_str(), "jira/prod/PROJ-123");
        assert_eq!(NodeRef::parse(r.as_str()).unwrap(), r);
    }

    #[test]
    fn from_segments_rejects_empty_segment() {
        let err = NodeRef::from_segments(["jira", "", "x"]).unwrap_err();
        assert_eq!(err, NodeRefParseError::EmptySegment);
    }

    #[test]
    fn from_segments_rejects_slash_in_segment() {
        let err = NodeRef::from_segments(["jira", "prod/leaked", "x"]).unwrap_err();
        assert_eq!(err, NodeRefParseError::SeparatorInSegment);
    }

    #[test]
    fn from_segments_rejects_empty_input() {
        let err = NodeRef::from_segments::<_, &str>(std::iter::empty()).unwrap_err();
        assert_eq!(err, NodeRefParseError::Empty);
    }

    #[test]
    fn display_matches_as_str() {
        let r = NodeRef::parse("jira/prod/PROJ-123").unwrap();
        assert_eq!(format!("{r}"), "jira/prod/PROJ-123");
    }

    #[test]
    fn from_str_works() {
        let r: NodeRef = "tasks/abc".parse().unwrap();
        assert_eq!(r.head(), "tasks");
    }

    #[test]
    fn canonical_round_trip() {
        let inputs = [
            "tasks/abc",
            "tracking/9f8e",
            "jira/prod/PROJ-123",
            "jira/prod/PROJ-123/comment/abc",
            "taiga/main/task:123",
        ];
        for s in inputs {
            let r = NodeRef::parse(s).unwrap();
            assert_eq!(r.as_str(), s);
            assert_eq!(format!("{r}"), s);
        }
    }
}
