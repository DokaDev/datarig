//! `docs/keybindings.md` must match the keymap it is generated from. After changing
//! bindings, actions or their labels, regenerate it:
//!
//! ```sh
//! DATARIG_BLESS=1 cargo test -p datarig-tui --test keybindings_doc
//! ```

use std::path::PathBuf;

fn doc_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/keybindings.md")
}

#[test]
fn keybindings_doc_is_up_to_date() {
    let want = datarig_tui::keymap::doc::render();
    let path = doc_path();
    if std::env::var("DATARIG_BLESS").is_ok_and(|v| v == "1") {
        std::fs::write(&path, &want).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        return;
    }
    let have = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    // A Windows checkout may have CRLF line endings despite .gitattributes.
    let have = have.replace("\r\n", "\n");
    if have != want {
        let line = have
            .lines()
            .zip(want.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(have.lines().count().min(want.lines().count()));
        panic!(
            "docs/keybindings.md is out of date (first difference at line {}); regenerate it with \
             DATARIG_BLESS=1 cargo test -p datarig-tui --test keybindings_doc",
            line + 1
        );
    }
}

/// The document says where the command line opens as it does now: a popup near the top by
/// default, the last line only with `commands.position = bottom`.
#[test]
fn keybindings_doc_places_the_command_line_as_it_is_drawn() {
    let doc = datarig_tui::keymap::doc::render();
    let line = doc.lines().find(|l| l.starts_with("- **Commands**")).expect("the Commands line");
    assert!(line.contains("a popup near the top of the screen"), "{line}");
    assert!(!line.contains("opens the command line at the bottom"), "{line}");
}
