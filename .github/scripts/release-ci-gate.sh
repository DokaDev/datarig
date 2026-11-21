#!/usr/bin/env bash
# Whether a tagged commit may be released, for the `gate` job of release.yml: the commit must be
# on main or on a release branch (`release/X.Y`, where patch releases of X.Y are made), and the
# `CI` workflow's run on that branch for it (its push, or one started by hand when the push
# event was lost) must have finished with every job successful. The branch is main when main
# has the commit; otherwise the release branch that has it, preferring one whose head is the
# commit, then the first by name. Waits (polling every INTERVAL seconds, up to TIMEOUT seconds)
# while that run has not started yet or is still running; a re-run of it counts as the same
# run, judged by its latest attempt. If the commit has several such runs, the most recent one
# decides. Any failed API call stops it with an error, so nothing is released on a guess.
# Read-only GitHub API calls through gh: GH_REPO, SHA (the full commit), and GH_TOKEN on CI.
# The verdict also goes to $GITHUB_STEP_SUMMARY when it is set.
set -euo pipefail

timeout=${TIMEOUT:-2400}
interval=${INTERVAL:-30}
summary=${GITHUB_STEP_SUMMARY:-/dev/null}

verdict() {
  echo "$1" >&2
  echo "$1" >> "$summary"
}

# `identical` (the branch's head is the commit) or `ahead` (the branch descends from it): the
# commit is on the branch.
compare() {
  gh api "repos/$GH_REPO/compare/$SHA...$1" --jq .status
}

branch=main
on_main=$(compare main)
if [[ $on_main != identical && $on_main != ahead ]]; then
  branch=
  # Release branches only: `release/X.Y`, not the branches of release pull requests
  # (`release/X.Y.Z`).
  candidates=$(gh api --paginate "repos/$GH_REPO/branches?per_page=100" \
    --jq '.[].name | select(test("^release/[0-9]+\\.[0-9]+$"))' | LC_ALL=C sort)
  ahead=
  for name in $candidates; do
    status=$(compare "$name")
    if [[ $status == identical ]]; then
      branch=$name
      break
    fi
    if [[ $status == ahead && -z $ahead ]]; then
      ahead=$name
    fi
  done
  branch=${branch:-$ahead}
  if [[ -z $branch ]]; then
    verdict "Not released: $SHA is not on main or a release branch (compared with main: $on_main). Tag a commit of main or of a release/X.Y branch."
    exit 1
  fi
  echo "$SHA is on $branch" >&2
fi

deadline=$((SECONDS + timeout))
while true; do
  run=$(gh api "repos/$GH_REPO/actions/workflows/ci.yml/runs?head_sha=$SHA&branch=$branch&per_page=100" \
    --jq '[.workflow_runs[] | select(.event == "push" or .event == "workflow_dispatch")]
      | sort_by(.created_at, .id) | last // empty
      | "\(.id) \(.run_attempt) \(.status) \(.conclusion // "none") \(.html_url)"')
  if [[ -z $run ]]; then
    state="no CI run on $branch for $SHA yet"
  else
    read -r id attempt status conclusion url <<< "$run"
    state="CI run $url (attempt $attempt) is $status"
    if [[ $status == completed ]]; then
      failed=$(gh api --paginate "repos/$GH_REPO/actions/runs/$id/jobs?filter=latest&per_page=100" \
        --jq '.jobs[] | select(.conclusion != "success") | "- \(.name): \(.conclusion // .status)"')
      if [[ $conclusion == success && -z $failed ]]; then
        verdict "CI passed on $branch for $SHA: $url (attempt $attempt)."
        exit 0
      fi
      verdict "Not released: CI on $branch for $SHA concluded $conclusion: $url (attempt $attempt)."
      if [[ -n $failed ]]; then
        verdict "$failed"
      fi
      verdict "Re-run the failed CI jobs, and once they pass, re-run this release workflow's failed jobs."
      exit 1
    fi
  fi
  if ((SECONDS >= deadline)); then
    verdict "Not released: gave up after $((timeout / 60)) min waiting for CI ($state)."
    exit 1
  fi
  echo "$state; checking again in ${interval}s" >&2
  sleep "$interval"
done
