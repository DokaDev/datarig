#!/usr/bin/env bash
# The version a release run builds, from the workspace (the `datarig` binary's package). A pushed
# tag must be `v` and exactly that version: `vMAJOR.MINOR.PATCH`, or `vMAJOR.MINOR.0-rc.N` for a
# release candidate (a GitHub pre-release only, never published to the Homebrew tap). Any other
# run (a pull request, a manual run) builds the workspace version and publishes nothing.
# Writes version, tag, publish and prerelease to $GITHUB_OUTPUT (stdout without it).
set -euo pipefail

version=$(cargo metadata --no-deps --locked --format-version 1 |
  jq -r '.packages[] | select(.name == "datarig-tui") | .version')
semver='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-rc\.[1-9][0-9]*)?$'
if ! [[ $version =~ $semver ]]; then
  echo "the workspace version $version is not MAJOR.MINOR.PATCH or MAJOR.MINOR.0-rc.N" >&2
  exit 1
fi
if [[ $version == *-rc.* && ! $version =~ ^[0-9]+\.[0-9]+\.0-rc\. ]]; then
  echo "a release candidate is of a minor or major release (MAJOR.MINOR.0-rc.N), not $version" >&2
  exit 1
fi

if [[ ${GITHUB_EVENT_NAME:-} == push && ${GITHUB_REF_TYPE:-} == tag ]]; then
  tag=$GITHUB_REF_NAME
  if [[ $tag != "v$version" ]]; then
    echo "the tag $tag is not v$version, the version of the workspace (Cargo.toml) at that commit" >&2
    exit 1
  fi
  publish=true
else
  tag=v$version
  publish=false
fi
prerelease=false
if [[ $version == *-rc.* ]]; then
  prerelease=true
fi

out=${GITHUB_OUTPUT:-/dev/stdout}
{
  echo "version=$version"
  echo "tag=$tag"
  echo "publish=$publish"
  echo "prerelease=$prerelease"
} >> "$out"
echo "version $version, tag $tag, publish $publish, pre-release $prerelease" >&2
