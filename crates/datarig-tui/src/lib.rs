//! datarig terminal UI (Ratatui). The library holds all state and rendering so it can be
//! tested headless; the `datarig` binary (`main.rs`) only owns the terminal and the event loop.
//!
//! * [`app`] — application state and update logic (actions, profiles, execution, overlays).
//! * [`keymap`] — key contexts, default bindings, remapping, conflict checks, key docs.
//! * [`screens`] — the workspace layout (explorer, tabs, welcome panel) and the overlay order.
//! * [`widgets`] — editor, result grid, explorer tree, text input and the smaller drawn parts.
//! * [`input`] — key event normalization and the kitty keyboard protocol.
//! * [`icons`] — Nerd Font icons of connection profiles.
//! * [`clipboard`] — where copies go: the system clipboard or OSC 52.
//! * [`terminal`] — the terminal modes the binary sets and restores, and the cursor's shape.
//! * [`external`] — editing a query in the user's own editor (`$VISUAL` / `$EDITOR`).
//!
//! UI-independent logic lives in `datarig-core`; drivers are registered in `drivers`.

pub mod app;
pub mod clipboard;
pub mod drivers;
pub mod external;
pub mod icons;
pub mod input;
pub mod keymap;
pub mod screens;
pub mod terminal;
pub mod text;
pub mod theme;
pub mod widgets;
