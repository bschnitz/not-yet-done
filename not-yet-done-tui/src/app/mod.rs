use crate::config::{Action, KeyBindingConfig};
use crate::tabs::Tab;

pub struct App {
    pub active_tab: Tab,
    pub keybindings: KeyBindingConfig,
    pub should_quit: bool,
}

impl App {
    pub fn new(keybindings: KeyBindingConfig) -> Self {
        Self {
            active_tab: Tab::Welcome,
            keybindings,
            should_quit: false,
        }
    }

    pub fn handle_action(&mut self, action: Action) {
        match action {
            Action::Quit         => self.should_quit = true,
            Action::TabWelcome   => self.active_tab = Tab::Welcome,
            Action::TabTasks     => self.active_tab = Tab::Tasks,
            Action::TabTrackings => self.active_tab = Tab::Trackings,
            Action::TabNext      => self.active_tab = self.active_tab.next(),
            Action::TabPrev      => self.active_tab = self.active_tab.prev(),
        }
    }

    /// Resolve a raw key string to an Action, if one is mapped
    pub fn resolve_key(&self, key: &str) -> Option<Action> {
        for (action, binding) in &self.keybindings.bindings {
            if binding.as_str() == key {
                return Some(action.clone());
            }
        }
        None
    }
}
