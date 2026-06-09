mod common;

use predicates::prelude::*;

// ── helpers ──────────────────────────────────────────────────────────────────

fn create_child(db_url: &str, description: &str, parent_id: &str) -> String {
    let output = common::nyd(db_url)
        .args(["task", "add", description, "--parent", parent_id])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).expect("Invalid UTF-8");
    common::parse_bracketed_uuid(&stdout)
        .unwrap_or_else(|| panic!("Could not parse task UUID from: {stdout}"))
}

/// Build a small tree:
///   root
///     ├─ child_a
///     │   └─ leaf_a1
///     └─ child_b
///         └─ leaf_b1
fn build_tree(db_url: &str) -> (String, String, String, String, String) {
    let root = common::create_task(db_url, "Root");
    let child_a = create_child(db_url, "ChildA", &root);
    let leaf_a1 = create_child(db_url, "LeafA1", &child_a);
    let child_b = create_child(db_url, "ChildB", &root);
    let leaf_b1 = create_child(db_url, "LeafB1", &child_b);
    (root, child_a, leaf_a1, child_b, leaf_b1)
}

// ── task tree <id> ──────────────────────────────────────────────────────────

#[test]
fn tree_returns_nested_json() {
    let (_dir, db_url) = common::setup();
    let (root, _child_a, _leaf_a1, _child_b, _leaf_b1) = build_tree(&db_url);

    let output = common::nyd(&db_url)
        .args(["task", "tree", &root])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: serde_json::Value =
        serde_json::from_slice(&output).expect("Output must be valid JSON");

    // Top level is an array with one root node.
    let arr = json.as_array().expect("Expected JSON array");
    assert_eq!(arr.len(), 1, "Expected one root node");

    let root_node = &arr[0];
    assert_eq!(root_node["description"], "Root");

    let children = root_node["children"].as_array().unwrap();
    assert_eq!(children.len(), 2, "Root should have 2 children");

    let descs: Vec<&str> = children.iter()
        .map(|c| c["description"].as_str().unwrap())
        .collect();
    assert!(descs.contains(&"ChildA"));
    assert!(descs.contains(&"ChildB"));

    // Check nesting: ChildA should have LeafA1.
    let child_a_node = children.iter().find(|c| c["description"] == "ChildA").unwrap();
    let a_children = child_a_node["children"].as_array().unwrap();
    assert_eq!(a_children.len(), 1);
    assert_eq!(a_children[0]["description"], "LeafA1");
}

#[test]
fn tree_with_pretty_flag_produces_indented_json() {
    let (_dir, db_url) = common::setup();
    let (root, ..) = build_tree(&db_url);

    let output = common::nyd(&db_url)
        .args(["task", "tree", &root, "--pretty"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).unwrap();
    // Pretty JSON has newlines and indentation.
    assert!(stdout.contains('\n'));
    assert!(stdout.contains("  "));
}

#[test]
fn tree_by_description_prefix() {
    let (_dir, db_url) = common::setup();
    let (_root, ..) = build_tree(&db_url);

    common::nyd(&db_url)
        .args(["task", "tree", "Root"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Root"));
}

#[test]
fn tree_nonexistent_id_fails() {
    let (_dir, db_url) = common::setup();

    common::nyd(&db_url)
        .args(["task", "tree", "00000000-0000-0000-0000-000000000000"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error:"));
}

#[test]
fn tree_ambiguous_description_fails() {
    let (_dir, db_url) = common::setup();
    // Create two tasks with the same prefix.
    common::create_task(&db_url, "Ambiguous Alpha");
    common::create_task(&db_url, "Ambiguous Beta");

    common::nyd(&db_url)
        .args(["task", "tree", "Ambiguous"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Ambiguous"));
}

// ── task tree --last-tracked-since ──────────────────────────────────────────

#[test]
fn tree_last_tracked_since_prunes_untracked_leaves() {
    let (_dir, db_url) = common::setup();
    let (root, _child_a, leaf_a1, _child_b, _leaf_b1) = build_tree(&db_url);

    // Track leaf_a1 (this sets last_tracked_at), but not leaf_b1.
    common::start_tracking(&db_url, &leaf_a1);
    common::nyd(&db_url).args(["track", "stop"]).assert().success();

    // Use a date in the past so the tracking qualifies.
    let output = common::nyd(&db_url)
        .args(["task", "tree", &root, "--last-tracked-since", "2020-01-01"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let stdout = serde_json::to_string(&json).unwrap();

    // LeafA1 should be present (it was tracked).
    assert!(stdout.contains("LeafA1"), "LeafA1 should be in output");

    // LeafB1 should be pruned (never tracked).
    assert!(!stdout.contains("LeafB1"), "LeafB1 should be pruned");

    // ChildB should also be pruned (no tracked descendants).
    assert!(!stdout.contains("ChildB"), "ChildB should be pruned (no tracked leaves)");

    // ChildA should still be there (ancestor of tracked LeafA1).
    assert!(stdout.contains("ChildA"), "ChildA should be present as ancestor");
}

#[test]
fn tree_last_tracked_since_future_date_returns_empty_tree() {
    let (_dir, db_url) = common::setup();
    let (root, _child_a, leaf_a1, ..) = build_tree(&db_url);

    // Track a leaf.
    common::start_tracking(&db_url, &leaf_a1);
    common::nyd(&db_url).args(["track", "stop"]).assert().success();

    // Use a future date so nothing qualifies.
    let output = common::nyd(&db_url)
        .args(["task", "tree", &root, "--last-tracked-since", "2099-01-01"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let arr = json.as_array().unwrap();
    assert!(arr.is_empty(), "Expected empty array for future date filter");
}
