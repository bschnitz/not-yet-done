//! The backend against small `sh` programs that speak the protocol.

use std::collections::HashMap;
use std::path::Path;

use chrono::{TimeZone, Utc};
use not_yet_done_content::{ActionInput, HostContext, InMemoryHostBus, InputSpec, PromptAnswer};

use super::*;

/// A backend program: says hello, notes every line it reads in `heard`, and
/// answers each call with what `answers` says for it — `case` arms that may
/// use `$id` and append to `$heard`.
fn program(dir: &Path, hello: &str, answers: &str) -> String {
    let heard = dir.join("heard");
    let script = dir.join("backend.sh");
    std::fs::write(
        &script,
        format!(
            r#"heard='{heard}'
read -r line; printf '%s\n' "$line" >> "$heard"
echo '{hello}'
while read -r line; do
  printf '%s\n' "$line" >> "$heard"
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
{answers}
    *'"cancel"'*) exit 0 ;;
  esac
done
"#,
            heard = heard.display(),
        ),
    )
    .unwrap();
    format!("sh '{}'", script.display())
}

const HELLO: &str = r#"{"hello":{"protocol":1}}"#;

fn heard(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("heard"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn backend(id: &str, yaml: &str) -> Box<dyn CalendarBackend> {
    let ctx = HostContext {
        event_bus: Arc::new(InMemoryHostBus::new(4)),
        anonymize: false,
    };
    ExternalBackendFactory::new()
        .create(id, yaml, &ctx)
        .expect("config is valid")
}

fn command_yaml(command: &str) -> String {
    format!("command: {}\n", serde_json::to_string(command).unwrap())
}

fn week() -> TimeRange {
    TimeRange::new(
        Utc.with_ymd_and_hms(2026, 1, 5, 0, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 1, 12, 0, 0, 0).unwrap(),
    )
}

#[tokio::test]
async fn events_come_back_labelled_with_the_connection() {
    let dir = tempfile::tempdir().unwrap();
    let command = program(
        dir.path(),
        r#"{"hello":{"protocol":1,"empty_is_authoritative":true}}"#,
        r#"    *'"list_events"'*) echo "{\"reply\":{\"id\":$id,\"ok\":[{\"uid\":\"a\",\"title\":\"Standup\",\"start\":\"2026-01-05T09:00:00Z\",\"end\":\"2026-01-05T09:15:00Z\",\"show_as\":\"busy\"},{\"uid\":\"b\",\"calendar\":\"Team\",\"title\":\"Review\",\"start\":\"2026-01-06T14:00:00Z\",\"end\":\"2026-01-06T15:00:00Z\"}]}}" ;;"#,
    );
    let backend = backend("work", &format!("{}name: Work\n", command_yaml(&command)));
    assert!(!backend.empty_is_authoritative(), "unknown before hello");

    let events = backend.list_events(&week()).await.expect("listed");

    assert_eq!(events.len(), 2);
    assert_eq!(events[0].title, "Standup");
    assert_eq!(events[0].calendar, "Work", "unnamed calendar takes the label");
    assert_eq!(events[1].calendar, "Team");
    assert!(backend.empty_is_authoritative(), "taken from hello");
}

#[tokio::test]
async fn start_carries_the_config_block_and_the_resolved_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let command = program(
        dir.path(),
        HELLO,
        r#"    *'"list_events"'*) echo "{\"reply\":{\"id\":$id,\"ok\":[]}}" ;;"#,
    );
    let yaml = format!(
        "{}secrets:\n  password:\n    type: literal\n    value: hunter2\nconfig:\n  account: alice@example.com\n",
        command_yaml(&command)
    );
    let backend = backend("work", &yaml);

    backend.list_events(&week()).await.expect("listed");

    let start: serde_json::Value = serde_json::from_str(&heard(dir.path())[0]).unwrap();
    assert_eq!(
        start,
        serde_json::json!({"start": {
            "protocol": 1,
            "connection": "work",
            "config": {"account": "alice@example.com"},
            "secrets": {"password": "hunter2"}
        }})
    );
}

#[tokio::test]
async fn a_call_the_backend_does_not_offer_is_no_failure() {
    let dir = tempfile::tempdir().unwrap();
    let command = program(
        dir.path(),
        HELLO,
        r#"    *'"list_events"'*) echo "{\"reply\":{\"id\":$id,\"ok\":[]}}" ;;
    *) echo "{\"reply\":{\"id\":$id,\"error\":{\"kind\":\"unsupported\",\"message\":\"not here\"}}}" ;;"#,
    );
    let backend = backend("work", &command_yaml(&command));

    let window = backend.toggle_window().await;
    assert!(
        matches!(&window, Err(CalendarError::Other(m)) if m.contains("not running")),
        "no window before the backend runs: {window:?}"
    );
    assert!(heard(dir.path()).is_empty(), "toggling started nothing");

    assert!(backend.list_calendars().await.expect("no targets").is_empty());
    let window = backend.toggle_window().await;
    assert!(
        matches!(&window, Err(CalendarError::Other(m)) if m.contains("no window")),
        "{window:?}"
    );
    let deleted = backend.delete_event("a").await;
    assert!(
        matches!(&deleted, Err(CalendarError::Other(m)) if m == "not here"),
        "{deleted:?}"
    );
}

#[tokio::test]
async fn error_kinds_reach_the_adapter_as_calendar_errors() {
    let dir = tempfile::tempdir().unwrap();
    let command = program(
        dir.path(),
        HELLO,
        r#"    *'"delete_event"'*) echo "{\"reply\":{\"id\":$id,\"error\":{\"kind\":\"not_found\",\"message\":\"gone\"}}}" ;;
    *'"list_events"'*) echo "{\"reply\":{\"id\":$id,\"error\":{\"kind\":\"auth\",\"message\":\"signed out\"}}}" ;;"#,
    );
    let backend = backend("work", &command_yaml(&command));

    assert!(matches!(
        backend.delete_event("a").await,
        Err(CalendarError::NotFound(m)) if m == "gone"
    ));
    assert!(matches!(
        backend.list_events(&week()).await,
        Err(CalendarError::Auth(m)) if m == "signed out"
    ));
}

#[tokio::test]
async fn connections_sharing_a_key_share_one_process() {
    let dir = tempfile::tempdir().unwrap();
    let command = program(
        dir.path(),
        HELLO,
        r#"    *'"list_events"'*) echo "{\"reply\":{\"id\":$id,\"ok\":[]}}" ;;"#,
    );
    let shared = format!("{}share: account\n", command_yaml(&command));
    let first = backend("mine", &shared);
    let second = backend("theirs", &shared);

    first.list_events(&week()).await.expect("listed");
    second.list_events(&week()).await.expect("listed");

    let starts = heard(dir.path())
        .iter()
        .filter(|l| l.starts_with("{\"start\""))
        .count();
    assert_eq!(starts, 1, "one process for both: {:?}", heard(dir.path()));
}

#[tokio::test]
async fn attention_and_progress_reach_the_subscribers() {
    let dir = tempfile::tempdir().unwrap();
    let command = program(
        dir.path(),
        HELLO,
        r#"    *'"list_events"'*)
      echo '{"progress":{"fraction":0.5,"done":false}}'
      echo '{"attention":"Approve the sign-in on your phone"}'
      echo "{\"reply\":{\"id\":$id,\"ok\":[]}}" ;;
    *'"list_calendars"'*)
      echo '{"attention":null}'
      echo '{"progress":{"fraction":1.0,"done":true}}'
      echo "{\"reply\":{\"id\":$id,\"ok\":[]}}" ;;"#,
    );
    let backend = backend("work", &command_yaml(&command));
    let mut ready = backend.subscribe_ready().expect("progress stream");
    let mut attention = backend.subscribe_attention().expect("attention");
    let patience = Duration::from_secs(5);

    backend.list_events(&week()).await.expect("listed");
    tokio::time::timeout(
        patience,
        attention.wait_for(|said| said.as_deref() == Some("Approve the sign-in on your phone")),
    )
    .await
    .expect("attention arrives in time")
    .expect("attention stream open");
    assert_eq!(backend.sign_in_prompts(), 1, "an attention asks a person");

    backend.list_calendars().await.expect("listed");
    tokio::time::timeout(patience, attention.wait_for(Option::is_none))
        .await
        .expect("attention withdrawn in time")
        .expect("attention stream open");
    assert_eq!(backend.sign_in_prompts(), 1, "withdrawing asks nobody");

    let first = ready.recv().await.expect("progress");
    assert_eq!(first.fraction, Some(0.5));
    assert!(!first.done);
    let last = tokio::time::timeout(patience, ready.recv())
        .await
        .expect("progress in time")
        .expect("progress");
    assert!(last.done);
}

#[tokio::test]
async fn a_form_is_put_to_the_person_and_their_answer_sent_back() {
    let dir = tempfile::tempdir().unwrap();
    let command = program(
        dir.path(),
        HELLO,
        r#"    *'"list_events"'*)
      echo '{"form":{"id":7,"header":"One-time code","fields":[{"name":"code","label":"Code"}]}}'
      read -r answer; printf '%s\n' "$answer" >> "$heard"
      echo "{\"reply\":{\"id\":$id,\"ok\":[]}}" ;;"#,
    );
    let backend = backend("work", &command_yaml(&command));
    let mut prompts = backend.take_prompt_requests().expect("prompt stream");
    assert!(backend.take_prompt_requests().is_none(), "taken once");

    let answering = tokio::spawn(async move {
        let request = prompts.recv().await.expect("a prompt");
        assert_eq!(request.source, "work");
        assert_eq!(request.prompt, "One-time code");
        let InputSpec::Form { fields } = &request.input else {
            panic!("a form");
        };
        assert_eq!(fields[0].key, "code");
        let values = HashMap::from([("code".to_string(), "123456".to_string())]);
        let _ = request
            .respond
            .send(PromptAnswer::Provided(ActionInput::Form(values)));
    });

    backend.list_events(&week()).await.expect("listed after the form");
    answering.await.unwrap();

    let input = heard(dir.path())
        .into_iter()
        .find(|l| l.contains("\"input\""))
        .expect("the answer went back");
    let input: serde_json::Value = serde_json::from_str(&input).unwrap();
    assert_eq!(
        input,
        serde_json::json!({"input": {"form": 7, "values": {"code": "123456"}}})
    );
    assert_eq!(backend.sign_in_prompts(), 1);
}

#[tokio::test]
async fn a_backend_of_another_protocol_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let command = program(dir.path(), r#"{"hello":{"protocol":99}}"#, "");
    let backend = backend("work", &command_yaml(&command));

    let listed = backend.list_events(&week()).await;
    assert!(
        matches!(&listed, Err(CalendarError::Config(m)) if m.contains("protocol 99")),
        "{listed:?}"
    );
}

#[tokio::test]
async fn a_backend_that_exits_is_started_again_on_the_next_call() {
    let dir = tempfile::tempdir().unwrap();
    let command = program(
        dir.path(),
        HELLO,
        r#"    *'"list_events"'*) echo "{\"reply\":{\"id\":$id,\"ok\":[]}}"; exit 0 ;;"#,
    );
    let backend = backend("work", &command_yaml(&command));

    backend.list_events(&week()).await.expect("first run");
    // Let the reader notice the exit.
    tokio::time::sleep(Duration::from_millis(200)).await;
    backend.list_events(&week()).await.expect("second run");

    let starts = heard(dir.path())
        .iter()
        .filter(|l| l.starts_with("{\"start\""))
        .count();
    assert_eq!(starts, 2);
}
