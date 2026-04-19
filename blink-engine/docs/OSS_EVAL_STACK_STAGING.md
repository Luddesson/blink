# OSS eval stack staging (minimal footprint)

This stages a local-only evaluation stack around existing Blink endpoints/logs without touching hot-path trading code.

## What is staged

- **ClickHouse** (`127.0.0.1:8123`) for warehouse experiments.
- **Prometheus** (`127.0.0.1:9090`) + **blackbox-exporter** to probe:
  - `http://host.docker.internal:3030/health`
  - `http://host.docker.internal:3030/api/status`
  - `http://host.docker.internal:3030/api/metrics`
  - `http://host.docker.internal:3030/api/gates`
- **OpenTelemetry Collector** (`127.0.0.1:4317/4318`, metrics on `9464`) tailing:
  - `logs/engine.log.*`
  - `logs/sessions/*.log`
- **Grafana** (`127.0.0.1:3001`) with provisioned Prometheus + ClickHouse datasources.
- **MLflow** (`127.0.0.1:5001`) with local sqlite/artifact storage.

Everything binds to localhost by default, and no secrets are committed.

## Files

- `infra/oss-eval-stack/docker-compose.oss-eval.yml`
- `infra/oss-eval-stack/prometheus.yml`
- `infra/oss-eval-stack/blackbox.yml`
- `infra/oss-eval-stack/otel-collector.yml`
- `infra/oss-eval-stack/grafana/provisioning/datasources/datasources.yml`
- `infra/oss-eval-stack/.env.eval.example`
- `infra/oss-eval-stack/stage-stack.ps1`
- `tools/quantstats_from_eval.py`

## Run (Windows / PowerShell)

```powershell
Set-Location C:\path\to\blink\blink-engine
.\infra\oss-eval-stack\stage-stack.ps1 -Action validate
.\infra\oss-eval-stack\stage-stack.ps1 -Action up
.\infra\oss-eval-stack\stage-stack.ps1 -Action ps
```

Then run Blink in paper mode:

```powershell
PAPER_TRADING=true WEB_UI=true cargo run -p engine
```

Stop stack:

```powershell
.\infra\oss-eval-stack\stage-stack.ps1 -Action down
```

## QuantStats path (optional)

Use existing eval snapshots from `tools/run_eval_cycle.py`:

```powershell
python tools\quantstats_from_eval.py --snapshots logs\eval-cycle --out logs\eval-cycle\quantstats
```

If `quantstats`/`pandas` are installed, this also renders `quantstats-report.html`; otherwise it still writes `returns.csv`.

## PSR/DSR in `report.json`

`tools/run_eval_cycle.py report` now includes `risk_adjusted` with probabilistic Sharpe diagnostics derived from run NAV returns (`/api/portfolio.total_nav` deltas):

- `psr_probability_sharpe_gt_0` — probability Sharpe is above 0.
- `dsr_probability` — practical deflated Sharpe probability.
- `dsr_benchmark_sharpe_per_period` and `dsr_trials_assumed` — benchmark used by DSR.

Assumptions are emitted in `risk_adjusted.assumptions` and are intentionally explicit:
- returns are simple snapshot-to-snapshot NAV deltas,
- PSR uses skew/kurtosis-adjusted non-normal formula,
- DSR uses a practical trial proxy `trials = sqrt(n_returns)`,
- annualization uses median snapshot interval and is approximate for irregular cadence.

## Monthly strategy review packet (automated)

Compile monthly eval runs into:
- machine-readable `summary.json`
- human-readable `review-packet.md`

```powershell
python tools\run_eval_cycle.py monthly-strategy-review --month 2026-04 --eval-root logs\eval-cycle --out-dir logs\eval-cycle\monthly-review
```

Default output path:
- `logs\eval-cycle\monthly-review\<YYYY-MM>\summary.json`
- `logs\eval-cycle\monthly-review\<YYYY-MM>\review-packet.md`

The packet is deterministic for the same artifacts and gracefully reports coverage gaps when daily runs are missing.

### Scheduler hook

Enable monthly packet generation directly from the eval scheduler:

```powershell
python tools\eval_scheduler.py --cadence 24h --run-id-prefix eval --monthly-review-on-report
```

Hook behavior:
- runs after a `report` step,
- triggers only when a window closes exactly at `00:00:00Z` on day 1 of a month,
- compiles the just-completed month.
