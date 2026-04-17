#!/usr/bin/env bash
# Install project git hooks by pointing git to deploy/hooks/.
# Run once per clone: bash deploy/install-hooks.sh

set -e
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOKS_DIR="$SCRIPT_DIR/hooks"

cd "$REPO_ROOT"
git config core.hooksPath "$HOOKS_DIR"
chmod +x "$HOOKS_DIR"/*

echo "Git hooks installed from deploy/hooks/"
echo "Active hooks: $(ls "$HOOKS_DIR" | grep -v '\.sample' | tr '\n' ' ')"
