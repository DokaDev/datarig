#!/usr/bin/env bash
# Tests of .github/scripts/docs-rule.sh, against a scratch git repository with a pull request
# per case: bash .github/scripts/tests/docs-rule-test.sh
set -euo pipefail

root=$(cd "$(dirname "$0")/../../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
repo=$work/repo
failures=0

g() { git -C "$repo" "$@"; }

put() { # put <path> <content>
  mkdir -p "$(dirname "$repo/$1")"
  printf '%s' "$2" > "$repo/$1"
}

mkdir -p "$repo"
g init -q -b main
g config user.name test
g config user.email test@example.com
g config commit.gpgsign false
mkdir -p "$repo/.github/scripts" "$repo/scripts"
cp "$root/.github/scripts/docs-rule.sh" "$repo/.github/scripts/"
cp "$root/scripts/changes.sh" "$repo/scripts/"
put Cargo.toml $'[workspace]\nmembers = ["crates/*"]\n\n[workspace.package]\nversion = "0.36.0"\nedition = "2024"\n\n[workspace.dependencies]\nfoo = { version = "1.0" }\n'
put README.md $'# datarig\n'
put crates/tui/src/keymap/defaults.rs $'// keys\n'
put crates/tui/src/keymap/tests.rs $'// tests\n'
put crates/tui/src/app.rs $'// app\n'
put crates/core/src/policy.rs $'// policy\n'
put crates/core/src/theme.rs $'// theme files\n'
put crates/core/src/paths.rs $'// paths\n'
put locales/en.toml $'a = "A"\n'
put locales/ko.toml $'a = "A"\n'
put docs/site/src/SUMMARY.md $'# Summary\n'
put docs/site/src/guide.md $'# Guide\n\nHow to.\n'
put docs/site/src/old.md $'# Old\n\nGone soon.\n'
put docs/site/src/reference/configuration.md $'# Configuration\n\n<!-- Generated from crates/core/src/config/doc.rs. Do not edit;\n     DATARIG_BLESS=1 ... -->\n\nText.\n'
put docs/site/src/reference/keybindings.md $'# Keybindings\n\n{{#include ../../../keybindings.md:contexts}}\n'
put docs/site/src/screens/main.txt $'screen\n'
put docs/site/src/whats-new.md $'# What\'s new\n\nIntro.\n\n## 0.36.0 — 2026-10-01\n\n- Tips.\n'
put changes/README.md $'# Changelog fragments\n'
put changes/fixed/kept.md $'- An entry of the next release.\n'
# Pages over 500 KB: nothing may stop reading early (SIGPIPE under pipefail).
big=$(for i in $(seq 1 12000); do printf 'Line %d of a long page with enough text in it.\n' "$i"; done)
put docs/site/src/big.md "# Big"$'\n\n'"$big"$'\n'
put docs/site/src/reference/big-generated.md "# Big generated"$'\n\n<!-- Generated from somewhere. Do not edit. -->\n\n'"$big"$'\n'
g add -A
g commit -q -m base
base=$(g rev-parse HEAD)

ok() { echo "ok   $1"; }
bad() {
  echo "FAIL $1" >&2
  failures=$((failures + 1))
}

# case_start <name>: a branch from the base for one pull request.
case_start() {
  g checkout -q -B "case-$1" "$base"
}

# expect <pass|fail> <name> <body> [text in the output]
expect() {
  local want=$1 name=$2 body=$3 text=${4:-} head rc=0
  g add -A
  g commit -q --allow-empty -m "$name"
  head=$(g rev-parse HEAD)
  (cd "$repo" && BASE=$base HEAD=$head BODY=$body GITHUB_STEP_SUMMARY=$work/summary \
    bash .github/scripts/docs-rule.sh) > "$work/out" 2>&1 || rc=$?
  if [[ $want == pass && $rc -ne 0 ]] || [[ $want == fail && $rc -ne 1 ]]; then
    bad "$name: expected $want, exit $rc:"$'\n'"$(cat "$work/out")"
  elif [[ -n $text ]] && ! grep -qF -- "$text" "$work/out"; then
    bad "$name: output lacks \"$text\":"$'\n'"$(cat "$work/out")"
  else
    ok "$name"
  fi
}

frag=$'- The grid copies a range as TSV.\n'

case_start nothing
put README.md $'# datarig\n\nMore.\n'
expect pass "no crates change needs nothing" ""

case_start visible-ok
put crates/tui/src/keymap/defaults.rs $'// keys 2\n'
put docs/site/src/guide.md $'# Guide\n\nHow to, better.\n'
put changes/added/grid-tsv.md "$frag"
expect pass "a user-visible change with a page and a fragment" "" "fragments added: changes/added/grid-tsv.md"

case_start visible-no-page
put crates/tui/src/keymap/defaults.rs $'// keys 2\n'
put changes/added/grid-tsv.md "$frag"
expect fail "a user-visible change without a page" "" "no doc change: <why no page changes>"

case_start visible-generated
put locales/en.toml $'a = "B"\n'
put docs/site/src/reference/configuration.md $'# Configuration\n\n<!-- Generated from crates/core/src/config/doc.rs. Do not edit;\n     DATARIG_BLESS=1 ... -->\n\nText 2.\n'
put docs/site/src/reference/keybindings.md $'# Keybindings\n\n{{#include ../../../keybindings.md:commands}}\n'
put docs/site/src/screens/main.txt $'screen 2\n'
put docs/site/src/SUMMARY.md $'# Summary\n\n- x\n'
put changes/changed/label.md "$frag"
expect fail "generated pages, include stubs, screens and SUMMARY.md do not count" "" "locales/en.toml"

case_start visible-new-page
put crates/core/src/policy.rs $'// policy 2\n'
put docs/site/src/policy.md $'# Policy\n\nNew page.\n'
put changes/added/policy.md "$frag"
expect pass "a new hand-written page counts" ""

case_start visible-deleted-page
put crates/core/src/policy.rs $'// policy 2\n'
g rm -q docs/site/src/old.md
put changes/removed/old.md "$frag"
expect pass "deleting a hand-written page counts" ""

case_start visible-escape
put crates/tui/src/keymap/defaults.rs $'// keys 2\n'
put changes/fixed/keys.md "$frag"
expect pass "no doc change with a reason" $'Some text.\r\n\r\nno doc change: the generated reference covers it\r\n' \
  "no doc change: the generated reference covers it"

case_start core-theme
put crates/core/src/theme.rs $'// theme files 2\n'
put changes/changed/theme.md "$frag"
expect fail "the theme file format in core is user-visible" "" "crates/core/src/theme.rs"

case_start core-paths
put crates/core/src/paths.rs $'// paths 2\n'
put changes/changed/paths.md "$frag"
expect fail "paths.rs is user-visible" "" "crates/core/src/paths.rs"

case_start whats-new-only
put locales/en.toml $'a = "B"\n'
put docs/site/src/whats-new.md $'# What\'s new\n\nIntro.\n\n- Hand-added.\n\n## 0.36.0 — 2026-10-01\n\n- Tips.\n'
put changes/changed/label.md "$frag"
expect fail "What's new is no page that describes a change" "" "no doc change: <why no page changes>"

case_start big-page
put locales/en.toml $'a = "B"\n'
printf 'One more line.\n' >> "$repo/docs/site/src/big.md"
put changes/changed/label.md "$frag"
expect pass "a hand-written page over 500 KB counts" ""

case_start big-generated
put locales/en.toml $'a = "B"\n'
printf 'One more line.\n' >> "$repo/docs/site/src/reference/big-generated.md"
put changes/changed/label.md "$frag"
expect fail "a generated page over 500 KB does not count" "" "no doc change: <why no page changes>"

case_start case-insensitive
put crates/tui/src/app.rs $'// app 2\n'
expect pass "the prefix in any case" $'No Changelog: refactor only\n' "refactor only"

case_start fenced
put crates/tui/src/app.rs $'// app 2\n'
expect fail "a line in a fenced code block does not count" $'Example:\n\n```text\nno changelog: example\n```\n'

case_start unclosed-comment
put crates/tui/src/app.rs $'// app 2\n'
expect fail "a line after an unclosed comment does not count" $'<!-- a hint\nno changelog: hidden\n'

case_start renamed-fragment
put crates/tui/src/app.rs $'// app 2\n'
g mv changes/fixed/kept.md changes/fixed/moved.md
expect fail "a renamed fragment is no new entry" "" "adds no changelog fragment"

case_start dotfile
put crates/tui/src/app.rs $'// app 2\n'
put changes/added/.gitkeep ""
put changes/added/x.md "$frag"
expect pass "dotfiles under changes/ are ignored" ""

case_start fenced-tilde
put crates/tui/src/app.rs $'// app 2\n'
expect fail "a line in a ~~~ block does not count" $'~~~\nno changelog: example\n~~~\n'

case_start indented-code
put crates/tui/src/app.rs $'// app 2\n'
expect fail "a line in an indented code block does not count" $'Example:\n\n    no changelog: example\n'

# The pull request rewords a fragment of main under a new name, while main's release pull
# request has consumed the old one: the diff lists it deleted, BASE no longer has it.
case_start fragment-gone-from-base
put crates/tui/src/app.rs $'// app 2\n'
g rm -q changes/fixed/kept.md
put changes/fixed/kept-reworded.md $'- The next release\'s entry, reworded.\n'
g add -A
g commit -q -m "reword a fragment"
pr_head=$(g rev-parse HEAD)
g checkout -q -B main-moved "$base"
g rm -q changes/fixed/kept.md
g commit -q -m "release consumed the fragment"
base_saved=$base
base=$(g rev-parse HEAD)
g checkout -q "case-fragment-gone-from-base"
[[ $(g rev-parse HEAD) == "$pr_head" ]]
expect pass "a fragment deleted by the pull request and by BASE" "" "kept-reworded.md"
base=$base_saved

case_start visible-escape-placeholder
put crates/tui/src/keymap/defaults.rs $'// keys 2\n'
put changes/fixed/keys.md "$frag"
expect fail "the template's placeholder is no reason" $'no doc change: <reason>\n'

case_start visible-escape-empty
put crates/tui/src/keymap/defaults.rs $'// keys 2\n'
put changes/fixed/keys.md "$frag"
expect fail "an empty reason is no reason" $'no doc change:   \n'

case_start visible-escape-comment
put crates/tui/src/keymap/defaults.rs $'// keys 2\n'
put changes/fixed/keys.md "$frag"
expect fail "a line inside an HTML comment does not count" $'<!--\nno doc change: hidden\n-->\n'

case_start tests-only
put crates/tui/src/keymap/tests.rs $'// tests 2\n'
expect fail "a test change needs a fragment or the line" "" "no changelog: <why>"

case_start tests-only-escape
put crates/tui/src/keymap/tests.rs $'// tests 2\n'
expect pass "a test change is not user-visible; no changelog with a reason" $'no changelog: tests only\n' "no user-visible change"

case_start other-locale
put locales/ko.toml $'a = "B"\n'
put changes/fixed/ko.md "$frag"
expect pass "another catalog needs a fragment, not a page" ""

case_start internal
put crates/tui/src/app.rs $'// app 2\n'
expect fail "an internal change without a fragment" "" "changes/<kind>/<slug>.md"

for spec in "notakind/x.md:unknown kind" "added/Bad.md:lowercase letters" "added/empty.md:empty" \
  "loose.md:changes/<kind>/<slug>.md" "added/sub/x.md:changes/<kind>/<slug>.md"; do
  path=${spec%%:*}
  case_start "bad-$(echo "$path" | tr '/.' '--')"
  put crates/tui/src/app.rs $'// app 2\n'
  if [[ $path == added/empty.md ]]; then put "changes/$path" ""; else put "changes/$path" "$frag"; fi
  expect fail "an invalid fragment fails: $path" $'no changelog: x\n' "${spec#*:}"
done

case_start hangul
put crates/tui/src/app.rs $'// app 2\n'
put changes/added/x.md "- $(printf '\xed\x95\x9c')"$'\n'
expect fail "a fragment not in English fails" "" "English"

case_start edit-fragment
put changes/added/old.md "$frag"
g add -A
g commit -q -m "a fragment on main"
base_saved=$base
base=$(g rev-parse HEAD)
put changes/added/old.md $'- Reworded.\n'
expect pass "rewording an existing fragment is checked and passes" ""
put changes/added/old.md $'Reworded badly.\n'
expect fail "rewording it badly fails" "" "every line is a bullet"
base=$base_saved

case_start release-rc
perl -pi -e 's/^version = "0.36.0"/version = "0.37.0-rc.1"/' "$repo/Cargo.toml"
expect pass "a release candidate needs no section and keeps the fragments" "" "Release candidate"

case_start release-big
perl -pi -e 's/^version = "0.36.0"/version = "0.37.0"/' "$repo/Cargo.toml"
g rm -q changes/fixed/kept.md
{ printf '# What\x27s new\n\n## 0.37.0 — 2026-10-30\n\n- X.\n\n'; for i in $(seq 1 12000); do printf '## 0.0.%d — 2020-01-01\n\n- Old.\n\n' "$i"; done; } > "$repo/docs/site/src/whats-new.md"
expect pass "a release with What's new over 500 KB" "" "Release: 0.36.0 -> 0.37.0"

case_start release-left
perl -pi -e 's/^version = "0.36.0"/version = "0.37.0"/' "$repo/Cargo.toml"
put changes/added/left.md "$frag"
put docs/site/src/whats-new.md $'# What\'s new\n\nIntro.\n\n## 0.37.0 — 2026-10-30\n\n- X.\n\n## 0.36.0 — 2026-10-01\n\n- Tips.\n'
expect fail "a release with fragments left fails" "" "changes/ still has fragments"

case_start release-no-section
perl -pi -e 's/^version = "0.36.0"/version = "0.37.0"/' "$repo/Cargo.toml"
expect fail "a release without its section fails" "" "scripts/changes.sh release 0.37.0 --note"

case_start release-earlier
perl -pi -e 's/^version = "0.36.0"/version = "0.37.0"/' "$repo/Cargo.toml"
put docs/site/src/whats-new.md $'# What\'s new\n\n## 0.37.0 and earlier\n\n- See GitHub.\n'
expect fail "\"## 0.37.0 and earlier\" is not the section" "" "has no"

case_start release-dependency-version
perl -pi -e 's/version = "1.0"/version = "1.1"/' "$repo/Cargo.toml"
expect pass "another version in Cargo.toml is no release" ""

# A release pull request as CONTRIBUTING describes it: fragments on main, then changes.sh.
case_start release-ok
put changes/added/a.md "$frag"
put changes/fixed/b.md $'- A fix.\n'
g add -A
g commit -q -m "fragments on main"
base_saved=$base
base=$(g rev-parse HEAD)
perl -pi -e 's/^version = "0.36.0"/version = "0.37.0"/' "$repo/Cargo.toml"
(cd "$repo" && bash scripts/changes.sh release 0.37.0 2>/dev/null)
expect pass "a release made with changes.sh passes" "" "Release: 0.36.0 -> 0.37.0"
base=$base_saved

if grep -q '^## Docs and changelog rule' "$work/summary"; then ok "the job summary is written"; else
  bad "the job summary is written"
fi

if [[ $failures -gt 0 ]]; then
  echo "$failures failed" >&2
  exit 1
fi
echo "all passed"
