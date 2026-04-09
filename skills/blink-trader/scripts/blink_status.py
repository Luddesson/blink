#!/usr/bin/env python3
"""
blink_status.py — Quick status report of the Blink engine.
Calls the blink CLI and formats a human-readable summary.
"""

import subprocess
import sys
import json

BLINK_BIN = "blink"  # assume on PATH, or set full path


def run(args: list[str], output_json: bool = True) -> dict | None:
    cmd = [BLINK_BIN] + args
    if output_json:
        cmd += ["--output", "json"]
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=15)
        if result.returncode != 0:
            print(f"  [!] Command failed: {' '.join(cmd)}", file=sys.stderr)
            print(f"      {result.stderr.strip()}", file=sys.stderr)
            return None
        if output_json:
            return json.loads(result.stdout)
        return {"text": result.stdout}
    except FileNotFoundError:
        print(f"  [!] '{BLINK_BIN}' not found. Build with: cargo build -p blink-cli", file=sys.stderr)
        return None
    except subprocess.TimeoutExpired:
        print(f"  [!] Command timed out: {' '.join(cmd)}", file=sys.stderr)
        return None


def main():
    print("\n╔══════════════════════════════════════════╗")
    print("║         Blink Engine Status Report      ║")
    print("╚══════════════════════════════════════════╝\n")

    # Engine status
    status = run(["engine", "status"])
    if status:
        mode     = status.get("mode", "—")
        ws       = "✓ Connected" if status.get("ws_connected") else "✗ Disconnected"
        paused   = "⏸ Paused" if status.get("trading_paused") else "▶ Running"
        risk_st  = status.get("risk_status", "—")
        subs     = len(status.get("subscriptions", []))
        msgs     = status.get("messages_total", 0)
        print(f"  Mode:         {mode}")
        print(f"  WebSocket:    {ws}")
        print(f"  Trading:      {paused}")
        print(f"  Risk:         {risk_st}")
        print(f"  Subscriptions:{subs}")
        print(f"  Messages:     {msgs:,}")
    else:
        print("  Engine is not reachable. Is Blink running?")
        return

    print()

    # Portfolio summary
    portfolio = run(["portfolio", "balances"])
    if portfolio:
        nav      = portfolio.get("nav_usdc", 0)
        cash     = portfolio.get("cash_usdc", 0)
        unr      = portfolio.get("unrealized_pnl_usdc", 0)
        real     = portfolio.get("realized_pnl_usdc", 0)
        wr       = portfolio.get("win_rate_pct", 0)
        fill_r   = portfolio.get("fill_rate_pct", 0)
        uptime   = portfolio.get("uptime_secs", 0)

        pnl_sign = "+" if unr >= 0 else ""
        print(f"  NAV:          ${nav:,.4f}")
        print(f"  Cash:         ${cash:,.4f}")
        print(f"  Unrealised PnL: {pnl_sign}${unr:,.4f}")
        pnl_sign = "+" if real >= 0 else ""
        print(f"  Realised PnL:   {pnl_sign}${real:,.4f}")
        print(f"  Win Rate:       {wr:.1f}%   Fill Rate: {fill_r:.1f}%")
        hours = uptime // 3600
        mins  = (uptime % 3600) // 60
        print(f"  Uptime:         {hours}h {mins}m")

    print()
    print("  Run 'blink portfolio positions' for open positions.")
    print("  Run 'blink market discover' for trending markets.")
    print()


if __name__ == "__main__":
    main()
