#!/usr/bin/env bash
# Runs the iai-callgrind cycle gates. Returns non-zero if any regression
# threshold is breached. Intended for CI on the colo host where valgrind
# is guaranteed to be present.
set -euo pipefail
if ! command -v valgrind >/dev/null 2>&1; then
  echo "error: valgrind not on PATH; cannot run cycle gates." >&2
  exit 2
fi
cd "$(dirname "$0")/../../.."
exec cargo bench -p blink-benches --bench cycles -- "$@"
