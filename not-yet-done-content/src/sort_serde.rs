//! Compact serialization for [`SortKey`] vectors used in adapter
//! `view_sort_state` tables.
//!
//! Format: `col:dir,col:dir,...` where `dir` is `asc` or `desc`. Empty
//! columns and malformed pairs are silently dropped on parse — the goal
//! is round-trip-safe storage of valid sort specs, not strict validation.

use crate::{SortDirection, SortKey};

pub fn serialize(sort: &[SortKey]) -> String {
    sort.iter()
        .map(|k| {
            let dir = match k.direction {
                SortDirection::Asc => "asc",
                SortDirection::Desc => "desc",
            };
            format!("{}:{}", k.column, dir)
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub fn parse(s: &str) -> Vec<SortKey> {
    s.split(',')
        .filter_map(|p| {
            let (col, dir) = p.trim().split_once(':')?;
            let col = col.trim();
            if col.is_empty() {
                return None;
            }
            let direction = match dir.trim() {
                "asc" => SortDirection::Asc,
                "desc" => SortDirection::Desc,
                _ => return None,
            };
            Some(SortKey {
                column: col.into(),
                direction,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_single() {
        let keys = vec![SortKey { column: "title".into(), direction: SortDirection::Asc }];
        let s = serialize(&keys);
        assert_eq!(s, "title:asc");
        assert_eq!(parse(&s), keys);
    }

    #[test]
    fn round_trip_multi() {
        let keys = vec![
            SortKey { column: "prio".into(), direction: SortDirection::Desc },
            SortKey { column: "title".into(), direction: SortDirection::Asc },
        ];
        let s = serialize(&keys);
        assert_eq!(s, "prio:desc,title:asc");
        assert_eq!(parse(&s), keys);
    }

    #[test]
    fn empty_in_empty_out() {
        assert_eq!(serialize(&[]), "");
        assert!(parse("").is_empty());
    }

    #[test]
    fn malformed_pairs_dropped() {
        assert!(parse("garbage").is_empty());
        assert!(parse(":asc").is_empty());
        assert!(parse("title:weird").is_empty());
    }

    #[test]
    fn partial_parse_keeps_valid() {
        let parsed = parse("title:asc,garbage,prio:desc");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].column, "title");
        assert_eq!(parsed[1].column, "prio");
    }

    #[test]
    fn whitespace_tolerated() {
        let parsed = parse(" title : asc , prio : desc ");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], SortKey { column: "title".into(), direction: SortDirection::Asc });
        assert_eq!(parsed[1], SortKey { column: "prio".into(), direction: SortDirection::Desc });
    }
}
