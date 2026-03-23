#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Welcome,
    Tasks,
    Trackings,
}

impl Tab {
    pub const ALL: &'static [Tab] = &[Tab::Welcome, Tab::Tasks, Tab::Trackings];

    pub fn title(&self) -> &'static str {
        match self {
            Tab::Welcome   => "Welcome",
            Tab::Tasks     => "Tasks",
            Tab::Trackings => "Trackings",
        }
    }

    pub fn index(&self) -> usize {
        match self {
            Tab::Welcome   => 0,
            Tab::Tasks     => 1,
            Tab::Trackings => 2,
        }
    }

    pub fn next(&self) -> Tab {
        match self {
            Tab::Welcome   => Tab::Tasks,
            Tab::Tasks     => Tab::Trackings,
            Tab::Trackings => Tab::Welcome,
        }
    }

    pub fn prev(&self) -> Tab {
        match self {
            Tab::Welcome   => Tab::Trackings,
            Tab::Tasks     => Tab::Welcome,
            Tab::Trackings => Tab::Tasks,
        }
    }
}
