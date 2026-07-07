#!/usr/bin/env bash
# Create (or update) the labels that drive GitHub's release-notes categories
# (.github/release.yml). Idempotent: `gh label create --force` updates an
# existing label instead of failing. The reused GitHub defaults are re-created
# with their existing color/description so no release.yml category ever
# references a missing label. Requires `gh` authenticated with write access.
set -euo pipefail

ensure() {
  gh label create "$1" --color "$2" --description "$3" --force
}

# New labels for release-notes categories.
ensure "breaking-change" "B60205" "Backwards-incompatible change"
ensure "performance"     "0E8A16" "Performance improvement"
ensure "dependencies"    "0366D6" "Dependency update"
ensure "skip-changelog"  "CFD3D7" "Exclude this PR from the release notes"

# Reused GitHub default labels — re-created idempotently with their existing
# metadata so their categories always match.
ensure "enhancement"   "A2EEEF" "New feature or request"
ensure "bug"           "D73A4A" "Something isn't working"
ensure "documentation" "0075CA" "Improvements or additions to documentation"

echo "Release-notes labels ensured."
