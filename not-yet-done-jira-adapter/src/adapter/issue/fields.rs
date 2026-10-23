//! `set_fields` action — write the header fields of an issue (summary,
//! labels, assignee, story points, fix versions) on their own.
//!
//! The editor actions (`edit_full`, `from_markdown`) can set these too, but
//! only as part of a round-trip that rewrites the description along with
//! them. A caller that wants to add one label has to send the whole ticket
//! back to do it, and whatever it read a moment ago is what the description
//! becomes — a stale buffer silently reverts somebody else's edit. This
//! action touches nothing it was not given.
//!
//! Every field is optional; what is absent stays as it is. The one thing
//! absence cannot express is *emptying* a field, so a lone `-` says that:
//! `labels=-` removes every label, `assignee=-` unassigns the issue,
//! `story_points=-` takes the estimate off, `fix_versions=-` takes the
//! issue out of every release.

use std::collections::HashMap;

use not_yet_done_content::*;

use super::super::util::other_err;
use super::JiraIssueNode;
use super::slugs::{USER_PREFIX, build_slug_tables};

/// The value that means "empty this field" rather than "leave it alone".
const CLEAR: &str = "-";

/// A form value, trimmed, or `None` when it was not given at all. An empty
/// string is the same as absent: a form the user tabbed past unchanged must
/// not blank the field on the server.
fn given(values: &HashMap<String, String>, key: &str) -> Option<String> {
    values
        .get(key)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Read the story-points value: `Some(None)` clears the estimate, a
/// `Some(Some(n))` sets it, and `None` leaves it alone. A decimal comma is
/// accepted alongside the point -- the form is typed by a human, and half
/// a point is a real estimate on some boards.
fn given_points(values: &HashMap<String, String>, key: &str) -> Result<Option<Option<f64>>> {
    let Some(raw) = given(values, key) else {
        return Ok(None);
    };
    if raw == CLEAR {
        return Ok(Some(None));
    }
    raw.replace(',', ".")
        .parse::<f64>()
        .map(|n| Some(Some(n)))
        .map_err(|_| other_err(format!("story_points: '{raw}' is not a number")))
}

/// Split a comma-separated label list. Mirrors the `create` form, so the
/// same spelling works in both places.
fn split_labels(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Read the fix-version list: `Some(empty)` clears it, `Some(names)` sets
/// it, `None` leaves it alone. Split like labels — a version name never
/// carries a comma, and one spelling for both lists is one thing less to
/// remember.
fn given_fix_versions(values: &HashMap<String, String>) -> Option<Vec<String>> {
    given(values, "fix_versions").map(|raw| {
        if raw == CLEAR {
            Vec::new()
        } else {
            split_labels(&raw)
        }
    })
}

/// The project an issue key belongs to (`PROJ-12` → `PROJ`).
fn project_of(key: &str) -> &str {
    key.split('-').next().unwrap_or(key)
}

impl JiraIssueNode {
    pub(super) async fn execute_set_fields(
        &self,
        values: &HashMap<String, String>,
    ) -> Result<ActionOutcome> {
        let summary = given(values, "summary");
        let labels = given(values, "labels").map(|raw| {
            if raw == CLEAR {
                Vec::new()
            } else {
                split_labels(&raw)
            }
        });
        let assignee = match given(values, "assignee") {
            // `update_issue_full` reads the empty key as "unassign".
            Some(who) if who == CLEAR => Some(String::new()),
            // A `uu_` slug is how the editor and the CACHE legend name a
            // user; a caller that copied one from there means that user.
            // An unknown slug is an error, never sent on as a username.
            Some(who) if who.starts_with(USER_PREFIX) => {
                let users = build_slug_tables(&self.cache).users;
                match users.original_for(&who) {
                    Some(name) => Some(name.to_string()),
                    None => {
                        return Err(other_err(format!(
                            "unknown user slug `{who}` -- see the CACHE legend"
                        )));
                    }
                }
            }
            other => other,
        };

        let points = given_points(values, "story_points")?;
        let fix_versions = given_fix_versions(values);

        if summary.is_none()
            && labels.is_none()
            && assignee.is_none()
            && points.is_none()
            && fix_versions.is_none()
        {
            return Err(other_err(
                "nothing to set — pass at least one of summary, labels, assignee, \
                 story_points, fix_versions"
                    .to_string(),
            ));
        }

        // Checked before anything is written: Jira answers an unknown
        // version with a bare 400, and a caller that named one release of
        // four must not find the other three already gone.
        let fix_versions = match &fix_versions {
            Some(names) if !names.is_empty() => Some(self.known_versions(names).await?),
            other => other.clone(),
        };

        // Skipped entirely when only the estimate was named, so a
        // story-points write does not send an empty header update.
        if summary.is_some() || labels.is_some() || assignee.is_some() {
            self.client
                .update_issue_full(
                    &self.key,
                    summary.as_deref(),
                    // Never the description: that is what this action exists
                    // to keep its hands off.
                    None,
                    labels.as_deref(),
                    assignee.as_deref(),
                )
                .await
                .map_err(other_err)?;
        }

        if let Some(p) = points {
            self.client
                .set_story_points(&self.key, p)
                .await
                .map_err(other_err)?;
        }

        if let Some(names) = &fix_versions {
            self.client
                .set_fix_versions(&self.key, names)
                .await
                .map_err(other_err)?;
        }

        let mut wrote: Vec<String> = Vec::new();
        if let Some(s) = &summary {
            wrote.push(format!("summary={s}"));
        }
        if let Some(ls) = &labels {
            wrote.push(if ls.is_empty() {
                "labels cleared".to_string()
            } else {
                format!("labels={}", ls.join(","))
            });
        }
        if let Some(a) = &assignee {
            wrote.push(if a.is_empty() {
                "unassigned".to_string()
            } else {
                format!("assignee={a}")
            });
        }
        if let Some(p) = points {
            wrote.push(match p {
                None => "story points cleared".to_string(),
                Some(n) if n.fract() == 0.0 => format!("story_points={}", n as i64),
                Some(n) => format!("story_points={n}"),
            });
        }
        if let Some(vs) = &fix_versions {
            wrote.push(if vs.is_empty() {
                "fix versions cleared".to_string()
            } else {
                format!("fix_versions={}", vs.join(","))
            });
        }

        Ok(ActionOutcome::Done {
            message: Some(format!("{}: {}", self.key, wrote.join(", "))),
        })
    }

    /// Resolve the named versions against the project's own list, and
    /// answer with the project's spelling of each.
    ///
    /// Case is forgiven because the name is typed by a human; an unknown
    /// name is not, and the error names what the project does have —
    /// a release is misspelled far more often than it is missing.
    async fn known_versions(&self, names: &[String]) -> Result<Vec<String>> {
        let project = project_of(&self.key);
        let known = self
            .client
            .project_versions(project)
            .await
            .map_err(other_err)?;

        let mut resolved = Vec::with_capacity(names.len());
        for name in names {
            match known
                .iter()
                .find(|v| v.name.eq_ignore_ascii_case(name.trim()))
            {
                Some(v) => resolved.push(v.name.clone()),
                None => {
                    let open: Vec<&str> = known
                        .iter()
                        .filter(|v| !v.released && !v.archived)
                        .map(|v| v.name.as_str())
                        .collect();
                    return Err(other_err(format!(
                        "{project} has no version '{name}' — open ones: {}",
                        if open.is_empty() {
                            "none".to_string()
                        } else {
                            open.join(", ")
                        }
                    )));
                }
            }
        }
        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn a_blank_value_counts_as_not_given() {
        let values = form(&[("summary", "   "), ("labels", "")]);
        assert!(given(&values, "summary").is_none());
        assert!(given(&values, "labels").is_none());
        assert!(given(&values, "assignee").is_none());
    }

    #[test]
    fn values_are_trimmed() {
        let values = form(&[("assignee", "  someone  ")]);
        assert_eq!(given(&values, "assignee").as_deref(), Some("someone"));
    }

    #[test]
    fn story_points_read_the_three_cases() {
        let clear = given_points(&form(&[("story_points", "-")]), "story_points").unwrap();
        assert_eq!(clear, Some(None));

        let set = given_points(&form(&[("story_points", " 5 ")]), "story_points").unwrap();
        assert_eq!(set, Some(Some(5.0)));

        let keep = given_points(&form(&[("story_points", "")]), "story_points").unwrap();
        assert_eq!(keep, None);
    }

    /// The form is typed by hand, and a German keyboard writes half a point
    /// with a comma.
    #[test]
    fn a_decimal_comma_is_a_decimal_point() {
        let half = given_points(&form(&[("story_points", "2,5")]), "story_points").unwrap();
        assert_eq!(half, Some(Some(2.5)));
    }

    #[test]
    fn a_non_number_is_refused_rather_than_written() {
        let err = given_points(&form(&[("story_points", "viele")]), "story_points");
        assert!(err.is_err());
    }

    #[test]
    fn fix_versions_read_the_three_cases() {
        let clear = given_fix_versions(&form(&[("fix_versions", "-")]));
        assert_eq!(clear, Some(Vec::new()));

        let set = given_fix_versions(&form(&[("fix_versions", "v14-2026, v15-2026")]));
        assert_eq!(
            set,
            Some(vec!["v14-2026".to_string(), "v15-2026".to_string()])
        );

        let keep = given_fix_versions(&form(&[("fix_versions", "  ")]));
        assert_eq!(keep, None);
    }

    /// A release name carries dashes of its own, so only the FIRST one
    /// separates the project from the number.
    #[test]
    fn the_project_is_the_first_segment_of_the_key() {
        assert_eq!(project_of("ACMEOPS-1234"), "ACMEOPS");
        assert_eq!(project_of("PROJ"), "PROJ");
    }

    #[test]
    fn labels_split_on_commas_and_drop_the_gaps() {
        assert_eq!(split_labels("one, two ,,three"), ["one", "two", "three"]);
        assert!(split_labels("  ").is_empty());
    }
}
