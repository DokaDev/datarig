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

/// Variables only the tests' helpers and the benchmarks read, with the reason they are not
/// documented: a name, or every variable read under a path.
const INTERNAL: &[(&str, &str)] = &[
    ("crates/bench/", "datarig-bench, the performance harness of docs/perf.md, is not shipped"),
    ("DATARIG_BENCH_STATS", "the counters file of the performance harness (docs/perf.md)"),
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

/// `(variable, file)` for every environment variable the sources read: a name passed to
/// `env::var`, `var_os` or an injected lookup (also through a `const`), and the names of
/// an array looked up one by one on that line or the next.
fn reads() -> Vec<(String, String)> {
    let mut files = Vec::new();
    sources(&root().join("crates"), &mut files);
    // The lookups' names as the sources write them (`xdg`: core's paths, one under `datarig`).
    let call = Regex::new(r#"(?:\b(?:var|var_os|env|xdg)|\.env\))\(\s*"([A-Z][A-Z0-9_]*)""#).unwrap();
    let by_const = Regex::new(r"\bvar(?:_os)?\(\s*([A-Z][A-Z0-9_]*)\s*\)").unwrap();
    let array = Regex::new(r#"\[\s*("[A-Z][A-Z0-9_]*"\s*,\s*)*"[A-Z][A-Z0-9_]*"\s*\]"#).unwrap();
    let word = Regex::new(r#""([A-Z][A-Z0-9_]*)""#).unwrap();
    let texts: Vec<(String, String)> = files
        .iter()
        .map(|f| {
            let rel = f.strip_prefix(root()).unwrap_or(f).to_string_lossy().replace('\\', "/");
            let rel = rel.trim_start_matches("./").to_string();
            (rel, std::fs::read_to_string(f).unwrap())
        })
        .collect();
    // The file's own `const` first: two files may have one of the same name.
    let constant = |name: &str, own: &str| {
        let def = Regex::new(&format!(r#"const {name}: &str = "([A-Z][A-Z0-9_]*)""#)).unwrap();
        let mut files = texts.iter().filter(|(rel, _)| rel == own).chain(texts.iter().filter(|(rel, _)| rel != own));
        files.find_map(|(_, t)| def.captures(t).map(|c| c[1].to_string()))
    };
    let mut out = Vec::new();
    for (rel, text) in &texts {
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for c in call.captures_iter(line) {
                out.push((c[1].to_string(), rel.clone()));
            }
            for c in by_const.captures_iter(line) {
                let name =
                    constant(&c[1], rel).unwrap_or_else(|| panic!("{rel}:{}: no `const {}: &str`", i + 1, &c[1]));
                out.push((name, rel.clone()));
            }
            let next = lines.get(i + 1).copied().unwrap_or_default();
            if let Some(a) = array.find(line)
                && (line.contains("env(") || next.contains("env("))
            {
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
    let reads = reads();
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
