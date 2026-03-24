use crate::config::{GlobalAction, KeyBindingConfig, TasksAction, TuiConfig};
use crate::tabs::{Tab, TasksForm, TasksState};
use crate::ui::theme::Theme;

pub struct App {
    pub active_tab:   Tab,
    pub tasks_state:  TasksState,
    pub keybindings:  KeyBindingConfig,
    pub theme:        Theme,
    pub config:       TuiConfig,
    pub should_quit:  bool,
}

impl App {
    pub fn new(config: TuiConfig, theme: Theme) -> Self {
        let keybindings = config.keybindings.clone();
        Self {
            active_tab:  Tab::Welcome,
            tasks_state: TasksState::new(),
            keybindings,
            theme,
            config,
            should_quit: false,
        }
    }

    // -----------------------------------------------------------------------
    // Key routing
    // -----------------------------------------------------------------------

    /// Route a raw key string to the correct handler depending on context.
    /// Returns true if the key was consumed.
    pub fn handle_key(&mut self, key: &str) -> bool {
        // Tasks-specific keys take priority when Tasks tab is active
        if self.active_tab == Tab::Tasks {
            if let Some(action) = self.resolve_tasks_key(key) {
                self.handle_tasks_action(action);
                return true;
            }
        }

        // Global keys — but block tab switching while a form is open
        if let Some(action) = self.resolve_global_key(key) {
            match action {
                GlobalAction::TabWelcome
                | GlobalAction::TabTasks
                | GlobalAction::TabTrackings
                | GlobalAction::TabNext
                | GlobalAction::TabPrev
                    if self.tasks_state.form_visible() =>
                {
                    // Form is open — swallow tab-switch silently
                }
                other => self.handle_global_action(other),
            }
            return true;
        }

        false
    }

    fn resolve_global_key(&self, key: &str) -> Option<GlobalAction> {
        for (action, binding) in &self.keybindings.global.bindings {
            if binding.as_str() == key {
                return Some(action.clone());
            }
        }
        None
    }

    fn resolve_tasks_key(&self, key: &str) -> Option<TasksAction> {
        for (action, binding) in &self.keybindings.tasks.bindings {
            if binding.as_str() == key {
                return Some(action.clone());
            }
        }
        None
    }

    // -----------------------------------------------------------------------
    // Action handlers
    // -----------------------------------------------------------------------

    fn handle_global_action(&mut self, action: GlobalAction) {
        match action {
            GlobalAction::Quit         => self.should_quit = true,
            GlobalAction::TabWelcome   => self.active_tab = Tab::Welcome,
            GlobalAction::TabTasks     => self.active_tab = Tab::Tasks,
            GlobalAction::TabTrackings => self.active_tab = Tab::Trackings,
            GlobalAction::TabNext      => self.active_tab = self.active_tab.next(),
            GlobalAction::TabPrev      => self.active_tab = self.active_tab.prev(),
        }
    }

    fn handle_tasks_action(&mut self, action: TasksAction) {
        use crate::tabs::TasksView;
        match action {
            TasksAction::ViewList   => {
                self.tasks_state.active_view = TasksView::List;
            }
            TasksAction::ViewTree   => {
                self.tasks_state.active_view = TasksView::Tree;
            }
            TasksAction::FormFilter => self.tasks_state.open_form(TasksForm::Filter),
            TasksAction::FormAdd    => self.tasks_state.open_form(TasksForm::Add),
            TasksAction::FormDelete => self.tasks_state.open_form(TasksForm::Delete),
            TasksAction::FormClose  => self.tasks_state.close_form(),
        }
    }
}
