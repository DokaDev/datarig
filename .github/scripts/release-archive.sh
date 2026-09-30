#!/usr/bin/env bash
# Packs the release build of `target` as dist/datarig-<tag>-<target>.tar.gz (.zip on Windows):
# the binary, both licenses, the README, THIRD-PARTY-NOTICES.html (generated beforehand by
# scripts/third-party-notices.sh) and the notices of the C code pg_query compiles in, in one
# top-level directory; next to it <archive>.sha256. When the runner can run the binary, its
# `--version` must be the tag's version.
# Usage: release-archive.sh <tag> <target>
set -euo pipefail

tag=$1
target=$2
name=datarig-$tag-$target
exe=datarig
if [[ $target == *windows* ]]; then
  exe=datarig.exe
fi

rm -rf "dist/$name"
mkdir -p "dist/$name/licenses/pg_query"
cp "target/$target/release/$exe" "dist/$name/"
cp LICENSE-MIT LICENSE-APACHE README.md THIRD-PARTY-NOTICES.html "dist/$name/"
cp vendor/licenses/pg_query/* "dist/$name/licenses/pg_query/"

host=$(rustc -vV | sed -n 's/^host: //p')
if [[ $host == "$target" ]]; then
  said=$("dist/$name/$exe" --version | tr -d '\r')
  if [[ $said != "datarig ${tag#v}" ]]; then
    echo "the binary says \"$said\", not \"datarig ${tag#v}\"" >&2
    exit 1
  fi
  echo "$said"
fi

cd dist
if [[ $target == *windows* ]]; then
  archive=$name.zip
  7z a -tzip "$archive" "$name" > /dev/null
else
  archive=$name.tar.gz
  # No AppleDouble (._*) files from macOS tar.
  COPYFILE_DISABLE=1 tar -czf "$archive" "$name"
fi
if command -v sha256sum > /dev/null; then
  sha256sum "$archive" > "$archive.sha256"
else
  shasum -a 256 "$archive" > "$archive.sha256"
fi
rm -rf "$name"
cat "$archive.sha256"
