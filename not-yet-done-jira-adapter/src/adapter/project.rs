//! Projects and their versions — the `jira:project` / `jira:version` levels
//! — plus the root's `project_types` action.
//!
//! A project is the level that owns its versions, so that is where they
//! hang: `jira:project` lists the projects, and a project's children are
//! its `jira:version` rows. An issue names its fix versions but cannot say
//! whether one of them has shipped; the project's version list is the only
//! place that knows (`released`, `release_date`, `archived`), and it keeps
//! them in the project's release *sequence* — which is what makes "the next
//! release still open" a question with an answer:
//!
//! ```text
//! nyd adapter jira:project:version MOBIT ls -q unreleased   # first row = next
//! ```
//!
//! Neither level speaks JQL — Jira offers no search over projects or
//! versions, and both lists arrive whole. So the query is a filter applied
//! here: a status word for versions ([`VersionFilter`]), a substring of key
//! or name for projects. A word neither level knows is an error rather than
//! a silently unfiltered listing.
//!
//! [`create`](super::create) names its type (`issuetype: { name }`), which
//! is friendly to write and unforgiving to guess: a misspelt name comes back
//! as a server-side rejection after the POST, with no hint of what would
//! have worked. This action is the lookup that makes the name checkable
//! first — and, listed rather than validated, it also answers "what can this
//! project even hold?" for a caller building a menu.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use not_yet_done_content::*;

use crate::client::{JiraClient, JiraProject, JiraVersion};

use super::types::{project_node_type, version_node_type};
use super::util::other_err;

/// The root's `project_types` action: one required project key.
pub(super) fn project_types_action() -> NodeAction {
    NodeAction::new(
        "project_types",
        "Issue types of a project",
        InputSpec::Form {
            fields: vec![FormFieldSpec::text("project", "Project key (e.g. PROJ)")],
        },
    )
}

/// `execute("project_types")` — a `# project <id>` header line, then one
/// type per line as `id<TAB>name<TAB>kind`, `kind` being `standard` or
/// `subtask`. Tab-separated because the answer is read by scripts as often
/// as by people, and a type name may contain spaces; the order is Jira's
/// own. The header carries the project's numeric id, which a caller that
/// goes on to address the project (a create-issue URL, say) needs and would
/// otherwise have to fetch a second time; `#` keeps it out of the way of a
/// reader that only wants the rows.
pub(super) async fn execute_project_types(
    client: &Arc<JiraClient>,
    values: &HashMap<String, String>,
) -> Result<ActionOutcome> {
    let project = values
        .get("project")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| other_err("project is required".to_string()))?;

    let (project_id, types) = client
        .get_project_issue_types(project)
        .await
        .map_err(other_err)?;

    let mut lines = vec![format!("# project {project_id}")];
    lines.extend(types.iter().map(|t| {
        let kind = if t.subtask { "subtask" } else { "standard" };
        format!("{}\t{}\t{}", t.id, t.name, kind)
    }));
    let listing = lines.join("\n");

    Ok(ActionOutcome::Done {
        message: Some(listing),
    })
}

// ---------------------------------------------------------------------------
// Columns
// ---------------------------------------------------------------------------

/// The project list's columns. Jira answers the whole list in one call, so
/// it is held complete and sorted locally ([`apply_sort`]) — every column is
/// sortable, unlike the paged issue list.
pub(super) fn project_columns() -> Vec<ColumnSchema> {
    vec![
        ColumnSchema::new("key", "Key"),
        ColumnSchema::new("name", "Name"),
        ColumnSchema::new("id", "Id").typed("number"),
    ]
}

/// The version list's columns, likewise held complete and sorted locally.
///
/// Unsorted, the rows keep the project's release sequence — deliberately,
/// because sorting by `name` would order `RL-9` after `RL-10` and sorting by
/// `release_date` would drop every version that has none. Ask for a sort and
/// you get one; ask for nothing and you get the order the project keeps.
pub(super) fn version_columns() -> Vec<ColumnSchema> {
    let flag = |key: &str, label: &str| {
        ColumnSchema::new(key, label).with_options(vec!["true".into(), "false".into()])
    };
    vec![
        ColumnSchema::new("name", "Name"),
        flag("released", "Released"),
        flag("archived", "Archived"),
        ColumnSchema::new("release_date", "Release Date").typed("datetime"),
        ColumnSchema::new("start_date", "Start Date").typed("datetime"),
        flag("overdue", "Overdue"),
        ColumnSchema::new("description", "Description"),
        ColumnSchema::new("id", "Id").typed("number"),
    ]
}

// ---------------------------------------------------------------------------
// Listings
// ---------------------------------------------------------------------------

fn field(key: &str, label: &str, value: impl Into<String>) -> MetadataField {
    MetadataField {
        key: key.into(),
        value: value.into(),
        display_label: label.into(),
        editable: false,
        allowed_values: None,
    }
}

fn project_metadata(p: &JiraProject) -> Metadata {
    Metadata {
        fields: vec![
            field("key", "Key", p.key.clone()),
            field("name", "Name", p.name.clone()),
            field("id", "Id", p.id.clone()),
        ],
    }
}

/// Metadata for one version, shared by the list row and the node so both
/// render the same cells. Keys mirror [`version_columns`]; `project` (the
/// key it hangs under) is extra detail, not a column — every row of one
/// listing carries the same value.
fn version_metadata(project_key: &str, v: &JiraVersion) -> Metadata {
    Metadata {
        fields: vec![
            field("name", "Name", v.name.clone()),
            field("released", "Released", v.released.to_string()),
            field("archived", "Archived", v.archived.to_string()),
            field("release_date", "Release Date", v.release_date.clone()),
            field("start_date", "Start Date", v.start_date.clone()),
            field("overdue", "Overdue", v.overdue.to_string()),
            field("description", "Description", v.description.clone()),
            field("id", "Id", v.id.clone()),
            field("project", "Project", project_key),
        ],
    }
}

/// The composite id of a version row: `{project key}/version/{version id}`,
/// the same shape the issue-level leaves use, so `get_by_id` can resolve one
/// back without a second lookup of which project it belongs to.
pub(super) fn version_id(project_key: &str, version_id: &str) -> String {
    format!("{project_key}/version/{version_id}")
}

/// The version level's query: which versions to keep. The list is small
/// and the only thing anyone narrows it by is whether the release is out,
/// so the query is one word rather than an expression language.
///
/// `archived` is not a third state next to the other two — a version can be
/// archived and released both — it is the separate question "which ones has
/// the project put away", and asking it is the only way to see them
/// alongside the rest. The default, `all`, hides nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VersionFilter {
    All,
    Released,
    Unreleased,
    Archived,
}

impl VersionFilter {
    /// Parse the `-q` word. Empty or absent means [`VersionFilter::All`];
    /// anything unrecognised is refused by name, because a mistyped filter
    /// that silently listed everything would read as "nothing matched the
    /// other way round".
    pub(super) fn parse(query: Option<&str>) -> Result<Self> {
        match query.map(str::trim).unwrap_or("").to_ascii_lowercase().as_str() {
            "" | "all" => Ok(Self::All),
            "released" => Ok(Self::Released),
            "unreleased" | "open" => Ok(Self::Unreleased),
            "archived" => Ok(Self::Archived),
            other => Err(other_err(format!(
                "unknown version filter '{other}' — use all, released, unreleased or archived"
            ))),
        }
    }

    fn keeps(&self, v: &JiraVersion) -> bool {
        match self {
            Self::All => true,
            Self::Released => v.released,
            Self::Unreleased => !v.released,
            Self::Archived => v.archived,
        }
    }
}

/// List the projects visible to the current user — the single fetch source
/// behind the root's `jira:project` child. A query narrows the list to the
/// projects whose key or name contains it (case-insensitively). Rows arrive
/// in Jira's order; a requested sort is applied locally.
pub(super) async fn list_projects(
    client: &Arc<JiraClient>,
    params: ListParams,
) -> Result<ListResult> {
    let projects = client.all_projects().await.map_err(other_err)?;
    let needle = params
        .query
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(str::to_ascii_lowercase);

    let mut items: Vec<NodeSummary> = projects
        .iter()
        .filter(|p| match &needle {
            None => true,
            Some(n) => {
                p.key.to_ascii_lowercase().contains(n) || p.name.to_ascii_lowercase().contains(n)
            }
        })
        .map(|p| NodeSummary {
            id: p.key.clone(),
            label: if p.name.is_empty() {
                p.key.clone()
            } else {
                p.name.clone()
            },
            node_type: project_node_type(),
            metadata: project_metadata(p),
            has_children: Some(true),
        })
        .collect();

    let applied_sort = apply_sort(&mut items, &params.sort, &project_columns());

    Ok(ListResult {
        items,
        applied_sort,
        page: None,
        batch_download_available: false,
        downloaded: vec![],
    })
}

/// List one project's versions — the single fetch source behind the
/// `jira:version` child of a project. Reconstructs from adapter state
/// (`client`) plus the project key (= the parent node's `id()`); the query
/// narrows the result to a status (see [`VersionFilter`]). Unsorted, the
/// rows keep the project's release sequence, so with `unreleased` the first
/// row is the next release still to come.
pub(super) async fn list_versions(
    client: &Arc<JiraClient>,
    project_key: &str,
    params: ListParams,
) -> Result<ListResult> {
    let filter = VersionFilter::parse(params.query.as_deref())?;
    let versions = client
        .project_versions(project_key)
        .await
        .map_err(other_err)?;

    let mut items: Vec<NodeSummary> = versions
        .iter()
        .filter(|v| filter.keeps(v))
        .map(|v| NodeSummary {
            id: version_id(project_key, &v.id),
            label: v.name.clone(),
            node_type: version_node_type(),
            metadata: version_metadata(project_key, v),
            has_children: None,
        })
        .collect();

    let applied_sort = apply_sort(&mut items, &params.sort, &version_columns());

    Ok(ListResult {
        items,
        applied_sort,
        page: None,
        batch_download_available: false,
        downloaded: vec![],
    })
}

// ---------------------------------------------------------------------------
// Addressing
// ---------------------------------------------------------------------------

/// Whether a bare root-level id names a project rather than an issue.
///
/// The two cannot collide: a Jira project key is uppercase letters and
/// digits starting with a letter, and an issue key is that key, a hyphen and
/// a number. So the hyphen decides, and this is a rule rather than a guess —
/// the same job [`crate::adapter`]'s composite-id split does one level down.
pub(super) fn looks_like_project_key(id: &str) -> bool {
    !id.is_empty()
        && id.starts_with(|c: char| c.is_ascii_uppercase())
        && id
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

// ---------------------------------------------------------------------------
// Nodes
// ---------------------------------------------------------------------------

/// A project node. Built from the key alone (`get_by_id`) or from a listed
/// row; either way its children are its versions, which are fetched on
/// demand — so addressing a project costs nothing until something is listed
/// under it.
pub(super) struct JiraProjectNode {
    client: Arc<JiraClient>,
    project: JiraProject,
    cached_metadata: Metadata,
}

impl JiraProjectNode {
    pub(super) fn new(client: Arc<JiraClient>, project: JiraProject) -> Self {
        let cached_metadata = project_metadata(&project);
        Self {
            client,
            project,
            cached_metadata,
        }
    }

    /// A node for a project known only by its key — no call made. The name
    /// stays empty until something needs it; nothing under this node does.
    pub(super) fn from_key(client: Arc<JiraClient>, key: String) -> Self {
        Self::new(
            client,
            JiraProject {
                id: String::new(),
                key,
                name: String::new(),
            },
        )
    }
}

#[async_trait]
impl Node for JiraProjectNode {
    fn id(&self) -> &str {
        &self.project.key
    }

    fn label(&self) -> &str {
        if self.project.name.is_empty() {
            &self.project.key
        } else {
            &self.project.name
        }
    }

    fn node_type(&self) -> &NodeType {
        static PROJECT_TYPE: std::sync::LazyLock<NodeType> =
            std::sync::LazyLock::new(project_node_type);
        &PROJECT_TYPE
    }

    fn metadata(&self) -> &Metadata {
        &self.cached_metadata
    }

    /// Children of a project are its versions, addressed by the version's
    /// own id — either bare (`10501`) or as the composite the rows carry
    /// (`PROJ/version/10501`).
    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        let wanted = id
            .rsplit_once("/version/")
            .map(|(_, v)| v)
            .unwrap_or(id)
            .to_string();
        let version = self
            .client
            .project_versions(&self.project.key)
            .await
            .map_err(other_err)?
            .into_iter()
            .find(|v| v.id == wanted)
            .ok_or_else(|| {
                ContentError::NotFound(format!(
                    "No version {wanted} in project {}",
                    self.project.key
                ))
            })?;
        Ok(Box::new(JiraVersionNode::new(
            self.project.key.clone(),
            version,
        )))
    }

    fn content(&self) -> Option<&dyn Content> {
        None
    }
}

/// A single version. A leaf: everything it knows is in its row, and Jira
/// offers nothing below it.
pub(super) struct JiraVersionNode {
    composite_id: String,
    version: JiraVersion,
    cached_metadata: Metadata,
}

impl JiraVersionNode {
    pub(super) fn new(project_key: String, version: JiraVersion) -> Self {
        let cached_metadata = version_metadata(&project_key, &version);
        let composite_id = version_id(&project_key, &version.id);
        Self {
            composite_id,
            version,
            cached_metadata,
        }
    }
}

#[async_trait]
impl Node for JiraVersionNode {
    fn id(&self) -> &str {
        &self.composite_id
    }

    fn label(&self) -> &str {
        &self.version.name
    }

    fn node_type(&self) -> &NodeType {
        static VERSION_TYPE: std::sync::LazyLock<NodeType> =
            std::sync::LazyLock::new(version_node_type);
        &VERSION_TYPE
    }

    fn metadata(&self) -> &Metadata {
        &self.cached_metadata
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        Err(ContentError::NotFound(format!("No child: {id}")))
    }

    fn content(&self) -> Option<&dyn Content> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(name: &str, released: bool) -> JiraVersion {
        JiraVersion {
            id: "10501".into(),
            name: name.into(),
            description: String::new(),
            released,
            archived: false,
            start_date: String::new(),
            release_date: "2026-04-13".into(),
            overdue: false,
        }
    }

    /// The columns promise what every row carries; a row that misses one
    /// would make a pane show a blank cell and a sort silently do nothing.
    #[test]
    fn version_rows_carry_every_declared_column() {
        let v = version("RL-14-2026", false);
        let row = NodeSummary {
            id: version_id("PROJ", &v.id),
            label: v.name.clone(),
            node_type: version_node_type(),
            metadata: version_metadata("PROJ", &v),
            has_children: None,
        };
        let rows = [row];
        let columns = version_columns();
        let missing = not_yet_done_content::children::check_rows(&columns, &rows);
        assert!(missing.is_empty(), "row misses columns: {missing:?}");
    }

    #[test]
    fn project_rows_carry_every_declared_column() {
        let p = JiraProject {
            id: "10000".into(),
            key: "PROJ".into(),
            name: "A Project".into(),
        };
        let row = NodeSummary {
            id: p.key.clone(),
            label: p.name.clone(),
            node_type: project_node_type(),
            metadata: project_metadata(&p),
            has_children: Some(true),
        };
        let rows = [row];
        let columns = project_columns();
        let missing = not_yet_done_content::children::check_rows(&columns, &rows);
        assert!(missing.is_empty(), "row misses columns: {missing:?}");
    }

    /// `released` is the whole point of the level: it is the one fact an
    /// issue's `fix_versions` cannot carry, so it has to arrive as a cell
    /// a caller can read and sort on, not as prose.
    #[test]
    fn released_is_a_readable_cell() {
        let cell = |v: &JiraVersion| {
            version_metadata("PROJ", v)
                .fields
                .iter()
                .find(|f| f.key == "released")
                .map(|f| f.value.clone())
                .expect("released field present")
        };
        assert_eq!(cell(&version("RL-12-2026", true)), "true");
        assert_eq!(cell(&version("RL-14-2026", false)), "false");
    }

    /// The hyphen is what separates the two kinds of bare id at the root.
    #[test]
    fn a_project_key_is_told_apart_from_an_issue_key() {
        assert!(looks_like_project_key("PROJ"));
        assert!(looks_like_project_key("MOB2"));
        assert!(!looks_like_project_key("PROJ-1"));
        assert!(!looks_like_project_key("proj"));
        assert!(!looks_like_project_key(""));
        assert!(!looks_like_project_key("PROJ/version/1"));
    }

    #[test]
    fn a_version_row_is_addressable_as_a_composite_id() {
        assert_eq!(version_id("PROJ", "10501"), "PROJ/version/10501");
    }

    #[test]
    fn the_version_filter_reads_the_words_a_caller_would_write() {
        assert_eq!(VersionFilter::parse(None).unwrap(), VersionFilter::All);
        assert_eq!(VersionFilter::parse(Some("  ")).unwrap(), VersionFilter::All);
        assert_eq!(
            VersionFilter::parse(Some("Unreleased")).unwrap(),
            VersionFilter::Unreleased
        );
        assert_eq!(
            VersionFilter::parse(Some("open")).unwrap(),
            VersionFilter::Unreleased
        );
        assert_eq!(
            VersionFilter::parse(Some("archived")).unwrap(),
            VersionFilter::Archived
        );
    }

    /// A mistyped filter that quietly listed everything would read as the
    /// opposite of what it is — so it is refused, by name.
    #[test]
    fn an_unknown_version_filter_is_refused_by_name() {
        let err = VersionFilter::parse(Some("releasd")).expect_err("must refuse");
        let msg = err.to_string();
        assert!(msg.contains("releasd"), "names the typo: {msg}");
        assert!(msg.contains("unreleased"), "names a valid one: {msg}");
    }

    #[test]
    fn the_filter_keeps_what_its_name_says() {
        let out = version("RL-12-2026", true);
        let open = version("RL-14-2026", false);
        let mut put_away = version("RL-99-2025", true);
        put_away.archived = true;

        assert!(VersionFilter::Released.keeps(&out));
        assert!(!VersionFilter::Released.keeps(&open));
        assert!(VersionFilter::Unreleased.keeps(&open));
        assert!(!VersionFilter::Unreleased.keeps(&out));
        assert!(VersionFilter::Archived.keeps(&put_away));
        assert!(!VersionFilter::Archived.keeps(&out));
        assert!(VersionFilter::All.keeps(&put_away));
    }

    /// The levels are declared by type alone, so a caller can walk
    /// `jira:project:version` without fetching anything — which is what
    /// `nyd adapter jira:project:version help` does.
    #[tokio::test]
    async fn the_levels_hang_where_jira_keeps_them() {
        let adapter = crate::adapter::test_adapter().await;
        let root_type = NodeType {
            type_id: "jira:root".into(),
            mime_type: String::new(),
            syntax: None,
            file_extension: String::new(),
            display_name: "Jira Root".into(),
        };

        let under_root: Vec<String> =
            not_yet_done_content::child_types_of_type(&adapter, &root_type)
                .into_iter()
                .map(|t| t.type_id)
                .collect();
        assert!(
            under_root.iter().any(|t| t == "jira:project"),
            "the root offers the project level: {under_root:?}"
        );

        let under_project: Vec<String> =
            not_yet_done_content::child_types_of_type(&adapter, &project_node_type())
                .into_iter()
                .map(|t| t.type_id)
                .collect();
        assert_eq!(under_project, vec!["jira:version".to_string()]);

        let under_version =
            not_yet_done_content::child_types_of_type(&adapter, &version_node_type());
        assert!(under_version.is_empty(), "a version is a leaf");
    }
}
