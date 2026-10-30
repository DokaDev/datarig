//! The configuration reference (`docs/site/src/reference/configuration.md`) is generated from
//! the key list of `datarig_core::config::doc` and the `:set` settings, and that list is held to
//! the config parser: every documented default reads back as the file without the key, and
//! every table has exactly the keys the parser takes. After changing a key, regenerate it:
//!
//! ```sh
//! DATARIG_BLESS=1 cargo test -p datarig-tui --test config_doc
//! ```

use datarig_core::config::doc::{KEYS, KeyDoc, Kind, TABLES, TableDoc};
use datarig_core::config::{self, Config, ConfigError};
use datarig_core::i18n::Lang;
use datarig_tui::app::App;
use datarig_tui::app::command::{SETTINGS, SettingSpec, Values};
use std::collections::BTreeSet;
use std::path::PathBuf;

const PAGE: &str = "docs/site/src/reference/configuration.md";
const ID: &str = "0b7c4f8e-3d2a-4e61-9a55-6f1d2c3b4a59";

fn page_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(PAGE)
}

/// The `:set` setting of key path `path`.
fn setting(path: &str) -> Option<&'static SettingSpec> {
    SETTINGS.iter().find(|s| s.key == path)
}

/// `|` would end a table cell.
fn cell(text: &str) -> String {
    assert!(!text.contains('|'), "a `|` in a cell: {text}");
    text.to_string()
}

fn render() -> String {
    let mut s = String::new();
    s.push_str("# Configuration\n\n");
    s.push_str(
        "<!-- Generated from crates/core/src/config/doc.rs and the `:set` settings. Do not edit;\n     \
         run `DATARIG_BLESS=1 cargo test -p datarig-tui --test config_doc` at the repository root. -->\n\n",
    );
    s.push_str(
        "The config file is `--config <path>`, else `$XDG_CONFIG_HOME/datarig/config.toml`, else \
         `~/.config/datarig/config.toml` (on every OS). Without it datarig starts with no profiles and \
         creates it on the first save. The app edits the file in place: comments and key order are \
         kept, a setting is written only when it differs from its default, and passwords are never \
         written. A file with an unknown key, or a value a key does not take, is not used at all: \
         datarig says which key and runs with the defaults, saving nothing.\n\n\
         Keys marked `:set` can also be changed from the `:` command line (`:set <key>=<value>`) \
         and on the settings screen, which save them here.\n",
    );
    for t in TABLES {
        table(&mut s, t);
    }
    s
}

fn table(s: &mut String, t: &TableDoc) {
    if t.name.is_empty() {
        s.push_str("\n## Top level\n\n");
    } else {
        s.push_str(&format!("\n## `{}`\n\n", t.header));
    }
    s.push_str(&format!("{}\n", t.about));
    let keys: Vec<&KeyDoc> = KEYS.iter().filter(|k| k.table == t.name).collect();
    if keys.is_empty() {
        return;
    }
    s.push_str("\n| Key | Default | Values | Description |\n|---|---|---|---|\n");
    for k in keys {
        let (default, values, about) = match k.kind {
            Kind::Alias(of) => {
                (String::new(), String::new(), format!("Old name of `{of}`, read the same; a table may not have both."))
            }
            Kind::Retired => (String::new(), k.values.to_string(), k.about.to_string()),
            Kind::Key => {
                let default = k.default.map_or_else(|| k.unset.to_string(), |d| format!("`{d}`"));
                let about = match setting(&k.path()) {
                    // Catalog text is plain: `<name>` would be read as HTML.
                    Some(st) => format!("{} Also `:set {}`.", st.about.text(Lang::En).replace('<', "&lt;"), st.key),
                    None => k.about.to_string(),
                };
                (default, k.values.to_string(), about)
            }
        };
        s.push_str(&format!("| `{}` | {} | {} | {} |\n", k.key, cell(&default), cell(&values), cell(&about)));
    }
}

#[test]
fn configuration_reference_is_up_to_date() {
    let want = render();
    let path = page_path();
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
            "{PAGE} is out of date (first difference at line {}); regenerate it with \
             DATARIG_BLESS=1 cargo test -p datarig-tui --test config_doc",
            line + 1
        );
    }
}

/// The file with table `t` (and what it cannot do without) holding `extra` lines.
fn file(t: &str, extra: &str) -> String {
    match t {
        "" => format!("{extra}\n"),
        "connections" => format!("[[connections]]\nid = \"{ID}\"\n{extra}\n"),
        "connections.ssh" => format!("[[connections]]\nid = \"{ID}\"\n[connections.ssh]\n{extra}\n"),
        "policy.<env>" => format!("[policy.prod]\n{extra}\n"),
        "tunnels.<name>" => {
            format!("[tunnels.office]\nid = \"{ID}\"\nhost = \"h\"\nuser = \"u\"\nkey_file = \"k\"\n{extra}\n")
        }
        "startup.<name>" => format!("[startup.reporting]\ndbms = \"postgres\"\n{extra}\n"),
        t => format!("[{t}]\n{extra}\n"),
    }
}

fn parse(text: &str) -> Config {
    config::parse(text).unwrap_or_else(|e| panic!("{e:?} in\n{text}"))
}

/// What table `t` of `cfg` amounts to, as text (ids that are new on every read left out).
fn state(cfg: &Config, t: &str) -> String {
    let ssh = |s: &datarig_core::profile::ssh::SshSettings| {
        format!(
            "{:?}",
            (s.enabled, &s.host, s.port, &s.user, s.auth, &s.key_file, s.source(), s.keepalive(), s.timeout())
        )
    };
    match t {
        "connections" => format!("{:?}", cfg.connections),
        "connections.ssh" => ssh(cfg.connections[0].ssh.as_ref().expect("the table")),
        "policy.<env>" => {
            let e = cfg.policies.environment("prod").expect("the environment");
            format!("{:?}", (&e.policy, &e.color, &e.badge))
        }
        "tunnels.<name>" => {
            let p = &cfg.tunnels[0];
            format!("{:?} {}", (p.id, &p.name), ssh(&p.settings))
        }
        "startup.<name>" => {
            let p = &cfg.sessions[0];
            format!("{:?}", (&p.name, p.dbms, &p.sql))
        }
        _ => format!(
            "{:?}",
            (
                (cfg.version, &cfg.language, cfg.icons, &cfg.theme, cfg.page_size, cfg.result_window_rows),
                (cfg.spill_limit, &cfg.folders, cfg.last_used, cfg.default_source, cfg.prefs, &cfg.keymap),
            )
        ),
    }
}

/// Each default, written into the file, reads back as the file without the key: the key exists
/// (an unknown one is an error) and the documented value is the one it has without it.
#[test]
fn every_documented_default_is_what_the_file_has_without_the_key() {
    for k in KEYS {
        let Some(default) = k.default else { continue };
        assert_eq!(k.kind, Kind::Key, "{}: an alias has no default of its own", k.path());
        let without = parse(&file(k.table, ""));
        let with = parse(&file(k.table, &format!("{} = {default}", k.key)));
        assert_eq!(state(&with, k.table), state(&without, k.table), "{} = {default}", k.path());
    }
}

/// The keys the parser names when table `t` has a key it does not take.
fn parser_keys(t: &str) -> BTreeSet<String> {
    let text = file(t, "zz_unknown = 1");
    let err = match config::parse(&text) {
        Err(ConfigError::Syntax(f)) => f.to_string(),
        other => panic!("{t}: an unknown key is not a syntax error: {:?}", other.err()),
    };
    // serde's "expected `a`", "expected `a` or `b`", "expected one of `a`, `b`, `c`".
    let list = err.split_once("expected ").unwrap_or_else(|| panic!("{t}: the error lists no keys: {err}")).1;
    let list = list.lines().next().unwrap_or_default();
    list.split('`').skip(1).step_by(2).map(str::to_string).collect()
}

/// The keys the reference documents for table `t`, with its tables (`ssh` of `connections`).
fn documented_keys(t: &str) -> BTreeSet<String> {
    let mut keys: BTreeSet<String> = KEYS.iter().filter(|k| k.table == t).map(|k| k.key.to_string()).collect();
    let prefix = if t.is_empty() { String::new() } else { format!("{t}.") };
    for other in TABLES {
        if let Some(rest) = other.name.strip_prefix(&prefix)
            && !rest.is_empty()
            && other.name != t
        {
            keys.insert(rest.split('.').next().unwrap_or(rest).to_string());
        }
    }
    keys
}

/// Every table documents the keys the parser takes, no more and no fewer.
#[test]
fn every_table_documents_exactly_the_keys_the_parser_takes() {
    for t in TABLES {
        if t.name.starts_with("keymap.") || t.alias_of.is_some() {
            continue;
        }
        assert_eq!(documented_keys(t.name), parser_keys(t.name), "[{}]", t.name);
    }
    // An old table name is read as the table it is now.
    for t in TABLES.iter().filter(|t| t.alias_of.is_some()) {
        let now = t.alias_of.unwrap();
        assert!(TABLES.iter().any(|x| x.name == now), "{}: {now} is not a table", t.name);
        let (old, _) = t.name.split_once('.').unwrap();
        let text = format!("[{old}.reporting]\ndbms = \"postgres\"\nzz_unknown = 1\n");
        assert!(matches!(config::parse(&text), Err(ConfigError::Syntax(_))), "[{}]", t.name);
    }
    // The aliases the parser has are the ones documented.
    let documented = |table: &str, old: &str, now: &'static str| {
        KEYS.iter().any(|k| k.table == table && k.key == old && k.kind == Kind::Alias(now))
    };
    for (now, old) in config::ENV_ALIASES {
        assert!(documented("policy.<env>", old, now), "[policy.<env>] {old}");
    }
    for (now, old) in &config::STARTUP_ALIASES[1..] {
        assert!(documented("connections", old, now), "[[connections]] {old}");
    }
    let (now, old) = config::STARTUP_ALIASES[0];
    assert!(TABLES.iter().any(|t| t.name == format!("{old}.<name>") && t.alias_of == Some(&format!("{now}.<name>"))));
}

/// The `:set` settings are keys of the file with the same default, described by the setting.
#[test]
fn the_set_settings_are_documented_with_their_defaults() {
    let app = App::new(&Config::default(), None, Lang::En);
    for (i, st) in SETTINGS.iter().enumerate() {
        let k = KEYS.iter().find(|k| k.path() == st.key).unwrap_or_else(|| panic!("{}: not documented", st.key));
        let default = k.default.unwrap_or_else(|| panic!("{}: no default", st.key)).trim_matches('"');
        let now = match st.values {
            Values::Themes => Config::default().theme,
            v => v.fixed()[app.setting_value(i).unwrap_or_else(|| panic!("{}: no value", st.key))].0.to_string(),
        };
        assert_eq!(default, now, "{}", st.key);
        assert!(k.about.is_empty(), "{}: described by its setting, not twice", st.key);
    }
    for k in KEYS.iter().filter(|k| k.kind == Kind::Key) {
        assert!(!k.about.is_empty() || setting(&k.path()).is_some(), "{}: no description", k.path());
        assert!(!k.values.is_empty(), "{}: no values", k.path());
        assert!(k.default.is_some() || !k.unset.is_empty(), "{}: nothing says what leaving it out means", k.path());
    }
}

/// The lookups the cross-check of hand-written pages uses.
#[test]
fn named_tables_resolve_to_their_placeholder() {
    use datarig_core::config::doc::{find, table};
    assert_eq!(table("policy.prod").map(|t| t.name), Some("policy.<env>"));
    assert_eq!(table("editor").map(|t| t.name), Some("editor"));
    assert_eq!(table("nope"), None);
    assert!(find("policy.prod", "read_only").is_some());
    assert!(find("", "page_size").is_some());
    assert!(find("editor", "nope").is_none());
}
