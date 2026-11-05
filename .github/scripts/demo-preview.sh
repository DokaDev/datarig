#!/usr/bin/env bash
# The comment of .github/workflows/demo-preview.yml: the stills of a Demo run (<dir>/*.png, the
# fixed names below only, each checked to be a PNG) are pushed to the `demo-preview` branch as
# one commit with no history, under the run's id, and one comment on the pull request (found by
# its marker) shows them, created or updated in place. Reads REPO, RUN_ID, RUN_URL, CONCLUSION,
# HEAD_SHA and HEAD_BRANCH from the environment, and GH_TOKEN for git and gh.
#   demo-preview.sh <dir>
set -euo pipefail
dir=$1
marker='<!-- demo-preview -->'
stills=(hero plans search environments completion social)

pr=$(gh api "repos/$REPO/pulls?state=open&head=${REPO%%/*}:$HEAD_BRANCH" --jq '.[0].number // empty')
if [ -z "$pr" ]; then
  echo "no open pull request for $HEAD_BRANCH; nothing to comment on"
  exit 0
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/$RUN_ID"
found=()
for name in "${stills[@]}"; do
  file=$dir/$name.png
  [ -f "$file" ] || continue
  # A PNG and nothing else (the artifact comes from a pull request's run).
  [ "$(head -c 8 "$file" | od -An -tx1 | tr -d ' \n')" = 89504e470d0a1a0a ] || continue
  cp "$file" "$work/$RUN_ID/$name.png"
  found+=("$name")
done

body=$marker$'\n'"**Demo recordings** of $HEAD_SHA: $CONCLUSION ([run]($RUN_URL); the GIFs are in its \`demo\` artifact)."$'\n'
if [ ${#found[@]} -gt 0 ]; then
  git -C "$work" init -q -b demo-preview
  git -C "$work" add "$RUN_ID"
  git -C "$work" -c user.name="github-actions[bot]" -c user.email="41898282+github-actions[bot]@users.noreply.github.com" \
    commit -q -m "Stills of Demo run $RUN_ID"
  git -C "$work" push -q -f "https://x-access-token:$GH_TOKEN@github.com/$REPO.git" demo-preview
  commit=$(git -C "$work" rev-parse HEAD)
  for name in "${found[@]}"; do
    body+=$'\n'"**$name**"$'\n\n'"![$name](https://raw.githubusercontent.com/$REPO/$commit/$RUN_ID/$name.png)"$'\n'
  done
else
  body+=$'\n'"No stills in this run."$'\n'
fi

# The first comment of this workflow (no `head`: under pipefail it could kill gh mid-write).
ids=$(gh api "repos/$REPO/issues/$pr/comments" --paginate \
  --jq ".[] | select(.user.login == \"github-actions[bot]\" and (.body | startswith(\"$marker\"))) | .id")
id=${ids%%$'\n'*}
if [ -n "$id" ]; then
  gh api -X PATCH "repos/$REPO/issues/comments/$id" -f body="$body" >/dev/null
else
  gh api -X POST "repos/$REPO/issues/$pr/comments" -f body="$body" >/dev/null
fi
echo "comment on #$pr: ${found[*]:-none}"
