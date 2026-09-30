#!/usr/bin/env bash
# Whether this run's tag should update the Homebrew tap formula, for the `homebrew` job of
# release.yml. A stable release (`PRERELEASE=false`) always is. A release candidate is too,
# but only until the project's first stable (non-prerelease, non-draft) GitHub release exists:
# until then the tap tracks the latest release candidate; once a stable release has shipped,
# rc tags go back to being a GitHub pre-release only and the tap keeps tracking stable.
# Fails closed: if checking for a stable release errors, the answer is "not eligible" rather
# than risking the tap being updated on bad information.
# Writes tap to $GITHUB_OUTPUT (stdout without it).
set -euo pipefail

if [[ ${PRERELEASE:-} != true ]]; then
  tap=true
elif releases=$(gh release list --repo "$GH_REPO" --exclude-drafts --exclude-pre-releases \
  --limit 1 --json tagName 2>&1); then
  if [[ $(jq 'length' <<< "$releases") -gt 0 ]]; then
    tap=false
  else
    tap=true
  fi
else
  echo "could not tell whether a stable release exists, not updating the tap: $releases" >&2
  tap=false
fi

out=${GITHUB_OUTPUT:-/dev/stdout}
echo "tap=$tap" >> "$out"
echo "tap $tap" >&2
