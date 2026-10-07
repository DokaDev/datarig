#!/usr/bin/env bash
# Whether a tagged commit may be released, for the `gate` job of release.yml: the commit must be
# on main, and the `CI` workflow's push run on main for it must have finished with every job
# successful. Waits (polling every INTERVAL seconds, up to TIMEOUT seconds) while that run has
# not started yet or is still running; a re-run of it counts as the same run, judged by its
# latest attempt. If the commit has several such runs, the most recent one decides.
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

# `identical` or `ahead` (main is the commit or descends from it): the commit is on main.
on_main=$(gh api "repos/$GH_REPO/compare/$SHA...main" --jq .status)
if [[ $on_main != identical && $on_main != ahead ]]; then
  verdict "Not released: $SHA is not on main (compared with main: $on_main). Tag a commit of main."
  exit 1
fi

deadline=$((SECONDS + timeout))
while true; do
  run=$(gh api "repos/$GH_REPO/actions/workflows/ci.yml/runs?head_sha=$SHA&event=push&branch=main&per_page=100" \
    --jq '.workflow_runs | sort_by(.created_at, .id) | last // empty
      | "\(.id) \(.run_attempt) \(.status) \(.conclusion // "none") \(.html_url)"')
  if [[ -z $run ]]; then
    state="no CI run on main for $SHA yet"
  else
    read -r id attempt status conclusion url <<< "$run"
    state="CI run $url (attempt $attempt) is $status"
    if [[ $status == completed ]]; then
      failed=$(gh api --paginate "repos/$GH_REPO/actions/runs/$id/jobs?filter=latest&per_page=100" \
        --jq '.jobs[] | select(.conclusion != "success") | "- \(.name): \(.conclusion // .status)"')
      if [[ $conclusion == success && -z $failed ]]; then
        verdict "CI passed on main for $SHA: $url (attempt $attempt)."
        exit 0
      fi
      verdict "Not released: CI on main for $SHA concluded $conclusion: $url (attempt $attempt)."
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
