#!/usr/bin/env bash
# Rename the product everywhere in one go.
#   scripts/rename-brand.sh NewName            (display name, e.g. "Orbitly"; lower case is derived: "orbitly")
# Changes: crate/package names, binary names, directories, config paths, text in docs and UI. Review `git diff` afterwards.
set -euo pipefail
cd "$(dirname "$0")/.."
NEW="${1:?usage: $0 NewName}"
new_lower="$(echo "$NEW" | tr '[:upper:]' '[:lower:]')"
[[ "$new_lower" =~ ^[a-z][a-z0-9]{1,20}$ ]] || { echo "use letters and digits only" >&2; exit 2; }
OLD="NexDesk"; old_lower="nexdesk"; old_upper="NEXDESK"; new_upper="$(echo "$NEW" | tr '[:lower:]' '[:upper:]')"
files=$(git ls-files 2>/dev/null || find . -type f -not -path './target/*' -not -path './.git/*')
for f in $files; do
  [ -f "$f" ] || continue
  case "$f" in vendor/*|Cargo.lock|*.png|*.otf|*.ttf|*.svg) continue;; esac
  grep -Iq . "$f" 2>/dev/null || continue
  sed -i "s/$old_upper/$new_upper/g; s/$OLD/$NEW/g; s/$old_lower/$new_lower/g" "$f"
done
# rename paths that contain the old name, deepest first
find . -depth -name "*$old_lower*" -not -path './target/*' -not -path './.git/*' -not -path './vendor/*' | while read -r p; do
  d=$(dirname "$p"); b=$(basename "$p"); mv "$p" "$d/${b//$old_lower/$new_lower}"
done
rm -f Cargo.lock
echo "renamed to $NEW. Next: cargo build --workspace (regenerates Cargo.lock), review git diff, update domain/URLs/logo."
