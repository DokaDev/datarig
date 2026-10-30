//! The hand-written docs name what the code has. Every inline code span of the docs site's
//! hand-written pages, `README.md` and `CONTRIBUTING.md` that looks like a name of the app is
//! looked up: `:commands` (and `:set` assignments), action ids, context names, config keys and
//! tables (`[table] key`, `table.key`), key chords (which must be bound by default) and theme names. A span that is meant as
//! something else goes in [`ALLOWED`] with the reason. The same files hold no Hangul and no
//! absolute paths of a local machine.

use datarig_core::config::doc::{self as config_doc, TABLES};
use datarig_tui::app::{action, command};
use datarig_tui::keymap::{Ctx, Keymap, parse_keys};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// `(span, why it is not a name of the app)`.
const ALLOWED: &[(&str, &str)] = &[
    (":cq", "Vim's own command, named as Vim's"),
    (":d", "the roadmap: an Ex command datarig does not have yet"),
    (":g", "the roadmap: an Ex command datarig does not have yet"),
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            // The build output of `mdbook build docs/site`.
            if path.file_name().is_some_and(|n| n == "book") && path.parent().is_some_and(|p| p.ends_with("site")) {
                continue;
            }
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn rel(path: &Path) -> String {
    path.strip_prefix(root()).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

/// A page made by a generator (it says so), or one that only includes another file.
fn generated_or_include(text: &str) -> bool {
    text.lines().any(|l| l.starts_with("<!-- Generated"))
        || text.lines().filter(|l| !l.trim().is_empty()).all(|l| l.starts_with('#') || l.starts_with("{{#include"))
}

/// The Markdown pages to check: the site's hand-written pages, the README and CONTRIBUTING.
fn pages() -> Vec<(String, String)> {
    let mut files = Vec::new();
    walk(&root().join("docs/site/src"), &mut files);
    files.retain(|f| f.extension().is_some_and(|e| e == "md"));
    files.sort();
    files.push(root().join("README.md"));
    files.push(root().join("CONTRIBUTING.md"));
    files
        .iter()
        .map(|f| (rel(f), std::fs::read_to_string(f).unwrap_or_else(|e| panic!("{}: {e}", f.display()))))
        .filter(|(_, t)| !generated_or_include(t))
        .collect()
}

/// `(line, span)` for every inline code span outside fenced blocks (a span may go on over a
/// line break; its text then has a space there).
fn spans(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut fence: Option<String> = None;
    // The paragraph so far: its first line's number and its lines.
    let mut para: Vec<(usize, &str)> = Vec::new();
    let flush = |para: &mut Vec<(usize, &str)>, out: &mut Vec<(usize, String)>| {
        let joined: Vec<(usize, char)> =
            para.iter().flat_map(|(n, l)| l.chars().map(move |c| (*n, c)).chain([(*n, '\n')])).collect();
        let mut i = 0;
        while i < joined.len() {
            if joined[i].1 != '`' {
                i += 1;
                continue;
            }
            let run = joined[i..].iter().take_while(|(_, c)| *c == '`').count();
            let start = i + run;
            // The closing run of the same length.
            let mut j = start;
            let mut end = None;
            while j < joined.len() {
                if joined[j].1 == '`' {
                    let r = joined[j..].iter().take_while(|(_, c)| *c == '`').count();
                    if r == run {
                        end = Some(j);
                        break;
                    }
                    j += r;
                } else {
                    j += 1;
                }
            }
            let Some(end) = end else {
                i = start;
                continue;
            };
            let raw: String = joined[start..end].iter().map(|(_, c)| if *c == '\n' { ' ' } else { *c }).collect();
            let text = match raw.strip_prefix(' ').and_then(|r| r.strip_suffix(' ')) {
                Some(inner) if !inner.trim().is_empty() => inner.to_string(),
                _ => raw,
            };
            out.push((joined[start.min(joined.len() - 1)].0, text));
            i = end + run;
        }
        para.clear();
    };
    for (n, line) in text.lines().enumerate() {
        let t = line.trim_start();
        let marker = if t.starts_with("```") {
            Some("```")
        } else if t.starts_with("~~~") {
            Some("~~~")
        } else {
            None
        };
        match (&fence, marker) {
            (None, Some(m)) => {
                flush(&mut para, &mut out);
                fence = Some(m.to_string());
                continue;
            }
            (Some(f), Some(m)) if f == m => {
                fence = None;
                continue;
            }
            (Some(_), _) => continue,
            _ => {}
        }
        if t.is_empty() {
            flush(&mut para, &mut out);
        } else {
            para.push((n + 1, line));
        }
    }
    flush(&mut para, &mut out);
    out
}

/// The default bindings' key sequences.
fn bound() -> Vec<Vec<datarig_tui::keymap::keys::KeyChord>> {
    Keymap::default().bindings().iter().map(|b| b.keys.clone()).collect()
}

/// A key as the docs write one: `Ctrl+E`, `Space`, `F5`, `Shift+Tab`, a single character.
fn key_token(t: &str) -> bool {
    const NAMED: &[&str] = &[
        "Space",
        "Enter",
        "Esc",
        "Tab",
        "Backspace",
        "Delete",
        "Insert",
        "Up",
        "Down",
        "Left",
        "Right",
        "Home",
        "End",
        "PageUp",
        "PageDown",
        "Menu",
    ];
    let lower = t.to_ascii_lowercase();
    let base = ["ctrl+", "alt+", "shift+"]
        .iter()
        .fold(t, |b, m| if b.to_ascii_lowercase().starts_with(m) && b.len() > m.len() { &b[m.len()..] } else { b });
    let modified = base.len() < t.len() || lower.starts_with("ctrl+") || lower.starts_with("alt+");
    let f_key = base.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()).is_some_and(|n| (1..=24).contains(&n));
    NAMED.contains(&base) || f_key || (modified && base.chars().count() == 1) || t.chars().count() == 1
}

/// Whether a span is a key sequence: keys only, at least one of them named or modified.
fn chord(span: &str) -> bool {
    let tokens: Vec<&str> = span.split(' ').collect();
    tokens.iter().all(|t| key_token(t)) && tokens.iter().any(|t| t.chars().count() > 1)
}

/// The tables' first parts (`policy` of `policy.<env>`) and the action ids' (`grid`).
fn heads() -> (BTreeSet<&'static str>, BTreeSet<&'static str>) {
    let tables = TABLES.iter().filter(|t| !t.name.is_empty()).map(|t| t.name.split('.').next().unwrap()).collect();
    let actions = action::REGISTRY.iter().map(|a| a.id.split('.').next().unwrap()).collect();
    (tables, actions)
}

/// A config key or table written `a.b.c`: a key of a table, a top-level key or a table.
fn config_path(span: &str) -> bool {
    if config_doc::table(span).is_some_and(|t| !t.name.is_empty()) {
        return true;
    }
    match span.rsplit_once('.') {
        Some((table, key)) => config_doc::find(table, key).is_some(),
        None => config_doc::find("", span).is_some(),
    }
}

/// What is wrong with `span`, or `None` when it names something that exists (or is not a name
/// of the app at all).
fn problem(span: &str) -> Option<String> {
    let s = span.trim();
    // `:` commands.
    if let Some(cmd) = s.strip_prefix(':')
        && !cmd.is_empty()
    {
        let first = cmd.chars().next().unwrap();
        if !first.is_ascii_alphabetic() {
            // A range or an editor Ex form (`:12`, `:%s/a/b/`, `:'<,'>format`, `:&&`).
            return None;
        }
        let name: String = cmd.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
        let rest = &cmd[name.len()..];
        // `:s/a/b/`, `:substitute`: the editor's.
        if matches!(name.as_str(), "s" | "substitute") && !rest.starts_with(|c: char| c.is_ascii_alphanumeric()) {
            return None;
        }
        let Some(spec) = command::by_name(&name) else { return Some(format!("`:{name}` is not a command")) };
        if spec.name == "set" {
            let arg = rest.trim();
            if arg.is_empty() || arg.starts_with('<') {
                return None;
            }
            return match arg.split_once('=') {
                Some((key, values)) => values
                    .split('|')
                    .filter(|v| !v.starts_with('<') && !v.contains('…'))
                    .find(|v| command::parse_set(&format!("{key}={v}")).is_err())
                    .map(|v| format!("`:set {key}={v}` is not a value of that setting")),
                None => {
                    (!command::SETTINGS.iter().any(|st| st.key == arg)).then(|| format!("`{arg}` is not a setting"))
                }
            };
        }
        return None;
    }
    // `theme = "<name>"`.
    if let Some(v) = s.strip_prefix("theme = \"").and_then(|v| v.strip_suffix('"')) {
        return (!datarig_tui::theme::NAMES.contains(&v)).then(|| format!("`{v}` is not a built-in theme"));
    }
    // `[table]`, `[table] key`, `[[connections]]`.
    if s.starts_with('[') {
        let inner = s.trim_start_matches('[');
        let (table, rest) = inner.split_once(']')?;
        let rest = rest.trim_start_matches(']').trim();
        if !table.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-<>\"".contains(c)) {
            return None;
        }
        let table = table.replace('"', "");
        let (tables, _) = heads();
        let head = table.split('.').next().unwrap_or_default();
        if !tables.contains(head) {
            // `[x]` of a checkbox: not a table, unless a key follows as in `[table] key`.
            let keyish = !rest.is_empty() && rest.chars().all(|c| c.is_ascii_lowercase() || c == '_');
            return keyish.then(|| format!("`[{table}]` is not a table of the config file"));
        }
        let Some(t) = config_doc::table(&table) else {
            return Some(format!("`[{table}]` is not a table of the config file"));
        };
        let key = rest.split([' ', '=']).next().unwrap_or_default();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
            return None;
        }
        let t = t.alias_of.unwrap_or(t.name);
        if config_doc::find(t, key).is_none() {
            return Some(format!("`{key}` is not a key of `[{table}]`"));
        }
        // `[editor] auto_pairs = "on"`: a value of a `:set` setting is one it takes.
        let path = format!("{t}.{key}");
        let value = rest.split_once('=').map(|(_, v)| v.trim().trim_matches('"'));
        return match value {
            Some(v) if command::SETTINGS.iter().any(|st| st.key == path) => command::parse_set(&format!("{path}={v}"))
                .is_err()
                .then(|| format!("`{v}` is not a value of `[{table}] {key}`")),
            _ => None,
        };
    }
    // Key sequences.
    if chord(s) {
        let Ok(keys) = parse_keys(s) else { return Some(format!("`{s}` is not a key sequence")) };
        return (!bound().iter().any(|b| b.starts_with(&keys))).then(|| format!("`{s}` is not bound by default"));
    }
    // Dotted names: an action id, a context, a config key.
    let dotted = s.contains('.')
        && !s.starts_with('.')
        && !s.ends_with('.')
        && s.split('.').all(|p| {
            !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_<>-".contains(c))
        });
    if dotted {
        const FILES: &[&str] = &[
            "rs", "toml", "md", "sh", "yml", "yaml", "snap", "txt", "json", "csv", "sql", "pem", "rb", "html", "lock",
            "log", "key", "crt", "tsv", "xlsx", "parquet", "hbs",
        ];
        if FILES.iter().any(|e| s.ends_with(&format!(".{e}"))) {
            return None;
        }
        if action::by_id(s).is_some() || Ctx::from_name(s).is_some() || config_path(s) {
            return None;
        }
        let (tables, actions) = heads();
        let head = s.split('.').next().unwrap();
        if tables.contains(head) || actions.contains(head) {
            return Some(format!("`{s}` is no action id, context or config key"));
        }
    }
    None
}

#[test]
fn hand_written_docs_name_what_exists() {
    let allowed: BTreeSet<&str> = ALLOWED.iter().map(|(s, _)| *s).collect();
    let mut used = BTreeSet::new();
    let mut wrong = Vec::new();
    let mut checked = 0;
    for (file, text) in pages() {
        for (line, span) in spans(&text) {
            checked += 1;
            if let Some(why) = problem(&span) {
                if allowed.contains(span.as_str()) {
                    used.insert(span.clone());
                } else {
                    wrong.push(format!("{file}:{line}: `{span}`: {why}"));
                }
            }
        }
    }
    assert!(checked > 100, "only {checked} code spans found");
    assert!(wrong.is_empty(), "{} code spans name nothing the app has:\n{}", wrong.len(), wrong.join("\n"));
    let unused: Vec<&&str> = allowed.iter().filter(|s| !used.contains(**s)).collect();
    assert!(unused.is_empty(), "ALLOWED spans that are fine now (remove them): {unused:?}");
}

#[test]
fn spans_and_problems_are_found() {
    let text = "a `:conn x` b ``a `b` c``\n`Ctrl+E`\n\n```toml\n`:nope`\n```\nc `ui.\ntips`";
    let spans: Vec<String> = spans(text).into_iter().map(|(_, s)| s).collect();
    assert_eq!(spans, [":conn x", "a `b` c", "Ctrl+E", "ui. tips"]);
    for ok in [":conn x", ":set ui.tips=off", ":set theme", ":%s/a/b/", ":s#a#b#", "editor.format", "editor.vim.normal"]
    {
        assert_eq!(problem(ok), None, "{ok}");
    }
    for ok in ["ui.tips", "[editor] format_indent", "[policy.prod] read_only", "[[connections]]", "Space", "Ctrl+E"] {
        assert_eq!(problem(ok), None, "{ok}");
    }
    for ok in ["theme = \"nord\"", "config.toml", "[x]", "x.y", "Ctrl+K", "[explorer] size_heat = \"5\""] {
        assert_eq!(problem(ok), None, "{ok}");
    }
    for bad in [":nope", ":set ui.tips=maybe", ":set nope", "grid.nope", "ui.nope", "[editor] nope", "[nope] key"] {
        assert!(problem(bad).is_some(), "{bad}");
    }
    for bad in ["theme = \"nope\"", "[policy.prod] nope", "[ui] tips = \"maybe\"", "Alt+Home"] {
        assert!(problem(bad).is_some(), "{bad}");
    }
}

/// Hangul in the ranges of its jamo, compatibility jamo and syllables.
fn hangul(c: char) -> bool {
    matches!(c as u32, 0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF)
}

/// The docs are English and name no file of the machine they were written on.
#[test]
fn docs_have_no_hangul_and_no_local_paths() {
    let mut files = Vec::new();
    walk(&root().join("docs/site"), &mut files);
    walk(&root().join("changes"), &mut files);
    files.push(root().join("README.md"));
    files.push(root().join("CONTRIBUTING.md"));
    const LOCAL: &[&str] = &["/Users/", "/home/", "/private/tmp", "/var/folders", "C:\\Users", "C:/Users"];
    let mut wrong = Vec::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        for (n, line) in text.lines().enumerate() {
            if line.chars().any(hangul) {
                wrong.push(format!("{}:{}: Hangul", rel(&f), n + 1));
            }
            if let Some(p) = LOCAL.iter().find(|p| line.contains(**p)) {
                wrong.push(format!("{}:{}: a local path ({p})", rel(&f), n + 1));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
