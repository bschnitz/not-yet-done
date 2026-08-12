#[derive(Clone)]
pub struct LocalOffset {
    pub duration: chrono::Duration,
}

impl std::str::FromStr for LocalOffset {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // The grammar lives in the shared, app-agnostic `natural-date` crate.
        natural_date::resolve_offset(s)
            .map(|duration| LocalOffset { duration })
            .ok_or_else(|| {
                format!(
                    "Invalid offset '{}': must start with '+' or '-' and a unit \
                     (e.g. +1h, -30min, +2days)",
                    s.trim()
                )
            })
    }
}
