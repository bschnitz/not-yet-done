//! The shipped `examples/release.md` is living documentation of the file format.
//! This test guards it against parser drift: if the format changes in a way that
//! breaks the example, this fails and the example (or the parser) must be fixed.

use not_yet_done_workflow::{parse_workflow, RouteCondition, RouteTarget, StepMode};

fn load_example() -> not_yet_done_workflow::WorkflowDef {
    let raw = include_str!("../examples/release.md");
    parse_workflow("release", raw)
}

#[test]
fn example_release_parses_as_documented() {
    let wf = load_example();

    // Frontmatter.
    assert_eq!(wf.name, "release");
    assert_eq!(wf.title, "Release cutting");
    assert_eq!(wf.mode, StepMode::Manual);
    assert_eq!(wf.log_runs, Some(true));
    assert!(wf.description.starts_with("Any prose before the first"));

    // Steps, in document order — ids come from the `yaml meta` blocks.
    let ids: Vec<&str> = wf.steps.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "build",
            "tests-green",
            "recover",
            "update-changelog",
            "tag",
            "announce",
            "update-website",
        ]
    );

    // Build: auto command, routes green→tests-green / else→fail.
    let build = wf.step("build").unwrap();
    assert_eq!(build.command.as_deref(), Some("cargo build --release"));
    assert_eq!(build.resolved_mode(wf.mode), StepMode::Auto);
    assert_eq!(
        build.routes[0].condition,
        RouteCondition::Expr("exit == 0".into())
    );
    assert_eq!(
        build.routes[0].targets,
        vec![RouteTarget::Step("tests-green".into())]
    );
    assert_eq!(build.routes[1].condition, RouteCondition::Else);
    assert_eq!(build.routes[1].targets, vec![RouteTarget::Fail]);

    // Tests: convenience guards on_success / on_failure.
    let tests = wf.step("tests-green").unwrap();
    assert_eq!(tests.routes[0].condition, RouteCondition::OnSuccess);
    assert_eq!(
        tests.routes[0].targets,
        vec![RouteTarget::Step("update-changelog".into())]
    );
    assert_eq!(tests.routes[1].condition, RouteCondition::OnFailure);
    assert_eq!(
        tests.routes[1].targets,
        vec![RouteTarget::Step("recover".into())]
    );

    // Recover: manual, no routing → linear fall-through.
    let recover = wf.step("recover").unwrap();
    assert_eq!(recover.mode, Some(StepMode::Manual));
    assert!(!recover.has_routing());

    // Changelog: an AI step — instruction prose, no command.
    let changelog = wf.step("update-changelog").unwrap();
    assert_eq!(changelog.mode, Some(StepMode::Ai));
    assert!(changelog.command.is_none());
    assert!(changelog.description.contains("Summarise the commits"));

    // Tag: manual; its non-`command` fence is kept verbatim, not run; it fans
    // out to both notification steps.
    let tag = wf.step("tag").unwrap();
    assert_eq!(tag.mode, Some(StepMode::Manual));
    assert!(tag.command.is_none());
    assert!(tag.description.contains("git tag -a vX.Y.Z"));
    assert_eq!(tag.routes[0].condition, RouteCondition::Else);
    assert_eq!(
        tag.routes[0].targets,
        vec![
            RouteTarget::Step("announce".into()),
            RouteTarget::Step("update-website".into()),
        ]
    );

    // Notify steps: optional, auto, each routing to `end`.
    for id in ["announce", "update-website"] {
        let s = wf.step(id).unwrap();
        assert_eq!(s.mode, Some(StepMode::Auto));
        assert!(s.optional, "{id} should be optional");
        assert_eq!(s.routes[0].targets, vec![RouteTarget::End]);
    }
}
