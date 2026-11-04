#!/usr/bin/env bash
# Lists upstream commits not yet reviewed (since UPSTREAM.md's "Last reviewed"
# marker), oldest first, each with the files it touches. Files this fork
# removed are marked, so commits that only touch removed code are easy to skip.
#
#   scripts/upstream-review.sh            # fetch, then list
#   scripts/upstream-review.sh --no-fetch
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

if [[ "${1:-}" != "--no-fetch" ]]; then
  git fetch --quiet upstream
fi

last=$(grep -m1 '^Last reviewed:' UPSTREAM.md | grep -oE '[0-9a-f]{7,40}' | head -1)
if [[ -z "$last" ]]; then
  echo "UPSTREAM.md has no 'Last reviewed: <sha>' line" >&2
  exit 1
fi

commits=$(git rev-list --reverse "$last..upstream/dev")
if [[ -z "$commits" ]]; then
  echo "Nothing new upstream since $last."
  exit 0
fi

echo "Upstream commits since $last (oldest first). [removed] = path absent here."
echo
for sha in $commits; do
  total=0
  removed=0
  lines=()
  while IFS= read -r path; do
    [[ -z "$path" ]] && continue
    total=$((total + 1))
    if [[ -e "$path" ]] || git cat-file -e "HEAD:$path" 2>/dev/null; then
      lines+=("    $path")
    else
      removed=$((removed + 1))
      lines+=("    $path [removed]")
    fi
  done < <(git diff-tree --no-commit-id --name-only -r "$sha")
  verdict=""
  if ((total > 0 && removed == total)); then
    verdict="  << only removed code: likely skip"
  elif ((removed > 0)); then
    verdict="  << partly removed code"
  fi
  git log -1 --format="%h %ad %s%n" --date=short "$sha" | sed '/^$/d'
  [[ -n "$verdict" ]] && echo "$verdict"
  printf '%s\n' "${lines[@]}"
  echo
done
echo "$(echo "$commits" | wc -l | tr -d ' ') commit(s). Newest: $(git rev-parse --short upstream/dev)"
