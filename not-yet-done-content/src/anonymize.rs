//! Anonymization: a content-layer (frontend-independent) facility that lets any
//! adapter's user-visible output be replaced with plausible, deterministic fake
//! data — for product screenshots/screencasts taken against a live, productive
//! instance without leaking real customer/ticket/person data.
//!
//! # Why it lives here, and why it's a decorator
//!
//! Anonymization is **not** a frontend concern: the TUI, CLI and Waybar each
//! resolve the same adapters and read their [`NodeSummary`] / [`Metadata`]
//! directly, so a per-frontend scrub would have to be re-implemented (and could
//! be forgotten) three times. Instead every frontend obtains its adapters from
//! the host factory registry, and *that* is the single, non-bypassable
//! chokepoint: each factory is wrapped in an [`AnonymizingFactory`] whose
//! `create` — when [`HostContext::anonymize`] is set — wraps the produced
//! adapter in an [`AnonymizingAdapter`]. From then on every row, detail field
//! and tree level the adapter hands out passes through the adapter's chosen
//! [`Anonymizer`] before any frontend sees it.
//!
//! # Standard vs. domain anonymizers
//!
//! Anonymization is a **contract obligation**, not an opt-in capability: when
//! requested, *every* adapter is anonymized. An adapter that knows its domain
//! (Tasks, Trackings, Projects — stable pseudo-name lookup; Jira/Taiga —
//! format-preserving keys) overrides [`ContentAdapter::anonymizer`] to return
//! its own strategy. Every other adapter inherits the default
//! [`StandardAnonymizer`], which is the mandatory fallback: it must be safe
//! (never leak) even without domain knowledge.
//!
//! # What is and isn't scrubbed
//!
//! Scrubbed (display surfaces): list rows, eager subtrees, the post-edit row
//! projection, live-tick rows, detail `metadata()` field values + `label()`,
//! value-picker labels, tree-search hit titles.
//!
//! **Not** scrubbed, on purpose:
//! - `id()` and `TreeFindHit::path` — internal addressing; scrubbing them would
//!   break navigation / get_by_id / lazy-expand.
//! - editable bodies and edit-prefill (`content()`, `prepare()`, `form_prep()`,
//!   `picker_options()` values, batch `downloaded` nodes, custom-query results)
//!   — these feed the **write/export** path. Scrubbing a body the user then
//!   saves would overwrite the real data with the placeholder. Anonymization is
//!   a *read/display* mask; the underlying store stays untouched. (So: don't
//!   screenshot an open editor / preview of a real body — the rows behind it
//!   are clean, the open body is not.)
//!
//! Numbers, durations and timestamps are preserved verbatim (a time-tracker's
//! durations are the point of the screenshot), so only genuine free text is
//! replaced.

use crate::*;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Deterministic, dependency-free string hash. Stable across runs *and* across
/// Rust versions (unlike `std`'s `DefaultHasher`, which the standard library
/// explicitly does not guarantee to be stable) — so a screenshot re-taken
/// tomorrow maps the same real value to the same fake. Used both by
/// [`StandardAnonymizer`] and by domain anonymizers to index into a fixed
/// pseudo-name lookup list (`hash(value) % list.len()`).
pub fn stable_hash(s: &str) -> u64 {
    s.bytes()
        .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64))
}

/// The strategy an adapter uses to replace its user-visible values. Implementors
/// only need [`scrub_value`](Anonymizer::scrub_value); the field/summary walkers
/// have correct defaults built on top of it.
pub trait Anonymizer: Send + Sync {
    /// Replace a single value, given its column `key` for context. The contract:
    /// **deterministic** (same input → same output, within and across runs) and
    /// **safe** (never returns anything derived recognisably from real data).
    /// Implementations should leave structural values (empty, numeric,
    /// timestamp, duration) untouched so totals/dates stay realistic.
    fn scrub_value(&self, key: &str, value: &str) -> String;

    /// Replace a node's tree/row **label**, given its [`NodeType`] for context.
    ///
    /// Why this exists separately from [`scrub_value`](Anonymizer::scrub_value):
    /// a label always arrives keyed `"label"`, so `scrub_value` alone cannot tell
    /// a Postgres *schema* name from a *table* name from a Discord *channel* — yet
    /// a good screenshot mask wants the result to still read like "a schema" /
    /// "a channel". Domain anonymizers override this to branch on
    /// `node_type.type_id`. The default keeps the historical behaviour (scrub the
    /// label as a plain free-text value), so every adapter that does *not*
    /// override it is unaffected.
    fn scrub_label(&self, _node_type: &NodeType, label: &str) -> String {
        self.scrub_value("label", label)
    }

    /// Replace one metadata field's value in place.
    fn scrub_field(&self, field: &mut MetadataField) {
        field.value = self.scrub_value(&field.key, &field.value);
    }

    /// Replace every field of a metadata block in place.
    fn scrub_metadata(&self, metadata: &mut Metadata) {
        for field in metadata.fields.iter_mut() {
            self.scrub_field(field);
        }
    }

    /// Replace a list/detail row in place: the `label` (keyed `"label"`) plus
    /// every metadata field. `id` / `node_type` / `has_children` are left as-is
    /// (addressing + structure, not display text).
    fn scrub_summary(&self, summary: &mut NodeSummary) {
        summary.label = self.scrub_label(&summary.node_type, &summary.label);
        self.scrub_metadata(&mut summary.metadata);
    }
}

/// The mandatory fallback anonymizer every adapter inherits unless it overrides
/// [`ContentAdapter::anonymizer`]. Domain-agnostic: it cannot make a Jira key
/// *look* like a key, but it is guaranteed safe — it replaces every free-text
/// token with a fixed neutral word (keyed by the token, so repeated tokens map
/// consistently) while leaving numbers, durations and timestamps verbatim.
#[derive(Debug, Default, Clone, Copy)]
pub struct StandardAnonymizer;

impl StandardAnonymizer {
    pub fn new() -> Self {
        Self
    }
}

/// Neutral, fully invented replacement tokens. No real customer/person/project
/// terms — these ship in the repo, so they must stay invented.
const WORD_POOL: &[&str] = &[
    "Falcon", "Harbor", "Maple", "Quartz", "Cedar", "Lumen", "Vega", "Onyx", "Pine", "Cobalt",
    "Sage", "Drift", "Ember", "Flint", "Grove", "Halcyon", "Indigo", "Juniper", "Koan", "Larch",
    "Mesa", "Nimbus", "Orbit", "Pylon", "Quill", "Rowan", "Slate", "Tundra", "Umber", "Verdant",
    "Willow", "Xenon", "Yarrow", "Zephyr", "Anchor", "Beacon", "Cinder", "Delta", "Echo", "Fable",
];

/// A value the [`StandardAnonymizer`] leaves untouched: empty, a number, a
/// timestamp, or a duration. Keeping these verbatim is what makes a screenshot
/// of a time-tracker still show realistic totals and dates.
fn is_structural(value: &str) -> bool {
    let v = value.trim();
    if v.is_empty() {
        return true;
    }
    if v.parse::<f64>().is_ok() {
        return true;
    }
    // ISO-ish timestamp: starts with YYYY-MM-DD.
    let b = v.as_bytes();
    if b.len() >= 10
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7] == b'-'
        && b[8..10].iter().all(u8::is_ascii_digit)
    {
        return true;
    }
    // Duration / clock token: only digits and time punctuation, at least one digit.
    if v.chars().any(|c| c.is_ascii_digit())
        && v.chars()
            .all(|c| c.is_ascii_digit() || matches!(c, ':' | '.' | ',' | ' ' | 'h' | 'm' | 's'))
    {
        return true;
    }
    false
}

/// Replace free text word-by-word, preserving word count. A token carrying any
/// letter is mapped to a pool word (keyed by the token, so it's stable and
/// consistent); a purely numeric/punctuation token (e.g. `42`, `10:30`) is kept.
fn pseudo_text(value: &str) -> String {
    let parts: Vec<String> = value
        .split_whitespace()
        .map(|tok| {
            if tok.chars().any(|c| c.is_ascii_alphabetic()) {
                let idx = (stable_hash(tok) % WORD_POOL.len() as u64) as usize;
                WORD_POOL[idx].to_string()
            } else {
                tok.to_string()
            }
        })
        .collect();
    if parts.is_empty() {
        value.to_string()
    } else {
        parts.join(" ")
    }
}

impl Anonymizer for StandardAnonymizer {
    fn scrub_value(&self, _key: &str, value: &str) -> String {
        if is_structural(value) {
            value.to_string()
        } else {
            pseudo_text(value)
        }
    }
}

// ---------------------------------------------------------------------------
// Shared building blocks for domain anonymizers
// ---------------------------------------------------------------------------
//
// These helpers are not used by the StandardAnonymizer (which stays minimal and
// domain-blind); they exist so the issue-tracker adapters (Jira/Taiga/
// Confluence) — which all face the same shapes: a person name, a project code,
// an email, a filename — can produce *plausible* replacements without each
// re-inventing the pools. Every one is deterministic (hash-keyed) and draws
// from fully invented, repo-safe pools. A value carrying no ascii letter (and
// so nothing to leak — a bare number, a date) is returned verbatim.

/// Invented person names. Repo-safe: no real person. English throughout — the
/// pools ship in the repo and the whole mask reads in one language.
const PERSON_POOL: &[&str] = &[
    "Mara Fields",
    "Jonas Brennan",
    "Lena Crowe",
    "Tobias Reed",
    "Nina Walters",
    "Felix Somers",
    "Clara Burke",
    "David Hooper",
    "Sophie Long",
    "Paul Adler",
    "Hanna Vosse",
    "Erik Dale",
    "Mira Sharpe",
    "Lucas Freed",
    "Anya Poole",
    "Timo Rennick",
    "Greta Sayle",
    "Niles Bowers",
    "Ida Markwell",
    "Ben Lawrence",
    "Romy Farber",
    "Joel Wendell",
    "Selma Rothe",
    "Kai Manning",
    "Lea Birch",
    "Aaron Stone",
    "Mia Holloway",
    "Finn Overton",
    "Tara Nolden",
    "Ollie Graves",
];

/// Invented adjectives for the `<adjective>_<noun>` label scheme (Postgres
/// schemas/tables, Stoat servers/channels, …). Repo-safe, English. Picked so
/// that `big_database`, `nifty_table`, `mellow_channel` read as obvious
/// placeholders while still telling the viewer *what kind of thing* it is.
const ADJ_POOL: &[&str] = &[
    "big",
    "beautiful",
    "nifty",
    "swift",
    "mellow",
    "bright",
    "clever",
    "jolly",
    "quiet",
    "brave",
    "calm",
    "eager",
    "fancy",
    "gentle",
    "happy",
    "keen",
    "lively",
    "merry",
    "noble",
    "proud",
    "rapid",
    "shiny",
    "tidy",
    "witty",
    "amber",
    "crisp",
    "dapper",
    "fleet",
    "humble",
    "plucky",
];

/// Invented uppercase project codes — shaped like Jira keys / Confluence space
/// keys / Taiga slugs. Repo-safe: no real project.
const CODE_POOL: &[&str] = &[
    "ACME", "NOVA", "ZEPH", "LUMO", "VEGA", "ONYX", "ATLS", "HLX", "ORB", "QZ", "MAPL", "CEDR",
    "FLNT", "GRV", "NIMB", "PYLN", "RWN", "SLT", "TND", "UMBR", "VRD", "WLW", "XEN", "YRW",
];

/// Map a real person/display name to a stable invented one. Empty / letterless
/// values (no name present) pass through.
pub fn pseudo_person(real: &str) -> String {
    if !real.chars().any(|c| c.is_ascii_alphabetic()) {
        return real.to_string();
    }
    let idx = (stable_hash(real) % PERSON_POOL.len() as u64) as usize;
    PERSON_POOL[idx].to_string()
}

/// Derive a stable invented username/handle (lowercase, dotted) from the same
/// pool as [`pseudo_person`], so a user's display name and username stay
/// consistent. `Mara Feldt` → `mara.feldt`.
pub fn pseudo_username(real: &str) -> String {
    if !real.chars().any(|c| c.is_ascii_alphabetic()) {
        return real.to_string();
    }
    pseudo_person(real).to_lowercase().replace(' ', ".")
}

/// Derive a stable invented email from the same pool, on the reserved
/// `.invalid` TLD (RFC 6761 — can never resolve). `Mara Feldt` →
/// `mara.feldt@example.invalid`.
pub fn pseudo_email(real: &str) -> String {
    if !real.chars().any(|c| c.is_ascii_alphabetic()) {
        return real.to_string();
    }
    format!("{}@example.invalid", pseudo_username(real))
}

/// Map a real project code (Jira key prefix, Confluence space key, Taiga slug)
/// to a stable invented uppercase code from [`CODE_POOL`]. Letterless values
/// pass through.
pub fn pseudo_project_code(real: &str) -> String {
    if !real.chars().any(|c| c.is_ascii_alphabetic()) {
        return real.to_string();
    }
    let idx = (stable_hash(real) % CODE_POOL.len() as u64) as usize;
    CODE_POOL[idx].to_string()
}

/// Format-preserving anonymization of an issue key `PREFIX-123`: map the alpha
/// `PREFIX` (the project, often the customer) to an invented code, keep the
/// `-123` (a non-sensitive issue counter) so the key still *looks* like a key.
/// Anything not matching `<letters>-<digits>` falls back to free-text scrub.
pub fn pseudo_issue_key(real: &str) -> String {
    if let Some((prefix, num)) = real.rsplit_once('-') {
        if !num.is_empty()
            && num.chars().all(|c| c.is_ascii_digit())
            && prefix.chars().any(|c| c.is_ascii_alphabetic())
        {
            return format!("{}-{num}", pseudo_project_code(prefix));
        }
    }
    pseudo_text(real)
}

/// Format-preserving anonymization of a Taiga ref `slug#123` (or bare `#123`):
/// map the `slug` to an invented lowercase slug, keep `#123`. A bare ref with
/// no slug passes through unchanged.
pub fn pseudo_ref(real: &str) -> String {
    match real.split_once('#') {
        Some((slug, num)) if slug.chars().any(|c| c.is_ascii_alphabetic()) => {
            format!("{}#{num}", pseudo_project_code(slug).to_lowercase())
        }
        // Bare `#123` or no `#`: nothing customer-identifying to map.
        _ => real.to_string(),
    }
}

/// Anonymize a filename, preserving its extension so it still reads like a
/// file: `Customer quote.pdf` → `<pool words>.pdf`.
pub fn pseudo_filename(real: &str) -> String {
    match real.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.contains(' ') => {
            format!("{}.{ext}", pseudo_text(stem))
        }
        _ => pseudo_text(real),
    }
}

/// Build an `<adjective>_<noun>` label that hides the real name yet still tells
/// the viewer *what kind of thing* it is: `pseudo_labeled("customer_prod", "database")`
/// → `big_database`. The adjective is chosen deterministically from [`ADJ_POOL`]
/// keyed by the real value (so the same source name always maps the same way and
/// two distinct names rarely collide); the `noun` is the fixed level name the
/// caller passes (`"schema"`, `"table"`, `"channel"`, …). A letterless value
/// (nothing to leak) passes through unchanged. Shared by the Postgres and Stoat
/// domain anonymizers via their [`Anonymizer::scrub_label`] overrides.
pub fn pseudo_labeled(real: &str, noun: &str) -> String {
    if !real.chars().any(|c| c.is_ascii_alphabetic()) {
        return real.to_string();
    }
    let idx = (stable_hash(real) % ADJ_POOL.len() as u64) as usize;
    format!("{}_{noun}", ADJ_POOL[idx])
}

// ---------------------------------------------------------------------------
// Factory decorator — the single injection point
// ---------------------------------------------------------------------------

/// Wrap a factory so that, when [`HostContext::anonymize`] is set, every adapter
/// it produces is wrapped in an [`AnonymizingAdapter`]. The host wraps every
/// registered factory in this, making anonymization universal across frontends.
pub fn anonymizing_factory(inner: Box<dyn AdapterFactory>) -> Box<dyn AdapterFactory> {
    Box::new(AnonymizingFactory { inner })
}

struct AnonymizingFactory {
    inner: Box<dyn AdapterFactory>,
}

impl AdapterFactory for AnonymizingFactory {
    fn adapter_type(&self) -> &str {
        self.inner.adapter_type()
    }

    fn create(
        &self,
        instance_id: &str,
        config: &str,
        ctx: &HostContext,
    ) -> Result<Box<dyn ContentAdapter>> {
        let adapter = self.inner.create(instance_id, config, ctx)?;
        if ctx.anonymize {
            Ok(Box::new(AnonymizingAdapter::new(adapter)))
        } else {
            Ok(adapter)
        }
    }

    fn config_schema(&self) -> fieldsmith::TypeSchema {
        self.inner.config_schema()
    }

    fn auth_mechanisms(&self) -> &'static [crate::MechanismSpec] {
        self.inner.auth_mechanisms()
    }
}

// ---------------------------------------------------------------------------
// Adapter decorator
// ---------------------------------------------------------------------------

/// Wraps a [`ContentAdapter`], scrubbing every data-bearing return through the
/// inner adapter's chosen [`Anonymizer`]. Everything else is delegated verbatim.
pub struct AnonymizingAdapter {
    inner: Box<dyn ContentAdapter>,
    anon: Arc<dyn Anonymizer>,
}

impl AnonymizingAdapter {
    pub fn new(inner: Box<dyn ContentAdapter>) -> Self {
        let anon = inner.anonymizer();
        Self { inner, anon }
    }
}

#[async_trait]
impl ContentAdapter for AnonymizingAdapter {
    fn adapter_type(&self) -> &str {
        self.inner.adapter_type()
    }
    fn instance_id(&self) -> &str {
        self.inner.instance_id()
    }
    fn instance_data_dir(&self) -> PathBuf {
        self.inner.instance_data_dir()
    }

    async fn root(&self) -> Result<Box<dyn Node>> {
        Ok(wrap_node(self.inner.root().await?, self.anon.clone()))
    }
    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        Ok(wrap_node(
            self.inner.get_by_id(id).await?,
            self.anon.clone(),
        ))
    }

    fn childs<'a>(&'a self, node: &'a dyn Node) -> Vec<crate::children::Child<'a>> {
        // Delegate to the inner adapter — it only reads `node.id()`/
        // `node.node_type()`, both of which the wrapping node forwards
        // unchanged — then scrub each fetched row as the closure runs.
        self.inner
            .childs(node)
            .into_iter()
            .map(|c| {
                let anon = self.anon.clone();
                crate::children::Child {
                    node_type: c.node_type,
                    columns: c.columns,
                    list: Box::new(move |params| {
                        Box::pin(async move {
                            let mut result = (c.list)(params).await?;
                            for summary in result.items.iter_mut() {
                                anon.scrub_summary(summary);
                            }
                            Ok(result)
                        })
                    }),
                }
            })
            .collect()
    }

    async fn eager_subtree(
        &self,
        node: &dyn Node,
        params: &ListParams,
        depth: u32,
    ) -> Option<Result<Subtree>> {
        // Preserve the inner adapter's one-pass expansion (and its per-level
        // sort), scrubbing the whole subtree afterwards.
        match self.inner.eager_subtree(node, params, depth).await {
            Some(Ok(mut subtree)) => {
                scrub_subtree(&mut subtree, &*self.anon);
                Some(Ok(subtree))
            }
            other => other,
        }
    }

    fn actions_for_type(&self, node_type: &NodeType) -> Vec<NodeAction> {
        self.inner.actions_for_type(node_type)
    }
    fn child_process_env(&self, node: &NodeRef) -> HashMap<String, String> {
        self.inner.child_process_env(node)
    }
    async fn augment_editor_buffer(&self, node: &NodeRef, buffer: String) -> String {
        self.inner.augment_editor_buffer(node, buffer).await
    }
    fn strip_editor_hints(&self, text: &str) -> String {
        self.inner.strip_editor_hints(text)
    }
    fn capabilities(&self) -> AdapterCapabilities {
        self.inner.capabilities()
    }
    fn has_active_tracking(&self) -> bool {
        self.inner.has_active_tracking()
    }

    async fn list_values(&self, source: &str) -> Result<Vec<ValueOption>> {
        let mut values = self.inner.list_values(source).await?;
        for opt in values.iter_mut() {
            // Scrub the visible label; keep `value` (the opaque handle fed back
            // into the value-accepting action) intact.
            opt.label = self.anon.scrub_value("label", &opt.label);
        }
        Ok(values)
    }

    fn subscribe_status(&self) -> tokio::sync::watch::Receiver<AdapterStatus> {
        self.inner.subscribe_status()
    }
    fn subscribe_invalidations(&self) -> tokio::sync::broadcast::Receiver<Invalidation> {
        self.inner.subscribe_invalidations()
    }

    async fn live_rows(&self) -> Vec<NodeSummary> {
        let mut rows = self.inner.live_rows().await;
        for row in rows.iter_mut() {
            self.anon.scrub_summary(row);
        }
        rows
    }
    async fn bucket_for_now(&self, group_by: &GroupSpec) -> Option<String> {
        // Returns a bucket *id* used to target a reload, not display text.
        self.inner.bucket_for_now(group_by).await
    }
    async fn live_group_rows(&self, group_by: &GroupSpec, query: Option<&str>) -> Vec<NodeSummary> {
        let mut rows = self.inner.live_group_rows(group_by, query).await;
        for row in rows.iter_mut() {
            self.anon.scrub_summary(row);
        }
        rows
    }

    async fn revalidate(&self) {
        self.inner.revalidate().await
    }
    async fn submit_credentials(&self, fields: HashMap<String, String>) -> Result<()> {
        self.inner.submit_credentials(fields).await
    }

    async fn cancel_credentials(&self) -> Result<()> {
        self.inner.cancel_credentials().await
    }
    async fn try_refresh_session(&self) -> Result<()> {
        self.inner.try_refresh_session().await
    }
    async fn invalidate_session(&self) -> Result<()> {
        self.inner.invalidate_session().await
    }
    async fn invalidate_credentials(&self) -> Result<()> {
        self.inner.invalidate_credentials().await
    }
    async fn load_view_sort(&self, scope: &str) -> Result<Vec<SortKey>> {
        self.inner.load_view_sort(scope).await
    }
    async fn save_view_sort(&self, scope: &str, sort: &[SortKey]) -> Result<()> {
        self.inner.save_view_sort(scope, sort).await
    }
    fn query_variables(&self, query: &str) -> Vec<QueryVariable> {
        self.inner.query_variables(query)
    }
    fn render_query(&self, query: &str, vars: &HashMap<String, String>) -> String {
        self.inner.render_query(query, vars)
    }
    async fn execute_custom_query(
        &self,
        query: &str,
        context: &CustomQueryContext,
    ) -> Result<CustomQueryResult> {
        // Custom-query results feed the query editor (a write/inspect path), not
        // the row display — left unscrubbed deliberately (see module docs).
        self.inner.execute_custom_query(query, context).await
    }
    fn saved_query_store(&self) -> Option<&dyn SavedQueryStore> {
        self.inner.saved_query_store()
    }
    /// Forwarded, unlike most display-facing strings: it names the *language*
    /// the wrapped adapter's queries are written in — which is also where
    /// `query_language()` derives from, so leaving it at the default would
    /// have an anonymised Jira view reject its own `jql` fences.
    fn query_body_suffix(&self) -> &str {
        self.inner.query_body_suffix()
    }
    fn script_store(&self) -> Option<&dyn ScriptStore> {
        self.inner.script_store()
    }
    async fn search_in_tree(&self, query: &str, limit: u32) -> Result<Option<TreeSearchResults>> {
        let mut results = self.inner.search_in_tree(query, limit).await?;
        if let Some(results) = results.as_mut() {
            for hit in results.hits.iter_mut() {
                // `path` is addressing (node ids) — leave it; scrub display text.
                hit.label = self.anon.scrub_value("label", &hit.label);
                hit.space_key = self.anon.scrub_value("space_key", &hit.space_key);
            }
        }
        Ok(results)
    }
    /// Pure addressing (node ids) — nothing to scrub, just forward.
    async fn locate_node_path(&self, node_id: &str) -> Result<Option<Vec<String>>> {
        self.inner.locate_node_path(node_id).await
    }
    fn hooks(&self) -> Vec<&str> {
        self.inner.hooks()
    }
    fn anonymizer(&self) -> Arc<dyn Anonymizer> {
        self.anon.clone()
    }
    async fn describe_columns(&self, node_type: &str) -> Vec<ColumnSchema> {
        // Delegate: this is the outer wrapper, so the default (empty) would
        // shadow any columns the inner adapter (e.g. custom-columns) describes.
        // The schema carries only structural metadata (key/type), not user
        // values — those flow through the scrubbed metadata path — so it needs
        // no anonymization of its own; labels are scrubbed defensively.
        self.inner
            .describe_columns(node_type)
            .await
            .into_iter()
            .map(|mut c| {
                if let Some(label) = c.label.take() {
                    c.label = Some(self.anon.scrub_value("label", &label));
                }
                c
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Node decorator
// ---------------------------------------------------------------------------

fn wrap_node(inner: Box<dyn Node>, anon: Arc<dyn Anonymizer>) -> Box<dyn Node> {
    Box::new(AnonymizingNode::new(inner, anon))
}

/// Recursively scrub every summary in an eagerly-expanded subtree.
fn scrub_subtree(subtree: &mut Subtree, anon: &dyn Anonymizer) {
    for node in subtree.items.iter_mut() {
        anon.scrub_summary(&mut node.summary);
        scrub_subtree(&mut node.children, anon);
    }
}

/// Wraps a [`Node`]. The borrow-returning accessors (`label`, `metadata`) can't
/// scrub on the fly, so the anonymized projections are computed once at
/// construction and refreshed in [`Node::hydrate`] (the only call that mutates
/// a node's display fields). All owned/async returns are scrubbed per call.
pub struct AnonymizingNode {
    inner: Box<dyn Node>,
    anon: Arc<dyn Anonymizer>,
    label: String,
    metadata: Metadata,
}

impl AnonymizingNode {
    fn new(inner: Box<dyn Node>, anon: Arc<dyn Anonymizer>) -> Self {
        let label = anon.scrub_label(inner.node_type(), inner.label());
        let mut metadata = inner.metadata().clone();
        anon.scrub_metadata(&mut metadata);
        Self {
            inner,
            anon,
            label,
            metadata,
        }
    }

    fn refresh_cached(&mut self) {
        self.label = self
            .anon
            .scrub_label(self.inner.node_type(), self.inner.label());
        self.metadata = self.inner.metadata().clone();
        self.anon.scrub_metadata(&mut self.metadata);
    }
}

#[async_trait]
impl Node for AnonymizingNode {
    fn id(&self) -> &str {
        self.inner.id() // addressing — never anonymized
    }
    fn label(&self) -> &str {
        &self.label
    }
    fn node_type(&self) -> &NodeType {
        self.inner.node_type()
    }
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    async fn hydrate(&mut self) {
        self.inner.hydrate().await;
        self.refresh_cached();
    }

    fn row_summary(&self) -> NodeSummary {
        // Use the adapter's own list-row projection (Jira et al. diverge from
        // metadata()), then scrub it — don't reconstruct from our cached fields.
        let mut summary = self.inner.row_summary();
        self.anon.scrub_summary(&mut summary);
        summary
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        Ok(wrap_node(
            self.inner.get_child(id).await?,
            self.anon.clone(),
        ))
    }

    fn content(&self) -> Option<&dyn Content> {
        // Editable/exportable body — left raw to avoid overwrite-on-save.
        self.inner.content()
    }
    async fn invoke_action(&self, name: &str, ctx: &ActionContext) -> Result<ActionDispatch> {
        self.inner.invoke_action(name, ctx).await
    }
    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        self.inner.prepare(action_id).await
    }
    async fn picker_options(&self, action_id: &str) -> Result<Vec<ActionOption>> {
        self.inner.picker_options(action_id).await
    }
    async fn form_prep(&self, action_id: &str) -> Result<HashMap<String, String>> {
        self.inner.form_prep(action_id).await
    }
    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        self.inner.execute(action_id, input).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(key: &str, value: &str) -> MetadataField {
        MetadataField {
            key: key.into(),
            value: value.into(),
            display_label: key.into(),
            editable: false,
            allowed_values: None,
        }
    }

    #[test]
    fn standard_leaves_numbers_durations_dates_empty() {
        let a = StandardAnonymizer::new();
        for v in [
            "",
            "   ",
            "42",
            "3.5",
            "2026-06-22T10:00:00Z",
            "2026-06-22",
            "1:30:00",
            "2h 15m",
        ] {
            assert_eq!(
                a.scrub_value("x", v),
                v,
                "structural value must survive: {v:?}"
            );
        }
    }

    #[test]
    fn standard_replaces_free_text_deterministically() {
        let a = StandardAnonymizer::new();
        let once = a.scrub_value("label", "Call Example Corp");
        let twice = a.scrub_value("label", "Call Example Corp");
        assert_eq!(once, twice, "must be deterministic");
        assert!(!once.contains("Example"), "real token must not survive");
        assert_eq!(once.split_whitespace().count(), 3, "word count preserved");
    }

    #[test]
    fn standard_keeps_same_token_consistent_across_values() {
        let a = StandardAnonymizer::new();
        // "Acme" maps the same wherever it appears -> referential consistency.
        let from_a = a.scrub_value("label", "Acme report");
        let from_b = a.scrub_value("project", "Acme");
        let acme_in_a = from_a.split_whitespace().next().unwrap();
        assert_eq!(acme_in_a, from_b);
    }

    #[test]
    fn scrub_summary_hits_label_and_fields_not_id() {
        let a = StandardAnonymizer::new();
        let mut s = NodeSummary {
            id: "task-123".into(),
            label: "Secret Project".into(),
            node_type: NodeType {
                type_id: "t".into(),
                mime_type: "text/plain".into(),
                syntax: None,
                file_extension: String::new(),
                display_name: "T".into(),
            },
            metadata: Metadata {
                fields: vec![
                    field("summary", "Confidential thing"),
                    field("minutes", "90"),
                ],
            },
            has_children: None,
        };
        a.scrub_summary(&mut s);
        assert_eq!(s.id, "task-123", "id is addressing, never scrubbed");
        assert!(!s.label.contains("Secret"));
        assert!(!s.metadata.fields[0].value.contains("Confidential"));
        assert_eq!(s.metadata.fields[1].value, "90", "numeric field preserved");
    }

    #[test]
    fn shared_helpers_are_deterministic_safe_and_format_preserving() {
        // Person: stable, drawn from the pool, real text gone.
        let p1 = pseudo_person("Jane Roe");
        assert_eq!(p1, pseudo_person("Jane Roe"));
        assert!(PERSON_POOL.contains(&p1.as_str()));
        assert!(!p1.contains("Roe"));
        // Username/email derive from the same name → consistent.
        assert_eq!(
            pseudo_username("Jane Roe"),
            p1.to_lowercase().replace(' ', ".")
        );
        assert!(pseudo_email("Jane Roe").ends_with("@example.invalid"));

        // Issue key: prefix mapped, numeric tail kept, still key-shaped.
        let k = pseudo_issue_key("DEMO-4711");
        assert!(k.ends_with("-4711"), "issue counter preserved: {k}");
        assert!(!k.contains("DEMO"));
        assert!(CODE_POOL.contains(&k.split('-').next().unwrap()));
        // Non-key falls back to free text, never verbatim.
        assert!(!pseudo_issue_key("Secret Phrase").contains("Secret"));

        // Taiga ref: slug mapped lowercase, #num kept; bare ref untouched.
        let r = pseudo_ref("demoproject#12");
        assert!(r.ends_with("#12") && !r.contains("demoproject"));
        assert_eq!(pseudo_ref("#42"), "#42");

        // Project / space code: mapped, letterless passes through.
        assert!(CODE_POOL.contains(&pseudo_project_code("DEMO").as_str()));
        assert_eq!(pseudo_project_code("123"), "123");

        // Filename: extension preserved, stem scrubbed.
        let f = pseudo_filename("Customer quote.pdf");
        assert!(f.ends_with(".pdf") && !f.contains("Customer") && !f.contains("quote"));
    }

    #[test]
    fn pseudo_labeled_keeps_noun_hides_name_is_deterministic() {
        let a = pseudo_labeled("customer_prod", "database");
        assert!(a.ends_with("_database"), "noun (kind) preserved: {a}");
        assert!(!a.contains("customer"), "real name must not survive: {a}");
        assert_eq!(
            a,
            pseudo_labeled("customer_prod", "database"),
            "deterministic"
        );
        // Adjective is keyed by the value: same value, different noun → same adj.
        let adj_db = pseudo_labeled("billing", "database");
        let adj_schema = pseudo_labeled("billing", "schema");
        assert_eq!(
            adj_db.rsplit_once('_').unwrap().0,
            adj_schema.rsplit_once('_').unwrap().0,
            "same source name → same adjective across nouns",
        );
        assert!(ADJ_POOL.contains(&adj_db.rsplit_once('_').unwrap().0));
        // Letterless passes through (nothing to leak).
        assert_eq!(pseudo_labeled("123", "table"), "123");
    }

    #[test]
    fn scrub_label_default_matches_label_keyed_scrub_value() {
        // The default scrub_label must behave exactly like the historical
        // scrub_value("label", …) so non-overriding adapters are unaffected.
        let a = StandardAnonymizer::new();
        let nt = NodeType {
            type_id: "whatever".into(),
            mime_type: "text/plain".into(),
            syntax: None,
            file_extension: String::new(),
            display_name: "W".into(),
        };
        assert_eq!(
            a.scrub_label(&nt, "Secret Project Phrase"),
            a.scrub_value("label", "Secret Project Phrase"),
        );
    }

    #[test]
    fn stable_hash_is_fixed() {
        // Guards the cross-run determinism promise.
        assert_eq!(stable_hash(""), 0);
        assert_eq!(stable_hash("a"), 97);
        assert_eq!(stable_hash("ab"), 97 * 31 + 98);
    }
}
