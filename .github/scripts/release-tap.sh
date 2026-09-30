#!/usr/bin/env bash
# Commits <formula> as Formula/datarig.rb of the Homebrew tap $TAP_REPO and pushes it, over SSH
# with the tap's deploy key ($TAP_DEPLOY_KEY, a write key of that repository only). GitHub's
# host keys come from its API, so the connection never trusts an unknown key. With DRY_RUN=1
# everything but the push happens, and `git push --dry-run` checks that the key may write.
# Usage: release-tap.sh <tag> <formula>
set -euo pipefail

tag=$1
formula=$(realpath "$2")
if [[ -z ${TAP_DEPLOY_KEY:-} && ${DRY_RUN:-} == 1 ]]; then
  echo "dry run without the deploy key (a pull request from a fork): not checked"
  exit 0
fi
if [[ -z ${TAP_DEPLOY_KEY:-} ]]; then
  echo "the deploy key of $TAP_REPO (secret HOMEBREW_TAP_DEPLOY_KEY) is not set" >&2
  exit 1
fi

ssh_dir=$(mktemp -d)
trap 'rm -rf "$ssh_dir"' EXIT
printf '%s\n' "$TAP_DEPLOY_KEY" > "$ssh_dir/key"
chmod 600 "$ssh_dir/key"
curl -fsSL https://api.github.com/meta | jq -r '.ssh_keys[] | "github.com " + .' > "$ssh_dir/known_hosts"
export GIT_SSH_COMMAND="ssh -i $ssh_dir/key -o IdentitiesOnly=yes -o UserKnownHostsFile=$ssh_dir/known_hosts -o StrictHostKeyChecking=yes"

git clone --depth 1 "git@github.com:$TAP_REPO.git" "$ssh_dir/tap"
cd "$ssh_dir/tap"
mkdir -p Formula
cp "$formula" Formula/datarig.rb
git add Formula/datarig.rb
if git diff --cached --quiet; then
  echo "Formula/datarig.rb is already the formula of $tag"
  exit 0
fi
git -c user.name="github-actions[bot]" -c user.email="41898282+github-actions[bot]@users.noreply.github.com" \
  commit -q -m "datarig ${tag#v}"
if [[ ${DRY_RUN:-} == 1 ]]; then
  git push --dry-run origin HEAD
  echo "dry run: the deploy key may push to $TAP_REPO; nothing was pushed"
  exit 0
fi
git push -q origin HEAD
echo "pushed the formula of $tag to $TAP_REPO"
