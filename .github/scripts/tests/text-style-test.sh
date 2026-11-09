#!/usr/bin/env bash
# Tests of scripts/text-style.py, against a scratch git repository per case:
# bash .github/scripts/tests/text-style-test.sh
set -euo pipefail

root=$(cd "$(dirname "$0")/../../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0
dash=$'—'

# case <name> <expect: pass|fail> <expected output or ""> <path> <content> [<path> <content>]...
case_() {
  local name=$1 expect=$2 want=$3 repo out status=0
  shift 3
  repo=$work/$name
  mkdir -p "$repo"
  git -C "$repo" init -q
  while (($#)); do
    mkdir -p "$(dirname "$repo/$1")"
    printf '%s' "$2" > "$repo/$1"
    shift 2
  done
  git -C "$repo" add -A
  out=$(cd "$repo" && python3 "$root/scripts/text-style.py" 2>&1) || status=$?
  if [[ $expect == pass && $status -ne 0 ]] || [[ $expect == fail && $status -ne 1 ]] \
      || [[ -n $want && $out != *"$want"* ]]; then
    echo "FAIL $name (exit $status, want $expect${want:+ with \"$want\"}):"
    echo "$out" | sed 's/^/    /'
    failures=$((failures + 1))
  else
    echo "ok   $name"
  fi
}

case_ clean pass "" \
  README.md $'# datarig\n\nThe colour of a profile: light, fast.\n\nThe `color` key, `[colors]`, a_color, ANALYZE.\n\n```toml\ncolor = "red"\n```\n\n<https://example.com/color>\n' \
  crates/i18n/locales/en.toml $'"a" = "Colour"\n"b" = "set it under [colors]"\n"c" = ":explain analyze runs it"\n"commands.arg.analyze" = "analyze"\n"d" = "{color} resize the size"\n' \
  crates/x/src/lib.rs $'// a comment '"$dash"$' is fine\n/* so is '"$dash"$' this */\nfn f() -> char { \'a\' }\nfn g<\'a>(x: &\'a str) {}\n'
case_ readme-dash fail "README.md:3: em dash" \
  README.md $'# datarig\n\nLight '"$dash"$' fast.\n'
case_ ko-dash fail "crates/i18n/locales/ko.toml:1: em dash" \
  crates/i18n/locales/ko.toml $'"a" = "x '"$dash"$' y"\n'
case_ snapshot-dash fail "s.snap:2: em dash" \
  crates/tui/tests/snapshots/s.snap $'---\n"Changes '"$dash"$' Result 1"\n'
case_ rust-string fail "lib.rs:2: em dash in a string literal" \
  crates/x/src/lib.rs $'// fine '"$dash"$'\nconst A: &str = "a '"$dash"$' b";\n'
case_ rust-raw-string fail "lib.rs:3: em dash in a string literal" \
  crates/x/src/lib.rs $'const A: &str = r#"one\ntwo "quoted"\nthree '"$dash"$'"#;\n'
case_ rust-escape fail "lib.rs:1: em dash in a string literal" \
  crates/x/src/lib.rs $'const A: &str = "a \\u{2014} b";\n'
case_ prose-color fail "docs/site/src/a.md:1: US spelling 'color': write 'colour'" \
  docs/site/src/a.md $'The color of a tab.\n'
case_ prose-ize fail "write 'Organise'" \
  changes/added/x.md $'- Organize profiles in folders.\n'
case_ catalog-word fail "crates/i18n/locales/en.toml:2: US spelling 'analyze': write 'analyse'" \
  crates/i18n/locales/en.toml $'"commands.arg.analyze" = "analyze"\n"x" = "We analyze it"\n'

if ((failures)); then
  echo "$failures case(s) failed"
  exit 1
fi
