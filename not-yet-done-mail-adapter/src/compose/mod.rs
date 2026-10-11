//! Writing mail: the editor buffer, the quote, the message that goes out.
//!
//! Phase 7 of `docs/plan-mail-compose.md`. The split follows how far a piece
//! is from the network: [`buffer`] is pure text and knows nothing else, which
//! is what makes the header block — the place that can cost a user their
//! typing — cheap to test.

pub(crate) mod buffer;
pub(crate) mod outbox;
pub(crate) mod quote;
pub(crate) mod render;
pub(crate) mod send;
