//! Writing mail: the editor buffer, the quote, the message that goes out.
//!
//! Phase 7 of `docs/plan-mail-compose.md`. The split follows how far a piece
//! is from the network: [`buffer`] is pure text and knows nothing else, which
//! is what makes the two places that can cost a user their typing — the
//! header block and the quote guard — cheap to test.

pub(crate) mod buffer;
pub(crate) mod quote;
pub(crate) mod render;
