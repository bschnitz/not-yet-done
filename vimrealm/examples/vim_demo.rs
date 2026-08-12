//! A runnable playground for the editor:
//!
//! ```sh
//! cargo run -p vimrealm --example vim_demo
//! ```
//!
//! Type `:wq` to leave and print the text, `:q!` to leave discarding it. The
//! example drives the component directly through `Component::view` and
//! `VimEditor::on_key`, which is the smallest possible host — a real tuirealm
//! application mounts it and receives [`VimEvent`]s from `AppComponent::on`.

use std::io;

use crossterm::event::{self, Event as TermEvent, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Paragraph;
use tuirealm::component::Component;
use tuirealm::event::KeyEvent;
use vimrealm::{VimEditor, VimEvent, VimStyle, VimStyleType};

const SAMPLE: &str = "\
Ein modaler Editor als Widget.

Normal mode: h j k l  w b e  0 ^ $  gg G, counts like 3w or 2dd,
edits x d c y p P D C, undo u and redo Ctrl+R.
Insert mode: i a I A o O, Esc to leave.
Ex commands: :w  :wq  :x  :q  :q!";

fn main() -> io::Result<()> {
    let mut editor = VimEditor::default()
        .with_text(SAMPLE)
        .with_title(" message ")
        .with_line_numbers(true)
        .with_style(
            VimStyle::new()
                .with(
                    VimStyleType::Mode,
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )
                .with(
                    VimStyleType::CommandLine,
                    Style::default().fg(Color::Yellow),
                )
                .with(VimStyleType::Gutter, Style::default().fg(Color::DarkGray)),
        );

    let mut terminal = ratatui::init();
    let mut status = String::from("unsaved");
    let outcome = loop {
        terminal.draw(|frame| {
            let [help, body] =
                Layout::vertical([Constraint::Length(1), Constraint::Min(3)]).areas(frame.area());
            frame.render_widget(
                Paragraph::new(format!(
                    "vimrealm demo — :wq to finish, :q! to discard   ({status})"
                ))
                .style(Style::default().add_modifier(Modifier::DIM)),
                help,
            );
            editor.view(frame, body);
        })?;

        let TermEvent::Key(key) = event::read()? else {
            continue;
        };
        // Windows terminals report press *and* release; only act on the press.
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match editor.on_key(KeyEvent::from(key)) {
            Some(VimEvent::Save) => status = "saved".into(),
            Some(VimEvent::SaveAndClose) => break Some(editor.text()),
            Some(VimEvent::Cancel) => break None,
            Some(VimEvent::Changed) | None => {}
        }
    };
    ratatui::restore();

    match outcome {
        Some(text) => println!("{text}"),
        None => println!("discarded"),
    }
    Ok(())
}
