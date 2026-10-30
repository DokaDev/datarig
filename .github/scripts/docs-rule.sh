#!/usr/bin/env bash
# The docs and changelog rule of a pull request (.github/workflows/docs-rule.yml), over the
# files it changes (`git diff BASE...HEAD`):
#   1. A change users can see (the English catalog, key bindings, actions, `:` commands, config
#      and policy keys, connection profiles, the command line and environment variables, the
#      theme format and built-in themes) also changes a hand-written page of the docs site
#      (docs/site/src; generated pages and screens/ do not count), or the body has a line
#      `no doc change: <reason>`.
#   2. A change under crates/ or locales/ adds a changelog fragment changes/<kind>/<slug>.md, or
#      the body has a line `no changelog: <reason>`.
#   3. Every fragment the pull request adds or changes is valid (scripts/changes.sh check).
#   4. A release pull request (it changes `version` in [workspace.package] of Cargo.toml) leaves
#      no fragments in changes/, and What's new (docs/site/src/whats-new.md) has a section
#      `## <new version>`: scripts/changes.sh release <version> does both.
# Reads BASE and HEAD (commits) and BODY (the pull request's text) from the environment and only
# git objects, never the working tree, so it judges exactly HEAD. HTML comments (the template's
# hints) and code blocks of the body are ignored, and the line prefixes are matched without
# regard to case. A stacked pull request is judged against its base branch as it is now: after
# that branch was rewritten, rebase onto it (or re-run) for an exact verdict. No pipe here ends
# early (`grep -q`): under pipefail the writer's SIGPIPE would fail it. Exits 1 with what to do
# when a rule fails; the verdict also goes to $GITHUB_STEP_SUMMARY when it is set.
set -euo pipefail

: "${BASE:?BASE (the base commit) is not set}" "${HEAD:?HEAD (the head commit) is not set}"
body=${BODY:-}
summary=${GITHUB_STEP_SUMMARY:-/dev/null}
repo=$(cd "$(dirname "$0")/../.." && pwd)
changes_sh=$repo/scripts/changes.sh
page=docs/site/src/whats-new.md

for c in "$BASE" "$HEAD"; do
  git cat-file -e "$c^{commit}" 2>/dev/null || {
    echo "docs-rule: commit $c is not in this clone (check out with fetch-depth: 0)" >&2
    exit 2
  }
done

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

errors=()
notes=()
fail() { errors+=("$1"); }

# A body line `<prefix>: <reason>` with a real reason (not empty, not the template's
# `<reason>`), outside HTML comments (an unclosed one runs to the end) and code blocks (fenced
# with ``` or ~~~, or indented by four spaces or a tab); prints the reason.
body_reason() {
  BODY_TEXT=$body perl -e '
    my $prefix = shift;
    my $t = $ENV{BODY_TEXT};
    $t =~ s/\r//g;
    $t =~ s/<!--.*?(?:-->|\z)//gs;
    $t =~ s/^[ \t]*(```|~~~).*?(?:^[ \t]*\1[^\n]*$|\z)//gms;
    for (split /\n/, $t) {
      next unless /^ {0,3}\Q$prefix\E:\s*(.*\S)\s*$/i;
      next if $1 =~ /^<.*>$/;
      print $1;
      last;
    }' "$1"
}

# has_heading <file> <version>: the file has the version's heading, `## <version>` alone or
# followed by ` — <date>` (the same test as scripts/changes.sh).
has_heading() {
  [[ -n $(awk -v h="## $2" '{ l = $0; sub(/[ \t\r]+$/, "", l) }
    l == h || index(l, h " — ") == 1 { print NR; exit }' "$1") ]]
}

# Users see what these files define (tests.rs and tests/ excepted).
user_visible() {
  case $1 in
    */tests.rs | */tests/*) return 1 ;;
    locales/en.toml) ;;
    crates/tui/src/keymap.rs | crates/tui/src/keymap/*) ;;
    crates/tui/src/app/action.rs | crates/tui/src/app/action/*) ;;
    crates/tui/src/app/command.rs | crates/tui/src/app/command/*) ;;
    crates/core/src/config.rs | crates/core/src/config/*) ;;
    crates/core/src/policy.rs | crates/core/src/policy/*) ;;
    crates/core/src/profile.rs | crates/core/src/profile/*) ;;
    crates/tui/src/cli.rs | crates/tui/src/cli/*) ;;
    crates/tui/src/lib.rs) ;; # ENV_VARS, the environment variables
    crates/tui/src/theme.rs | crates/tui/src/theme/authoring.rs | crates/tui/src/theme/builtins.rs) ;;
    crates/core/src/theme.rs | crates/core/src/theme/*) ;; # theme files
    crates/core/src/paths.rs) ;; # where config and data live
    *) return 1 ;;
  esac
}

# A hand-written page of the site: Markdown under docs/site/src, not SUMMARY.md, not screens/,
# not What's new (scripts/changes.sh writes it), not a page with the "Generated ... Do not
# edit" header, not a page that only includes a generated file. Judged at HEAD, or at the merge
# base for a page the pull request deletes.
hand_written_page() {
  local path=$1 rev=$2 file=$tmp/page
  case $path in
    docs/site/src/SUMMARY.md | docs/site/src/screens/* | "$page") return 1 ;;
    docs/site/src/*.md) ;;
    *) return 1 ;;
  esac
  git show "$rev:$path" > "$file" 2>/dev/null || return 1
  if grep '^<!-- Generated' "$file" > /dev/null; then
    return 1
  fi
  # Only headings and {{#include}} lines (as tests/book_refs.rs tells generated pages apart).
  grep -v -e '^[[:space:]]*$' -e '^#' -e '^{{#include' "$file" > /dev/null
}

# `version` of [workspace.package] in Cargo.toml at a commit.
workspace_version() {
  git show "$1:Cargo.toml" > "$tmp/Cargo.toml" 2>/dev/null || return 0
  awk '
    /^\[/ { in_pkg = ($0 ~ /^\[workspace\.package\][[:space:]]*$/) ; next }
    in_pkg && /^version[[:space:]]*=/ { sub(/^[^"]*"/, ""); sub(/".*$/, ""); print; exit }' "$tmp/Cargo.toml"
}

visible=()
crates=()
docs=()
added_fragments=()
added_blobs=()
deleted_blobs=" "
bad_fragments=0
# First, so that a failing diff stops here (set -e) instead of judging nothing.
diff=$(git diff --name-status --no-renames "$BASE...$HEAD")
# What the three-dot diff compares HEAD with: a file it lists as deleted is there, while BASE
# may have dropped it since.
merge_base=$(git merge-base "$BASE" "$HEAD")
while IFS=$'\t' read -r status path; do
  [[ -n $path ]] || continue
  if user_visible "$path"; then visible+=("$path"); fi
  case $path in
    crates/* | locales/*) crates+=("$path") ;;
  esac
  rev=$HEAD
  [[ $status == D ]] && rev=$merge_base
  if hand_written_page "$path" "$rev"; then docs+=("$path"); fi
  case $path in
    changes/README.md | changes/.* | changes/*/.*) ;;
    changes/*)
      if [[ $status == D ]]; then
        deleted_blobs+="$(git rev-parse "$merge_base:$path") "
        continue
      fi
      mkdir -p "$tmp/${path%/*}"
      git show "$HEAD:$path" > "$tmp/$path"
      if out=$(cd "$tmp" && bash "$changes_sh" check "$path" 2>&1); then
        if [[ $status == A ]]; then
          added_fragments+=("$path")
          added_blobs+=("$(git rev-parse "$HEAD:$path")")
        fi
      else
        bad_fragments=1
        fail "Invalid changelog fragment:"$'\n'"$out"
      fi
      ;;
  esac
done <<< "$diff"

# A fragment moved or renamed unchanged is no new entry.
new_fragments=()
for i in ${added_fragments[@]+"${!added_fragments[@]}"}; do
  [[ $deleted_blobs == *" ${added_blobs[$i]} "* ]] || new_fragments+=("${added_fragments[$i]}")
done

no_doc=$(body_reason 'no doc change')
no_changelog=$(body_reason 'no changelog')

# 1. Docs.
if [[ ${#visible[@]} -gt 0 ]]; then
  if [[ ${#docs[@]} -gt 0 ]]; then
    notes+=("Docs: user-visible change, docs site pages changed: ${docs[*]}")
  elif [[ -n $no_doc ]]; then
    notes+=("Docs: user-visible change, no doc change: $no_doc")
  else
    fail "This pull request changes what users see (${visible[*]}) but no hand-written page of the
docs site (docs/site/src/**/*.md; generated pages, SUMMARY.md, whats-new.md and screens/
do not count).
Update the page that describes it, or, if no page needs to change, add this line to the
pull request description:
    no doc change: <why no page changes>"
  fi
else
  notes+=("Docs: no user-visible change")
fi

# 2. Changelog.
if [[ ${#crates[@]} -gt 0 ]]; then
  if [[ ${#new_fragments[@]} -gt 0 ]]; then
    notes+=("Changelog: fragments added: ${new_fragments[*]}")
  elif [[ -n $no_changelog ]]; then
    notes+=("Changelog: no changelog: $no_changelog")
  elif [[ $bad_fragments -eq 0 ]]; then
    fail "This pull request changes crates/ or locales/ but adds no changelog fragment.
Add a file changes/<kind>/<slug>.md, <kind> one of added, changed, fixed, removed,
security, <slug> lowercase letters, digits and dashes, with one or more bullets in English:
    - The result grid copies a range as TSV with Space y t.
(see changes/README.md). If users will not notice the change, add this line to the pull
request description instead:
    no changelog: <why>"
  fi
else
  notes+=("Changelog: nothing under crates/ or locales/ changed")
fi

# 4. Release.
old_version=$(workspace_version "$BASE")
new_version=$(workspace_version "$HEAD")
if [[ -n $new_version && $new_version != "$old_version" && $new_version == *-rc.* ]]; then
  # A release candidate's notes are GitHub's generated list; the fragments wait for the stable
  # release.
  notes+=("Release candidate: $old_version -> $new_version (no What's new section needed)")
elif [[ -n $new_version && $new_version != "$old_version" ]]; then
  left=$(git ls-tree -r --name-only "$HEAD" -- changes | grep -v -e '^changes/README\.md$' -e '/\.' || true)
  git show "$HEAD:$page" > "$tmp/whats-new.md" 2>/dev/null || : > "$tmp/whats-new.md"
  if [[ -n $left ]]; then
    fail "This is the release pull request of $new_version, but changes/ still has fragments:
$left
Run scripts/changes.sh release $new_version and commit the result: it moves them to
$page and deletes them."
  fi
  if ! has_heading "$tmp/whats-new.md" "$new_version"; then
    fail "This is the release pull request of $new_version, but $page has no
\"## $new_version\" section (the release notes come from it, and the release fails without
one). Run scripts/changes.sh release $new_version, or for an urgent patch without
fragments: scripts/changes.sh release $new_version --note \"Fix ...\""
  fi
  notes+=("Release: $old_version -> $new_version")
fi

{
  echo "## Docs and changelog rule"
  echo
  for n in ${notes[@]+"${notes[@]}"}; do
    echo "- $n"
  done
  if [[ ${#errors[@]} -gt 0 ]]; then
    echo
    echo "### Failed"
    for e in "${errors[@]}"; do
      echo
      echo '```text'
      echo "$e"
      echo '```'
    done
  fi
} >> "$summary"

for n in ${notes[@]+"${notes[@]}"}; do
  echo "$n"
done
if [[ ${#errors[@]} -gt 0 ]]; then
  for e in "${errors[@]}"; do
    echo >&2
    if [[ -n ${GITHUB_ACTIONS:-} ]]; then
      # One annotation per failure: %0A is a line break in a workflow command.
      e_esc=${e//'%'/'%25'}
      e_esc=${e_esc//$'\r'/'%0D'}
      e_esc=${e_esc//$'\n'/'%0A'}
      echo "::error title=Docs and changelog rule::$e_esc"
    else
      echo "$e" >&2
    fi
  done
  exit 1
fi
echo "docs and changelog rule: ok"
