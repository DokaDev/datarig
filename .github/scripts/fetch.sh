# shellcheck shell=bash
# Sourced by the scripts that download CI tools (docs-tools.sh, demo-tools.sh, check-tools.sh).
#
# fetch <url> <sha256> <dir>: the file <dir>/<last part of the url>, checked against <sha256>.
# A file already there with that digest (restored by an actions/cache step keyed by the calling
# script, which pins every version and digest) is used as it is, so most runs download nothing.
# Otherwise it is downloaded with retries that wait longer each time (1, 2, 4, ... seconds, up to
# 10 minutes in all: GitHub's release downloads answer 504 for minutes at a time), then checked;
# a file that does not match fails here and is not kept.
fetch() {
  local url=$1 sum=$2 dir=$3
  local file=$dir/${url##*/}
  mkdir -p "$dir"
  if [[ -f $file ]] && echo "$sum  $file" | sha256sum -c --status -; then
    echo "$file: cached"
    return 0
  fi
  rm -f "$file" "$file.part"
  curl -sSfL --retry 9 --retry-all-errors --retry-max-time 600 --connect-timeout 30 \
    -o "$file.part" "$url"
  if ! echo "$sum  $file.part" | sha256sum -c --status -; then
    echo "$url: the download does not match its pinned SHA-256 ($sum)" >&2
    rm -f "$file.part"
    return 1
  fi
  mv "$file.part" "$file"
  echo "$file: downloaded"
}
