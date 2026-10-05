#!/usr/bin/env bash
set -euo pipefail

: "${VERSION:?VERSION is required}"
: "${TAP_REPOSITORY:?TAP_REPOSITORY is required}"
: "${TAP_DIR:?TAP_DIR is required}"
: "${FORMULA:?FORMULA is required}"
: "${GH_TOKEN:?GH_TOKEN is required}"

askpass="$(mktemp)"
trap 'rm -f "$askpass"' EXIT
cat > "$askpass" <<'EOF'
#!/bin/sh
case "$1" in
  *Username*) printf '%s' x-access-token ;;
  *Password*) printf '%s' "$GH_TOKEN" ;;
  *) exit 1 ;;
esac
EOF
chmod 700 "$askpass"
export GIT_ASKPASS="$askpass"
export GIT_TERMINAL_PROMPT=0

branch="agentdust-$VERSION"
base="$(gh api "repos/$TAP_REPOSITORY" --jq '.default_branch')"
git -C "$TAP_DIR" fetch origin "+refs/heads/$base:refs/remotes/origin/$base"

if git -C "$TAP_DIR" ls-remote --exit-code origin "refs/heads/$branch" >/dev/null 2>&1; then
  git -C "$TAP_DIR" fetch origin "+refs/heads/$branch:refs/remotes/origin/$branch"
  git -C "$TAP_DIR" checkout -B "$branch" "origin/$branch"
  branch_start="$(git -C "$TAP_DIR" rev-parse HEAD)"
  if ! git -C "$TAP_DIR" merge-base --is-ancestor "origin/$base" HEAD; then
    git -C "$TAP_DIR" -c user.name="github-actions[bot]" -c user.email="41898282+github-actions[bot]@users.noreply.github.com" merge --no-edit "origin/$base"
  fi
else
  git -C "$TAP_DIR" checkout -b "$branch" "origin/$base"
  branch_start="$(git -C "$TAP_DIR" rev-parse HEAD)"
fi

python3 scripts/tap_update.py --tap "$TAP_DIR" --formula "$FORMULA" --version "$VERSION"
git -C "$TAP_DIR" add Formula

if ! git -C "$TAP_DIR" diff --cached --quiet; then
  git -C "$TAP_DIR" -c user.name="github-actions[bot]" -c user.email="41898282+github-actions[bot]@users.noreply.github.com" commit -m "agentdust $VERSION"
fi

if [ "$(git -C "$TAP_DIR" rev-parse HEAD)" != "$branch_start" ]; then
  git -C "$TAP_DIR" push origin "HEAD:$branch"
fi

if git -C "$TAP_DIR" diff --quiet "origin/$base" -- Formula; then
  echo "Formula already matches $base"
  exit 0
fi

existing_pr="$(gh pr list --repo "$TAP_REPOSITORY" --head "$branch" --state open --json number --jq '.[0].number // ""')"
if [ -n "$existing_pr" ]; then
  echo "Pull request #$existing_pr is already open"
else
  gh pr create --repo "$TAP_REPOSITORY" --base "$base" --head "$branch" --title "agentdust $VERSION" --body "Updates the formula to agentdust $VERSION. Publish the release before merging this pull request."
fi
