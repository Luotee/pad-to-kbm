#!/usr/bin/env bash
# Prints docs_only=true to $GITHUB_OUTPUT when a pull request changes nothing
# but README.md, CLAUDE.md and docs/** (the README pictures live there), so
# ci.yml can skip its install and test steps. The jobs still run and report
# success: a check that never reports (a path-filtered workflow) would block
# the merge once it is made a required check on main.
#
# An allow-list, not a deny-list: any other path runs the full suite.
# CHANGELOG.md is deliberately not on it: rust/tests/version_consistency.rs
# reads its top heading.
#
# On pull_request the checkout is the PR's merge commit, so HEAD^1 is the tip
# of the base branch and the diff is exactly what the PR changes (needs
# fetch-depth: 2). Anything else (workflow_dispatch) runs the full suite.
set -euo pipefail

docs_only=false
if [ "${EVENT_NAME:-}" = pull_request ]; then
  files=$(git diff --name-only HEAD^1 HEAD)
  echo "changed files:"
  echo "$files"
  if [ -n "$files" ] && ! grep -vqE '^(README\.md|CLAUDE\.md|docs/.+)$' <<<"$files"; then
    docs_only=true
  fi
fi
echo "docs_only=$docs_only"
echo "docs_only=$docs_only" >> "${GITHUB_OUTPUT:-/dev/null}"
