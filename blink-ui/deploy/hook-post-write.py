#!/usr/bin/env python3
"""Stub that delegates to the real hook at ../deploy/hook-post-write.py.

The PostToolUse hook is invoked with the cwd set to whichever directory the
edited file lives in. When Claude writes files inside `blink-ui/`, the relative
path `deploy/hook-post-write.py` resolves here instead of the project root.
This stub forwards stdin/stdout/exit-code to the real script.
"""
import os
import sys
import subprocess

here = os.path.dirname(os.path.abspath(__file__))
real = os.path.normpath(os.path.join(here, "..", "..", "deploy", "hook-post-write.py"))

if not os.path.exists(real):
    sys.exit(0)

stdin_data = sys.stdin.read()
result = subprocess.run(
    [sys.executable, real],
    input=stdin_data,
    capture_output=True,
    text=True,
)
if result.stdout:
    sys.stdout.write(result.stdout)
if result.stderr:
    sys.stderr.write(result.stderr)
sys.exit(result.returncode)
