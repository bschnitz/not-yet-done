use std::sync::Arc;

use not_yet_done_core::entity::task::Model as Task;
use not_yet_done_core::service::TaskService;

use crate::config::{FormAction, GlobalAction, KeyBindingConfig, TasksAction, TuiConfig};
use crate::filter_builder;
use crate::tabs::{FilterField, LoadState, Tab, TasksForm, TasksState, TasksView};
use crate::ui::theme::Theme;

// ---------------------------------------------------------------------------
// Messages from the async loader back to the main thread
// ---------------------------------------------------------------------------

pub enum LoadMsg {
    Tasks(Vec<Task>),
    Error(String),
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

pub struct App {
    pub active_tab:  Tab,
    pub tasks_state: TasksState,
    pub keybindings: KeyBindingConfig,
    pub theme:       Theme,
    pub config:      TuiConfig,
    pub should_quit: bool,

    task_service: Arc<dyn TaskService>,

    pub load_rx: tokio::sync::mpsc::UnboundedReceiver<LoadMsg>,
    load_tx:     tokio::sync::mpsc::UnboundedSender<LoadMsg>,
}

impl App {
    pub fn new(
        config: TuiConfig,
        theme: Theme,
        task_service: Arc<dyn TaskService>,
    ) -> Self {
        let keybindings = config.keybindings.clone();
        let (load_tx, load_rx) = tokio::sync::mpsc::unbounded_channel();

        let mut app = Self {
            active_tab:  Tab::Welcome,
            tasks_state: TasksState::new(),
            keybindings,
            theme,
            config,
            should_quit: false,
            task_service,
            load_rx,
            load_tx,
        };
        app.spawn_load();
        app
    }

    // -----------------------------------------------------------------------
    // Async task loading
    // -----------------------------------------------------------------------

    pub fn spawn_load(&mut self) {
        self.tasks_state.load_state = LoadState::Loading;

        let build_result = filter_builder::build(&self.tasks_state.filter);

        self.tasks_state.filter.created_after_err  = None;
        self.tasks_state.filter.created_before_err = None;
        self.tasks_state.filter.priority_err       = None;
        for e in &build_result.errors {
            match e.field {
                "Created after"  => self.tasks_state.filter.created_after_err  = Some(e.message.clone()),
                "Created before" => self.tasks_state.filter.created_before_err = Some(e.message.clone()),
                "Priority \u{2265}" => self.tasks_state.filter.priority_err    = Some(e.message.clone()),
                _ => {}
            }
        }

        let expr    = build_result.expr;
        let service = Arc::clone(&self.task_service);
        let tx      = self.load_tx.clone();

        tokio::spawn(async move {
            let msg = match service.list_filtered(&expr).await {
                Ok(tasks) => LoadMsg::Tasks(tasks),
                Err(e)    => LoadMsg::Error(e.to_string()),
            };
            let _ = tx.send(msg);
        });
    }

    pub fn poll_load(&mut self) {
        if let Ok(msg) = self.load_rx.try_recv() {
            match msg {
                LoadMsg::Tasks(tasks) => self.tasks_state.set_tasks(tasks),
                LoadMsg::Error(e)     => self.tasks_state.set_load_error(e),
            }
        }
    }

    // -----------------------------------------------------------------------
    // Key routing
    // -----------------------------------------------------------------------

    pub fn handle_key(&mut self, key: &str) -> bool {
        // ── FuzzyFilter-Modus: alle Keys abfangen ────────────────────────
        if self.active_tab == Tab::Tasks && self.tasks_state.fuzzy_active {
            return self.handle_fuzzy_key(key);
        }

        // ── Filter form intercepts keys first (when open) ────────────────
        if self.active_tab == Tab::Tasks {
            if let Some(TasksForm::Filter) = self.tasks_state.active_form {
                if self.handle_filter_key(key) {
                    return true;
                }
            }
        }

        // ── Tasks actions ────────────────────────────────────────────────
        if self.active_tab == Tab::Tasks {
            if let Some(action) = self.resolve_tasks_key(key) {
                self.handle_tasks_action(action);
                return true;
            }
        }

        // ── Global actions (blocked while a form is open) ────────────────
        if let Some(action) = self.resolve_global_key(key) {
            match action {
                GlobalAction::TabWelcome
                | GlobalAction::TabTasks
                | GlobalAction::TabTrackings
                | GlobalAction::TabNext
                | GlobalAction::TabPrev
                    if self.tasks_state.form_visible() => {}
                other => self.handle_global_action(other),
            }
            return true;
        }

        false
    }

    // -----------------------------------------------------------------------
    // FuzzyFilter key handling
    // -----------------------------------------------------------------------

    fn handle_fuzzy_key(&mut self, key: &str) -> bool {
        // Prüfe zuerst ob es der Exit-Key ist (konfigurierbar)
        if let Some(exit_binding) = self.keybindings.tasks.get(&TasksAction::FuzzyFilterExit) {
            if exit_binding.as_str() == key {
                self.tasks_state.fuzzy_close();
                return true;
            }
        }

        match key {
            "esc" => {
                // Esc: Query leeren oder Modus verlassen
                if self.tasks_state.fuzzy_query.is_empty() {
                    self.tasks_state.fuzzy_close();
                } else {
                    self.tasks_state.fuzzy_query.clear();
                    self.tasks_state.fuzzy_cursor = 0;
                    self.tasks_state.tree_filter.clear();
                }
                true
            }
            "backspace" => {
                self.tasks_state.fuzzy_backspace();
                true
            }
            "left"  => { self.tasks_state.fuzzy_cursor_left();  true }
            "right" => { self.tasks_state.fuzzy_cursor_right(); true }
            "enter" => {
                // Live-Filter — kein extra Commit nötig
                true
            }
            ch if is_printable(ch) => {
                let c = ch.chars().next().unwrap();
                self.tasks_state.fuzzy_insert(c);
                true
            }
            _ => false,
        }
    }

    // -----------------------------------------------------------------------
    // Filter form key handling
    // -----------------------------------------------------------------------

    fn handle_filter_key(&mut self, key: &str) -> bool {
        let focused = self.tasks_state.filter.focused_field;

        if let Some(action) = self.resolve_form_key(key) {
            match action {
                FormAction::Next => {
                    self.tasks_state.filter.focus_next();
                    return true;
                }
                FormAction::Prev => {
                    self.tasks_state.filter.focus_prev();
                    return true;
                }
                FormAction::MultiselectNext => {
                    if focused == FilterField::Status {
                        self.tasks_state.filter.status_cursor_next();
                        return true;
                    }
                }
                FormAction::MultiselectPrev => {
                    if focused == FilterField::Status {
                        self.tasks_state.filter.status_cursor_prev();
                        return true;
                    }
                }
            }
        }

        match key {
            "left" if focused == FilterField::Status => {
                self.tasks_state.filter.focus_prev();
                return true;
            }
            "right" if focused == FilterField::Status => {
                self.tasks_state.filter.focus_next();
                return true;
            }
            " " | "enter" if focused == FilterField::Status => {
                self.tasks_state.filter.toggle_status_cursor();
                self.spawn_load();
                return true;
            }
            " " | "enter" if focused == FilterField::ShowDeleted => {
                self.tasks_state.filter.toggle_show_deleted();
                self.spawn_load();
                return true;
            }
            "left"  => { self.tasks_state.filter.cursor_left();  return true; }
            "right" => { self.tasks_state.filter.cursor_right(); return true; }
            "backspace" => {
                self.tasks_state.filter.backspace();
                self.spawn_load();
                return true;
            }
            "enter" => {
                self.spawn_load();
                return true;
            }
            "ctrl+r" => {
                self.tasks_state.filter.reset();
                self.spawn_load();
                return true;
            }
            ch if is_printable(ch)
                && focused != FilterField::Status
                && focused != FilterField::ShowDeleted =>
            {
                let c = ch.chars().next().unwrap();
                self.tasks_state.filter.insert_char(c);
                self.spawn_load();
                return true;
            }
            _ => {}
        }

        false
    }

    // -----------------------------------------------------------------------
    // Key resolvers
    // -----------------------------------------------------------------------

    fn resolve_global_key(&self, key: &str) -> Option<GlobalAction> {
        for (action, binding) in &self.keybindings.global.bindings {
            if binding.as_str() == key { return Some(action.clone()); }
        }
        None
    }

    fn resolve_tasks_key(&self, key: &str) -> Option<TasksAction> {
        for (action, binding) in &self.keybindings.tasks.bindings {
            if binding.as_str() == key { return Some(action.clone()); }
        }
        None
    }

    fn resolve_form_key(&self, key: &str) -> Option<FormAction> {
        for (action, binding) in &self.keybindings.form.bindings {
            if binding.as_str() == key { return Some(action.clone()); }
        }
        None
    }

    // -----------------------------------------------------------------------
    // Action handlers
    // -----------------------------------------------------------------------

    fn handle_global_action(&mut self, action: GlobalAction) {
        match action {
            GlobalAction::Quit         => self.should_quit = true,
            GlobalAction::TabWelcome   => self.active_tab  = Tab::Welcome,
            GlobalAction::TabTasks     => {
                self.active_tab = Tab::Tasks;
                if self.tasks_state.load_state == LoadState::Idle {
                    self.spawn_load();
                }
            }
            GlobalAction::TabTrackings => self.active_tab = Tab::Trackings,
            GlobalAction::TabNext      => self.active_tab = self.active_tab.next(),
            GlobalAction::TabPrev      => self.active_tab = self.active_tab.prev(),
        }
    }

    fn handle_tasks_action(&mut self, action: TasksAction) {
        match action {
            TasksAction::ViewList => {
                self.tasks_state.active_view = TasksView::List;
            }
            TasksAction::ViewTree => {
                self.tasks_state.active_view = TasksView::Tree;
            }
            TasksAction::FormFilter => self.tasks_state.open_form(TasksForm::Filter),
            TasksAction::FormAdd    => self.tasks_state.open_form(TasksForm::Add),
            TasksAction::FormDelete => self.tasks_state.open_form(TasksForm::Delete),
            TasksAction::FormClose  => self.tasks_state.close_form(),
            TasksAction::FuzzyFilterOpen => {
                // Öffnet FuzzyFilter; schließt ggf. offene Form-Dialoge
                self.tasks_state.close_form();
                self.tasks_state.fuzzy_open();
            }
            // FuzzyFilterExit wird direkt in handle_fuzzy_key behandelt
            TasksAction::FuzzyFilterExit => {}
            TasksAction::ListNext   => self.tasks_state.select_next(20),
            TasksAction::ListPrev   => self.tasks_state.select_prev(),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_printable(key: &str) -> bool {
    let mut chars = key.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if !c.is_control())
}
