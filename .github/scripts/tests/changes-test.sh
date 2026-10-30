#!/usr/bin/env bash
# Tests of scripts/changes.sh, each in a scratch copy: bash .github/scripts/tests/changes-test.sh
set -euo pipefail

src=$(cd "$(dirname "$0")/../../.." && pwd)/scripts/changes.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
today=$(date -u +%Y-%m-%d)
failures=0
case_dir=

# A fresh tree with the script and a What's new page with an intro and one release.
setup() {
  case_dir=$work/$1
  mkdir -p "$case_dir/scripts" "$case_dir/docs/site/src" "$case_dir/changes"
  cp "$src" "$case_dir/scripts/changes.sh"
  printf '# Changes\n' > "$case_dir/changes/README.md"
  cat > "$case_dir/docs/site/src/whats-new.md" <<'EOF'
# What's new

The changes of each release, newest first.

## 0.36.0 — 2026-10-01

- Contextual tips.
EOF
}

fragment() { # fragment <kind/slug.md> <content>
  mkdir -p "$case_dir/changes/${1%/*}"
  printf '%s' "$2" > "$case_dir/changes/$1"
}

run() { (cd "$case_dir" && bash scripts/changes.sh "$@"); }

ok() { echo "ok   $1"; }
bad() {
  echo "FAIL $1" >&2
  failures=$((failures + 1))
}

# expect_out <name> <expected stdout> <command...>
expect_out() {
  local name=$1 want=$2 got
  shift 2
  if got=$("$@" 2>"$work/err"); then
    if [[ $got == "$want" ]]; then ok "$name"; else
      bad "$name: got"$'\n'"$got"$'\n'"want"$'\n'"$want"
    fi
  else
    bad "$name: failed: $(cat "$work/err")"
  fi
}

# expect_fail <name> <text in stderr> <command...>
expect_fail() {
  local name=$1 want=$2
  shift 2
  if "$@" >/dev/null 2>"$work/err"; then
    bad "$name: succeeded"
  elif grep -qF -- "$want" "$work/err"; then ok "$name"; else
    bad "$name: stderr lacks \"$want\": $(cat "$work/err")"
  fi
}

setup release
fragment fixed/grid-crash.md $'- The grid no longer crashes on an empty result.\n'
fragment added/zz-copy.md $'- Copy a range as TSV.\n  It goes to the clipboard.\n'
fragment added/aa-export.md $'- Export to Parquet.\n'
fragment security/tls.md $'- TLS 1.0 is refused.\r\n'
run release 0.37.0 2>/dev/null
expect_out "release writes the groups in order, below the intro" "# What's new

The changes of each release, newest first.

## 0.37.0 — $today

### Added

- Export to Parquet.
- Copy a range as TSV.
  It goes to the clipboard.

### Fixed

- The grid no longer crashes on an empty result.

### Security

- TLS 1.0 is refused.

## 0.36.0 — 2026-10-01

- Contextual tips." cat "$case_dir/docs/site/src/whats-new.md"
if [[ -z $(find "$case_dir/changes" -type f ! -name README.md) && ! -d $case_dir/changes/added ]]; then
  ok "release deletes the fragments"
else
  bad "release deletes the fragments"
fi
expect_out "notes prints the section body" "### Added

- Export to Parquet.
- Copy a range as TSV.
  It goes to the clipboard.

### Fixed

- The grid no longer crashes on an empty result.

### Security

- TLS 1.0 is refused." run notes 0.37.0
expect_out "notes of an older section" "- Contextual tips." run notes 0.36.0
expect_fail "release refuses an existing section" 'has a "## 0.37.0" section already' run release 0.37.0 --note x
expect_fail "notes of a missing section says how to add it" 'scripts/changes.sh release 0.38.0 --note' run notes 0.38.0
expect_fail "notes rejects a bad version" 'is not MAJOR.MINOR.PATCH' run notes 1.2

setup empty
expect_fail "release refuses without fragments" 'no fragments in changes/' run release 0.36.1
expect_fail "--allow-empty needs a note" '--allow-empty needs a --note' run release 0.36.1 --allow-empty
expect_fail "release refuses a bad version" 'is not MAJOR.MINOR.PATCH' run release 0.36.1-beta --note x
expect_fail "release refuses a multi-line note" 'one line' run release 0.36.1 --note $'a\nb'
expect_fail "release refuses a non-English note" 'English' run release 0.36.1 --note "$(printf '\xed\x95\x9c')"
run release 0.36.1 --note "Fix the crash on start." 2>/dev/null
expect_out "an urgent patch: a note and no fragments" "Fix the crash on start." run notes 0.36.1
run release 0.36.2 --allow-empty --note "Fix the build." 2>/dev/null
expect_out "--allow-empty with a note" "Fix the build." run notes 0.36.2

setup note-and-fragments
fragment changed/config.md $'- The config format of [ui] changed: rename `x` to `y`.\n'
expect_fail "release refuses a release candidate" 'release candidate' run release 0.37.0-rc.1 --note x
if [[ -f $case_dir/changes/changed/config.md ]]; then ok "an rc leaves the fragments"; else bad "an rc leaves the fragments"; fi
expect_fail "release refuses a note that is a heading" 'not a heading' run release 0.37.0 --note "## x"
run release 0.37.0 --note "The config changes." 2>/dev/null
expect_out "a note above the groups" "The config changes.

### Changed

- The config format of [ui] changed: rename \`x\` to \`y\`." run notes 0.37.0
printf '\n## 0.36.5-rc.1 — 2026-09-01\n\n- Candidate.\n' >> "$case_dir/docs/site/src/whats-new.md"
expect_fail "an rc heading is not the 0.36.5 section" 'no "## 0.36.5" section' run notes 0.36.5
expect_out "notes of a hand-written rc section" "- Candidate." run notes 0.36.5-rc.1

setup invalid
fragment added/ok.md $'- Fine.\n'
fragment added/Bad_Name.md $'- Fine.\n'
expect_fail "release refuses an invalid fragment" 'lowercase letters' run release 0.37.0
if [[ -f $case_dir/changes/added/ok.md ]] && ! grep -q '0.37.0' "$case_dir/docs/site/src/whats-new.md"; then
  ok "a refused release changes nothing"
else
  bad "a refused release changes nothing"
fi

setup check
fragment added/ok.md $'- One.\n- Two,\n  continued.\n'
fragment fixed/no-newline.md '- No final newline.'
fragment notakind/x.md $'- One.\n'
fragment added/empty.md ''
fragment added/prose.md $'Some prose.\n'
fragment added/blank.md $'- One.\n\n- Two.\n'
fragment added/heading.md $'### Added\n- One.\n'
fragment added/korean.md "- $(printf '\xed\x95\x9c')"$'\n'
fragment added/indent.md $'  - One.\n'
fragment added/nospace.md $'-One.\n'
printf '%s\n' '- Loose.' > "$case_dir/changes/loose.md"
expect_out "check accepts valid fragments" "" run check changes/added/ok.md changes/fixed/no-newline.md
for f in notakind/x.md:'unknown kind' added/empty.md:'empty' added/prose.md:'every line is a bullet' \
  added/blank.md:'no blank lines' added/heading.md:'every line is a bullet' \
  added/korean.md:'English' added/indent.md:'every line is a bullet' added/nospace.md:'every line is a bullet'; do
  expect_fail "check rejects ${f%%:*}" "${f#*:}" run check "changes/${f%%:*}"
done
expect_fail "check rejects a fragment outside a kind" 'changes/<kind>/<slug>.md' run check changes/loose.md
expect_fail "check reports a missing file" 'not a file' run check changes/added/missing.md

setup no-sections
cat > "$case_dir/docs/site/src/whats-new.md" <<'EOF'
# What's new

Nothing released yet.
EOF
fragment added/first.md $'- The first one.\n'
run release 0.1.0 2>/dev/null
expect_out "release appends to a page without sections" "# What's new

Nothing released yet.

## 0.1.0 — $today

### Added

- The first one." cat "$case_dir/docs/site/src/whats-new.md"

setup earlier
cat > "$case_dir/docs/site/src/whats-new.md" <<'EOS'
# What's new

Release notes appear here from the next release on.

## 0.36.0 and earlier

See GitHub Releases.
EOS
expect_fail "\"## 0.36.0 and earlier\" is not the 0.36.0 section" 'no "## 0.36.0" section' run notes 0.36.0
fragment fixed/x.md $'- A fix.\n'
run release 0.36.0 2>/dev/null
expect_out "a release next to \"0.36.0 and earlier\"" "### Fixed

- A fix." run notes 0.36.0
printf '\n## 0.35.0\n\n- Old.\n' >> "$case_dir/docs/site/src/whats-new.md"
expect_out "a heading of the version alone" "- Old." run notes 0.35.0

setup large
# Over 1 MB of older sections after the one asked for: nothing may stop reading early.
{
  for i in $(seq 1 20000); do printf '## 0.0.%d — 2020-01-01\n\n- Old entry %d with some text to make it longer.\n\n' "$i" "$i"; done
} >> "$case_dir/docs/site/src/whats-new.md"
fragment fixed/big.md $'- A fix.\n'
run release 0.37.0 2>/dev/null
expect_out "notes on a page over 1 MB" "### Fixed

- A fix." run notes 0.37.0
expect_out "notes of a section deep in a page over 1 MB" "- Old entry 19999 with some text to make it longer." run notes 0.0.19999
expect_fail "check of an unmatched glob" 'no fragments match' run check 'changes/*/*.md'

setup empty-section
printf '\n## 0.36.1 — 2026-10-02\n\n' >> "$case_dir/docs/site/src/whats-new.md"
expect_fail "notes refuses an empty section" 'is empty' run notes 0.36.1

if [[ $failures -gt 0 ]]; then
  echo "$failures failed" >&2
  exit 1
fi
echo "all passed"
