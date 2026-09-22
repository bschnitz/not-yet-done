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

/// The caller's notice outranks the key hints, a field message outranks the
/// notice, and neither survives into the collected values.
#[test]
fn notice_and_field_message_share_the_footer() {
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

    // The field message is about a value that is still there, so navigating
    // away no longer wipes it; editing the field does, because the value it
    // objected to is gone. The notice explains why the form is on screen at
    // all and only the caller drops it.
    form.handle_key("tab");
    assert!(render(&mut form).contains("This field is required"));
    type_str(&mut form, "t");
    let screen = render(&mut form);
    assert!(
        !screen.contains("This field is required"),
        "editing the field must drop its message"
    );
    assert!(screen.contains("wrong passphrase"));
    form.set_notice(None);
    assert!(!render(&mut form).contains("wrong passphrase"));
}

// --- field messages -------------------------------------------------------

/// Helper: renders `form` into `area` of a fresh test terminal and returns the
/// screen row by row, trailing blanks trimmed.
fn rows_of(form: &mut Form, w: u16, h: u16, area: ratatui::layout::Rect) -> Vec<String> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|frame| form.render(frame, area)).unwrap();
    let buf = terminal.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

/// A message attached to a field paints that field's label in the theme's error
/// colour and puts its text in the footer — no extra row per field, so nothing
/// on screen moves when a script complains.
#[test]
fn field_message_marks_the_label_and_fills_the_footer() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    let specs = vec![
        FormFieldSpec::text("title", "Title"),
        FormFieldSpec::text("hours", "Hours"),
    ];
    let mut form = Form::new("Work item", specs, &no_prefill(), &style(), &opts());
    form.set_field_errors(HashMap::from([(
        "hours".to_string(),
        "must be a number".to_string(),
    )]));

    let mut terminal = Terminal::new(TestBackend::new(60, 14)).unwrap();
    terminal
        .draw(|frame| form.render(frame, frame.area()))
        .unwrap();
    let buf = terminal.backend().buffer();

    let row_of = |needle: &str| {
        (0..buf.area.height)
            .find(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, *y)].symbol())
                    .collect::<String>()
                    .contains(needle)
            })
            .unwrap_or_else(|| panic!("no row contains {needle:?}"))
    };

    // The marked label wears the palette's error colour; the untouched one does
    // not — the colour comes from the style, never from a literal in the driver.
    let marked = row_of("Hours");
    let untouched = row_of("Title");
    let fg_at = |x: u16, y: u16| buf[(x, y)].fg;
    assert_eq!(fg_at(2, marked), Color::Red, "marked label");
    assert_eq!(fg_at(1, marked), Color::Red, "marked gutter");
    assert_ne!(fg_at(2, untouched), Color::Red, "untouched label");

    // The text itself is in the footer, and only while its field is focused.
    let screen: String = buf.content().iter().map(|c| c.symbol()).collect();
    assert!(
        !screen.contains("must be a number"),
        "the message belongs to the unfocused field, so the footer stays quiet"
    );
    form.handle_key("tab");
    let rows = rows_of(&mut form, 60, 14, ratatui::layout::Rect::new(0, 0, 60, 14));
    assert!(
        rows.iter().any(|r| r.contains("must be a number")),
        "focusing the field puts its message in the footer:\n{}",
        rows.join("\n")
    );
}

/// `set_field_errors` replaces the whole set — a round answers with every
/// message it still stands by — and quietly drops keys that name no field, so
/// a typo in a script cannot conjure a message onto nothing.
#[test]
fn set_field_errors_replaces_the_set_and_ignores_unknown_keys() {
    let specs = vec![FormFieldSpec::text("a", "A"), FormFieldSpec::text("b", "B")];
    let mut form = Form::new("T", specs, &no_prefill(), &style(), &opts());

    form.set_field_errors(HashMap::from([
        ("a".to_string(), "first".to_string()),
        ("nope".to_string(), "typo".to_string()),
    ]));
    assert_eq!(form.field_errors().len(), 1);
    assert_eq!(form.field_errors().get("a").unwrap(), "first");

    form.set_field_errors(HashMap::from([("b".to_string(), "second".to_string())]));
    assert!(
        form.field_errors().get("a").is_none(),
        "the previous round's message must not linger"
    );
    assert_eq!(form.field_errors().get("b").unwrap(), "second");
}

/// A field's message dies when *that* field's value changes, and only then:
/// moving the cursor inside it or stepping to another field changes no value.
#[test]
fn a_field_message_dies_only_when_its_value_changes() {
    let specs = vec![FormFieldSpec::text("a", "A"), FormFieldSpec::text("b", "B")];
    let mut form = Form::new("T", specs, &no_prefill(), &style(), &opts());
    type_str(&mut form, "x");
    form.set_field_errors(HashMap::from([("a".to_string(), "bad".to_string())]));

    form.handle_key("home");
    form.handle_key("end");
    assert!(form.field_errors().contains_key("a"), "cursor keys");

    form.handle_key("tab");
    type_str(&mut form, "z");
    assert!(
        form.field_errors().contains_key("a"),
        "typing into another field leaves this one's message alone"
    );

    form.handle_key("shift+tab");
    form.handle_key("backspace");
    assert!(!form.field_errors().contains_key("a"), "value changed");
}

/// A message too long for the footer row is written out below it, over the
/// border and whatever is behind the form; past three rows it is cut with an
/// ellipsis. A *notice*, which belongs to no field and cannot be focused, just
/// gets the ellipsis.
#[test]
fn a_long_field_message_is_written_out_below_the_footer() {
    use not_yet_done_ratatui::FormNotice;
    use ratatui::layout::Rect;

    let long = "the tracker rejected this value because the sprint it names was \
                closed last friday and will not take new work items any more, so \
                pick one of the open sprints instead or leave the field empty";

    let specs = vec![FormFieldSpec::text("sprint", "Sprint")];
    let mut form = Form::new("Work item", specs, &no_prefill(), &style(), &opts());

    // The notice is truncated in place: there is no field to focus, so there is
    // nothing the user could do to see the rest.
    form.set_notice(Some(FormNotice::Alert(long.to_string())));
    // The form is drawn into the top ten rows, so its bordered block ends on
    // row 9 and everything below is the bare terminal — exactly what the
    // expansion is allowed to paint over.
    let rows = rows_of(&mut form, 44, 16, Rect::new(0, 0, 44, 10));
    assert!(rows[8].contains('…'), "truncated notice: {:?}", rows[8]);
    assert!(
        rows[10].trim().is_empty(),
        "a notice does not expand past the form: {:?}",
        rows[10]
    );

    // The field message does expand, over the border and the rows beneath it.
    form.set_notice(None);
    form.set_field_errors(HashMap::from([("sprint".to_string(), long.to_string())]));
    let rows = rows_of(&mut form, 44, 16, Rect::new(0, 0, 44, 10));
    assert!(
        rows[8].contains("⚠ the tracker rejected"),
        "the expansion starts on the footer row: {:?}",
        rows[8]
    );
    for y in 9..=10 {
        assert!(
            rows[y].trim_matches(['│', ' ']).len() > 10,
            "row {y} should carry the message, got {:?} in:\n{}",
            rows[y],
            rows.join("\n")
        );
    }
    assert!(
        rows[10].ends_with('…'),
        "the last row is cut: {:?}",
        rows[10]
    );
    assert!(
        rows[11].trim().is_empty(),
        "the expansion stops after three rows: {:?}",
        rows[11]
    );
}

// --- the discard prompt ---------------------------------------------------

/// `esc` only throws the form away while nothing has been entered; once a value
/// differs from the one it started with, it asks first — and anything but `y`
/// goes back to the form, so `esc esc` can never lose work.
#[test]
fn esc_asks_before_discarding_typed_input() {
    let specs = vec![FormFieldSpec::text("title", "Title")];
    let mut form = Form::new("T", specs, &no_prefill(), &style(), &opts());

    // Moving the focus around changes no value, so `esc` is still free.
    form.handle_key("tab");
    assert!(!form.is_dirty());

    type_str(&mut form, "a");
    assert!(form.is_dirty());
    assert_eq!(form.handle_key("esc"), FormEvent::Consumed);
    assert!(form.discard_pending());

    // Esc answers "keep" — the reflex must not be the destructive one.
    assert_eq!(form.handle_key("esc"), FormEvent::Consumed);
    assert!(!form.discard_pending());
    assert_eq!(form.values().get("title").unwrap(), "a");

    // ... and neither does the key reach the form underneath.
    form.handle_key("esc");
    assert_eq!(form.handle_key("n"), FormEvent::Consumed);
    assert_eq!(
        form.values().get("title").unwrap(),
        "a",
        "the answer must not be typed into the field"
    );

    form.handle_key("esc");
    assert_eq!(form.handle_key("y"), FormEvent::Cancelled);
}

/// Typing a value back to what it was leaves nothing to discard: the prompt
/// asks about the input, not about whether keys were pressed.
#[test]
fn retyping_the_original_value_is_not_dirty() {
    let specs = vec![FormFieldSpec::text("title", "Title").with_default("ab")];
    let mut form = Form::new("T", specs, &no_prefill(), &style(), &opts());

    form.handle_key("end");
    form.handle_key("backspace");
    assert!(form.is_dirty());
    type_str(&mut form, "b");
    assert!(!form.is_dirty());
    assert_eq!(form.handle_key("esc"), FormEvent::Cancelled);
}

/// The prompt is on screen, and the form is still behind it.
#[test]
fn the_discard_prompt_is_rendered_over_the_form() {
    use ratatui::layout::Rect;

    let specs = vec![FormFieldSpec::text("title", "Title")];
    let mut form = Form::new("Work item", specs, &no_prefill(), &style(), &opts());
    type_str(&mut form, "draft");
    form.handle_key("esc");

    let rows = rows_of(&mut form, 60, 14, Rect::new(0, 0, 60, 14));
    let screen = rows.join("\n");
    assert!(screen.contains("Discard your input?"), "{screen}");
    assert!(screen.contains("y discard"), "{screen}");
    assert!(screen.contains("Work item"), "the form stays behind it");
}

// --- numeric, duration and multi-choice fields ----------------------------

/// A whole-number field refuses anything that could never become a number, and
/// up/down step it instead of moving between fields.
#[test]
fn a_number_field_takes_digits_only_and_steps_with_up_down() {
    let specs = vec![
        FormFieldSpec::number("n", "Count").with_step(5),
        FormFieldSpec::text("other", "Other"),
    ];
    let mut form = Form::new("T", specs, &no_prefill(), &style(), &opts());

    type_str(&mut form, "12a3.");
    assert_eq!(form.values().get("n").unwrap(), "123");

    // Unbounded below, so a minus is a legal start; with the cursor inside the
    // number it is not, because no number looks like that.
    form.handle_key("home");
    type_str(&mut form, "-");
    assert_eq!(form.values().get("n").unwrap(), "-123");
    form.handle_key("end");
    type_str(&mut form, "-");
    assert_eq!(form.values().get("n").unwrap(), "-123");

    // up/down belong to the field, so the text field next to it keeps its
    // distance.
    form.handle_key("up");
    assert_eq!(form.values().get("n").unwrap(), "-118");
    form.handle_key("down");
    form.handle_key("down");
    assert_eq!(form.values().get("n").unwrap(), "-128");
    assert!(form.values().get("other").unwrap().is_empty());
    type_str(&mut form, "x");
    assert_eq!(
        form.values().get("other").unwrap(),
        "",
        "up/down must not have moved the focus"
    );
}

/// A range is checked when the value is finished, not while it is being typed:
/// every number on the way to a legal one would fail it.
#[test]
fn a_number_outside_its_range_is_refused_on_submit_only() {
    let specs = vec![FormFieldSpec::number("n", "Count").with_range(Some(10.0), Some(20.0))];
    let mut form = Form::new("T", specs, &no_prefill(), &style(), &opts());

    // `1` is below the minimum but is also the first keystroke of `15`.
    type_str(&mut form, "1");
    assert_eq!(form.values().get("n").unwrap(), "1");
    assert_eq!(submit(&mut form), FormEvent::Consumed);
    assert_eq!(
        form.field_errors().get("n").map(String::as_str),
        Some("Must be at least 10")
    );

    type_str(&mut form, "00");
    assert_eq!(submit(&mut form), FormEvent::Consumed);
    assert_eq!(
        form.field_errors().get("n").map(String::as_str),
        Some("Must be at most 20")
    );

    // Stepping, unlike typing, is an unambiguous "the next allowed value" and
    // lands inside the range.
    form.handle_key("up");
    assert_eq!(form.values().get("n").unwrap(), "20");
    assert!(matches!(submit(&mut form), FormEvent::Submitted(_)));
}

/// The separator is what the user types and sees; what comes out of the form
/// is dot-canonical, so no consumer depends on the display setting.
#[test]
fn a_decimal_field_shows_its_separator_and_submits_a_dot() {
    let german = FormOptions {
        decimal_separator: ',',
        ..FormOptions::default()
    };
    let specs = vec![FormFieldSpec::decimal("d", "Hours").with_precision(2)];
    let prefill = HashMap::from([("d".to_string(), "2.5".to_string())]);
    let mut form = Form::new("T", specs, &prefill, &style(), &german);

    let screen = rows_of(&mut form, 40, 10, ratatui::layout::Rect::new(0, 0, 40, 10)).join("\n");
    assert!(screen.contains("2,5"), "the prefill is shown German-style");
    assert_eq!(form.values().get("d").unwrap(), "2.5");

    // The dot is not this form's separator, so it never lands; the comma does,
    // and precision stops the third decimal.
    form.handle_key("end");
    type_str(&mut form, "5");
    assert_eq!(form.values().get("d").unwrap(), "2.55");
    type_str(&mut form, "5");
    assert_eq!(form.values().get("d").unwrap(), "2.55");
    type_str(&mut form, ".");
    assert_eq!(form.values().get("d").unwrap(), "2.55");
    assert!(matches!(submit(&mut form), FormEvent::Submitted(_)));
}

/// A span is typed the way people say it and submitted the way a backend can
/// use it; the line under the field says which of the two it understood.
#[test]
fn a_duration_is_submitted_in_minutes_and_read_back_in_hours() {
    let specs = vec![FormFieldSpec::duration("d", "Effort")];
    let mut form = Form::new("T", specs, &no_prefill(), &style(), &opts());

    type_str(&mut form, "1h30");
    assert_eq!(form.values().get("d").unwrap(), "90");
    let screen = rows_of(&mut form, 40, 10, ratatui::layout::Rect::new(0, 0, 40, 10)).join("\n");
    assert!(screen.contains("1 h 30 min"), "{screen}");

    // A span that is not one stops the submit and says so, rather than being
    // guessed at.
    type_str(&mut form, "x");
    assert_eq!(submit(&mut form), FormEvent::Consumed);
    assert_eq!(
        form.field_errors().get("d").map(String::as_str),
        Some("Enter a duration like 1:30, 90m or 1h30")
    );
}

/// Several options at once: the cursor moves without picking, `space` picks,
/// and the value is newline-joined so an option with a comma in it survives.
#[test]
fn a_multiselect_picks_with_space_and_joins_with_newlines() {
    let options = vec![
        "alpha".to_string(),
        "beta, second".to_string(),
        "gamma".to_string(),
    ];
    let specs = vec![FormFieldSpec::multiselect("tags", "Tags", options)];
    let prefill = HashMap::from([("tags".to_string(), "gamma".to_string())]);
    let mut form = Form::new("T", specs, &prefill, &style(), &opts());

    assert_eq!(form.values().get("tags").unwrap(), "gamma");

    // Moving down twice leaves the cursor on the third option without taking
    // anything with it; space is the pick.
    form.handle_key("down");
    assert_eq!(form.values().get("tags").unwrap(), "gamma");
    form.handle_key(" ");
    assert_eq!(form.values().get("tags").unwrap(), "beta, second\ngamma");
    form.handle_key(" ");
    assert_eq!(form.values().get("tags").unwrap(), "gamma");
}
