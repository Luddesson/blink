#!/usr/bin/env bash
# Sync all authorized_keys.d/*.pub to the server's /root/.ssh/authorized_keys
# inside a managed marker block. Entries outside the block are preserved.
#
# Run from any machine that already has SSH access:
#   bash deploy/ssh/sync-authorized-keys.sh

set -euo pipefail

SERVER="${BLINK_SSH_HOST:-blink}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
KEYS_DIR="$SCRIPT_DIR/authorized_keys.d"

if [ ! -d "$KEYS_DIR" ]; then
  echo "ERROR: $KEYS_DIR not found" >&2
  exit 1
fi

# Build the managed block locally.
block=$(mktemp)
trap 'rm -f "$block"' EXIT
{
  echo "# BEGIN blink-repo authorized_keys (managed by sync-authorized-keys.sh — do not edit)"
  for f in "$KEYS_DIR"/*.pub; do
    [ -f "$f" ] || continue
    # Strip trailing whitespace/newlines, emit one line per key
    sed -e 's/[[:space:]]*$//' "$f"
  done
  echo "# END blink-repo authorized_keys"
} > "$block"

key_count=$(grep -c '^ssh-' "$block" || true)
if [ "$key_count" -eq 0 ]; then
  echo "ERROR: no public keys found in $KEYS_DIR" >&2
  exit 1
fi

echo "Syncing $key_count key(s) to $SERVER ..."

# Ship the block to a temp path on the server, then swap it in atomically.
scp -q "$block" "$SERVER:/tmp/blink-keys-block.$$"

ssh "$SERVER" "REMOTE_BLOCK=/tmp/blink-keys-block.$$ bash -s" <<'REMOTE'
set -euo pipefail
AUTH=/root/.ssh/authorized_keys
BEGIN="# BEGIN blink-repo authorized_keys"
END="# END blink-repo authorized_keys"

mkdir -p /root/.ssh
chmod 700 /root/.ssh
touch "$AUTH"

# Keep every line that is NOT between the markers.
awk -v b="$BEGIN" -v e="$END" '
  index($0, b) == 1 { skip = 1; next }
  index($0, e) == 1 { skip = 0; next }
  !skip { print }
' "$AUTH" > "$AUTH.outside"

# Append the new managed block.
cat "$AUTH.outside" "$REMOTE_BLOCK" > "$AUTH.new"

# Strip any runs of blank lines.
awk 'BEGIN{blank=0} /^[[:space:]]*$/{blank++; next} {if(blank && NR>1) print ""; blank=0; print}' "$AUTH.new" > "$AUTH.clean"

mv "$AUTH.clean" "$AUTH"
chmod 600 "$AUTH"
rm -f "$AUTH.outside" "$AUTH.new" "$REMOTE_BLOCK"

active=$(grep -c '^ssh-' "$AUTH" || true)
echo "Updated $AUTH — $active active key(s)"
REMOTE

echo "Done."
