#!/usr/bin/env bash
# sync_labels.sh: create or update the repository labels with the gh CLI.
#
# The list is hardcoded here (no yq dependency) and mirrors .github/labels.yml;
# a count check keeps the two in sync. `gh label create --force` updates an
# existing label's color and description, so the script is idempotent.
#
# Usage: scripts/sync_labels.sh [owner/repo]   (default: tonianev/eonmark)
# Requires: gh (authenticated with repo scope).
set -euo pipefail

repo="${1:-tonianev/eonmark}"
root="$(cd "$(dirname "$0")/.." && pwd)"

# name|color|description
labels=(
  "good first issue|7057ff|Small, self-contained, verifiable with \`cargo test -p sim\` or sim-cli on any OS"
  "help wanted|008672|Open for anyone; usually needs a Mac or hands-on playtesting"
  "needs-design|d4c5f9|Needs a docs/design or ADR decision before code"
  "status:claimed|fbca04|Someone has commented to claim this; unclaimed again after 14 days of silence"
  "status:blocked|b60205|Waiting on another issue, PR or an owner decision"
  "platform:macos|0e8a16|Needs an Apple Silicon Mac to work on or verify"
  "area:sim|1d76db|crates/sim, deterministic simulation"
  "area:rules|1d76db|crates/rules and data/rules RON schema and validation"
  "area:ai|1d76db|crates/ai, scripted skirmish opponents"
  "area:game|1d76db|crates/game, Bevy presentation layer and macOS integration"
  "area:ui|1d76db|In-game UI, command card, menus, HUD"
  "area:art|1d76db|Models, textures, audio, fonts under assets/"
  "area:docs|1d76db|README, CONTRIBUTING, docs/ and data/*/README.md"
  "area:infra|1d76db|CI, scripts, justfile, packaging, dependencies"
  "kind:bug|d73a4a|Something does not work as documented"
  "kind:feature|a2eeef|New capability or change in scope"
  "kind:balance|e99695|A number in data/rules should change"
  "kind:docs|0075ca|Documentation only"
  "kind:question|cfd3d7|Converted from a question; usually moves to Discussions"
)

if [ -f "$root/.github/labels.yml" ]; then
  yml_count="$(grep -c '^- name:' "$root/.github/labels.yml")"
  if [ "$yml_count" -ne "${#labels[@]}" ]; then
    echo "error: .github/labels.yml has $yml_count labels, this script has ${#labels[@]}; update both" >&2
    exit 1
  fi
fi

if ! command -v gh >/dev/null 2>&1; then
  echo "error: gh is not installed (brew install gh)" >&2
  exit 1
fi

for entry in "${labels[@]}"; do
  IFS='|' read -r name color description <<<"$entry"
  gh label create "$name" --repo "$repo" --color "$color" --description "$description" --force
  echo "synced: $name"
done
echo "sync_labels: ${#labels[@]} labels synced to $repo"
