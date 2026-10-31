//! The command line reference (`docs/site/src/reference/command-line.md`) is generated from
//! the real binary's `--help` and from `datarig_tui::ENV_VARS`, and every environment variable
//! the sources read must be in that list (or in [`INTERNAL`]). After changing either,
//! regenerate it:
//!
//! ```sh
//! DATARIG_BLESS=1 cargo test -p datarig-tui --test cli_doc
//! ```

use regex::Regex;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PAGE: &str = "docs/site/src/reference/command-line.md";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The binary with `args`, kept away from the user's files: no keychain, scratch directories.
fn datarig(args: &[&str]) -> Output {
    let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli_doc");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_datarig"));
    cmd.args(args).env("DATARIG_SECRET_STORE", "memory");
    for (var, dir) in [
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("DATARIG_DATA_DIR", "data"),
        ("DATARIG_STATE_DIR", "state"),
    ] {
        cmd.env(var, scratch.join(dir));
    }
    cmd.output().unwrap_or_else(|e| panic!("datarig {args:?}: {e}"))
}

fn stdout(out: &Output) -> String {
    assert!(out.status.success(), "{:?}: {}", out.status, String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout.clone()).expect("UTF-8").replace("\r\n", "\n")
}

fn render() -> String {
    let help = stdout(&datarig(&["--help"]));
    let mut s = String::new();
    s.push_str("# Command line\n\n");
    s.push_str(
        "<!-- Generated from `datarig --help` and ENV_VARS in crates/tui/src/lib.rs. Do not edit;\n     \
         run `DATARIG_BLESS=1 cargo test -p datarig-tui --test cli_doc` at the repository root. -->\n\n",
    );
    s.push_str("`datarig --help` prints:\n\n```text\n");
    s.push_str(&help);
    if !help.ends_with('\n') {
        s.push('\n');
    }
    s.push_str(
        "```\n\n`datarig --version` (or `-V`) prints `datarig` and the version, as `datarig 1.2.3`. An \
         option it does not know prints what is wrong and the usage line, and exits with status 2.\n",
    );
    s.push_str("\n## Environment variables\n\n| Variable | What it does |\n|---|---|\n");
    for (name, about) in datarig_tui::ENV_VARS {
        assert!(!about.contains('|'), "{name}: a `|` would end the cell");
        s.push_str(&format!("| `{name}` | {about} |\n"));
    }
    s.push_str(
        "\nA profile with `password_source = \"env\"` reads its password from the variable its \
         `password_env` names, and an SSH tunnel with `secret_source = \"env\"` its secret from the \
         one its `secret_env` names.\n",
    );
    s
}

#[test]
fn command_line_reference_is_up_to_date() {
    let want = render();
    let path = root().join(PAGE);
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
             DATARIG_BLESS=1 cargo test -p datarig-tui --test cli_doc",
            line + 1
        );
    }
}

/// What the page says about `--version` and an unknown option.
#[test]
fn version_and_unknown_options_behave_as_documented() {
    let version = stdout(&datarig(&["--version"]));
    assert_eq!(version, format!("datarig {}\n", env!("CARGO_PKG_VERSION")));
    assert_eq!(stdout(&datarig(&["-V"])), version);
    let out = datarig(&["--no-such-option"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage: datarig"), "{out:?}");
}

/// Variables that are not documented, with the reason: a name, or every variable read under a
/// path.
const INTERNAL: &[(&str, &str)] = &[
    ("crates/bench/", "datarig-bench, the performance harness of docs/perf.md, is not shipped"),
    (
        "DATARIG_BENCH_STATS",
        "read by the released binary too, but only for the performance harness (its counters file)",
    ),
];

/// Reads whose variable is not written where it is read, with the reason: `(file, argument)`.
/// They hand a lookup function on; the names it is called with are literals elsewhere, which
/// the scan finds.
const FORWARDED: &[(&str, &str, &str)] = &[
    ("crates/core/src/paths.rs", "k", "`Paths::resolve`'s lookup: its names are in that file"),
    ("crates/core/src/config.rs", "k", "`default_path`'s lookup: its names are in that file"),
    ("crates/tui/src/main.rs", "k", "the lookups of `detect_lang` and `external::editor_command`"),
    ("crates/tui/src/app.rs", "k", "the app's lookup (`App::env`): its callers name the variables"),
    ("crates/tui/src/app/password.rs", "k", "`password_env`: a variable the user names"),
    ("crates/bench/src/ssh.rs", "k", "the benchmark harness's lookup"),
];

/// Every source file of the crates, without the tests.
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.unwrap().path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if path.is_dir() {
            if !matches!(name, "tests" | "target" | "snapshots") {
                sources(&path, out);
            }
        } else if name.ends_with(".rs")
            && !name.ends_with("tests.rs")
            && path.components().any(|c| c.as_os_str() == "src")
        {
            out.push(path);
        }
    }
}

/// The crates' sources, `(path from the root, text without its comment lines)`; a line kept as
/// an empty one so that positions still count lines.
fn texts() -> Vec<(String, String)> {
    let mut files = Vec::new();
    sources(&root().join("crates"), &mut files);
    files
        .iter()
        .map(|f| {
            let rel = f.strip_prefix(root()).unwrap_or(f).to_string_lossy().replace('\\', "/");
            let text = std::fs::read_to_string(f).unwrap().replace("\r\n", "\n");
            let text: Vec<&str> = text.lines().map(|l| if l.trim_start().starts_with("//") { "" } else { l }).collect();
            (rel.trim_start_matches("./").to_string(), text.join("\n"))
        })
        .collect()
}

/// `(variable, file)` for every environment variable the sources read: a name passed to
/// `env::var`, `var_os` or an injected lookup (also through a `const`), and the names of an
/// array looked up one by one just after it. Calls may go over several lines. `Command::env(k,
/// v)` sets a child's variable and is not a read. `problems` gets every read the scan cannot
/// name: `env::vars()`, and an `env::var` or an injected lookup (`var(k)`) of an argument that
/// is neither a literal nor a `const` and not in [`FORWARDED`].
fn reads(problems: &mut Vec<String>) -> Vec<(String, String)> {
    let texts = texts();
    // The lookups' names as the sources write them (`xdg`: core's paths, one under `datarig`).
    let call = Regex::new(r#"(?:\b(?:var|var_os|env|xdg)|\.env\))\(\s*"([A-Z][A-Z0-9_]*)"\s*([,)])"#).unwrap();
    let std_call = Regex::new(r"\benv::var(?:_os)?\(\s*([^)]*?)\s*\)").unwrap();
    let by_const = Regex::new(r"\bvar(?:_os)?\(\s*([A-Z][A-Z0-9_]*)\s*\)").unwrap();
    // An injected lookup of a name it was given (`var(k)`, not `env::var`, which `std_call`
    // reads).
    let by_arg = Regex::new(r"(^|[^:\w])var(?:_os)?\(\s*([a-z_][a-z0-9_]*)\s*\)").unwrap();
    let all = Regex::new(r"\benv::vars(?:_os)?\(").unwrap();
    let array = Regex::new(r#"\[\s*("[A-Z][A-Z0-9_]*"\s*,\s*)*"[A-Z][A-Z0-9_]*"\s*,?\s*\]"#).unwrap();
    let word = Regex::new(r#""([A-Z][A-Z0-9_]*)""#).unwrap();
    // The file's own `const` first: two files may have one of the same name.
    let constant = |name: &str, own: &str| {
        let def = Regex::new(&format!(r#"const {name}: &str = "([A-Z][A-Z0-9_]*)""#)).unwrap();
        let mut files = texts.iter().filter(|(rel, _)| rel == own).chain(texts.iter().filter(|(rel, _)| rel != own));
        files.find_map(|(_, t)| def.captures(t).map(|c| c[1].to_string()))
    };
    let line = |text: &str, at: usize| text[..at].matches('\n').count() + 1;
    let mut out = Vec::new();
    for (rel, text) in &texts {
        for c in call.captures_iter(text) {
            // `.env("NAME", value)`: a child's environment.
            if &c[2] == "," && c[0].contains("env(") {
                continue;
            }
            out.push((c[1].to_string(), rel.clone()));
        }
        for c in by_const.captures_iter(text) {
            let at = line(text, c.get(0).unwrap().start());
            let name = constant(&c[1], rel).unwrap_or_else(|| panic!("{rel}:{at}: no `const {}: &str`", &c[1]));
            out.push((name, rel.clone()));
        }
        for c in std_call.captures_iter(text) {
            let arg = &c[1];
            let named =
                arg.starts_with('"') || arg.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
            if !named && !FORWARDED.iter().any(|(f, a, _)| f == rel && a == &arg) {
                problems.push(format!(
                    "{rel}:{}: env::var({arg}): a variable the scan cannot name",
                    line(text, c.get(0).unwrap().start())
                ));
            }
        }
        for c in by_arg.captures_iter(text) {
            let arg = &c[2];
            if !FORWARDED.iter().any(|(f, a, _)| f == rel && a == &arg) {
                problems.push(format!(
                    "{rel}:{}: var({arg}): a variable the scan cannot name",
                    line(text, c.get(2).unwrap().start())
                ));
            }
        }
        for m in all.find_iter(text) {
            problems.push(format!("{rel}:{}: env::vars(): every variable", line(text, m.start())));
        }
        for a in array.find_iter(text) {
            let after = &text[a.end()..text.len().min(a.end() + 160)];
            if after.contains("env(") {
                for c in word.captures_iter(a.as_str()) {
                    out.push((c[1].to_string(), rel.clone()));
                }
            }
        }
    }
    out
}

/// A variable the code reads is documented, and a documented one is read.
#[test]
fn every_environment_variable_read_is_documented() {
    let documented: BTreeSet<&str> = datarig_tui::ENV_VARS.iter().map(|(n, _)| *n).collect();
    let internal = |name: &str, file: &str| INTERNAL.iter().any(|(k, _)| *k == name || file.starts_with(k));
    let mut problems = Vec::new();
    let reads = reads(&mut problems);
    assert!(problems.is_empty(), "reads of the environment the scan cannot check:\n{}", problems.join("\n"));
    let missing: Vec<String> = reads
        .iter()
        .filter(|(name, file)| !documented.contains(name.as_str()) && !internal(name, file))
        .map(|(name, file)| format!("{name} ({file})"))
        .collect();
    assert!(missing.is_empty(), "read but not in ENV_VARS (crates/tui/src/lib.rs): {missing:?}");
    let read: BTreeSet<&str> = reads.iter().map(|(n, _)| n.as_str()).collect();
    let stale: Vec<&&str> = documented.iter().filter(|n| !read.contains(**n)).collect();
    assert!(stale.is_empty(), "in ENV_VARS but never read: {stale:?}");
    for (name, _) in INTERNAL.iter().filter(|(k, _)| !k.contains('/')) {
        assert!(read.contains(name), "INTERNAL lists {name}, which nothing reads");
    }
}

/// The scan's shapes: reads over several lines and through a `const`, a child's environment
/// that is not one.
#[test]
fn the_scan_reads_what_it_should() {
    let call = Regex::new(r#"(?:\b(?:var|var_os|env|xdg)|\.env\))\(\s*"([A-Z][A-Z0-9_]*)"\s*([,)])"#).unwrap();
    let found =
        |t: &str| call.captures(t).filter(|c| !(&c[2] == "," && c[0].contains("env("))).map(|c| c[1].to_string());
    assert_eq!(found("std::env::var(\n    \"X_Y\",\n)").as_deref(), Some("X_Y"));
    assert_eq!(found("std::env::var(\n    \"X_Y\"\n)").as_deref(), Some("X_Y"));
    assert_eq!(found("cmd.env(\"HOME\", home)"), None);
    assert_eq!(found("(self.env)(\"TMUX\")").as_deref(), Some("TMUX"));
}
