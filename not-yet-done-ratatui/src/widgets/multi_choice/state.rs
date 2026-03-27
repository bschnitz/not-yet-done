use crossterm::event::{Event, KeyEvent};
use crate::{MultiChoiceKeymap, widgets::common::keymap::KeyBinding};

/// Events, die das Widget an die App zurückgibt.
#[derive(Debug, Clone)]
pub enum MultiChoiceEvent {
    /// Die Auswahl hat sich geändert; enthält alle aktuell gewählten Indices.
    SelectionChanged(Vec<usize>),
    /// Das Widget wurde aktiviert (Menü klappt auf).
    Opened,
    /// Das Widget wurde deaktiviert (Menü klappt zu).
    Closed,
    /// Das Event wurde nicht verarbeitet.
    Ignored,
}

/// Zustand des MultiChoice-Widgets.
#[derive(Debug, Clone)]
pub struct MultiChoiceState {
    /// Gesamtzahl der Optionen (wird beim Erstellen gesetzt).
    item_count: usize,
    /// Welche Indices sind ausgewählt?
    pub selected: Vec<bool>,
    /// Ist das Menü aufgeklappt?
    pub open: bool,
    /// Welcher Eintrag ist gerade mit dem Cursor hervorgehoben?
    pub cursor: usize,
}

impl MultiChoiceState {
    pub fn new(item_count: usize) -> Self {
        Self {
            item_count,
            selected: vec![false; item_count],
            open: false,
            cursor: 0,
        }
    }

    /// Gibt die Indices aller ausgewählten Einträge zurück.
    pub fn selected_indices(&self) -> Vec<usize> {
        self.selected
            .iter()
            .enumerate()
            .filter_map(|(i, &s)| if s { Some(i) } else { None })
            .collect()
    }

    /// Öffnet das Menü (aktiviert das Widget von außen).
    pub fn open(&mut self) {
        self.open = true;
    }

    /// Schließt das Menü (deaktiviert das Widget von außen).
    pub fn close(&mut self) {
        self.open = false;
    }

    /// Verarbeitet ein Terminal-Event gemäß dem konfigurierten Keymap.
    /// Gibt zurück, was sich geändert hat.
    pub fn handle_event(
        &mut self,
        event: &Event,
        keymap: &MultiChoiceKeymap
    ) -> MultiChoiceEvent {
        // Nur Key-Events interessieren uns
        let Event::Key(KeyEvent { code, modifiers, .. }) = event else {
            return MultiChoiceEvent::Ignored;
        };

        let pressed = KeyBinding {
            code: *code,
            modifiers: *modifiers,
        };

        if !self.open {
            return MultiChoiceEvent::Ignored;
        }

        if pressed == keymap.move_down {
            if self.cursor + 1 < self.item_count {
                self.cursor += 1;
            }
            return MultiChoiceEvent::Ignored;
        }

        if pressed == keymap.move_up {
            if self.cursor > 0 {
                self.cursor -= 1;
            }
            return MultiChoiceEvent::Ignored;
        }

        if pressed == keymap.toggle {
            if self.item_count > 0 {
                self.selected[self.cursor] = !self.selected[self.cursor];
                return MultiChoiceEvent::SelectionChanged(self.selected_indices());
            }
        }

        MultiChoiceEvent::Ignored
    }
}
