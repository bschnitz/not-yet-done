//! Headless behaviour tests for the spec-driven [`Form`] driver.
//!
//! The form is driven exactly as the application drives it: normalized `&str`
//! key strings in, [`FormEvent`] out. No terminal is needed.

use std::collections::HashMap;

use not_yet_done_ratatui::{Form, FormEvent, FormFieldSpec, FormOptions, FormStyle, SelectStyle};

fn style() -> FormStyle {
    FormStyle::default()
}

fn opts() -> FormOptions {
    FormOptions::default()
}

fn no_prefill() -> HashMap<String, String> {
    HashMap::new()
}

/// Types each character of `s` into the currently focused field.
fn type_str(form: &mut Form, s: &str) {
    for ch in s.chars() {
        assert_eq!(form.handle_key(&ch.to_string()), FormEvent::Consumed);
    }
}

fn submit(form: &mut Form) -> FormEvent {
    form.handle_key("enter")
}

#[test]
fn typing_into_text_field() {
    let specs = vec![FormFieldSpec::text("title", "Title")];
    let mut form = Form::new("T", specs, &no_prefill(), &style(), &opts());

    type_str(&mut form, "Hello");
    assert_eq!(form.values().get("title").unwrap(), "Hello");
}

#[test]
fn backspace_deletes_last_char() {
    let specs = vec![FormFieldSpec::text("title", "Title")];
    let mut form = Form::new("T", specs, &no_prefill(), &style(), &opts());

    type_str(&mut form, "abc");
    form.handle_key("backspace");
    assert_eq!(form.values().get("title").unwrap(), "ab");
}

#[test]
fn select_pick_option_by_cursor() {
    let specs = vec![
        FormFieldSpec::text("title", "Title"),
        FormFieldSpec::select(
            "cal",
            "Calendar",
            vec!["Personal".into(), "Work".into(), "Shared".into()],
        ),
    ];
    let mut form = Form::new("T", specs, &no_prefill(), &style(), &opts());

    // Move focus from the text field to the select.
    form.handle_key("tab");
    // Single-choice: the selection follows the cursor, so moving it picks the
    // highlighted option — no separate space press.
    form.handle_key("down");

    assert_eq!(form.values().get("cal").unwrap(), "Work");
}

#[test]
fn select_up_down_do_not_change_field_when_in_select() {
    let specs = vec![
        FormFieldSpec::select("a", "A", vec!["x".into(), "y".into()]),
        FormFieldSpec::text("b", "B"),
    ];
    let mut form = Form::new("T", specs, &no_prefill(), &style(), &opts());

    // Focus starts on the select. 'down' moves the option cursor (and, being a
    // single-choice select, picks it), not the field.
    form.handle_key("down");
    assert_eq!(form.values().get("a").unwrap(), "y");
    // The text field must still be empty (focus never left the select).
    assert_eq!(form.values().get("b").unwrap(), "");
}

#[test]
fn toggle_flips_with_space() {
    let specs = vec![FormFieldSpec::toggle("all_day", "All day")];
    let mut form = Form::new("T", specs, &no_prefill(), &style(), &opts());

    assert_eq!(form.values().get("all_day").unwrap(), "false");
    form.handle_key(" ");
    assert_eq!(form.values().get("all_day").unwrap(), "true");
    form.handle_key(" ");
    assert_eq!(form.values().get("all_day").unwrap(), "false");
}

#[test]
fn datetime_keeps_raw_phrase() {
    let specs = vec![FormFieldSpec::datetime("start", "Start", true)];
    let mut form = Form::new("T", specs, &no_prefill(), &style(), &opts());

    type_str(&mut form, "tomorrow 9am");
    assert_eq!(form.values().get("start").unwrap(), "tomorrow 9am");
}

#[test]
fn required_field_blocks_submit() {
    let specs = vec![FormFieldSpec::text("title", "Title")];
    let mut form = Form::new("T", specs, &no_prefill(), &style(), &opts());

    // Empty required field: submit is swallowed.
    assert_eq!(submit(&mut form), FormEvent::Consumed);

    type_str(&mut form, "x");
    match submit(&mut form) {
        FormEvent::Submitted(values) => assert_eq!(values.get("title").unwrap(), "x"),
        other => panic!("expected Submitted, got {other:?}"),
    }
}

#[test]
fn optional_field_allows_submit_empty() {
    let specs = vec![FormFieldSpec::text("note", "Note").optional()];
    let mut form = Form::new("T", specs, &no_prefill(), &style(), &opts());

    match submit(&mut form) {
        FormEvent::Submitted(values) => assert_eq!(values.get("note").unwrap(), ""),
        other => panic!("expected Submitted, got {other:?}"),
    }
}

#[test]
fn esc_cancels() {
    let specs = vec![FormFieldSpec::text("title", "Title")];
    let mut form = Form::new("T", specs, &no_prefill(), &style(), &opts());
    assert_eq!(form.handle_key("esc"), FormEvent::Cancelled);
}

#[test]
fn prefill_populates_values() {
    let specs = vec![
        FormFieldSpec::text("title", "Title"),
        FormFieldSpec::select("cal", "Calendar", vec!["Personal".into(), "Work".into()]),
        FormFieldSpec::toggle("all_day", "All day"),
    ];
    let mut prefill = HashMap::new();
    prefill.insert("title".to_string(), "Standup".to_string());
    prefill.insert("cal".to_string(), "Work".to_string());
    prefill.insert("all_day".to_string(), "true".to_string());

    let form = Form::new("T", specs, &prefill, &style(), &opts());
    let values = form.values();
    assert_eq!(values.get("title").unwrap(), "Standup");
    assert_eq!(values.get("cal").unwrap(), "Work");
    assert_eq!(values.get("all_day").unwrap(), "true");
}

#[test]
fn default_populates_when_no_prefill() {
    let specs = vec![FormFieldSpec::text("title", "Title").with_default("Untitled")];
    let form = Form::new("T", specs, &no_prefill(), &style(), &opts());
    assert_eq!(form.values().get("title").unwrap(), "Untitled");
}

#[cfg(feature = "natural-date")]
#[test]
fn datetime_preview_resolves_injected_now() {
    use chrono::{TimeZone, Utc};
    let now = Utc.with_ymd_and_hms(2026, 7, 18, 10, 0, 0).unwrap();
    // A machine format resolves deterministically regardless of parser nuances.
    let out = not_yet_done_ratatui::datetime_preview("2026-07-20 15:30", true, now);
    assert_eq!(out.as_deref(), Some("2026-07-20 15:30"));

    let all_day = not_yet_done_ratatui::datetime_preview("2026-07-20 15:30", false, now);
    assert_eq!(all_day.as_deref(), Some("2026-07-20"));

    assert_eq!(
        not_yet_done_ratatui::datetime_preview("   ", true, now),
        None
    );
}

#[test]
fn tab_navigation_targets_the_right_field() {
    let specs = vec![FormFieldSpec::text("a", "A"), FormFieldSpec::text("b", "B")];
    let mut form = Form::new("T", specs, &no_prefill(), &style(), &opts());

    type_str(&mut form, "one");
    form.handle_key("tab");
    type_str(&mut form, "two");

    let values = form.values();
    assert_eq!(values.get("a").unwrap(), "one");
    assert_eq!(values.get("b").unwrap(), "two");

    // shift+tab goes back to the first field.
    form.handle_key("shift+tab");
    type_str(&mut form, "!");
    assert_eq!(form.values().get("a").unwrap(), "one!");
}

#[test]
fn inline_select_picks_option_by_cursor() {
    let specs = vec![
        FormFieldSpec::text("title", "Title"),
        FormFieldSpec::select("cal", "Calendar", vec!["Personal".into(), "Work".into()]),
    ];
    let options = FormOptions {
        select_style: SelectStyle::Inline,
        ..FormOptions::default()
    };
    let mut form = Form::new("T", specs, &no_prefill(), &style(), &options);

    // Inline selects drive exactly like dropdowns: tab onto it, move the cursor,
    // and the selection follows.
    form.handle_key("tab");
    form.handle_key("down");
    assert_eq!(form.values().get("cal").unwrap(), "Work");
}

#[test]
fn inline_select_up_down_stay_in_select() {
    let specs = vec![
        FormFieldSpec::select("a", "A", vec!["x".into(), "y".into()]),
        FormFieldSpec::text("b", "B"),
    ];
    let options = FormOptions {
        select_style: SelectStyle::Inline,
        ..FormOptions::default()
    };
    let mut form = Form::new("T", specs, &no_prefill(), &style(), &options);

    form.handle_key("down");
    assert_eq!(form.values().get("a").unwrap(), "y");
    assert_eq!(form.values().get("b").unwrap(), "");
}

#[test]
fn two_column_form_collects_all_values() {
    // Explicit column assignment must not disturb field identity / navigation.
    let specs = vec![
        FormFieldSpec::text("a", "A"),
        FormFieldSpec::text("b", "B"),
        FormFieldSpec::text("c", "C"),
    ];
    let options = FormOptions {
        columns: 2,
        column_of: vec![0, 1, 0],
        ..FormOptions::default()
    };
    let mut form = Form::new("T", specs, &no_prefill(), &style(), &options);

    type_str(&mut form, "1");
    form.handle_key("tab");
    type_str(&mut form, "2");
    form.handle_key("tab");
    type_str(&mut form, "3");

    let v = form.values();
    assert_eq!(v.get("a").unwrap(), "1");
    assert_eq!(v.get("b").unwrap(), "2");
    assert_eq!(v.get("c").unwrap(), "3");
}

/// A masked field hides its value on screen but hands the real one back:
/// masking is a rendering decision, never a change to what is submitted.
#[test]
fn masked_field_renders_bullets_but_yields_the_real_value() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let specs = vec![
        FormFieldSpec::text("user", "User"),
        FormFieldSpec::text("password", "Password").masked(),
    ];
    let mut form = Form::new("Login", specs, &no_prefill(), &style(), &opts());

    type_str(&mut form, "alice");
    form.handle_key("tab");
    type_str(&mut form, "hunter2");

    assert_eq!(form.values().get("user").unwrap(), "alice");
    assert_eq!(form.values().get("password").unwrap(), "hunter2");

    let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
    terminal
        .draw(|frame| form.render(frame, frame.area()))
        .unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();

    assert!(screen.contains("•••••••"), "the password must be bulleted");
    assert!(
        !screen.contains("hunter2"),
        "the password must not be legible"
    );
    assert!(screen.contains("alice"), "an unmasked field stays legible");
}

/// The panel sizes itself to its content: a one-field dialog leaves exactly one
/// blank row between the input and the submit bar, however tall the area is.
#[test]
fn the_panel_does_not_pad_a_small_form() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let specs = vec![FormFieldSpec::text("passphrase", "GPG passphrase")];
    let opts = FormOptions {
        field_bar: true,
        ..FormOptions::default()
    };
    let mut form = Form::new(
        "Password store locked",
        specs,
        &no_prefill(),
        &style(),
        &opts,
    );
    type_str(&mut form, "secret");

    let mut terminal = Terminal::new(TestBackend::new(60, 30)).unwrap();
    terminal
        .draw(|frame| form.render(frame, frame.area()))
        .unwrap();
    let buf = terminal.backend().buffer();
    let rows: Vec<String> = (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect();

    let input_row = rows
        .iter()
        .position(|r| r.contains("secret"))
        .expect("input row");
    let submit_row = rows
        .iter()
        .position(|r| r.contains("Save"))
        .expect("submit bar");
    assert_eq!(
        submit_row - input_row,
        2,
        "expected one blank row between input and submit bar, got:\n{}",
        rows[input_row..=submit_row].join("\n")
    );
}

/// The caller's notice outranks the key hints, a validation error outranks the
/// notice, and neither survives into the collected values.
#[test]
fn notice_and_validation_error_share_the_footer() {
    use not_yet_done_ratatui::FormNotice;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let specs = vec![FormFieldSpec::text("token", "Token")];
    let mut form = Form::new("Login", specs, &no_prefill(), &style(), &opts());

    let render = |form: &mut Form| {
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal
            .draw(|frame| form.render(frame, frame.area()))
            .unwrap();
        let s: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        s
    };

    form.set_notice(Some(FormNotice::Alert("wrong passphrase".into())));
    assert!(render(&mut form).contains("wrong passphrase"));

    // Submitting empty raises the form's own error, which wins while it stands.
    assert_eq!(submit(&mut form), FormEvent::Consumed);
    let screen = render(&mut form);
    assert!(screen.contains("This field is required"));
    assert!(!screen.contains("wrong passphrase"));

    // The notice survives the validation error being cleared by navigation —
    // it explains why the form is on screen at all, so only the caller drops it.
    form.handle_key("tab");
    assert!(render(&mut form).contains("wrong passphrase"));
    form.set_notice(None);
    assert!(!render(&mut form).contains("wrong passphrase"));
}
