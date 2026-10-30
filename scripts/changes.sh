#!/usr/bin/env bash
# Changelog fragments (changes/<kind>/<slug>.md, see changes/README.md) and the What's new page
# of the docs site (docs/site/src/whats-new.md), which is also the curated top of each GitHub
# release's notes.
#
#   scripts/changes.sh release X.Y.Z [--note TEXT] [--allow-empty]
#       In the release pull request: writes the fragments as a new `## X.Y.Z — YYYY-MM-DD`
#       section at the top of What's new (Added, Changed, Fixed, Removed, Security, in that
#       order, the bullets as written) and deletes them. --note puts a one-line paragraph at the
#       top of the section. Without fragments it refuses, unless a --note is given (the urgent
#       patch path: `scripts/changes.sh release 0.36.1 --note "Fix ..."`); --allow-empty says
#       so explicitly and still needs the --note. Refuses when the section exists already.
#   scripts/changes.sh notes X.Y.Z
#       Prints the body of that section (release.yml uses it); fails when it is missing.
#   scripts/changes.sh check FILE...
#       Checks fragments: the path (changes/<kind>/<slug>.md) and the content.
#
# Plain bash 3.2 (the macOS one) and POSIX tools, so it runs anywhere.
set -euo pipefail

kinds="added changed fixed removed security"
page=docs/site/src/whats-new.md
semver='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-rc\.[1-9][0-9]*)?$'

die() {
  echo "changes.sh: $*" >&2
  exit 1
}

usage() {
  sed -n '6,16p' "$0" | sed 's/^# \{0,1\}//' >&2
  exit 2
}

title() {
  case $1 in
    added) echo Added ;;
    changed) echo Changed ;;
    fixed) echo Fixed ;;
    removed) echo Removed ;;
    security) echo Security ;;
  esac
}

# Text in Korean, Chinese or Japanese script: fragments are English.
has_cjk() {
  ! perl -CSD -ne 'exit 1 if /[\p{Hangul}\p{Han}\p{Hiragana}\p{Katakana}]/' "$1" 2>/dev/null
}

# check_one <file> [<name shown>]: prints what is wrong with one fragment; returns 1 if
# anything is.
check_one() {
  local file=$1 name=${2:-$1} base dir parent ok=0 line n=0 bullets=0
  base=${name##*/}
  dir=${name%/*}
  parent=${dir##*/}
  dir=${dir%/*}
  if [[ $name != */*/* || ${dir##*/} != changes ]]; then
    echo "$name: a fragment is changes/<kind>/<slug>.md"
    return 1
  fi
  case " $kinds " in
    *" $parent "*) ;;
    *)
      echo "$name: unknown kind \"$parent\"; use one of: $kinds"
      ok=1
      ;;
  esac
  if ! [[ $base =~ ^[a-z0-9-]+\.md$ ]]; then
    echo "$name: the file name must be lowercase letters, digits and dashes, ending in .md (like grid-copy-tsv.md)"
    ok=1
  fi
  if [[ ! -f $file ]]; then
    echo "$name: not a file"
    return 1
  fi
  while IFS= read -r line || [[ -n $line ]]; do
    line=${line%$'\r'}
    n=$((n + 1))
    if [[ $line =~ ^-\ [^[:space:]] ]]; then
      bullets=$((bullets + 1))
    elif [[ $line =~ ^\ \ [^[:space:]] && $bullets -gt 0 ]]; then
      : # a bullet continued on the next line
    elif [[ -z ${line//[[:space:]]/} ]]; then
      echo "$name:$n: no blank lines: one bullet after the other"
      ok=1
    else
      echo "$name:$n: every line is a bullet (\"- text\") or continues one (indented by two spaces)"
      ok=1
    fi
  done < "$file"
  if [[ $bullets -eq 0 ]]; then
    echo "$name: empty; write one or more bullets, like: - The grid copies a range as TSV."
    ok=1
  fi
  if has_cjk "$file"; then
    echo "$name: fragments are written in English"
    ok=1
  fi
  return $ok
}

cmd_check() {
  [[ $# -gt 0 ]] || usage
  local f bad=0
  for f in "$@"; do
    check_one "$f" >&2 || bad=1
  done
  return $bad
}

# The line number of the heading of a version in $page, `## <version>` alone or followed by
# ` — <date>` (not `## 0.36.0 and earlier`), empty when there is none.
heading_line() {
  local re=${1//./\\.}
  grep -nE "^## $re( — .*)?[[:space:]]*$" "$page" | head -n 1 | cut -d: -f1 || true
}

cmd_release() {
  local version=${1:-} note="" allow_empty=0 fragments=() f kind bad=0 at
  [[ -n $version ]] || usage
  shift
  while [[ $# -gt 0 ]]; do
    case $1 in
      --note)
        [[ $# -ge 2 ]] || die "--note needs the text"
        note=$2
        shift 2
        ;;
      --allow-empty)
        allow_empty=1
        shift
        ;;
      *) usage ;;
    esac
  done
  [[ $version =~ $semver ]] || die "$version is not MAJOR.MINOR.PATCH or MAJOR.MINOR.0-rc.N"
  [[ -f $page ]] || die "$page is missing"
  if [[ $note == *$'\n'* || -n $note && -z ${note//[[:space:]]/} ]]; then
    die "--note is one line of text"
  fi
  if [[ -n $(heading_line "$version") ]]; then
    die "$page has a \"## $version\" section already"
  fi

  # Every file under changes/ but its README must be a valid fragment, or nothing is written.
  while IFS= read -r f; do
    [[ -n $f ]] || continue
    fragments+=("$f")
    check_one "$f" >&2 || bad=1
  done < <(find changes -type f ! -path changes/README.md 2>/dev/null | LC_ALL=C sort)
  [[ $bad -eq 0 ]] || die "fix the fragments above first"
  if [[ ${#fragments[@]} -eq 0 ]]; then
    if [[ -z $note ]]; then
      if [[ $allow_empty -eq 1 ]]; then
        die "--allow-empty needs a --note \"...\": the one line the section says"
      fi
      die "no fragments in changes/. For a release without any (an urgent patch), give the one line the section says: scripts/changes.sh release $version --note \"Fix ...\""
    fi
  fi
  if [[ -n $note ]] && has_cjk <(printf '%s\n' "$note"); then
    die "--note is written in English"
  fi

  # Not local: the EXIT trap runs after the function returns.
  section=$(mktemp)
  trap 'rm -f "$section" "$section.page"' EXIT
  {
    printf '## %s — %s\n' "$version" "$(date -u +%Y-%m-%d)"
    if [[ -n $note ]]; then
      printf '\n%s\n' "$note"
    fi
    for kind in $kinds; do
      local group=()
      for f in ${fragments[@]+"${fragments[@]}"}; do
        [[ $f == "changes/$kind/"* ]] && group+=("$f")
      done
      [[ ${#group[@]} -gt 0 ]] || continue
      printf '\n### %s\n\n' "$(title "$kind")"
      for f in "${group[@]}"; do
        sed -e 's/\r$//' -e 's/[[:space:]]*$//' "$f"
      done
    done
    echo
  } > "$section"

  # Above the first `## ` section, below the intro; at the end when there is none yet.
  at=$(grep -n '^## ' "$page" | head -n 1 | cut -d: -f1 || true)
  if [[ -n $at ]]; then
    { head -n $((at - 1)) "$page"; cat "$section"; tail -n +"$at" "$page"; } > "$section.page"
  else
    { cat "$page"; [[ -z $(tail -c 1 "$page") ]] || echo; echo; cat "$section"; } > "$section.page"
    # Drop the trailing blank line of the last section.
    sed -e '$ { /^$/d; }' "$section.page" > "$section.page.tmp" && mv "$section.page.tmp" "$section.page"
  fi
  cat "$section.page" > "$page"

  for f in ${fragments[@]+"${fragments[@]}"}; do
    rm -f "$f"
  done
  for kind in $kinds; do
    rmdir "changes/$kind" 2>/dev/null || true
  done
  echo "wrote \"## $version\" to $page from ${#fragments[@]} fragment(s) and deleted them; commit both" >&2
}

cmd_notes() {
  local version=${1:-} start
  [[ -n $version && $# -eq 1 ]] || usage
  [[ $version =~ $semver ]] || die "$version is not MAJOR.MINOR.PATCH or MAJOR.MINOR.0-rc.N"
  [[ -f $page ]] || die "$page is missing"
  start=$(heading_line "$version")
  if [[ -z $start ]]; then
    die "$page has no \"## $version\" section. In the release pull request run: scripts/changes.sh release $version (or, without fragments, an urgent patch: scripts/changes.sh release $version --note \"Fix ...\")"
  fi
  # The lines after the heading up to the next `## ` heading, without blank lines around them.
  tail -n +"$((start + 1))" "$page" |
    awk '/^## /{exit} {lines[++n]=$0} END {
      first=1; while (first<=n && lines[first] ~ /^[ \t]*$/) first++
      last=n; while (last>=first && lines[last] ~ /^[ \t]*$/) last--
      if (first>last) exit 3
      for (i=first; i<=last; i++) print lines[i]
    }' || die "the \"## $version\" section of $page is empty"
}

[[ $# -gt 0 ]] || usage
sub=$1
shift
case $sub in
  # These two work on the repository the script is in, from any directory.
  release) cd "$(dirname "$0")/.." && cmd_release "$@" ;;
  notes) cd "$(dirname "$0")/.." && cmd_notes "$@" ;;
  # Paths as given, relative to the current directory.
  check) cmd_check "$@" ;;
  *) usage ;;
esac
