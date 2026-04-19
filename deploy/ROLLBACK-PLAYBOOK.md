# Blink Rollback Playbook (Immediate)

This runbook defines the fastest safe rollback path for current server layout (`/opt/blink`, systemd services `blink-engine` and optional `blink-sidecar`).

## Scope

Use this when you need an immediate rollback from live/unsafe runtime state to a locked-down paper mode.

Rollback outcome:
- `TRADING_ENABLED=false` (runtime switch lock)
- `LIVE_TRADING=false` (disable live mode)
- `PAPER_TRADING=true` (safe mode reset)
- `ALPHA_TRADING_ENABLED=false` (disable autonomous sidecar-triggered orders)

---

## Fast Path (Recommended)

From repo root on your Windows operator machine:

```powershell
.\deploy\rollback-hetzner.ps1
```

This is a **preview-only** dry run (non-destructive default).

Apply rollback:

```powershell
.\deploy\rollback-hetzner.ps1 -Apply
```

---

## Manual Rollback Commands (Server)

SSH to server:

```bash
ssh root@<SERVER_IP>
```

1) Backup current environment:

```bash
sudo cp /opt/blink/.env /opt/blink/.env.rollback.$(date +%Y%m%d-%H%M%S).bak
```

2) Lock runtime switches + reset mode:

```bash
sudo sed -i 's/^TRADING_ENABLED=.*/TRADING_ENABLED=false/' /opt/blink/.env
sudo sed -i 's/^LIVE_TRADING=.*/LIVE_TRADING=false/' /opt/blink/.env
sudo sed -i 's/^PAPER_TRADING=.*/PAPER_TRADING=true/' /opt/blink/.env
sudo sed -i 's/^ALPHA_TRADING_ENABLED=.*/ALPHA_TRADING_ENABLED=false/' /opt/blink/.env
```

If any key does not exist, append it:

```bash
grep -q '^TRADING_ENABLED=' /opt/blink/.env || echo 'TRADING_ENABLED=false' | sudo tee -a /opt/blink/.env >/dev/null
grep -q '^LIVE_TRADING=' /opt/blink/.env || echo 'LIVE_TRADING=false' | sudo tee -a /opt/blink/.env >/dev/null
grep -q '^PAPER_TRADING=' /opt/blink/.env || echo 'PAPER_TRADING=true' | sudo tee -a /opt/blink/.env >/dev/null
grep -q '^ALPHA_TRADING_ENABLED=' /opt/blink/.env || echo 'ALPHA_TRADING_ENABLED=false' | sudo tee -a /opt/blink/.env >/dev/null
```

3) Restart protocol:

```bash
sudo systemctl restart blink-engine
if systemctl list-unit-files | grep -q '^blink-sidecar\.service'; then
  sudo systemctl restart blink-sidecar
fi
```

4) Verification checks (must pass):

```bash
systemctl is-active blink-engine
if systemctl list-unit-files | grep -q '^blink-sidecar\.service'; then systemctl is-active blink-sidecar; fi
grep -E '^(TRADING_ENABLED|LIVE_TRADING|PAPER_TRADING|ALPHA_TRADING_ENABLED)=' /opt/blink/.env
curl -sf http://127.0.0.1:3030/api/status
journalctl -u blink-engine -n 30 --no-pager
```

Expected:
- `blink-engine` is `active`
- `blink-sidecar` is `active` (if installed) or not present
- env values match rollback targets above
- `/api/status` returns HTTP 200

---

## Notes on Current Deploy Layout

- Hetzner deployment uses:
  - binary: `/opt/blink/engine`
  - env: `/opt/blink/.env`
  - services: `blink-engine`, `blink-sidecar`
- Oracle deployment uses:
  - env: `/opt/blink/.env`
  - service: `blink-engine`
  - binary under repo path via systemd unit (`/opt/blink/blink/blink-engine/target/release/engine`)

The rollback env keys are shared across both layouts.
