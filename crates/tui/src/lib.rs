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

/// The environment variables datarig reads, with what each does, for the generated command line
/// reference (`tests/cli_doc.rs`). That test fails when the code reads a variable that is not
/// here (or in its short list of variables only the tests and the benchmarks read). Text is
/// Markdown: no `|`.
pub const ENV_VARS: &[(&str, &str)] = &[
    (
        "DATARIG_SECRET_STORE",
        "`memory` keeps passwords in memory only: the OS keychain and the secrets file are never \
         touched. `keychain`, or unset: the profiles' own password stores. Any other value is an \
         error and datarig does not start.",
    ),
    (
        "XDG_CONFIG_HOME",
        "The config file is `$XDG_CONFIG_HOME/datarig/config.toml` when it is set (and `--config` is not given).",
    ),
    (
        "HOME",
        "The home directory: the config file is `~/.config/datarig/config.toml` without \
         `XDG_CONFIG_HOME`, the data and state directories default to places under it, and `~` \
         in an SSH key file is it.",
    ),
    ("USERPROFILE", "The home directory where `HOME` is not set (Windows)."),
    ("DATARIG_DATA_DIR", "The data directory (saved queries, datarig's own `known_hosts`), over every other rule."),
    (
        "DATARIG_STATE_DIR",
        "The state directory (restored tabs, console buffers, one-time notices, the log of deleted \
         passwords, the keychain offer's answer, what the tips remember), over every other rule.",
    ),
    (
        "XDG_DATA_HOME",
        "The data directory is `$XDG_DATA_HOME/datarig` when it is set, on every OS. Otherwise \
         `~/.local/share/datarig` (Linux and other Unix), `~/Library/Application Support/datarig` \
         (macOS) or `%APPDATA%\\datarig` (Windows).",
    ),
    (
        "XDG_STATE_HOME",
        "The state directory is `$XDG_STATE_HOME/datarig` when it is set, on every OS. Otherwise \
         `~/.local/state/datarig`, `~/Library/Application Support/datarig/state` or \
         `%LOCALAPPDATA%\\datarig`.",
    ),
    ("APPDATA", "Windows: the data directory's default is `%APPDATA%\\datarig`."),
    ("LOCALAPPDATA", "Windows: the state directory's default is `%LOCALAPPDATA%\\datarig`."),
    (
        "LC_ALL",
        "With `language = \"auto\"`, the first of `LC_ALL`, `LC_MESSAGES` and `LANG` that is set \
         picks the language: Korean when it starts with `ko`, else English.",
    ),
    ("LC_MESSAGES", "As `LC_ALL`, when that is not set."),
    ("LANG", "As `LC_ALL`, when neither it nor `LC_MESSAGES` is set."),
    (
        "VISUAL",
        "The editor a query is opened in to edit it outside datarig, split into words as a shell \
         would.",
    ),
    ("EDITOR", "As `VISUAL`, when that is not set; without either, `vi` (`notepad` on Windows)."),
    (
        "SSH_CONNECTION",
        "Set (or `SSH_TTY`): datarig runs in an SSH session, so `clipboard = \"auto\"` copies \
         through OSC 52 (the terminal), and the macOS keychain offer is never made.",
    ),
    ("SSH_TTY", "As `SSH_CONNECTION`."),
    ("SSH_CLIENT", "Set: the macOS keychain offer is never made (as in an SSH session)."),
    ("SSH_AUTH_SOCK", "The ssh-agent an SSH tunnel with `auth = \"agent\"` logs in with."),
    ("TMUX", "Set: the status bar says that an OSC 52 copy needs `set -g set-clipboard on` in tmux."),
];
