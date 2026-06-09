//! Cache scope helpers for the Confluence adapter.
//!
//! CF-2a only carries the scope-id derivation. User-cache /
//! label-cache helpers join in later phases (CF-7 / CF-12) once we
//! know which fan-out actually pays for itself.

use uuid::Uuid;

/// Stable cache-scope id derived from the Confluence URL. Same URL →
/// same id across restarts and across processes.
pub fn scope_id_for_url(url: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, url.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_per_url() {
        let a = scope_id_for_url("https://wiki.example.invalid/confluence");
        let b = scope_id_for_url("https://wiki.example.invalid/confluence");
        assert_eq!(a, b);
    }

    #[test]
    fn different_urls_distinct_scopes() {
        let a = scope_id_for_url("https://wiki-a.example.invalid/");
        let b = scope_id_for_url("https://wiki-b.example.invalid/");
        assert_ne!(a, b);
    }
}
