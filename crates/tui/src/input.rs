//! Input: event filtering/normalization, the kitty keyboard protocol flags, the Hangul (2-set)
//! to QWERTY key mapping, and which events the event loop handles as one frame.

pub mod burst;
pub mod hangul;
pub(crate) mod keyboard;
pub mod kitty;
