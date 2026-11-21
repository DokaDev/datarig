#!/usr/bin/env bash
# Tests of .github/scripts/release-ci-gate.sh against a stand-in for gh on the PATH, which
# answers each API call from a JSON file of the case (through jq, like gh --jq) and fails for a
# call the case does not answer: bash .github/scripts/tests/release-ci-gate-test.sh
set -euo pipefail

root=$(cd "$(dirname "$0")/../../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0
sha=1111111111111111111111111111111111111111
repo=owner/repo

mkdir -p "$work/bin"
cat > "$work/bin/gh" <<'EOF'
#!/usr/bin/env bash
# gh api [--paginate] <endpoint> --jq <filter>: the answer is $FIXTURES/<endpoint, with every
# character other than a letter, digit, dot or dash as _>.json; <same>.fail makes the call fail.
set -euo pipefail
[[ $1 == api ]] || { echo "unexpected: gh $*" >&2; exit 2; }
shift
endpoint= filter=.
while (($#)); do
  case $1 in
    --paginate) shift ;;
    --jq) filter=$2; shift 2 ;;
    *) endpoint=$1; shift ;;
  esac
done
key=$(printf '%s' "$endpoint" | tr -c 'A-Za-z0-9.-' '_')
if [[ -e $FIXTURES/$key.fail ]]; then
  echo "HTTP 502: Bad Gateway ($endpoint)" >&2
  exit 1
fi
if [[ ! -e $FIXTURES/$key.json ]]; then
  echo "HTTP 404: Not Found ($endpoint)" >&2
  exit 1
fi
jq -r "$filter" "$FIXTURES/$key.json"
EOF
chmod +x "$work/bin/gh"

fixtures=
# answer <endpoint> <json>: what the stand-in returns for that call in the current case.
answer() {
  printf '%s' "$2" > "$fixtures/$(printf '%s' "$1" | tr -c 'A-Za-z0-9.-' '_').json"
}
# fail <endpoint>: that call fails in the current case.
fail() {
  : > "$fixtures/$(printf '%s' "$1" | tr -c 'A-Za-z0-9.-' '_').fail"
}
new_case() {
  fixtures=$work/$1
  mkdir -p "$fixtures"
}
compare() { # compare <branch> <status>
  answer "repos/$repo/compare/$sha...$1" "{\"status\":\"$2\"}"
}
branches() { # branches <name>...: the repository's branches
  local json=[ name
  for name in "$@"; do
    json+="{\"name\":\"$name\",\"commit\":{\"sha\":\"0\"}},"
  done
  answer "repos/$repo/branches?per_page=100" "${json%,}]"
}
runs() { # runs <branch> <status> <conclusion|null>: one push run of ci.yml for the commit
  local run="{\"id\":7,\"run_attempt\":1,\"status\":\"$2\",\"conclusion\":$3,\"event\":\"push\",\"created_at\":\"2026-01-01T00:00:00Z\",\"html_url\":\"https://example.com/runs/7\"}"
  answer "repos/$repo/actions/workflows/ci.yml/runs?head_sha=$sha&branch=$1&per_page=100" \
    "{\"workflow_runs\":[$run]}"
}
no_runs() { # no_runs <branch>
  answer "repos/$repo/actions/workflows/ci.yml/runs?head_sha=$sha&branch=$1&per_page=100" \
    '{"workflow_runs":[]}'
}
jobs() { # jobs <conclusion>...: the run's jobs
  local json=[ c i=0
  for c in "$@"; do
    i=$((i + 1))
    json+="{\"name\":\"job $i\",\"status\":\"completed\",\"conclusion\":\"$c\"},"
  done
  answer "repos/$repo/actions/runs/7/jobs?filter=latest&per_page=100" "{\"jobs\":${json%,}]}"
}

# check <name> <expected exit: 0 or 1> <text the verdict must contain>
check() {
  local name=$1 want=$2 text=$3 got=0 out
  out=$(PATH="$work/bin:$PATH" FIXTURES=$fixtures GH_REPO=$repo SHA=$sha TIMEOUT=0 INTERVAL=0 \
    GITHUB_STEP_SUMMARY=/dev/null bash "$root/.github/scripts/release-ci-gate.sh" 2>&1) || got=$?
  if [[ $want == 1 && $got != 0 ]]; then
    got=1
  fi
  if [[ $got == "$want" && $out == *"$text"* ]]; then
    echo "ok   $name"
  else
    echo "FAIL $name: exit $got, want $want; output:"
    printf '     %s\n' "${out//$'\n'/$'\n     '}"
    failures=$((failures + 1))
  fi
}

new_case on_main
compare main ahead
runs main completed '"success"'
jobs success success
check on_main 0 "CI passed on main for $sha"

new_case on_main_ci_on_release_branch_only
compare main identical
no_runs main
runs release/0.41 completed '"success"'
check on_main_ci_on_release_branch_only 1 "no CI run on main for $sha yet"

new_case on_release_branch
compare main diverged
branches main release/0.41 feature/x
compare release/0.41 ahead
runs release/0.41 completed '"success"'
jobs success
check on_release_branch 0 "CI passed on release/0.41 for $sha"

new_case release_branch_head_preferred
compare main diverged
branches release/0.40 release/0.41
compare release/0.40 ahead
compare release/0.41 identical
runs release/0.41 completed '"success"'
jobs success
check release_branch_head_preferred 0 "CI passed on release/0.41"

new_case release_branch_first_by_name
compare main diverged
branches release/0.41 release/0.40
compare release/0.40 ahead
compare release/0.41 ahead
runs release/0.40 completed '"success"'
jobs success
check release_branch_first_by_name 0 "CI passed on release/0.40"

new_case on_neither
compare main diverged
branches main release/0.41
compare release/0.41 behind
check on_neither 1 "is not on main or a release branch (compared with main: diverged)"

new_case only_on_release_pull_request_branch
compare main diverged
branches release/0.41 release/0.41.5
compare release/0.41 diverged
compare release/0.41.5 identical
check only_on_release_pull_request_branch 1 "is not on main or a release branch"

new_case compare_main_fails
fail "repos/$repo/compare/$sha...main"
check compare_main_fails 1 "HTTP 502"

new_case branch_list_fails
compare main diverged
fail "repos/$repo/branches?per_page=100"
check branch_list_fails 1 "HTTP 502"

new_case compare_release_branch_fails
compare main diverged
branches release/0.41
fail "repos/$repo/compare/$sha...release/0.41"
check compare_release_branch_fails 1 "HTTP 502"

new_case runs_fail
compare main ahead
fail "repos/$repo/actions/workflows/ci.yml/runs?head_sha=$sha&branch=main&per_page=100"
check runs_fail 1 "HTTP 502"

new_case jobs_fail
compare main ahead
runs main completed '"success"'
fail "repos/$repo/actions/runs/7/jobs?filter=latest&per_page=100"
check jobs_fail 1 "HTTP 502"

new_case ci_not_started
compare main diverged
branches release/0.41
compare release/0.41 ahead
no_runs release/0.41
check ci_not_started 1 "gave up after 0 min waiting for CI (no CI run on release/0.41 for $sha yet)"

new_case ci_not_finished
compare main ahead
runs main in_progress null
check ci_not_finished 1 "gave up after 0 min waiting for CI (CI run https://example.com/runs/7 (attempt 1) is in_progress)"

new_case ci_failed
compare main diverged
branches release/0.41
compare release/0.41 ahead
runs release/0.41 completed '"failure"'
jobs success failure
check ci_failed 1 "CI on release/0.41 for $sha concluded failure"

new_case ci_job_not_successful
compare main ahead
runs main completed '"success"'
jobs success cancelled
check ci_job_not_successful 1 "- job 2: cancelled"

if ((failures > 0)); then
  echo "$failures failed"
  exit 1
fi
echo "all passed"
