#!/usr/bin/env python3
"""Claude Code PostToolUse hook — auto-deploy UI when blink-ui/src files change."""
import sys
import json
import subprocess
import os

data = json.load(sys.stdin)
fp = data.get("tool_input", {}).get("file_path", "")

if not fp:
    sys.exit(0)

# Normalize separators for cross-platform matching
fp_norm = fp.replace("\\", "/")

if "blink-ui/src/" not in fp_norm:
    sys.exit(0)

script = os.path.join(os.path.dirname(os.path.abspath(__file__)), "deploy-ui.sh")
print(f"[hook] blink-ui/src change detected — deploying UI...", flush=True)

result = subprocess.run(["bash", script], capture_output=True, text=True)
if result.stdout:
    print(result.stdout, end="", flush=True)
if result.returncode != 0:
    print(f"[hook] deploy-ui.sh failed:\n{result.stderr}", file=sys.stderr)
    sys.exit(result.returncode)
