#!/usr/bin/env bash
# Remove files that older NexDesk versions had and newer ones replaced. Safe to run any time.
# Needed when a newer source tree was extracted over an older one (extracting never deletes files).
set -euo pipefail
cd "$(dirname "$0")/.."
stale=(
  crates/nexdesk-peer/src/clip.rs
  crates/nexdesk-peer/src/overlay.rs
  crates/nexdesk-peer/src/inject.rs
  crates/nexdesk-peer/src/wayland.rs
  crates/nexdesk/
  crates/nexdesk-rdp/
)
for p in "${stale[@]}"; do
  if [ -e "$p" ]; then echo "removing $p"; rm -rf "$p"; fi
done
echo "done"
