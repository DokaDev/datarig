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
# git objects, never the working tree, so it judges exactly HEAD. HTML comments in the body
# (the template's hints) are ignored. Exits 1 with what to do when a rule fails; the verdict
# also goes to $GITHUB_STEP_SUMMARY when it is set.
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
# `<reason>`), outside HTML comments; prints the reason.
body_reason() {
  printf '%s\n' "$body" | tr -d '\r' | perl -0pe 's/<!--.*?-->//gs' |
    sed -n "s/^[[:space:]]*$1:[[:space:]]*\(.*[^[:space:]]\)[[:space:]]*\$/\1/p" |
    grep -v '^<.*>$' | head -n 1 || true
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
    *) return 1 ;;
  esac
}

# A hand-written page of the site: Markdown under docs/site/src, not SUMMARY.md, not screens/,
# not a page with the "Generated ... Do not edit" header, not a page that only includes a
# generated file. Judged at HEAD, or at BASE for a page the pull request deletes.
hand_written_page() {
  local path=$1 rev=$2 text
  case $path in
    docs/site/src/SUMMARY.md | docs/site/src/screens/*) return 1 ;;
    docs/site/src/*.md) ;;
    *) return 1 ;;
  esac
  text=$(git show "$rev:$path" 2>/dev/null) || return 1
  if printf '%s\n' "$text" | grep -q '^<!-- Generated'; then
    return 1
  fi
  # Only headings and {{#include}} lines (as tests/book_refs.rs tells generated pages apart).
  if ! printf '%s\n' "$text" | grep -v -e '^[[:space:]]*$' -e '^#' -e '^{{#include' | grep -q .; then
    return 1
  fi
  return 0
}

# `version` of [workspace.package] in Cargo.toml at a commit.
workspace_version() {
  git show "$1:Cargo.toml" 2>/dev/null | awk '
    /^\[/ { in_pkg = ($0 ~ /^\[workspace\.package\][[:space:]]*$/) ; next }
    in_pkg && /^version[[:space:]]*=/ { sub(/^[^"]*"/, ""); sub(/".*$/, ""); print; exit }'
}

visible=()
crates=()
docs=()
added_fragments=()
bad_fragments=0
while IFS=$'\t' read -r status path; do
  [[ -n $path ]] || continue
  if user_visible "$path"; then visible+=("$path"); fi
  case $path in
    crates/* | locales/*) crates+=("$path") ;;
  esac
  rev=$HEAD
  [[ $status == D ]] && rev=$BASE
  if hand_written_page "$path" "$rev"; then docs+=("$path"); fi
  case $path in
    changes/README.md) ;;
    changes/*)
      [[ $status == D ]] && continue
      mkdir -p "$tmp/${path%/*}"
      git show "$HEAD:$path" > "$tmp/$path"
      if out=$(cd "$tmp" && bash "$changes_sh" check "$path" 2>&1); then
        [[ $status == A ]] && added_fragments+=("$path")
      else
        bad_fragments=1
        fail "Invalid changelog fragment:"$'\n'"$out"
      fi
      ;;
  esac
done < <(git diff --name-status --no-renames "$BASE...$HEAD")

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
docs site (docs/site/src/**/*.md; generated pages, SUMMARY.md and screens/ do not count).
Update the page that describes it, or, if no page needs to change, add this line to the
pull request description:
    no doc change: <why no page changes>"
  fi
else
  notes+=("Docs: no user-visible change")
fi

# 2. Changelog.
if [[ ${#crates[@]} -gt 0 ]]; then
  if [[ ${#added_fragments[@]} -gt 0 ]]; then
    notes+=("Changelog: fragments added: ${added_fragments[*]}")
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
if [[ -n $new_version && $new_version != "$old_version" ]]; then
  left=$(git ls-tree -r --name-only "$HEAD" -- changes | grep -v -e '^changes/README\.md$' -e '/\.' || true)
  if [[ -n $left ]]; then
    fail "This is the release pull request of $new_version, but changes/ still has fragments:
$left
Run scripts/changes.sh release $new_version and commit the result: it moves them to
$page and deletes them."
  fi
  re=${new_version//./\\.}
  if ! git show "$HEAD:$page" 2>/dev/null | grep -qE "^## $re( — .*)?[[:space:]]*\$"; then
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
