//! Meanings: resolver passes over the canonical vocabulary. A meaning reads one
//! canonical phrase and either answers with a [`Reading`] or declines with
//! `None`, so the next meaning gets a turn. Meanings are language-free — every
//! [`Language`](crate::Language) rewrites into the phrases they read — which is
//! what lets one new meaning serve every language at once.

use std::fmt;
use std::sync::Arc;

use chrono::{Duration, NaiveDate, NaiveDateTime};
use date_periods::WeekStart;

/// The signature of a meaning's pass.
pub type Pass = dyn Fn(&str, &Context) -> Option<Reading> + Send + Sync;

/// A named resolver pass. See [`Resolver::meaning`](crate::Resolver::meaning).
#[derive(Clone)]
pub struct Meaning {
    name: &'static str,
    pass: Arc<Pass>,
}

impl Meaning {
    /// A meaning from a name (for debugging) and its pass.
    pub fn new(
        name: &'static str,
        pass: impl Fn(&str, &Context) -> Option<Reading> + Send + Sync + 'static,
    ) -> Self {
        Self {
            name,
            pass: Arc::new(pass),
        }
    }

    /// The meaning's name.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Run the pass on one canonical phrase.
    pub fn read(&self, phrase: &str, cx: &Context) -> Option<Reading> {
        (self.pass)(phrase, cx)
    }

    /// The built-in meanings, in the order they are tried.
    pub fn builtin() -> Vec<Self> {
        crate::passes::builtin()
    }
}

impl fmt::Debug for Meaning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Meaning").field(&self.name).finish()
    }
}

/// When a reading lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Moment {
    /// A wall-clock date and time in the reference time zone (`tomorrow
    /// 18:00`). On a DST gap or overlap the earliest valid instant is chosen.
    Wall(NaiveDateTime),
    /// An exact span after the reference instant (`in 2 hours`, `now`).
    After(Duration),
}

/// What a meaning answered with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reading {
    pub at: Moment,
    /// Whether the phrase named a time of day. `false` means only the date is
    /// meant and the time is the crate's convention (midnight, or `23:59:59`
    /// for the end of a period).
    pub has_time: bool,
}

impl Reading {
    /// A wall-clock reading.
    pub fn wall(at: NaiveDateTime, has_time: bool) -> Self {
        Self {
            at: Moment::Wall(at),
            has_time,
        }
    }

    /// A calendar day at midnight, without a time of day.
    pub fn day(date: NaiveDate) -> Self {
        Self::wall(date.and_hms_opt(0, 0, 0).expect("midnight exists"), false)
    }

    /// A span after the reference instant.
    pub fn after(span: Duration, has_time: bool) -> Self {
        Self {
            at: Moment::After(span),
            has_time,
        }
    }

    /// The reading as a wall clock, given the reference wall clock.
    pub fn local(&self, now: NaiveDateTime) -> NaiveDateTime {
        match self.at {
            Moment::Wall(w) => w,
            Moment::After(d) => now + d,
        }
    }
}

/// What a pass may know about the resolution it is part of.
pub struct Context<'a> {
    pub(crate) now: NaiveDateTime,
    pub(crate) week_start: WeekStart,
    pub(crate) resolve: &'a dyn Fn(&str) -> Option<Reading>,
}

impl Context<'_> {
    /// The reference instant as a wall clock in its own time zone.
    pub fn now(&self) -> NaiveDateTime {
        self.now
    }

    /// The reference date.
    pub fn today(&self) -> NaiveDate {
        self.now.date()
    }

    /// The configured first day of the week.
    pub fn week_start(&self) -> WeekStart {
        self.week_start
    }

    /// Resolve another canonical phrase with every meaning and fallback — how
    /// `tomorrow morning` resolves its `tomorrow`. The phrase must be shorter
    /// than the one being read, or the recursion does not end.
    pub fn resolve(&self, phrase: &str) -> Option<Reading> {
        (self.resolve)(phrase)
    }

    /// [`resolve`](Self::resolve) as a wall clock.
    pub fn resolve_local(&self, phrase: &str) -> Option<NaiveDateTime> {
        self.resolve(phrase).map(|r| r.local(self.now))
    }
}
