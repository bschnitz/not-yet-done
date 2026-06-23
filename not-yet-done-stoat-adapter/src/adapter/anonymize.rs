//! Stoat (Discord-like) realism anonymizer.
//!
//! The content layer already wraps every adapter in a safe
//! [`StandardAnonymizer`](not_yet_done_content::StandardAnonymizer) when
//! `NYD_ANON` is set, so server/channel names and message bodies are *never*
//! leaked even without this. This override only buys **realism** for a
//! screenshot: a server/category/channel name becomes an `<adjective>_<noun>`
//! placeholder (`big_server`, `nifty_channel`) so the tree still reads as a
//! server/category/channel, message bodies become neutral free text, and an
//! author stays a person name — instead of every value collapsing into the same
//! pool words. Mapping is deterministic (see [`not_yet_done_content::anonymize`]).

use not_yet_done_content::anonymize::{pseudo_labeled, pseudo_person};
use not_yet_done_content::{Anonymizer, NodeType, StandardAnonymizer};

#[derive(Debug, Clone, Copy, Default)]
pub struct StoatAnonymizer {
    std: StandardAnonymizer,
}

impl Anonymizer for StoatAnonymizer {
    fn scrub_value(&self, key: &str, value: &str) -> String {
        match key {
            // Author of a message → a person name.
            "author" => pseudo_person(value),
            // The detail `name` field mirrors a server/category/channel label;
            // keep its adjective consistent with the tree label (both derive from
            // the same real name) by reusing the labeled scheme.
            "name" => pseudo_labeled(value, "name"),
            // Channel/message type is a generic enum ("text", "voice") — verbatim.
            // IDs and timestamps are addressing/structural — verbatim.
            "type" | "id" | "author_id" | "time" | "edited" => value.to_string(),
            // Message body and anything else → safe free-text scrub.
            _ => self.std.scrub_value(key, value),
        }
    }

    fn scrub_label(&self, node_type: &NodeType, label: &str) -> String {
        match node_type.type_id.as_str() {
            "stoat:server" => pseudo_labeled(label, "server"),
            "stoat:category" => pseudo_labeled(label, "category"),
            "stoat:channel" => pseudo_labeled(label, "channel"),
            // A message's label is its body text → neutral free text.
            "stoat:message" => self.std.scrub_value("label", label),
            // Root carries no customer data.
            "stoat:root" => label.to_string(),
            _ => self.std.scrub_value("label", label),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nt(type_id: &str) -> NodeType {
        NodeType {
            type_id: type_id.into(),
            mime_type: "text/plain".into(),
            syntax: None,
            file_extension: String::new(),
            display_name: type_id.into(),
        }
    }

    #[test]
    fn server_category_channel_labels_become_adjective_noun() {
        let a = StoatAnonymizer::default();
        let s = a.scrub_label(&nt("stoat:server"), "ACME Internal");
        assert!(s.ends_with("_server") && !s.contains("ACME"), "{s}");
        assert_eq!(s, a.scrub_label(&nt("stoat:server"), "ACME Internal"), "deterministic");
        assert!(a.scrub_label(&nt("stoat:category"), "Sales").ends_with("_category"));
        assert!(a.scrub_label(&nt("stoat:channel"), "general").ends_with("_channel"));
    }

    #[test]
    fn message_body_is_scrubbed_to_neutral_text() {
        let a = StoatAnonymizer::default();
        let body = a.scrub_label(&nt("stoat:message"), "Ship the ACME contract today");
        assert!(!body.contains("ACME"), "real term leaked: {body}");
    }

    #[test]
    fn author_is_a_person_structural_fields_survive() {
        let a = StoatAnonymizer::default();
        assert!(!a.scrub_value("author", "John Doe").contains("Doe"));
        for (k, v) in [("type", "text"), ("time", "2026-06-22T10:00:00Z"), ("edited", "yes")] {
            assert_eq!(a.scrub_value(k, v), v);
        }
    }
}
