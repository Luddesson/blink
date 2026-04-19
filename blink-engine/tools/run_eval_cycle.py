#!/usr/bin/env python3
from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import math
import pathlib
import re
import statistics
import subprocess
import urllib.error
import urllib.request
from typing import Any


DEFAULT_ENDPOINTS = (
    "/api/status",
    "/api/mode",
    "/api/portfolio",
    "/api/history",
    "/api/alpha",
    "/api/metrics",
    "/api/gates",
    "/api/rejections",
    "/api/pnl-attribution",
)


def now_utc_iso() -> str:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat()


def fetch_json(base_url: str, path: str, timeout: float = 10.0) -> dict[str, Any]:
    url = f"{base_url.rstrip('/')}{path}"
    req = urllib.request.Request(url=url, method="GET")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as response:
            payload = response.read().decode("utf-8")
            parsed = json.loads(payload)
            if not isinstance(parsed, dict):
                raise ValueError(f"Expected JSON object from {url}")
            return parsed
    except urllib.error.HTTPError as exc:
        raise RuntimeError(f"HTTP {exc.code} for {url}") from exc
    except urllib.error.URLError as exc:
        raise RuntimeError(f"Network error for {url}: {exc.reason}") from exc


def safe_git_sha(repo_root: pathlib.Path) -> str:
    try:
        return (
            subprocess.check_output(
                ["git", "rev-parse", "HEAD"],
                cwd=repo_root,
                stderr=subprocess.DEVNULL,
                text=True,
            )
            .strip()
        )
    except Exception:
        return "unknown"


def file_sha256(path: pathlib.Path) -> str:
    if not path.exists():
        return "missing"
    hasher = hashlib.sha256()
    hasher.update(path.read_bytes())
    return hasher.hexdigest()


def ensure_dir(path: pathlib.Path) -> None:
    path.mkdir(parents=True, exist_ok=True)


def read_json_object(path: pathlib.Path) -> dict[str, Any]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise ValueError(f"Expected JSON object in {path}")
    return payload


def optional_json_object(path: pathlib.Path) -> dict[str, Any] | None:
    if not path.exists():
        return None
    return read_json_object(path)


def stable_hash(payload: dict[str, Any]) -> str:
    canonical = json.dumps(payload, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


def _stable_hash_any(payload: Any) -> str:
    canonical = json.dumps(payload, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


def safe_run_id_filename(run_id: str) -> str:
    safe = re.sub(r"[^A-Za-z0-9._-]", "_", run_id).strip("._")
    return safe or hashlib.sha256(run_id.encode("utf-8")).hexdigest()[:16]


def run_output_dir(output_dir: str, run_id: str) -> pathlib.Path:
    return pathlib.Path(output_dir).resolve() / safe_run_id_filename(run_id)


def to_relative_path(path: pathlib.Path, repo_root: pathlib.Path) -> str:
    path = path.resolve()
    repo_root = repo_root.resolve()
    try:
        return str(path.relative_to(repo_root))
    except ValueError:
        return str(path)


def resolve_input_path(value: str, *, repo_root: pathlib.Path) -> pathlib.Path:
    candidate = pathlib.Path(value)
    if candidate.is_absolute():
        return candidate.resolve()
    return (repo_root / candidate).resolve()


def latest_snapshot_path(out_dir: pathlib.Path) -> pathlib.Path | None:
    snapshots = sorted(out_dir.glob("snapshot-*.json"))
    if not snapshots:
        return None
    return snapshots[-1]


def extract_summary_kpis(report: dict[str, Any]) -> dict[str, Any]:
    capital = report.get("capital", {})
    health = report.get("health", {})
    if not isinstance(capital, dict):
        capital = {}
    if not isinstance(health, dict):
        health = {}
    return {
        "snapshot_count": report.get("snapshot_count"),
        "avg_invested_usdc": capital.get("avg_invested_usdc"),
        "peak_invested_usdc": capital.get("peak_invested_usdc"),
        "avg_cash_usdc": capital.get("avg_cash_usdc"),
        "avg_open_positions": capital.get("avg_open_positions"),
        "peak_open_positions": capital.get("peak_open_positions"),
        "ws_connected_ratio": health.get("ws_connected_ratio"),
        "messages_total_start": health.get("messages_total_start"),
        "messages_total_end": health.get("messages_total_end"),
    }


def collect_decision_tags(
    fingerprint: dict[str, Any] | None,
    report: dict[str, Any],
    cli_tags: list[str],
    decision_payload: dict[str, Any] | None,
) -> list[str]:
    tags: set[str] = {tag.strip() for tag in cli_tags if tag.strip()}
    strategy_mode = ""
    if fingerprint and isinstance(fingerprint.get("strategy_mode_hint"), str):
        strategy_mode = str(fingerprint["strategy_mode_hint"]).strip()
    if strategy_mode:
        tags.add(f"strategy:{strategy_mode}")
    report_tags = report.get("decision_tags")
    if isinstance(report_tags, list):
        for tag in report_tags:
            if isinstance(tag, str) and tag.strip():
                tags.add(tag.strip())
    if decision_payload and isinstance(decision_payload.get("decision_tags"), list):
        for tag in decision_payload["decision_tags"]:
            if isinstance(tag, str) and tag.strip():
                tags.add(tag.strip())
    return sorted(tags)


def build_registry_record(
    *,
    run_id: str,
    repo_root: pathlib.Path,
    output_dir: pathlib.Path,
    thresholds_file: pathlib.Path | None,
    decision_file: pathlib.Path | None,
    decision_tags: list[str],
) -> dict[str, Any]:
    fingerprint_path = output_dir / "fingerprint.json"
    report_path = output_dir / "report.json"

    report = read_json_object(report_path)
    fingerprint = optional_json_object(fingerprint_path)
    decision_payload = optional_json_object(decision_file) if decision_file else None

    summary_kpis = extract_summary_kpis(report)
    latest_snapshot = latest_snapshot_path(output_dir)

    file_links = {
        "output_dir": to_relative_path(output_dir, repo_root),
        "report": to_relative_path(report_path, repo_root),
        "fingerprint": to_relative_path(fingerprint_path, repo_root) if fingerprint_path.exists() else None,
        "latest_snapshot": to_relative_path(latest_snapshot, repo_root) if latest_snapshot else None,
        "thresholds": to_relative_path(thresholds_file, repo_root) if thresholds_file and thresholds_file.exists() else None,
        "decision_output": to_relative_path(decision_file, repo_root) if decision_file and decision_file.exists() else None,
    }

    return {
        "run_id": run_id,
        "created_at_utc": (fingerprint or {}).get("captured_at_utc") or report.get("generated_at_utc") or now_utc_iso(),
        "report_generated_at_utc": report.get("generated_at_utc"),
        "fingerprint_captured_at_utc": (fingerprint or {}).get("captured_at_utc"),
        "fingerprint_hashes": {
            "git_sha": (fingerprint or {}).get("git_sha"),
            "env_sha256": (fingerprint or {}).get("env_sha256"),
        },
        "summary_kpis": summary_kpis,
        "decision_tags": collect_decision_tags(fingerprint, report, decision_tags, decision_payload),
        "notes": (fingerprint or {}).get("notes", ""),
        "strategy_mode_hint": (fingerprint or {}).get("strategy_mode_hint", ""),
        "files": file_links,
        "artifact_hashes": {
            "fingerprint_sha256": file_sha256(fingerprint_path),
            "report_sha256": file_sha256(report_path),
            "thresholds_sha256": file_sha256(thresholds_file) if thresholds_file else None,
            "decision_output_sha256": file_sha256(decision_file) if decision_file else None,
        },
    }


def upsert_registry_record(registry_dir: pathlib.Path, record: dict[str, Any]) -> tuple[bool, pathlib.Path]:
    ensure_dir(registry_dir)
    runs_dir = registry_dir / "runs"
    ensure_dir(runs_dir)

    run_id = str(record.get("run_id", "")).strip()
    if not run_id:
        raise ValueError("Registry record is missing run_id")

    run_record_path = runs_dir / f"{safe_run_id_filename(run_id)}.json"

    existing = optional_json_object(run_record_path)
    existing_hash = existing.get("record_hash") if existing else None
    existing_created_at = existing.get("created_at_utc") if existing else None

    candidate = dict(record)
    candidate["created_at_utc"] = existing_created_at or candidate.get("created_at_utc") or now_utc_iso()
    candidate["updated_at_utc"] = now_utc_iso()

    candidate_for_hash = dict(candidate)
    candidate_for_hash.pop("updated_at_utc", None)
    candidate_for_hash.pop("record_hash", None)
    new_hash = stable_hash(candidate_for_hash)

    if existing_hash == new_hash:
        return False, run_record_path

    candidate["record_hash"] = new_hash
    run_record_path.write_text(json.dumps(candidate, indent=2, sort_keys=True), encoding="utf-8")

    index_path = registry_dir / "registry.jsonl"
    index_event = {
        "event": "upsert",
        "event_at_utc": now_utc_iso(),
        "run_id": run_id,
        "record_hash": new_hash,
        "record_path": f"runs/{run_record_path.name}",
        "summary_kpis": candidate.get("summary_kpis", {}),
        "decision_tags": candidate.get("decision_tags", []),
    }
    with index_path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(index_event, sort_keys=True) + "\n")

    return True, run_record_path


def command_registry_upsert(args: argparse.Namespace) -> int:
    if not args.run_id:
        raise ValueError("--run-id is required for registry-upsert")
    repo_root = pathlib.Path(args.repo_root).resolve()
    output_dir = run_output_dir(args.output_dir, args.run_id)
    thresholds_file = pathlib.Path(args.thresholds_file).resolve() if args.thresholds_file else None
    decision_file = pathlib.Path(args.decision_file).resolve() if args.decision_file else None
    registry_dir = pathlib.Path(args.registry_dir).resolve()

    record = build_registry_record(
        run_id=args.run_id,
        repo_root=repo_root,
        output_dir=output_dir,
        thresholds_file=thresholds_file,
        decision_file=decision_file,
        decision_tags=args.decision_tag,
    )
    changed, run_record_path = upsert_registry_record(registry_dir, record)
    if changed:
        print(f"Registry updated: {run_record_path}")
    else:
        print(f"Registry unchanged for run_id={args.run_id}")
    print(f"Index: {registry_dir / 'registry.jsonl'}")
    return 0


def command_registry_query(args: argparse.Namespace) -> int:
    registry_dir = pathlib.Path(args.registry_dir).resolve()
    runs_dir = registry_dir / "runs"
    if not runs_dir.exists():
        print("[]")
        return 0

    records: list[dict[str, Any]] = []
    for path in sorted(runs_dir.glob("*.json")):
        payload = optional_json_object(path)
        if payload:
            records.append(payload)

    if args.run_id:
        records = [record for record in records if record.get("run_id") == args.run_id]
    if args.tag:
        required_tags = {tag.strip() for tag in args.tag if tag.strip()}
        records = [
            record
            for record in records
            if required_tags.issubset({str(tag) for tag in record.get("decision_tags", []) if isinstance(tag, str)})
        ]

    records.sort(key=lambda item: str(item.get("updated_at_utc", "")), reverse=True)
    if args.limit is not None and args.limit >= 0:
        records = records[: args.limit]

    print(json.dumps(records, indent=2, sort_keys=True))
    return 0


def command_start(args: argparse.Namespace) -> int:
    if not args.run_id:
        raise ValueError("--run-id is required for start")
    out_dir = run_output_dir(args.output_dir, args.run_id)
    ensure_dir(out_dir)

    repo_root = pathlib.Path(args.repo_root).resolve()
    env_path = pathlib.Path(args.env_path).resolve()
    fingerprint = {
        "captured_at_utc": now_utc_iso(),
        "run_id": args.run_id,
        "base_url": args.base_url,
        "git_sha": safe_git_sha(repo_root),
        "env_path": str(env_path),
        "env_sha256": file_sha256(env_path),
        "notes": args.notes,
        "strategy_mode_hint": args.strategy_mode_hint,
    }
    (out_dir / "fingerprint.json").write_text(
        json.dumps(fingerprint, indent=2, sort_keys=True),
        encoding="utf-8",
    )
    print(f"Wrote {out_dir / 'fingerprint.json'}")
    return command_snapshot(args)


def command_snapshot(args: argparse.Namespace) -> int:
    if not args.run_id:
        raise ValueError("--run-id is required for snapshot")
    out_dir = run_output_dir(args.output_dir, args.run_id)
    ensure_dir(out_dir)

    snapshot = {
        "captured_at_utc": now_utc_iso(),
        "run_id": args.run_id,
        "base_url": args.base_url,
        "data": {},
    }
    for endpoint in DEFAULT_ENDPOINTS:
        snapshot["data"][endpoint] = fetch_json(args.base_url, endpoint)
    snapshot["derived"] = {
        "funnel": _derive_funnel(snapshot["data"]),
        "gate_rejections_total": _collect_gate_totals(snapshot["data"]),
    }

    ts = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    out_file = out_dir / f"snapshot-{safe_run_id_filename(args.run_id)}-{ts}.json"
    out_file.write_text(json.dumps(snapshot, indent=2), encoding="utf-8")
    _persist_window_rollup(out_dir, _collect_snapshots(out_dir))
    print(f"Wrote {out_file}")
    return 0


def _collect_snapshots(out_dir: pathlib.Path) -> list[dict[str, Any]]:
    snapshots: list[dict[str, Any]] = []
    for path in sorted(out_dir.glob("snapshot-*.json")):
        payload = json.loads(path.read_text(encoding="utf-8"))
        if isinstance(payload, dict):
            snapshots.append(payload)
    return snapshots


def _collect_gate_totals(data: dict[str, Any]) -> dict[str, int]:
    gates = data.get("/api/gates", {})
    if not isinstance(gates, dict):
        return {}
    rows = gates.get("gates")
    if not isinstance(rows, list):
        return {}

    totals: dict[str, int] = {}
    for gate in rows:
        if not isinstance(gate, dict):
            continue
        gate_name = gate.get("gate")
        total = gate.get("rejections_total")
        if isinstance(gate_name, str) and isinstance(total, int):
            totals[gate_name] = total
    return totals


def _derive_funnel(data: dict[str, Any]) -> dict[str, Any]:
    portfolio = data.get("/api/portfolio", {})
    if not isinstance(portfolio, dict):
        return {}

    total_signals = portfolio.get("total_signals")
    filled_orders = portfolio.get("filled_orders")
    skipped_orders = portfolio.get("skipped_orders")
    aborted_orders = portfolio.get("aborted_orders")
    if not all(isinstance(v, int) for v in [total_signals, filled_orders, skipped_orders, aborted_orders]):
        return {}

    accepted_signals = max(total_signals - skipped_orders, 0)
    return {
        "signals": total_signals,
        "accepted": accepted_signals,
        "fills": filled_orders,
        "aborts": aborted_orders,
        "rejections": skipped_orders,
        "accept_rate_pct": round((accepted_signals / total_signals) * 100.0, 4) if total_signals > 0 else 0.0,
        "fill_from_accept_pct": round((filled_orders / accepted_signals) * 100.0, 4)
        if accepted_signals > 0
        else 0.0,
    }


def _compute_window_rollup(snapshots: list[dict[str, Any]]) -> dict[str, Any]:
    if not snapshots:
        return {"available": False}

    with_derived = [snap for snap in snapshots if isinstance(snap.get("derived"), dict)]
    if not with_derived:
        return {"available": False}

    start = with_derived[0]["derived"]
    end = with_derived[-1]["derived"]
    start_funnel = start.get("funnel", {})
    end_funnel = end.get("funnel", {})
    start_gates = start.get("gate_rejections_total", {})
    end_gates = end.get("gate_rejections_total", {})

    gate_delta: dict[str, int] = {}
    for gate in sorted(set(start_gates.keys()) | set(end_gates.keys())):
        start_v = start_gates.get(gate, 0)
        end_v = end_gates.get(gate, 0)
        if isinstance(start_v, int) and isinstance(end_v, int):
            gate_delta[gate] = max(end_v - start_v, 0)

    funnel_delta: dict[str, int] = {}
    for key in ("signals", "accepted", "fills", "aborts", "rejections"):
        start_v = start_funnel.get(key)
        end_v = end_funnel.get(key)
        if isinstance(start_v, int) and isinstance(end_v, int):
            funnel_delta[key] = max(end_v - start_v, 0)

    return {
        "available": True,
        "window_start_utc": with_derived[0].get("captured_at_utc"),
        "window_end_utc": with_derived[-1].get("captured_at_utc"),
        "snapshot_count": len(with_derived),
        "funnel_start": start_funnel,
        "funnel_end": end_funnel,
        "funnel_delta": funnel_delta,
        "gate_rejections_start": start_gates,
        "gate_rejections_end": end_gates,
        "gate_rejections_delta": gate_delta,
    }


def _persist_window_rollup(out_dir: pathlib.Path, snapshots: list[dict[str, Any]]) -> None:
    rollup = _compute_window_rollup(snapshots)
    (out_dir / "funnel-rollup.json").write_text(json.dumps(rollup, indent=2), encoding="utf-8")


def _coerce_float(value: Any) -> float | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return float(value)
    if isinstance(value, str):
        try:
            return float(value.strip())
        except ValueError:
            return None
    return None


def _parse_snapshot_timestamp_utc(snapshot: dict[str, Any]) -> dt.datetime | None:
    captured = snapshot.get("captured_at_utc")
    if not isinstance(captured, str) or not captured.strip():
        return None
    try:
        normalized = captured.replace("Z", "+00:00")
        parsed = dt.datetime.fromisoformat(normalized)
        return parsed if parsed.tzinfo else parsed.replace(tzinfo=dt.timezone.utc)
    except ValueError:
        return None


def _extract_run_returns(snapshots: list[dict[str, Any]]) -> tuple[list[float], list[float]]:
    nav_points: list[tuple[dt.datetime | None, float]] = []
    for snapshot in snapshots:
        data = snapshot.get("data", {})
        if not isinstance(data, dict):
            continue
        portfolio = data.get("/api/portfolio", {})
        if not isinstance(portfolio, dict):
            continue
        nav = _coerce_float(portfolio.get("total_nav"))
        if nav is None:
            continue
        nav_points.append((_parse_snapshot_timestamp_utc(snapshot), nav))

    returns: list[float] = []
    interval_seconds: list[float] = []
    for idx in range(1, len(nav_points)):
        prev_ts, prev_nav = nav_points[idx - 1]
        curr_ts, curr_nav = nav_points[idx]
        if prev_nav <= 0:
            continue
        returns.append((curr_nav - prev_nav) / prev_nav)
        if prev_ts is not None and curr_ts is not None:
            delta = (curr_ts - prev_ts).total_seconds()
            if delta > 0:
                interval_seconds.append(delta)

    return returns, interval_seconds


def _sample_skewness(returns: list[float], mean: float) -> float:
    n = len(returns)
    if n < 3:
        return 0.0
    m2 = sum((x - mean) ** 2 for x in returns) / n
    if m2 <= 0:
        return 0.0
    m3 = sum((x - mean) ** 3 for x in returns) / n
    return m3 / (m2 ** 1.5)


def _sample_kurtosis(returns: list[float], mean: float) -> float:
    n = len(returns)
    if n < 4:
        return 3.0
    m2 = sum((x - mean) ** 2 for x in returns) / n
    if m2 <= 0:
        return 3.0
    m4 = sum((x - mean) ** 4 for x in returns) / n
    return m4 / (m2**2)


def _compute_psr(
    *,
    sharpe_per_period: float,
    benchmark_sharpe_per_period: float,
    sample_count: int,
    skewness: float,
    kurtosis: float,
) -> float | None:
    if sample_count < 2:
        return None
    denom_term = 1.0 - skewness * sharpe_per_period + ((kurtosis - 1.0) / 4.0) * (sharpe_per_period**2)
    if denom_term <= 0:
        return None
    z_score = ((sharpe_per_period - benchmark_sharpe_per_period) * math.sqrt(sample_count - 1)) / math.sqrt(denom_term)
    return statistics.NormalDist().cdf(z_score)


def _expected_max_sharpe_under_null(sample_count: int, trials: int) -> float:
    if sample_count < 2 or trials <= 1:
        return 0.0
    sharpe_std = 1.0 / math.sqrt(sample_count - 1)
    gamma = 0.5772156649
    normal = statistics.NormalDist()
    p1 = min(max(1.0 - (1.0 / trials), 1e-6), 1.0 - 1e-6)
    p2 = min(max(1.0 - (1.0 / (trials * math.e)), 1e-6), 1.0 - 1e-6)
    z1 = normal.inv_cdf(p1)
    z2 = normal.inv_cdf(p2)
    return sharpe_std * ((1.0 - gamma) * z1 + gamma * z2)


def _compute_probabilistic_metrics(snapshots: list[dict[str, Any]]) -> dict[str, Any]:
    returns, interval_seconds = _extract_run_returns(snapshots)
    if len(returns) < 2:
        return {
            "returns_count": len(returns),
            "psr_probability_sharpe_gt_0": None,
            "dsr_probability": None,
            "dsr_benchmark_sharpe_per_period": None,
            "assumptions": [
                "Requires at least two NAV return observations from /api/portfolio.total_nav to estimate Sharpe dispersion."
            ],
        }

    mean_return = statistics.fmean(returns)
    std_return = statistics.stdev(returns)
    if std_return <= 0:
        return {
            "returns_count": len(returns),
            "psr_probability_sharpe_gt_0": None,
            "dsr_probability": None,
            "dsr_benchmark_sharpe_per_period": None,
            "assumptions": [
                "Return variance is zero; PSR/DSR are undefined when Sharpe denominator collapses."
            ],
        }

    sample_count = len(returns)
    sharpe_per_period = mean_return / std_return
    skewness = _sample_skewness(returns, mean_return)
    kurtosis = _sample_kurtosis(returns, mean_return)

    median_interval_seconds = statistics.median(interval_seconds) if interval_seconds else None
    periods_per_year = (
        (365.25 * 24.0 * 3600.0) / median_interval_seconds
        if median_interval_seconds is not None and median_interval_seconds > 0
        else None
    )
    sharpe_annualized = (
        sharpe_per_period * math.sqrt(periods_per_year)
        if periods_per_year is not None and periods_per_year > 0
        else None
    )

    psr = _compute_psr(
        sharpe_per_period=sharpe_per_period,
        benchmark_sharpe_per_period=0.0,
        sample_count=sample_count,
        skewness=skewness,
        kurtosis=kurtosis,
    )

    dsr_trials_assumed = max(1, int(round(math.sqrt(sample_count))))
    dsr_benchmark = _expected_max_sharpe_under_null(sample_count, dsr_trials_assumed)
    dsr = _compute_psr(
        sharpe_per_period=sharpe_per_period,
        benchmark_sharpe_per_period=dsr_benchmark,
        sample_count=sample_count,
        skewness=skewness,
        kurtosis=kurtosis,
    )

    return {
        "returns_count": sample_count,
        "median_interval_seconds": round(median_interval_seconds, 6) if median_interval_seconds is not None else None,
        "periods_per_year_estimate": round(periods_per_year, 6) if periods_per_year is not None else None,
        "mean_return_per_period": round(mean_return, 10),
        "std_return_per_period": round(std_return, 10),
        "sharpe_per_period": round(sharpe_per_period, 6),
        "sharpe_annualized_estimate": round(sharpe_annualized, 6) if sharpe_annualized is not None else None,
        "skewness": round(skewness, 6),
        "kurtosis": round(kurtosis, 6),
        "psr_probability_sharpe_gt_0": round(psr, 6) if psr is not None else None,
        "dsr_probability": round(dsr, 6) if dsr is not None else None,
        "dsr_benchmark_sharpe_per_period": round(dsr_benchmark, 6),
        "dsr_trials_assumed": dsr_trials_assumed,
        "assumptions": [
            "Returns are simple NAV deltas between consecutive snapshots from /api/portfolio.total_nav.",
            "PSR uses the non-normal adjustment with sample skewness/kurtosis and benchmark Sharpe = 0.",
            "DSR uses a practical trial-count proxy trials=sqrt(number_of_returns) to estimate expected max Sharpe under null.",
            "Annualized Sharpe is an estimate using median snapshot interval; treat as approximate when sampling cadence is irregular.",
        ],
    }


def _pick_nested_map(payload: dict[str, Any], key: str) -> dict[str, Any]:
    direct = payload.get(key)
    if isinstance(direct, dict):
        return direct
    nested = payload.get("metrics")
    if isinstance(nested, dict):
        nested_value = nested.get(key)
        if isinstance(nested_value, dict):
            return nested_value
    return {}


def _pick_nested_scalar(payload: dict[str, Any], keys: tuple[str, ...]) -> float | None:
    for key in keys:
        direct = _coerce_float(payload.get(key))
        if direct is not None:
            return direct
        nested = payload.get("metrics")
        if isinstance(nested, dict):
            nested_value = _coerce_float(nested.get(key))
            if nested_value is not None:
                return nested_value
    return None


def _normalize_weights(raw: dict[str, Any]) -> dict[str, float]:
    values: dict[str, float] = {}
    total = 0.0
    for key, value in raw.items():
        if not isinstance(key, str):
            continue
        amount = _coerce_float(value)
        if amount is None or amount <= 0:
            continue
        values[key] = amount
        total += amount
    if total <= 0:
        return {}
    return {key: value / total for key, value in values.items()}


def _distribution_delta(
    *,
    baseline: dict[str, float],
    live: dict[str, float],
) -> dict[str, Any]:
    keys = sorted(set(baseline.keys()) | set(live.keys()))
    delta_by_key = {key: round(live.get(key, 0.0) - baseline.get(key, 0.0), 6) for key in keys}
    l1_distance = sum(abs(delta_by_key[key]) for key in keys)
    return {
        "baseline": baseline,
        "live": live,
        "delta_by_key": delta_by_key,
        "l1_distance": round(l1_distance, 6),
    }


def _bucketize_notional(samples: list[float]) -> dict[str, float]:
    bins = {
        "0-2": 0.0,
        "2-5": 0.0,
        "5-10": 0.0,
        "10-20": 0.0,
        "20+": 0.0,
    }
    for sample in samples:
        if sample < 2:
            bins["0-2"] += 1.0
        elif sample < 5:
            bins["2-5"] += 1.0
        elif sample < 10:
            bins["5-10"] += 1.0
        elif sample < 20:
            bins["10-20"] += 1.0
        else:
            bins["20+"] += 1.0
    return _normalize_weights(bins)


def _extract_reject_counts(snapshot: dict[str, Any]) -> tuple[dict[str, float], str]:
    data = snapshot.get("data", {})
    if not isinstance(data, dict):
        return {}, "unavailable"
    rejections = data.get("/api/rejections", {})
    if isinstance(rejections, dict):
        counts = rejections.get("counts_by_reason")
        if isinstance(counts, dict):
            normalized_counts = {
                key: float(value)
                for key, value in counts.items()
                if isinstance(key, str) and _coerce_float(value) is not None
            }
            if normalized_counts:
                return normalized_counts, "snapshot:/api/rejections.counts_by_reason"
    derived = snapshot.get("derived", {})
    if isinstance(derived, dict):
        gate_totals = derived.get("gate_rejections_total")
        if isinstance(gate_totals, dict):
            normalized_counts = {
                key: float(value)
                for key, value in gate_totals.items()
                if isinstance(key, str) and _coerce_float(value) is not None
            }
            if normalized_counts:
                return normalized_counts, "snapshot:derived.gate_rejections_total"
    return {}, "unavailable"


def _extract_notional_samples(snapshot: dict[str, Any]) -> tuple[list[float], str]:
    data = snapshot.get("data", {})
    if not isinstance(data, dict):
        return [], "unavailable"
    history = data.get("/api/history", {})
    if isinstance(history, dict):
        trades = history.get("trades")
        if isinstance(trades, list):
            from_history: list[float] = []
            for trade in trades:
                if not isinstance(trade, dict):
                    continue
                for key in ("notional_usdc", "usdc_spent", "size_usdc"):
                    amount = _coerce_float(trade.get(key))
                    if amount is not None and amount > 0:
                        from_history.append(amount)
                        break
            if from_history:
                return from_history, "snapshot:/api/history.trades"
    portfolio = data.get("/api/portfolio", {})
    if isinstance(portfolio, dict):
        open_positions = portfolio.get("open_positions")
        if isinstance(open_positions, list):
            from_positions: list[float] = []
            for position in open_positions:
                if not isinstance(position, dict):
                    continue
                amount = _coerce_float(position.get("usdc_spent"))
                if amount is not None and amount > 0:
                    from_positions.append(amount)
            if from_positions:
                return from_positions, "snapshot:/api/portfolio.open_positions.usdc_spent"
    rejections = data.get("/api/rejections", {})
    if isinstance(rejections, dict):
        events = rejections.get("events")
        if isinstance(events, list):
            from_rejections: list[float] = []
            for event in events:
                if not isinstance(event, dict):
                    continue
                amount = _coerce_float(event.get("signal_size"))
                if amount is not None and amount > 0:
                    from_rejections.append(amount)
            if from_rejections:
                return from_rejections, "snapshot:/api/rejections.events.signal_size"
    return [], "unavailable"


def _extract_category_samples(snapshot: dict[str, Any]) -> tuple[dict[str, float], str]:
    data = snapshot.get("data", {})
    if not isinstance(data, dict):
        return {}, "unavailable"
    portfolio = data.get("/api/portfolio", {})
    if isinstance(portfolio, dict):
        open_positions = portfolio.get("open_positions")
        if isinstance(open_positions, list):
            counts: dict[str, float] = {}
            for position in open_positions:
                if not isinstance(position, dict):
                    continue
                category = position.get("fee_category")
                if isinstance(category, str) and category.strip():
                    key = category.strip().lower()
                    counts[key] = counts.get(key, 0.0) + 1.0
            normalized = _normalize_weights(counts)
            if normalized:
                return normalized, "snapshot:/api/portfolio.open_positions.fee_category"
    attribution = data.get("/api/pnl-attribution", {})
    if isinstance(attribution, dict):
        by_category = attribution.get("by_category")
        if isinstance(by_category, dict):
            absolute_weights: dict[str, float] = {}
            for key, value in by_category.items():
                if not isinstance(key, str):
                    continue
                amount = _coerce_float(value)
                if amount is None:
                    continue
                absolute_weights[key] = abs(amount)
            normalized = _normalize_weights(absolute_weights)
            if normalized:
                return normalized, "snapshot:/api/pnl-attribution.by_category(abs)"
    return {}, "unavailable"


def _category_from_title(title: str) -> str:
    normalized = title.strip().lower()
    if not normalized:
        return "uncategorized"
    geopolitics_tokens = (
        "geopolit",
        "sanction",
        "nato",
        "war ",
        "military",
        "treaty",
        "united nations",
        "diplomacy",
        "ukraine",
        "russia",
        "israel",
        "iran",
        "china",
        "taiwan",
    )
    sports_tokens = (
        "vs ",
        "nba",
        "nfl",
        "mlb",
        "nhl",
        "soccer",
        "tennis",
        "f1 ",
        "premier league",
        "champions league",
        "basketball",
        "baseball",
        "golf",
    )
    politics_tokens = (
        "president",
        "election",
        "congress",
        "trump",
        "biden",
        "harris",
        "poll",
        "vote",
        "senate",
    )
    crypto_tokens = (
        "bitcoin",
        "btc",
        "ethereum",
        "eth ",
        "solana",
        "defi",
        "nft",
        "crypto",
    )
    if any(token in normalized for token in geopolitics_tokens):
        return "geopolitics"
    if any(token in normalized for token in sports_tokens):
        return "sports"
    if any(token in normalized for token in politics_tokens):
        return "politics"
    if any(token in normalized for token in crypto_tokens):
        return "crypto"
    return "other"


def _canonical_category(raw: Any) -> str | None:
    if not isinstance(raw, str):
        return None
    normalized = raw.strip().lower().replace("-", "_").replace(" ", "_")
    alias_map = {
        "sport": "sports",
        "sports": "sports",
        "politic": "politics",
        "politics": "politics",
        "crypto": "crypto",
        "geopolitic": "geopolitics",
        "geopolitics": "geopolitics",
        "other": "other",
        "uncategorized": "uncategorized",
        "unknown": "uncategorized",
    }
    return alias_map.get(normalized)


def _resolve_category(category_hint: Any, market_title_hint: Any) -> str:
    canonical = _canonical_category(category_hint)
    if canonical:
        return canonical
    if isinstance(market_title_hint, str) and market_title_hint.strip():
        return _category_from_title(market_title_hint)
    return "uncategorized"


def _round_or_none(value: float | None, digits: int = 6) -> float | None:
    if value is None:
        return None
    return round(value, digits)


def command_drift_matrix(args: argparse.Namespace) -> int:
    if not args.run_id:
        raise ValueError("--run-id is required for drift-matrix")

    out_dir = run_output_dir(args.output_dir, args.run_id)
    snapshots = _collect_snapshots(out_dir)
    if len(snapshots) < 1:
        raise RuntimeError(f"No snapshot files found in {out_dir}")

    baseline_path = pathlib.Path(args.baseline_json).resolve()
    baseline_payload = read_json_object(baseline_path)

    funnel_rollup_path = out_dir / "funnel-rollup.json"
    rollup = optional_json_object(funnel_rollup_path) or _compute_window_rollup(snapshots)
    end_snapshot = snapshots[-1]
    start_snapshot = snapshots[0]

    live_fill_rate_pct: float | None = None
    fill_source = "unavailable"
    funnel_delta = rollup.get("funnel_delta") if isinstance(rollup, dict) else {}
    if isinstance(funnel_delta, dict):
        fills = _coerce_float(funnel_delta.get("fills"))
        accepted = _coerce_float(funnel_delta.get("accepted"))
        if fills is not None and accepted is not None and accepted > 0:
            live_fill_rate_pct = (fills / accepted) * 100.0
            fill_source = "funnel-rollup:funnel_delta"
    if live_fill_rate_pct is None:
        end_portfolio = end_snapshot.get("data", {}).get("/api/portfolio", {})
        if isinstance(end_portfolio, dict):
            fallback_fill = _coerce_float(end_portfolio.get("fill_rate_pct"))
            if fallback_fill is not None:
                live_fill_rate_pct = fallback_fill
                fill_source = "snapshot:/api/portfolio.fill_rate_pct"
    if live_fill_rate_pct is None:
        live_fill_rate_pct = 0.0

    end_portfolio = end_snapshot.get("data", {}).get("/api/portfolio", {})
    live_slippage_bps = 0.0
    slippage_source = "unavailable"
    if isinstance(end_portfolio, dict):
        slippage_candidate = _coerce_float(end_portfolio.get("avg_slippage_bps"))
        if slippage_candidate is not None:
            live_slippage_bps = slippage_candidate
            slippage_source = "snapshot:/api/portfolio.avg_slippage_bps"

    start_reject_counts, start_reject_source = _extract_reject_counts(start_snapshot)
    end_reject_counts, end_reject_source = _extract_reject_counts(end_snapshot)
    reject_delta_counts: dict[str, float] = {}
    if start_reject_counts and end_reject_counts:
        for key in sorted(set(start_reject_counts.keys()) | set(end_reject_counts.keys())):
            reject_delta_counts[key] = max(end_reject_counts.get(key, 0.0) - start_reject_counts.get(key, 0.0), 0.0)
    elif isinstance(funnel_delta, dict):
        gate_delta = rollup.get("gate_rejections_delta", {})
        if isinstance(gate_delta, dict):
            reject_delta_counts = {
                key: float(value)
                for key, value in gate_delta.items()
                if isinstance(key, str) and _coerce_float(value) is not None
            }
    live_reject_mix = _normalize_weights(reject_delta_counts)

    notional_samples, notional_source = _extract_notional_samples(end_snapshot)
    live_notional_distribution = _bucketize_notional(notional_samples)

    live_category_mix, category_source = _extract_category_samples(end_snapshot)

    baseline_fill_rate_pct = _pick_nested_scalar(baseline_payload, ("fill_rate_pct", "fill_rate", "fill_rate_percent"))
    baseline_slippage_bps = _pick_nested_scalar(baseline_payload, ("avg_slippage_bps", "slippage_bps"))
    baseline_reject_mix = _normalize_weights(
        _pick_nested_map(baseline_payload, "reject_mix")
        or _pick_nested_map(baseline_payload, "reject_mix_distribution")
        or _pick_nested_map(baseline_payload, "reject_mix_share")
    )
    baseline_notional_distribution = _normalize_weights(
        _pick_nested_map(baseline_payload, "notional_distribution")
        or _pick_nested_map(baseline_payload, "notional_distribution_bins")
    )
    baseline_category_mix = _normalize_weights(
        _pick_nested_map(baseline_payload, "category_mix")
        or _pick_nested_map(baseline_payload, "category_distribution")
    )

    result = {
        "generated_at_utc": now_utc_iso(),
        "run_id": args.run_id,
        "artifacts_dir": str(out_dir),
        "baseline_json": str(baseline_path),
        "live": {
            "fill_rate_pct": round(live_fill_rate_pct, 6),
            "avg_slippage_bps": round(live_slippage_bps, 6),
            "reject_mix": live_reject_mix,
            "notional_distribution": live_notional_distribution,
            "category_mix": live_category_mix,
        },
        "baseline": {
            "fill_rate_pct": round(baseline_fill_rate_pct, 6) if baseline_fill_rate_pct is not None else None,
            "avg_slippage_bps": round(baseline_slippage_bps, 6) if baseline_slippage_bps is not None else None,
            "reject_mix": baseline_reject_mix,
            "notional_distribution": baseline_notional_distribution,
            "category_mix": baseline_category_mix,
        },
        "delta": {
            "fill_rate_pct_points": round(live_fill_rate_pct - (baseline_fill_rate_pct or 0.0), 6),
            "avg_slippage_bps": round(live_slippage_bps - (baseline_slippage_bps or 0.0), 6),
            "reject_mix": _distribution_delta(baseline=baseline_reject_mix, live=live_reject_mix),
            "notional_distribution": _distribution_delta(
                baseline=baseline_notional_distribution,
                live=live_notional_distribution,
            ),
            "category_mix": _distribution_delta(baseline=baseline_category_mix, live=live_category_mix),
        },
        "sources": {
            "fill_rate": fill_source,
            "avg_slippage_bps": slippage_source,
            "reject_mix_start": start_reject_source,
            "reject_mix_end": end_reject_source,
            "notional_distribution": notional_source,
            "category_mix": category_source,
        },
    }

    out_file = pathlib.Path(args.out_file).resolve() if args.out_file else (out_dir / "drift-matrix.json")
    out_file.write_text(json.dumps(result, indent=2, sort_keys=True), encoding="utf-8")

    print(f"Wrote {out_file}")
    print(
        "Fill rate Δ: {:+.2f}pp (live {:.2f}% vs baseline {:.2f}%)".format(
            result["delta"]["fill_rate_pct_points"],
            result["live"]["fill_rate_pct"],
            result["baseline"]["fill_rate_pct"] or 0.0,
        )
    )
    print(
        "Slippage Δ: {:+.2f}bps (live {:.2f} vs baseline {:.2f})".format(
            result["delta"]["avg_slippage_bps"],
            result["live"]["avg_slippage_bps"],
            result["baseline"]["avg_slippage_bps"] or 0.0,
        )
    )
    print(
        "Reject mix drift (L1): {:.4f} | Notional drift (L1): {:.4f} | Category drift (L1): {:.4f}".format(
            result["delta"]["reject_mix"]["l1_distance"],
            result["delta"]["notional_distribution"]["l1_distance"],
            result["delta"]["category_mix"]["l1_distance"],
        )
    )
    return 0


def _looks_like_microstructure_reason(reason: str) -> bool:
    normalized = reason.strip().lower()
    if not normalized:
        return False
    keywords = ("imbalance", "depth", "spread", "liquidity", "slippage", "failsafe", "drift")
    return any(token in normalized for token in keywords)


def _extract_orderbook_rows(snapshot: dict[str, Any]) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    data = snapshot.get("data", {})
    if isinstance(data, dict):
        orderbooks_payload = data.get("/api/orderbooks", {})
        if isinstance(orderbooks_payload, dict):
            orderbooks = orderbooks_payload.get("orderbooks")
            if isinstance(orderbooks, list):
                for row in orderbooks:
                    if isinstance(row, dict):
                        rows.append(row)

    fallback_books = snapshot.get("order_books")
    if isinstance(fallback_books, dict):
        for token_id, row in fallback_books.items():
            if not isinstance(token_id, str) or not isinstance(row, dict):
                continue
            fused = dict(row)
            fused["token_id"] = token_id
            rows.append(fused)
    return rows


def _extract_position_context(snapshot: dict[str, Any]) -> dict[str, dict[str, Any]]:
    context: dict[str, dict[str, Any]] = {}
    data = snapshot.get("data", {})
    if not isinstance(data, dict):
        return context
    portfolio = data.get("/api/portfolio", {})
    if not isinstance(portfolio, dict):
        return context
    open_positions = portfolio.get("open_positions")
    if not isinstance(open_positions, list):
        return context

    for position in open_positions:
        if not isinstance(position, dict):
            continue
        token_id = position.get("token_id")
        if not isinstance(token_id, str) or not token_id.strip():
            continue
        context[token_id] = {
            "side": str(position.get("side", "")).strip().upper(),
            "market_title": position.get("market_title") if isinstance(position.get("market_title"), str) else None,
        }
    return context


def _clamp_or_default(value: float | None, default: float) -> float:
    if value is None:
        return _clamp01(default)
    return _clamp01(value)


def _percentile(values: list[float], percentile: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    rank = int(math.ceil((_clamp01(percentile) * len(ordered)))) - 1
    rank = max(0, min(rank, len(ordered) - 1))
    return ordered[rank]


def command_microstructure_imbalance_scorer(args: argparse.Namespace) -> int:
    if not args.run_id:
        raise ValueError("--run-id is required for microstructure-imbalance-scorer")

    out_dir = run_output_dir(args.output_dir, args.run_id)
    ensure_dir(out_dir)
    snapshots_dir = pathlib.Path(args.snapshots_dir).resolve() if args.snapshots_dir else out_dir
    snapshots = _collect_snapshots(snapshots_dir)

    warnings: list[str] = []
    if not snapshots:
        warnings.append(f"no snapshot-*.json files found in {snapshots_dir}; falling back to conservative defaults")

    rollup = _compute_window_rollup(snapshots) if snapshots else {"available": False}
    micro_gate_delta_total = 0.0
    if isinstance(rollup, dict):
        gate_delta = rollup.get("gate_rejections_delta")
        if isinstance(gate_delta, dict):
            micro_gate_delta_total = sum(
                float(value)
                for key, value in gate_delta.items()
                if isinstance(key, str) and _coerce_float(value) is not None and _looks_like_microstructure_reason(key)
            )

    token_stats: dict[str, dict[str, Any]] = {}
    missing_orderbook_tokens: set[str] = set()
    snapshots_with_orderbooks = 0
    total_orderbook_rows = 0

    for snapshot in snapshots:
        rows = _extract_orderbook_rows(snapshot)
        if rows:
            snapshots_with_orderbooks += 1
        total_orderbook_rows += len(rows)
        position_context = _extract_position_context(snapshot)

        seen_tokens_this_snapshot: set[str] = set()
        for row in rows:
            token_id = row.get("token_id")
            if not isinstance(token_id, str) or not token_id.strip():
                continue
            token_id = token_id.strip()
            seen_tokens_this_snapshot.add(token_id)

            token_entry = token_stats.setdefault(
                token_id,
                {
                    "market_title": None,
                    "book_obs": 0,
                    "imbalance_abs_sum": 0.0,
                    "imbalance_abs_max": 0.0,
                    "spread_sum": 0.0,
                    "spread_obs": 0,
                    "spread_max": 0.0,
                    "depth_total_sum": 0.0,
                    "depth_obs": 0,
                    "depth_total_min": None,
                    "position_side_obs": 0,
                    "directional_adverse_obs": 0,
                    "proxy_rejection_events": 0,
                },
            )

            market_title = row.get("market_title")
            if isinstance(market_title, str) and market_title.strip():
                token_entry["market_title"] = market_title.strip()
            elif token_entry.get("market_title") is None:
                pos_meta = position_context.get(token_id, {})
                pos_title = pos_meta.get("market_title")
                if isinstance(pos_title, str) and pos_title.strip():
                    token_entry["market_title"] = pos_title.strip()

            bid_depth = _coerce_float(row.get("bid_depth"))
            ask_depth = _coerce_float(row.get("ask_depth"))
            spread_bps = _coerce_float(row.get("spread_bps"))
            imbalance = _coerce_float(row.get("imbalance"))
            best_bid = _coerce_float(row.get("best_bid"))
            best_ask = _coerce_float(row.get("best_ask"))

            if spread_bps is None and best_bid is not None and best_ask is not None and best_bid > 0 and best_ask >= best_bid:
                spread_bps = ((best_ask - best_bid) / best_bid) * 10_000.0
            if imbalance is None and bid_depth is not None and ask_depth is not None:
                total_depth = bid_depth + ask_depth
                if total_depth > 0:
                    imbalance = (bid_depth - ask_depth) / total_depth

            token_entry["book_obs"] += 1

            if imbalance is not None:
                abs_imbalance = abs(imbalance)
                token_entry["imbalance_abs_sum"] += abs_imbalance
                token_entry["imbalance_abs_max"] = max(token_entry["imbalance_abs_max"], abs_imbalance)

            if spread_bps is not None and spread_bps >= 0:
                token_entry["spread_sum"] += spread_bps
                token_entry["spread_obs"] += 1
                token_entry["spread_max"] = max(token_entry["spread_max"], spread_bps)

            if bid_depth is not None and ask_depth is not None and bid_depth >= 0 and ask_depth >= 0:
                total_depth = bid_depth + ask_depth
                token_entry["depth_total_sum"] += total_depth
                token_entry["depth_obs"] += 1
                current_min = token_entry["depth_total_min"]
                token_entry["depth_total_min"] = total_depth if current_min is None else min(current_min, total_depth)

            position_side = str(position_context.get(token_id, {}).get("side", "")).upper()
            if imbalance is not None and position_side in {"BUY", "YES", "SELL", "NO"}:
                token_entry["position_side_obs"] += 1
                adverse = (position_side in {"BUY", "YES"} and imbalance < 0) or (
                    position_side in {"SELL", "NO"} and imbalance > 0
                )
                if adverse:
                    token_entry["directional_adverse_obs"] += 1

        for token_id, ctx in position_context.items():
            token_entry = token_stats.setdefault(
                token_id,
                {
                    "market_title": None,
                    "book_obs": 0,
                    "imbalance_abs_sum": 0.0,
                    "imbalance_abs_max": 0.0,
                    "spread_sum": 0.0,
                    "spread_obs": 0,
                    "spread_max": 0.0,
                    "depth_total_sum": 0.0,
                    "depth_obs": 0,
                    "depth_total_min": None,
                    "position_side_obs": 0,
                    "directional_adverse_obs": 0,
                    "proxy_rejection_events": 0,
                },
            )
            if token_entry.get("market_title") is None:
                pos_title = ctx.get("market_title")
                if isinstance(pos_title, str) and pos_title.strip():
                    token_entry["market_title"] = pos_title.strip()
            if token_id not in seen_tokens_this_snapshot:
                missing_orderbook_tokens.add(token_id)

        data = snapshot.get("data", {})
        if isinstance(data, dict):
            rejections = data.get("/api/rejections", {})
            if isinstance(rejections, dict):
                events = rejections.get("events")
                if isinstance(events, list):
                    for event in events:
                        if not isinstance(event, dict):
                            continue
                        reason = event.get("reason")
                        token_id = event.get("token_id")
                        if not isinstance(reason, str) or not _looks_like_microstructure_reason(reason):
                            continue
                        if isinstance(token_id, str) and token_id.strip():
                            token_entry = token_stats.setdefault(
                                token_id.strip(),
                                {
                                    "market_title": None,
                                    "book_obs": 0,
                                    "imbalance_abs_sum": 0.0,
                                    "imbalance_abs_max": 0.0,
                                    "spread_sum": 0.0,
                                    "spread_obs": 0,
                                    "spread_max": 0.0,
                                    "depth_total_sum": 0.0,
                                    "depth_obs": 0,
                                    "depth_total_min": None,
                                    "position_side_obs": 0,
                                    "directional_adverse_obs": 0,
                                    "proxy_rejection_events": 0,
                                },
                            )
                            token_entry["proxy_rejection_events"] += 1

    token_scores: list[dict[str, Any]] = []
    default_token_score = 0.62 if not snapshots else 0.58

    if not token_stats and snapshots:
        warnings.append(
            "no token-level orderbook/rejection context found; score defaults were applied for global summary only"
        )
    if snapshots and snapshots_with_orderbooks == 0:
        warnings.append(
            "orderbook snapshots unavailable (/api/orderbooks missing in snapshot payloads); using proxy components"
        )

    for token_id in sorted(token_stats.keys()):
        row = token_stats[token_id]
        book_obs = int(row.get("book_obs", 0))
        spread_obs = int(row.get("spread_obs", 0))
        depth_obs = int(row.get("depth_obs", 0))
        position_side_obs = int(row.get("position_side_obs", 0))

        avg_abs_imbalance = (
            (float(row.get("imbalance_abs_sum", 0.0)) / book_obs)
            if book_obs > 0
            else None
        )
        avg_spread_bps = (
            (float(row.get("spread_sum", 0.0)) / spread_obs)
            if spread_obs > 0
            else None
        )
        avg_total_depth = (
            (float(row.get("depth_total_sum", 0.0)) / depth_obs)
            if depth_obs > 0
            else None
        )
        directional_adverse_ratio = (
            (float(row.get("directional_adverse_obs", 0)) / position_side_obs)
            if position_side_obs > 0
            else None
        )
        proxy_events = float(row.get("proxy_rejection_events", 0))

        components_raw = {
            "imbalance_pressure": _clamp_or_default(avg_abs_imbalance, 0.6),
            "spread_stress": _clamp_or_default((avg_spread_bps / 350.0) if avg_spread_bps is not None else None, 0.55),
            "depth_thinness": _clamp_or_default(
                (1.0 - _clamp01((avg_total_depth or 0.0) / 1200.0)) if avg_total_depth is not None else None,
                0.6,
            ),
            "directional_adverse_flow": _clamp_or_default(directional_adverse_ratio, 0.5),
            "proxy_gate_pressure": _clamp01((proxy_events + (micro_gate_delta_total * 0.25)) / 10.0),
        }
        weights = {
            "imbalance_pressure": 0.32,
            "spread_stress": 0.24,
            "depth_thinness": 0.18,
            "directional_adverse_flow": 0.16,
            "proxy_gate_pressure": 0.10,
        }

        score = sum(components_raw[key] * weights[key] for key in sorted(weights.keys()))
        if book_obs == 0:
            score = max(score, default_token_score)
        score = _clamp01(score)

        risk_level = "LOW"
        if score >= 0.75:
            risk_level = "HIGH"
        elif score >= 0.55:
            risk_level = "MEDIUM"

        token_warnings: list[str] = []
        if book_obs == 0:
            token_warnings.append("no direct orderbook observations; score uses conservative proxy defaults")
        if position_side_obs == 0:
            token_warnings.append("no directional side context; adverse-flow component defaulted")

        token_scores.append(
            {
                "token_id": token_id,
                "market_title": row.get("market_title"),
                "risk_score_normalized": round(score, 6),
                "risk_score_percent": round(score * 100.0, 2),
                "risk_level": risk_level,
                "observations": {
                    "orderbook": book_obs,
                    "spread": spread_obs,
                    "depth": depth_obs,
                    "position_side": position_side_obs,
                    "proxy_rejection_events": int(proxy_events),
                },
                "signals": {
                    "avg_abs_imbalance": round(avg_abs_imbalance, 6) if avg_abs_imbalance is not None else None,
                    "max_abs_imbalance": round(float(row.get("imbalance_abs_max", 0.0)), 6),
                    "avg_spread_bps": round(avg_spread_bps, 6) if avg_spread_bps is not None else None,
                    "max_spread_bps": round(float(row.get("spread_max", 0.0)), 6),
                    "avg_total_depth_usdc": round(avg_total_depth, 6) if avg_total_depth is not None else None,
                    "min_total_depth_usdc": round(float(row.get("depth_total_min", 0.0)), 6)
                    if row.get("depth_total_min") is not None
                    else None,
                    "directional_adverse_ratio": round(directional_adverse_ratio, 6)
                    if directional_adverse_ratio is not None
                    else None,
                },
                "components": {
                    key: {
                        "score": round(components_raw[key], 6),
                        "weight": weights[key],
                        "weighted_contribution": round(components_raw[key] * weights[key], 6),
                    }
                    for key in sorted(weights.keys())
                },
                "warnings": token_warnings,
            }
        )

    market_map: dict[str, list[float]] = {}
    for row in token_scores:
        market_key = row.get("market_title") if isinstance(row.get("market_title"), str) and row.get("market_title") else row["token_id"]
        market_map.setdefault(str(market_key), []).append(float(row["risk_score_normalized"]))

    market_scores: list[dict[str, Any]] = []
    for market_key in sorted(market_map.keys()):
        values = market_map[market_key]
        avg_score = statistics.fmean(values)
        max_score = max(values)
        market_level = "LOW"
        if avg_score >= 0.75:
            market_level = "HIGH"
        elif avg_score >= 0.55:
            market_level = "MEDIUM"
        market_scores.append(
            {
                "market_key": market_key,
                "token_count": len(values),
                "avg_risk_score_normalized": round(avg_score, 6),
                "avg_risk_score_percent": round(avg_score * 100.0, 2),
                "max_token_risk_score_normalized": round(max_score, 6),
                "risk_level": market_level,
            }
        )

    token_values = [float(row["risk_score_normalized"]) for row in token_scores]
    summary = {
        "token_count": len(token_scores),
        "market_count": len(market_scores),
        "high_risk_tokens": sum(1 for row in token_scores if row["risk_level"] == "HIGH"),
        "medium_risk_tokens": sum(1 for row in token_scores if row["risk_level"] == "MEDIUM"),
        "book_coverage_ratio": round((snapshots_with_orderbooks / len(snapshots)), 6) if snapshots else 0.0,
        "snapshots_analyzed": len(snapshots),
        "snapshots_with_orderbooks": snapshots_with_orderbooks,
        "orderbook_rows_total": total_orderbook_rows,
        "tokens_missing_orderbooks": sorted(missing_orderbook_tokens),
        "micro_gate_rejections_window_delta": int(round(micro_gate_delta_total)),
        "risk_score_mean": round(statistics.fmean(token_values), 6) if token_values else None,
        "risk_score_median": round(statistics.median(token_values), 6) if token_values else None,
        "risk_score_p95": round(_percentile(token_values, 0.95) or 0.0, 6) if token_values else None,
        "max_risk_score": round(max(token_values), 6) if token_values else None,
    }

    result = {
        "schema_version": "1.0.0",
        "generated_at_utc": now_utc_iso(),
        "run_id": args.run_id,
        "inputs": {
            "snapshots_dir": str(snapshots_dir),
            "snapshot_count": len(snapshots),
            "rollup_available": bool(isinstance(rollup, dict) and rollup.get("available") is True),
        },
        "component_definitions": {
            "imbalance_pressure": "Average absolute book imbalance |(bid_depth-ask_depth)/(bid_depth+ask_depth)|; higher means one-sided pressure.",
            "spread_stress": "Average spread in bps normalized by 350 bps; wider spreads raise execution fragility.",
            "depth_thinness": "Inverse normalized total depth (bid+ask); thinner books imply higher microstructure risk.",
            "directional_adverse_flow": "Share of observations where position side opposes imbalance sign (YES/BUY with sell-heavy or NO/SELL with buy-heavy book).",
            "proxy_gate_pressure": "Microstructure-related rejection pressure from snapshot rejections and gate deltas (imbalance/depth/spread/liquidity/drift/failsafe).",
        },
        "scoring_formula": {
            "weights": {
                "imbalance_pressure": 0.32,
                "spread_stress": 0.24,
                "depth_thinness": 0.18,
                "directional_adverse_flow": 0.16,
                "proxy_gate_pressure": 0.10,
            },
            "risk_level_thresholds": {
                "high_min": 0.75,
                "medium_min": 0.55,
            },
            "fallback_policy": "When orderbook components are missing, conservative defaults are used and warnings are emitted.",
        },
        "token_scores": token_scores,
        "market_scores": market_scores,
        "summary": summary,
        "warnings": warnings,
    }

    out_file = pathlib.Path(args.out_file).resolve() if args.out_file else (out_dir / "microstructure-imbalance.json")
    out_file.write_text(json.dumps(result, indent=2, sort_keys=True), encoding="utf-8")
    print(f"Wrote {out_file}")
    print(
        "Microstructure imbalance: tokens={} markets={} high_risk={} coverage={:.2f}%".format(
            summary["token_count"],
            summary["market_count"],
            summary["high_risk_tokens"],
            summary["book_coverage_ratio"] * 100.0,
        )
    )
    if warnings:
        print(f"Warnings: {len(warnings)} (graceful degradation applied)")
    return 0


def _normalize_position_side(side: Any) -> str:
    normalized = str(side or "").strip().upper()
    if normalized in {"BUY", "YES", "LONG"}:
        return "YES"
    if normalized in {"SELL", "NO", "SHORT"}:
        return "NO"
    return ""


def _normalize_probability_price(value: Any) -> float | None:
    price = _coerce_float(value)
    if price is None or price <= 0:
        return None
    if price > 1.25 and price <= 1_000.0:
        price = price / 1_000.0
    if price <= 0 or price > 1.25:
        return None
    return price


def _extract_open_positions(snapshot: dict[str, Any]) -> list[dict[str, Any]]:
    data = snapshot.get("data", {})
    if not isinstance(data, dict):
        return []
    portfolio = data.get("/api/portfolio", {})
    if not isinstance(portfolio, dict):
        return []
    positions = portfolio.get("open_positions")
    if not isinstance(positions, list):
        return []
    rows: list[dict[str, Any]] = []
    for position in positions:
        if isinstance(position, dict):
            rows.append(position)
    return rows


def _adverse_move_component(snapshots: list[dict[str, Any]]) -> tuple[float, dict[str, Any], list[str]]:
    warnings: list[str] = []
    last_prices: dict[str, float] = {}
    adverse_steps = 0
    total_steps = 0
    adverse_bps_sum = 0.0

    for snapshot in snapshots:
        for position in _extract_open_positions(snapshot):
            token_id = position.get("token_id")
            if not isinstance(token_id, str) or not token_id.strip():
                continue
            side = _normalize_position_side(position.get("side"))
            if not side:
                continue
            current_price = None
            for price_key in ("current_price", "mark_price", "price", "mid_price"):
                current_price = _normalize_probability_price(position.get(price_key))
                if current_price is not None:
                    break
            if current_price is None:
                continue

            token_key = token_id.strip()
            previous = last_prices.get(token_key)
            if previous is not None and previous > 0:
                move = (current_price - previous) / previous
                is_adverse = (side == "YES" and move < 0) or (side == "NO" and move > 0)
                total_steps += 1
                if is_adverse:
                    adverse_steps += 1
                    adverse_bps_sum += abs(move) * 10_000.0
            last_prices[token_key] = current_price

    adverse_step_ratio = (adverse_steps / total_steps) if total_steps > 0 else None
    avg_adverse_bps = (adverse_bps_sum / adverse_steps) if adverse_steps > 0 else None
    if total_steps == 0:
        warnings.append("adverse move proxy lacked sequential position pricing; conservative defaults applied")
    score = _clamp_or_default(
        (
            (0.65 * (adverse_step_ratio or 0.0))
            + (0.35 * _clamp01((avg_adverse_bps or 0.0) / 120.0))
        )
        if adverse_step_ratio is not None
        else None,
        0.62 if not snapshots else 0.58,
    )
    return score, {
        "adverse_step_ratio": round(adverse_step_ratio, 6) if adverse_step_ratio is not None else None,
        "avg_adverse_bps": round(avg_adverse_bps, 6) if avg_adverse_bps is not None else None,
        "adverse_steps": adverse_steps,
        "total_steps": total_steps,
    }, warnings


def _reject_pressure_component(
    *,
    snapshots: list[dict[str, Any]],
    report_payload: dict[str, Any],
    rollup: dict[str, Any],
    rejections_payload: dict[str, Any],
) -> tuple[float, dict[str, Any], list[str]]:
    warnings: list[str] = []
    report_window = report_payload.get("window", {})
    if not isinstance(report_window, dict):
        report_window = {}
    report_funnel = report_window.get("funnel_delta", {})
    if not isinstance(report_funnel, dict):
        report_funnel = {}
    rollup_funnel = rollup.get("funnel_delta", {})
    if not isinstance(rollup_funnel, dict):
        rollup_funnel = {}

    signals = _coerce_float(report_funnel.get("signals"))
    if signals is None:
        signals = _coerce_float(rollup_funnel.get("signals"))
    rejections = _coerce_float(report_funnel.get("rejections"))
    if rejections is None:
        rejections = _coerce_float(rollup_funnel.get("rejections"))
    aborts = _coerce_float(report_funnel.get("aborts"))
    if aborts is None:
        aborts = _coerce_float(rollup_funnel.get("aborts"))
    signals = max(signals or 0.0, 0.0)
    total_rejections = max((rejections or 0.0) + (aborts or 0.0), 0.0)

    counts_by_reason = rejections_payload.get("counts_by_reason")
    events = rejections_payload.get("events")
    micro_rejections = 0.0
    if isinstance(counts_by_reason, dict):
        micro_rejections += sum(
            _coerce_float(value) or 0.0
            for key, value in counts_by_reason.items()
            if isinstance(key, str) and _looks_like_microstructure_reason(key)
        )
    if isinstance(events, list):
        micro_rejections += sum(
            1.0
            for event in events
            if isinstance(event, dict) and isinstance(event.get("reason"), str) and _looks_like_microstructure_reason(event["reason"])
        )
    gate_delta = rollup.get("gate_rejections_delta")
    if isinstance(gate_delta, dict):
        micro_rejections += sum(
            _coerce_float(value) or 0.0
            for key, value in gate_delta.items()
            if isinstance(key, str) and _looks_like_microstructure_reason(key)
        )

    reject_ratio = (total_rejections / signals) if signals > 0 else None
    micro_ratio = (micro_rejections / total_rejections) if total_rejections > 0 else None
    if signals <= 0:
        warnings.append("signal count unavailable for reject-pressure normalization; conservative defaults applied")

    reject_ratio_norm = _clamp_or_default((reject_ratio / 0.35) if reject_ratio is not None else None, 0.55)
    micro_ratio_norm = _clamp_or_default(micro_ratio, 0.55)
    score = _clamp01((0.70 * reject_ratio_norm) + (0.30 * micro_ratio_norm))
    if not snapshots:
        score = max(score, 0.6)

    return score, {
        "signals_window": int(round(signals)) if signals > 0 else None,
        "rejections_plus_aborts_window": int(round(total_rejections)),
        "reject_ratio": round(reject_ratio, 6) if reject_ratio is not None else None,
        "micro_rejection_share": round(micro_ratio, 6) if micro_ratio is not None else None,
        "micro_rejections_proxy_count": int(round(micro_rejections)),
    }, warnings


def _spread_imbalance_component(
    *,
    snapshots: list[dict[str, Any]],
    microstructure_payload: dict[str, Any],
    execution_drag_payload: dict[str, Any],
) -> tuple[float, dict[str, Any], list[str]]:
    warnings: list[str] = []
    micro_summary = microstructure_payload.get("summary")
    if isinstance(micro_summary, dict):
        mean_score = _coerce_float(micro_summary.get("risk_score_mean"))
        max_score = _coerce_float(micro_summary.get("max_risk_score"))
        score = _clamp01((0.65 * _clamp_or_default(mean_score, 0.58)) + (0.35 * _clamp_or_default(max_score, 0.62)))
        return score, {
            "source": "microstructure-imbalance.json",
            "microstructure_risk_score_mean": round(mean_score, 6) if mean_score is not None else None,
            "microstructure_max_risk_score": round(max_score, 6) if max_score is not None else None,
            "orderbook_rows_observed": micro_summary.get("orderbook_rows_total"),
        }, warnings

    spread_values: list[float] = []
    imbalance_values: list[float] = []
    orderbook_rows = 0
    for snapshot in snapshots:
        for row in _extract_orderbook_rows(snapshot):
            orderbook_rows += 1
            spread_bps = _coerce_float(row.get("spread_bps"))
            imbalance = _coerce_float(row.get("imbalance"))
            bid_depth = _coerce_float(row.get("bid_depth"))
            ask_depth = _coerce_float(row.get("ask_depth"))
            best_bid = _coerce_float(row.get("best_bid"))
            best_ask = _coerce_float(row.get("best_ask"))

            if spread_bps is None and best_bid is not None and best_ask is not None and best_bid > 0 and best_ask >= best_bid:
                spread_bps = ((best_ask - best_bid) / best_bid) * 10_000.0
            if imbalance is None and bid_depth is not None and ask_depth is not None and (bid_depth + ask_depth) > 0:
                imbalance = (bid_depth - ask_depth) / (bid_depth + ask_depth)

            if spread_bps is not None and spread_bps >= 0:
                spread_values.append(spread_bps)
            if imbalance is not None:
                imbalance_values.append(abs(imbalance))

    if spread_values or imbalance_values:
        avg_spread_bps = statistics.fmean(spread_values) if spread_values else None
        avg_abs_imbalance = statistics.fmean(imbalance_values) if imbalance_values else None
        score = _clamp01(
            (0.55 * _clamp_or_default((avg_spread_bps / 350.0) if avg_spread_bps is not None else None, 0.55))
            + (0.45 * _clamp_or_default(avg_abs_imbalance, 0.6))
        )
        return score, {
            "source": "snapshots-orderbook-proxy",
            "avg_spread_bps": round(avg_spread_bps, 6) if avg_spread_bps is not None else None,
            "avg_abs_imbalance": round(avg_abs_imbalance, 6) if avg_abs_imbalance is not None else None,
            "orderbook_rows_observed": orderbook_rows,
        }, warnings

    aggregate = execution_drag_payload.get("aggregate")
    if isinstance(aggregate, dict):
        drag_bps = _coerce_float(aggregate.get("total_drag_bps_of_executed_notional"))
        if drag_bps is not None:
            score = _clamp01(drag_bps / 180.0)
            warnings.append("microstructure and orderbook inputs missing; spread/imbalance proxy used execution-drag fallback")
            return score, {
                "source": "execution-drag-attribution.json",
                "total_drag_bps_of_executed_notional": round(drag_bps, 6),
                "orderbook_rows_observed": 0,
            }, warnings

    warnings.append("spread/imbalance inputs unavailable; conservative proxy default applied")
    return 0.6 if not snapshots else 0.56, {
        "source": "fallback-default",
        "avg_spread_bps": None,
        "avg_abs_imbalance": None,
        "orderbook_rows_observed": 0,
    }, warnings


def _execution_drag_component(execution_drag_payload: dict[str, Any], report_payload: dict[str, Any]) -> tuple[float, dict[str, Any], list[str]]:
    warnings: list[str] = []
    aggregate = execution_drag_payload.get("aggregate")
    if isinstance(aggregate, dict):
        drag_bps = _coerce_float(aggregate.get("total_drag_bps_of_executed_notional"))
        drag_pct_nav = _coerce_float(aggregate.get("total_drag_pct_of_start_nav"))
        score = _clamp01((0.70 * _clamp_or_default((drag_bps / 160.0) if drag_bps is not None else None, 0.55)) + (0.30 * _clamp_or_default((drag_pct_nav / 12.0) if drag_pct_nav is not None else None, 0.5)))
        return score, {
            "source": "execution-drag-attribution.json",
            "total_drag_bps_of_executed_notional": round(drag_bps, 6) if drag_bps is not None else None,
            "total_drag_pct_of_start_nav": round(drag_pct_nav, 6) if drag_pct_nav is not None else None,
        }, warnings

    fee_drag_pct = _pick_nested_scalar(report_payload, ("fee_drag_pct",))
    if fee_drag_pct is not None:
        warnings.append("execution-drag artifact missing; fee_drag_pct proxy used")
        return _clamp01(fee_drag_pct / 30.0), {
            "source": "report.json",
            "fee_drag_pct": round(fee_drag_pct, 6),
        }, warnings

    warnings.append("execution-drag proxy unavailable; conservative default applied")
    return 0.5, {"source": "fallback-default"}, warnings


def _toxic_flow_tier(score: float) -> tuple[str, str]:
    if score >= 0.82:
        return "SEVERE", "HOLD"
    if score >= 0.65:
        return "HIGH", "HOLD"
    if score >= 0.45:
        return "ELEVATED", "CONDITIONAL"
    return "LOW", "PASS"


def _guardrail_actions_for_tier(tier: str) -> tuple[list[dict[str, Any]], dict[str, str]]:
    if tier == "SEVERE":
        return (
            [
                {
                    "priority": 1,
                    "action_id": "halt-live-trading",
                    "category": "safety",
                    "description": "Disable live trading immediately and switch to paper-only monitoring.",
                    "automation_ready": True,
                },
                {
                    "priority": 2,
                    "action_id": "run-reconciliation-audit",
                    "category": "diagnostics",
                    "description": "Run emergency reconciliation and review rejection/fill drift before any restart.",
                    "automation_ready": False,
                },
            ],
            {"TRADING_ENABLED": "false", "LIVE_TRADING": "false", "PAPER_TRADING": "true"},
        )
    if tier == "HIGH":
        return (
            [
                {
                    "priority": 1,
                    "action_id": "cut-order-size",
                    "category": "risk",
                    "description": "Reduce max order size and throughput while toxic conditions persist.",
                    "automation_ready": True,
                },
                {
                    "priority": 2,
                    "action_id": "tighten-entry-quality",
                    "category": "execution",
                    "description": "Raise minimum signal notional/edge gates and increase drift cooldown.",
                    "automation_ready": True,
                },
            ],
            {
                "MAX_SINGLE_ORDER_USDC": "4",
                "MAX_ORDERS_PER_SECOND": "1",
                "MIN_SIGNAL_NOTIONAL_USD": "8",
                "DRIFT_ABORT_COOLDOWN_SECS": "45",
            },
        )
    if tier == "ELEVATED":
        return (
            [
                {
                    "priority": 1,
                    "action_id": "soft-deleverage",
                    "category": "risk",
                    "description": "Apply temporary size haircut and lower order burst rate.",
                    "automation_ready": True,
                },
                {
                    "priority": 2,
                    "action_id": "monitor-microstructure",
                    "category": "monitoring",
                    "description": "Increase snapshot cadence and require operator check on next cycle.",
                    "automation_ready": False,
                },
            ],
            {"MAX_SINGLE_ORDER_USDC": "6", "MAX_ORDERS_PER_SECOND": "2", "MIN_SIGNAL_NOTIONAL_USD": "5"},
        )
    return (
        [
            {
                "priority": 1,
                "action_id": "maintain-guardrails",
                "category": "monitoring",
                "description": "No toxic-flow escalation detected; keep baseline guardrails and continue monitoring.",
                "automation_ready": True,
            }
        ],
        {},
    )


def command_toxic_flow_advisor(args: argparse.Namespace) -> int:
    if not args.run_id:
        raise ValueError("--run-id is required for toxic-flow-advisor")

    out_dir = run_output_dir(args.output_dir, args.run_id)
    ensure_dir(out_dir)
    snapshots_dir = pathlib.Path(args.snapshots_dir).resolve() if args.snapshots_dir else out_dir
    snapshots = _collect_snapshots(snapshots_dir)
    rollup = _compute_window_rollup(snapshots) if snapshots else {"available": False}
    warnings: list[str] = []
    if not snapshots:
        warnings.append(f"no snapshot-*.json files found in {snapshots_dir}; toxic-flow scoring is using conservative fallbacks")

    report_path = pathlib.Path(args.report_file).resolve() if args.report_file else (out_dir / "report.json")
    rejections_path = pathlib.Path(args.rejections_file).resolve() if args.rejections_file else (out_dir / "rejections.json")
    microstructure_path = (
        pathlib.Path(args.microstructure_file).resolve() if args.microstructure_file else (out_dir / "microstructure-imbalance.json")
    )
    execution_drag_path = (
        pathlib.Path(args.execution_drag_file).resolve()
        if args.execution_drag_file
        else (out_dir / "execution-drag-attribution.json")
    )

    report_payload = optional_json_object(report_path) or {}
    rejections_payload = optional_json_object(rejections_path) or {}
    microstructure_payload = optional_json_object(microstructure_path) or {}
    execution_drag_payload = optional_json_object(execution_drag_path) or {}

    if not report_payload:
        warnings.append(f"report artifact missing: {report_path}")
    if not rejections_payload:
        warnings.append(f"rejections artifact missing: {rejections_path} (snapshot proxies will be used)")
    if not microstructure_payload:
        warnings.append(f"microstructure artifact missing: {microstructure_path} (orderbook fallback enabled)")
    if not execution_drag_payload:
        warnings.append(f"execution-drag artifact missing: {execution_drag_path}")

    latest_snapshot_rejections: dict[str, Any] = {}
    if snapshots:
        latest_snapshot = snapshots[-1]
        data = latest_snapshot.get("data", {})
        if isinstance(data, dict):
            payload = data.get("/api/rejections", {})
            if isinstance(payload, dict):
                latest_snapshot_rejections = payload
    rejections_merged = dict(latest_snapshot_rejections)
    if isinstance(rejections_payload, dict):
        rejections_merged.update(rejections_payload)

    adverse_score, adverse_signals, adverse_warnings = _adverse_move_component(snapshots)
    reject_score, reject_signals, reject_warnings = _reject_pressure_component(
        snapshots=snapshots,
        report_payload=report_payload,
        rollup=rollup if isinstance(rollup, dict) else {},
        rejections_payload=rejections_merged,
    )
    spread_score, spread_signals, spread_warnings = _spread_imbalance_component(
        snapshots=snapshots,
        microstructure_payload=microstructure_payload,
        execution_drag_payload=execution_drag_payload,
    )
    execution_score, execution_signals, execution_warnings = _execution_drag_component(execution_drag_payload, report_payload)
    warnings.extend(adverse_warnings + reject_warnings + spread_warnings + execution_warnings)

    weights = {
        "adverse_move_proxy": 0.40,
        "reject_pressure": 0.25,
        "spread_imbalance_proxy": 0.25,
        "execution_drag_proxy": 0.10,
    }
    component_scores = {
        "adverse_move_proxy": adverse_score,
        "reject_pressure": reject_score,
        "spread_imbalance_proxy": spread_score,
        "execution_drag_proxy": execution_score,
    }
    toxic_flow_score = _clamp01(sum(component_scores[key] * weights[key] for key in sorted(weights.keys())))
    severity_tier, signoff_status = _toxic_flow_tier(toxic_flow_score)
    actions, env_overrides = _guardrail_actions_for_tier(severity_tier)

    result = {
        "schema_version": "1.0.0",
        "generated_at_utc": now_utc_iso(),
        "run_id": args.run_id,
        "inputs": {
            "snapshots_dir": str(snapshots_dir),
            "snapshot_count": len(snapshots),
            "report_file": {"path": str(report_path), "present": bool(report_payload)},
            "rejections_file": {"path": str(rejections_path), "present": bool(rejections_payload)},
            "microstructure_file": {"path": str(microstructure_path), "present": bool(microstructure_payload)},
            "execution_drag_file": {"path": str(execution_drag_path), "present": bool(execution_drag_payload)},
            "rollup_available": bool(isinstance(rollup, dict) and rollup.get("available") is True),
        },
        "toxic_flow_score_normalized": round(toxic_flow_score, 6),
        "toxic_flow_score_percent": round(toxic_flow_score * 100.0, 2),
        "severity_tier": severity_tier,
        "operator_signoff_status": signoff_status,
        "components": {
            "adverse_move_proxy": {
                "score": round(adverse_score, 6),
                "weight": weights["adverse_move_proxy"],
                "weighted_contribution": round(adverse_score * weights["adverse_move_proxy"], 6),
                "signals": adverse_signals,
            },
            "reject_pressure": {
                "score": round(reject_score, 6),
                "weight": weights["reject_pressure"],
                "weighted_contribution": round(reject_score * weights["reject_pressure"], 6),
                "signals": reject_signals,
            },
            "spread_imbalance_proxy": {
                "score": round(spread_score, 6),
                "weight": weights["spread_imbalance_proxy"],
                "weighted_contribution": round(spread_score * weights["spread_imbalance_proxy"], 6),
                "signals": spread_signals,
            },
            "execution_drag_proxy": {
                "score": round(execution_score, 6),
                "weight": weights["execution_drag_proxy"],
                "weighted_contribution": round(execution_score * weights["execution_drag_proxy"], 6),
                "signals": execution_signals,
            },
        },
        "severity_thresholds": {
            "SEVERE_min": 0.82,
            "HIGH_min": 0.65,
            "ELEVATED_min": 0.45,
            "LOW_max_exclusive": 0.45,
        },
        "guardrail_recommendations": {
            "tier": severity_tier,
            "actions": actions,
            "env_overrides": env_overrides,
            "automation_ready": bool(env_overrides),
        },
        "warnings": sorted(set(warnings)),
    }

    out_file = pathlib.Path(args.out_file).resolve() if args.out_file else (out_dir / "toxic-flow-advisor.json")
    out_file.write_text(json.dumps(result, indent=2, sort_keys=True), encoding="utf-8")
    print(f"Wrote {out_file}")
    print(
        f"Toxic flow score: {result['toxic_flow_score_percent']:.2f}% ({severity_tier}) | signoff={signoff_status} | actions={len(actions)}"
    )
    if warnings:
        print(f"Warnings: {len(warnings)} (graceful degradation applied)")
    return 0


def _read_env_file(path: pathlib.Path) -> dict[str, str]:
    if not path.exists():
        return {}
    env_map: dict[str, str] = {}
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        key = key.strip()
        if not key:
            continue
        value = value.strip().strip('"').strip("'")
        env_map[key] = value
    return env_map


def _extract_history_payload(snapshot: dict[str, Any]) -> tuple[dict[str, Any], str]:
    data = snapshot.get("data", {})
    if not isinstance(data, dict):
        return {}, "unavailable"
    history = data.get("/api/history", {})
    if isinstance(history, dict):
        return history, "snapshot:/api/history"
    return {}, "unavailable"


def _extract_notional_from_trade(trade: dict[str, Any]) -> float | None:
    explicit_notional = _coerce_float(trade.get("notional_usdc"))
    if explicit_notional is not None and explicit_notional > 0:
        return explicit_notional
    shares = _coerce_float(trade.get("shares"))
    entry_price = _coerce_float(trade.get("entry_price"))
    if shares is None or entry_price is None or shares <= 0 or entry_price <= 0:
        return None
    return shares * entry_price


def _estimate_history_notional(history_payload: dict[str, Any]) -> tuple[float, dict[str, Any]]:
    trades = history_payload.get("trades")
    if not isinstance(trades, list):
        return 0.0, {"observed_trades": 0, "total_trades": 0, "scaled": False}
    observed_values: list[float] = []
    for trade in trades:
        if not isinstance(trade, dict):
            continue
        notional = _extract_notional_from_trade(trade)
        if notional is not None and notional > 0:
            observed_values.append(notional)
    observed_total = sum(observed_values)
    observed_count = len(observed_values)
    total_trades = int(_coerce_float(history_payload.get("total")) or observed_count)
    scaled = False
    estimated_total = observed_total
    if observed_count > 0 and total_trades > observed_count:
        estimated_total = observed_total * (total_trades / observed_count)
        scaled = True
    return estimated_total, {
        "observed_trades": observed_count,
        "total_trades": total_trades,
        "scaled": scaled,
    }


def command_execution_drag_attribution(args: argparse.Namespace) -> int:
    if not args.run_id:
        raise ValueError("--run-id is required for execution-drag-attribution")

    out_dir = run_output_dir(args.output_dir, args.run_id)
    snapshots = _collect_snapshots(out_dir)
    if not snapshots:
        raise RuntimeError(f"No snapshot files found in {out_dir}")

    start_snapshot = snapshots[0]
    end_snapshot = snapshots[-1]
    rollup = optional_json_object(out_dir / "funnel-rollup.json") or _compute_window_rollup(snapshots)
    report_payload = optional_json_object(out_dir / "report.json")
    fingerprint_payload = optional_json_object(out_dir / "fingerprint.json")

    start_portfolio = start_snapshot.get("data", {}).get("/api/portfolio", {})
    end_portfolio = end_snapshot.get("data", {}).get("/api/portfolio", {})
    if not isinstance(start_portfolio, dict):
        start_portfolio = {}
    if not isinstance(end_portfolio, dict):
        end_portfolio = {}

    start_nav = _coerce_float(start_portfolio.get("nav_usdc")) or _coerce_float(start_portfolio.get("total_nav")) or 0.0
    end_nav = _coerce_float(end_portfolio.get("nav_usdc")) or _coerce_float(end_portfolio.get("total_nav")) or 0.0
    fees_start = _coerce_float(start_portfolio.get("fees_paid_usdc")) or 0.0
    fees_end = _coerce_float(end_portfolio.get("fees_paid_usdc")) or 0.0
    fees_drag_usdc = max(fees_end - fees_start, 0.0)

    funnel_delta = rollup.get("funnel_delta") if isinstance(rollup, dict) else {}
    if not isinstance(funnel_delta, dict):
        funnel_delta = {}
    fills_delta = int(_coerce_float(funnel_delta.get("fills")) or 0)
    rejections_delta = int(_coerce_float(funnel_delta.get("rejections")) or 0)
    aborts_delta = int(_coerce_float(funnel_delta.get("aborts")) or 0)
    accepted_delta = int(_coerce_float(funnel_delta.get("accepted")) or 0)
    signals_delta = int(_coerce_float(funnel_delta.get("signals")) or 0)

    avg_slippage_bps = _coerce_float(end_portfolio.get("avg_slippage_bps")) or 0.0

    history_payload, history_source = _extract_history_payload(end_snapshot)
    executed_notional_usdc, history_diag = _estimate_history_notional(history_payload)
    notional_samples, notional_source = _extract_notional_samples(end_snapshot)
    median_notional = statistics.median(notional_samples) if notional_samples else 0.0
    if executed_notional_usdc <= 0 and fills_delta > 0 and median_notional > 0:
        executed_notional_usdc = fills_delta * median_notional

    env_map: dict[str, str] = {}
    env_path_resolved: str | None = None
    if isinstance(fingerprint_payload, dict):
        env_path_raw = fingerprint_payload.get("env_path")
        if isinstance(env_path_raw, str) and env_path_raw.strip():
            env_path = pathlib.Path(env_path_raw).resolve()
            env_map = _read_env_file(env_path)
            env_path_resolved = str(env_path)

    spread_bps = (
        float(args.spread_bps)
        if args.spread_bps is not None
        else (_coerce_float(env_map.get("PAPER_ADVERSE_FILL_BPS")) or 10.0)
    )
    entry_delay_secs = (
        float(args.entry_delay_secs)
        if args.entry_delay_secs is not None
        else (_coerce_float(env_map.get("ENTRY_DELAY_SECS")) or 0.0)
    )
    delay_bps_per_second = float(args.delay_bps_per_second)
    rejection_edge_bps = float(args.rejection_edge_bps)

    spread_effective_bps = max(0.0, min(spread_bps, avg_slippage_bps))
    spread_drag_usdc = (executed_notional_usdc * spread_effective_bps) / 10_000.0 if executed_notional_usdc > 0 else 0.0
    residual_slippage_bps = max(avg_slippage_bps - spread_effective_bps, 0.0)
    slippage_drag_usdc = (
        executed_notional_usdc * residual_slippage_bps / 10_000.0 if executed_notional_usdc > 0 else 0.0
    )
    delay_drag_usdc = (
        executed_notional_usdc * (entry_delay_secs * delay_bps_per_second) / 10_000.0
        if executed_notional_usdc > 0 and entry_delay_secs > 0
        else 0.0
    )

    rejections_payload = end_snapshot.get("data", {}).get("/api/rejections", {})
    rejection_notional_est = 0.0
    rejection_count_observed = 0
    rejection_source = "unavailable"
    if isinstance(rejections_payload, dict):
        events = rejections_payload.get("events")
        if isinstance(events, list):
            for event in events:
                if not isinstance(event, dict):
                    continue
                signal_size = _coerce_float(event.get("signal_size"))
                if signal_size is None or signal_size <= 0:
                    continue
                rejection_notional_est += signal_size / 1_000.0
                rejection_count_observed += 1
            rejection_source = "snapshot:/api/rejections.events.signal_size"
        counts_by_reason = rejections_payload.get("counts_by_reason")
        if isinstance(counts_by_reason, dict):
            rejection_count_total = int(
                round(
                    sum(
                        _coerce_float(value) or 0.0
                        for value in counts_by_reason.values()
                    )
                )
            )
            if rejection_count_total > rejection_count_observed and rejection_count_observed > 0:
                rejection_notional_est *= rejection_count_total / rejection_count_observed

    if rejection_notional_est <= 0 and (rejections_delta + aborts_delta) > 0 and median_notional > 0:
        rejection_notional_est = (rejections_delta + aborts_delta) * median_notional
        rejection_source = "funnel_delta_count * median_notional"

    rejection_drag_usdc = max(rejection_notional_est * rejection_edge_bps / 10_000.0, 0.0)

    components_raw = {
        "spread": spread_drag_usdc,
        "slippage": slippage_drag_usdc,
        "fees": fees_drag_usdc,
        "delay": delay_drag_usdc,
        "rejections": rejection_drag_usdc,
    }
    total_drag_usdc = sum(components_raw.values())

    def component_payload(name: str, value: float) -> dict[str, Any]:
        return {
            "drag_usdc": round(value, 6),
            "drag_bps_of_executed_notional": round((value / executed_notional_usdc) * 10_000.0, 6)
            if executed_notional_usdc > 0
            else None,
            "share_of_total_drag_pct": round((value / total_drag_usdc) * 100.0, 6) if total_drag_usdc > 0 else 0.0,
            "method": {
                "spread": "min(avg_slippage_bps, PAPER_ADVERSE_FILL_BPS) applied to executed notional",
                "slippage": "residual avg_slippage_bps after removing spread proxy, applied to executed notional",
                "fees": "delta of /api/portfolio.fees_paid_usdc over run window",
                "delay": "ENTRY_DELAY_SECS * delay_bps_per_second on executed notional",
                "rejections": "rejected notional estimate * rejection_edge_bps opportunity cost",
            }[name],
        }

    result = {
        "schema_version": 1,
        "generated_at_utc": now_utc_iso(),
        "run_id": args.run_id,
        "artifacts_dir": str(out_dir),
        "window": {
            "snapshot_count": len(snapshots),
            "window_start_utc": rollup.get("window_start_utc") if isinstance(rollup, dict) else None,
            "window_end_utc": rollup.get("window_end_utc") if isinstance(rollup, dict) else None,
        },
        "funnel": {
            "signals": signals_delta,
            "accepted": accepted_delta,
            "fills": fills_delta,
            "rejections": rejections_delta,
            "aborts": aborts_delta,
        },
        "notional": {
            "executed_notional_usdc_est": round(executed_notional_usdc, 6),
            "rejected_notional_usdc_est": round(rejection_notional_est, 6),
            "history_source": history_source,
            "history_diagnostics": history_diag,
            "fallback_notional_source": notional_source,
            "median_notional_usdc_fallback": round(median_notional, 6),
        },
        "components": {
            "spread": component_payload("spread", spread_drag_usdc),
            "slippage": component_payload("slippage", slippage_drag_usdc),
            "fees": component_payload("fees", fees_drag_usdc),
            "delay": component_payload("delay", delay_drag_usdc),
            "rejections": component_payload("rejections", rejection_drag_usdc),
        },
        "aggregate": {
            "total_drag_usdc": round(total_drag_usdc, 6),
            "total_drag_bps_of_executed_notional": round((total_drag_usdc / executed_notional_usdc) * 10_000.0, 6)
            if executed_notional_usdc > 0
            else None,
            "total_drag_pct_of_start_nav": round((total_drag_usdc / start_nav) * 100.0, 6) if start_nav > 0 else None,
            "drag_per_signal_usdc": round(total_drag_usdc / signals_delta, 6) if signals_delta > 0 else None,
            "drag_per_fill_usdc": round(total_drag_usdc / fills_delta, 6) if fills_delta > 0 else None,
            "run_nav_change_usdc": round(end_nav - start_nav, 6),
            "run_nav_change_pct": round(((end_nav - start_nav) / start_nav) * 100.0, 6) if start_nav > 0 else None,
        },
        "assumptions": {
            "spread_bps_proxy": round(spread_bps, 6),
            "entry_delay_secs": round(entry_delay_secs, 6),
            "delay_bps_per_second": round(delay_bps_per_second, 6),
            "rejection_edge_bps": round(rejection_edge_bps, 6),
            "env_path_used": env_path_resolved,
            "rejection_notional_source": rejection_source,
            "report_present": report_payload is not None,
        },
    }

    out_file = pathlib.Path(args.out_file).resolve() if args.out_file else (out_dir / "execution-drag-attribution.json")
    out_file.write_text(json.dumps(result, indent=2, sort_keys=True), encoding="utf-8")

    print(f"Wrote {out_file}")
    print(
        "Execution drag total: ${:.2f} | {:.2f} bps on executed notional".format(
            result["aggregate"]["total_drag_usdc"],
            result["aggregate"]["total_drag_bps_of_executed_notional"] or 0.0,
        )
    )
    return 0


def command_market_category_heatmap(args: argparse.Namespace) -> int:
    if not args.run_id:
        raise ValueError("--run-id is required for market-category-heatmap")

    out_dir = run_output_dir(args.output_dir, args.run_id)
    snapshots = _collect_snapshots(out_dir)
    if not snapshots:
        raise RuntimeError(f"No snapshot files found in {out_dir}")

    rollup = optional_json_object(out_dir / "funnel-rollup.json") or _compute_window_rollup(snapshots)
    report_payload = optional_json_object(out_dir / "report.json")
    decision_payload = optional_json_object(out_dir / "decision.json")
    confidence_payload = optional_json_object(out_dir / "recommendation-confidence-v2.json")

    end_snapshot = snapshots[-1]
    data = end_snapshot.get("data", {})
    if not isinstance(data, dict):
        data = {}

    category_metrics: dict[str, dict[str, Any]] = {}
    token_to_category: dict[str, str] = {}
    warnings: list[str] = []

    def get_bucket(category: str) -> dict[str, Any]:
        bucket = category_metrics.get(category)
        if bucket is None:
            bucket = {
                "category": category,
                "trades_total": 0,
                "wins": 0,
                "losses": 0,
                "breakeven": 0,
                "realized_pnl_usdc": 0.0,
                "fees_paid_usdc": 0.0,
                "notional_usdc_est": 0.0,
                "notional_samples": 0,
                "reject_events": 0,
                "reject_notional_usdc_est": 0.0,
                "low_confidence_rejections": 0,
                "alpha_signals": 0,
                "alpha_confidence_sum": 0.0,
                "alpha_confidence_samples": 0,
                "alpha_status_counts": {},
                "alpha_realized_pnl_usdc": 0.0,
                "alpha_realized_samples": 0,
                "alpha_realized_wins": 0,
                "alpha_realized_losses": 0,
            }
            category_metrics[category] = bucket
        return bucket

    portfolio = data.get("/api/portfolio", {})
    if isinstance(portfolio, dict):
        open_positions = portfolio.get("open_positions")
        if isinstance(open_positions, list):
            for position in open_positions:
                if not isinstance(position, dict):
                    continue
                token_id = position.get("token_id")
                if not isinstance(token_id, str) or not token_id.strip():
                    continue
                category = _resolve_category(position.get("fee_category"), position.get("market_title"))
                token_to_category[token_id] = category
                get_bucket(category)

    history = data.get("/api/history", {})
    trades_source = "unavailable"
    if isinstance(history, dict):
        trades = history.get("trades")
        if isinstance(trades, list):
            trades_source = "snapshot:/api/history.trades"
            for trade in trades:
                if not isinstance(trade, dict):
                    continue
                category = _resolve_category(trade.get("fee_category"), trade.get("market_title"))
                bucket = get_bucket(category)
                bucket["trades_total"] += 1
                pnl = _coerce_float(trade.get("realized_pnl")) or 0.0
                fees = _coerce_float(trade.get("fees_paid_usdc")) or 0.0
                bucket["realized_pnl_usdc"] += pnl
                bucket["fees_paid_usdc"] += fees
                if pnl > 0:
                    bucket["wins"] += 1
                elif pnl < 0:
                    bucket["losses"] += 1
                else:
                    bucket["breakeven"] += 1

                notional = _extract_notional_from_trade(trade)
                if notional is None or notional <= 0:
                    for key in ("usdc_spent", "size_usdc"):
                        candidate = _coerce_float(trade.get(key))
                        if candidate is not None and candidate > 0:
                            notional = candidate
                            break
                if notional is not None and notional > 0:
                    bucket["notional_usdc_est"] += notional
                    bucket["notional_samples"] += 1

                token_id = trade.get("token_id")
                if isinstance(token_id, str) and token_id.strip():
                    token_to_category[token_id] = category

    rejections = data.get("/api/rejections", {})
    rejections_source = "unavailable"
    if isinstance(rejections, dict):
        events = rejections.get("events")
        if isinstance(events, list):
            rejections_source = "snapshot:/api/rejections.events"
            for event in events:
                if not isinstance(event, dict):
                    continue
                token_id = event.get("token_id")
                mapped_category = token_to_category.get(token_id) if isinstance(token_id, str) else None
                category = _resolve_category(mapped_category, event.get("market_title"))
                bucket = get_bucket(category)
                bucket["reject_events"] += 1
                signal_size = _coerce_float(event.get("signal_size"))
                if signal_size is not None and signal_size > 0:
                    bucket["reject_notional_usdc_est"] += signal_size / 1_000.0
                reason = event.get("reason")
                if isinstance(reason, str) and "confidence" in reason.lower():
                    bucket["low_confidence_rejections"] += 1

    alpha_payload = data.get("/api/alpha", {})
    alpha_source = "unavailable"
    if isinstance(alpha_payload, dict):
        signal_history = alpha_payload.get("signal_history")
        if isinstance(signal_history, list):
            alpha_source = "snapshot:/api/alpha.signal_history"
            for record in signal_history:
                if not isinstance(record, dict):
                    continue
                token_id = record.get("token_id")
                mapped_category = token_to_category.get(token_id) if isinstance(token_id, str) else None
                category = _resolve_category(mapped_category, record.get("market_question"))
                bucket = get_bucket(category)
                bucket["alpha_signals"] += 1
                confidence = _coerce_float(record.get("confidence"))
                if confidence is not None:
                    bucket["alpha_confidence_sum"] += confidence
                    bucket["alpha_confidence_samples"] += 1
                status_raw = record.get("status")
                status = status_raw.strip().lower() if isinstance(status_raw, str) and status_raw.strip() else "unknown"
                counts = bucket["alpha_status_counts"]
                if isinstance(counts, dict):
                    counts[status] = int(counts.get(status, 0)) + 1
                realized = _coerce_float(record.get("realized_pnl"))
                if realized is not None:
                    bucket["alpha_realized_pnl_usdc"] += realized
                    bucket["alpha_realized_samples"] += 1
                    if realized > 0:
                        bucket["alpha_realized_wins"] += 1
                    elif realized < 0:
                        bucket["alpha_realized_losses"] += 1

    categories = list(category_metrics.keys())
    preferred_order = ["sports", "politics", "crypto", "geopolitics", "other", "uncategorized"]
    ordered_categories = [name for name in preferred_order if name in categories]
    ordered_categories.extend(sorted(name for name in categories if name not in preferred_order))

    row_payloads: list[dict[str, Any]] = []
    reject_total_all = sum(int(category_metrics[name].get("reject_events", 0)) for name in ordered_categories)
    for category in ordered_categories:
        bucket = category_metrics[category]
        trades_total = int(bucket.get("trades_total", 0))
        wins = int(bucket.get("wins", 0))
        losses = int(bucket.get("losses", 0))
        fees_paid = float(bucket.get("fees_paid_usdc", 0.0))
        realized_pnl = float(bucket.get("realized_pnl_usdc", 0.0))
        notional_est = float(bucket.get("notional_usdc_est", 0.0))
        reject_events = int(bucket.get("reject_events", 0))
        reject_notional_est = float(bucket.get("reject_notional_usdc_est", 0.0))
        alpha_signals = int(bucket.get("alpha_signals", 0))
        alpha_conf_samples = int(bucket.get("alpha_confidence_samples", 0))
        alpha_conf_sum = float(bucket.get("alpha_confidence_sum", 0.0))
        alpha_status_counts = bucket.get("alpha_status_counts", {})
        if not isinstance(alpha_status_counts, dict):
            alpha_status_counts = {}
        sorted_status_counts = {key: int(alpha_status_counts[key]) for key in sorted(alpha_status_counts.keys())}
        accepted_like = sum(
            count
            for status, count in sorted_status_counts.items()
            if status in {"accepted", "opened", "closed"}
        )
        rejected_like = sum(
            count
            for status, count in sorted_status_counts.items()
            if status.startswith("rejected") or status == "engine_rejected"
        )
        alpha_realized_samples = int(bucket.get("alpha_realized_samples", 0))
        alpha_realized_wins = int(bucket.get("alpha_realized_wins", 0))
        alpha_realized_losses = int(bucket.get("alpha_realized_losses", 0))

        win_rate_pct = (wins / trades_total * 100.0) if trades_total > 0 else None
        gross_return_pct = (realized_pnl / notional_est * 100.0) if notional_est > 0 else None
        net_return_pct = ((realized_pnl - fees_paid) / notional_est * 100.0) if notional_est > 0 else None
        fee_drag_pct_notional = (fees_paid / notional_est * 100.0) if notional_est > 0 else None
        fee_drag_pct_abs_pnl = (fees_paid / abs(realized_pnl) * 100.0) if abs(realized_pnl) > 0 else None
        reject_pressure_pct_run = (reject_events / reject_total_all * 100.0) if reject_total_all > 0 else None
        rejects_per_trade = (reject_events / trades_total) if trades_total > 0 else None
        alpha_avg_confidence = (alpha_conf_sum / alpha_conf_samples) if alpha_conf_samples > 0 else None
        alpha_accept_rate_pct = (accepted_like / alpha_signals * 100.0) if alpha_signals > 0 else None
        alpha_reject_rate_pct = (rejected_like / alpha_signals * 100.0) if alpha_signals > 0 else None
        alpha_realized_win_rate_pct = (
            alpha_realized_wins / (alpha_realized_wins + alpha_realized_losses) * 100.0
            if (alpha_realized_wins + alpha_realized_losses) > 0
            else None
        )

        row_payloads.append(
            {
                "category": category,
                "trade_metrics": {
                    "trades_total": trades_total,
                    "wins": wins,
                    "losses": losses,
                    "win_rate_pct": _round_or_none(win_rate_pct, 4),
                    "realized_pnl_usdc": round(realized_pnl, 6),
                    "notional_usdc_est": round(notional_est, 6),
                    "gross_return_pct": _round_or_none(gross_return_pct, 6),
                    "net_return_after_fees_pct": _round_or_none(net_return_pct, 6),
                },
                "fee_drag_metrics": {
                    "fees_paid_usdc": round(fees_paid, 6),
                    "fee_drag_pct_of_notional": _round_or_none(fee_drag_pct_notional, 6),
                    "fee_drag_pct_of_abs_realized_pnl": _round_or_none(fee_drag_pct_abs_pnl, 6),
                },
                "reject_pressure_metrics": {
                    "reject_events": reject_events,
                    "reject_notional_usdc_est": round(reject_notional_est, 6),
                    "reject_pressure_pct_of_run": _round_or_none(reject_pressure_pct_run, 6),
                    "rejects_per_trade": _round_or_none(rejects_per_trade, 6),
                    "low_confidence_rejections": int(bucket.get("low_confidence_rejections", 0)),
                },
                "confidence_recommendation_metrics": {
                    "alpha_signals": alpha_signals,
                    "alpha_avg_confidence": _round_or_none(alpha_avg_confidence, 6),
                    "alpha_accept_rate_pct": _round_or_none(alpha_accept_rate_pct, 6),
                    "alpha_reject_rate_pct": _round_or_none(alpha_reject_rate_pct, 6),
                    "alpha_realized_pnl_usdc": round(float(bucket.get("alpha_realized_pnl_usdc", 0.0)), 6),
                    "alpha_realized_samples": alpha_realized_samples,
                    "alpha_realized_win_rate_pct": _round_or_none(alpha_realized_win_rate_pct, 6),
                    "alpha_status_counts": sorted_status_counts,
                },
            }
        )

    if not row_payloads:
        warnings.append("No category metrics available from snapshots; emitting empty heatmap payload")

    heatmap_columns = [
        {"key": "net_return_after_fees_pct", "direction": "higher_is_better"},
        {"key": "win_rate_pct", "direction": "higher_is_better"},
        {"key": "fee_drag_pct_of_notional", "direction": "lower_is_better"},
        {"key": "reject_pressure_pct_of_run", "direction": "lower_is_better"},
        {"key": "alpha_avg_confidence", "direction": "higher_is_better"},
        {"key": "alpha_accept_rate_pct", "direction": "higher_is_better"},
    ]
    heatmap_rows: list[dict[str, Any]] = []
    for row in row_payloads:
        trade_metrics = row.get("trade_metrics", {})
        fee_metrics = row.get("fee_drag_metrics", {})
        reject_metrics = row.get("reject_pressure_metrics", {})
        confidence_metrics = row.get("confidence_recommendation_metrics", {})
        heatmap_rows.append(
            {
                "category": row.get("category"),
                "values": {
                    "net_return_after_fees_pct": trade_metrics.get("net_return_after_fees_pct")
                    if isinstance(trade_metrics, dict)
                    else None,
                    "win_rate_pct": trade_metrics.get("win_rate_pct") if isinstance(trade_metrics, dict) else None,
                    "fee_drag_pct_of_notional": fee_metrics.get("fee_drag_pct_of_notional")
                    if isinstance(fee_metrics, dict)
                    else None,
                    "reject_pressure_pct_of_run": reject_metrics.get("reject_pressure_pct_of_run")
                    if isinstance(reject_metrics, dict)
                    else None,
                    "alpha_avg_confidence": confidence_metrics.get("alpha_avg_confidence")
                    if isinstance(confidence_metrics, dict)
                    else None,
                    "alpha_accept_rate_pct": confidence_metrics.get("alpha_accept_rate_pct")
                    if isinstance(confidence_metrics, dict)
                    else None,
                },
            }
        )

    report_return_pct = _pick_nested_scalar(report_payload or {}, ("pnl_return_pct", "return_pct", "net_return_pct", "roi_pct"))
    confidence_score_percent = (
        _coerce_float(confidence_payload.get("confidence_score_percent"))
        if isinstance(confidence_payload, dict)
        else None
    )
    decision_value = decision_payload.get("decision") if isinstance(decision_payload, dict) else None

    result = {
        "schema_version": 1,
        "generated_at_utc": now_utc_iso(),
        "run_id": args.run_id,
        "artifacts_dir": str(out_dir),
        "window": {
            "snapshot_count": len(snapshots),
            "window_start_utc": rollup.get("window_start_utc") if isinstance(rollup, dict) else None,
            "window_end_utc": rollup.get("window_end_utc") if isinstance(rollup, dict) else None,
        },
        "summary": {
            "categories_total": len(row_payloads),
            "trades_total": sum(row["trade_metrics"]["trades_total"] for row in row_payloads),
            "rejections_total": sum(row["reject_pressure_metrics"]["reject_events"] for row in row_payloads),
            "alpha_signals_total": sum(row["confidence_recommendation_metrics"]["alpha_signals"] for row in row_payloads),
            "report_pnl_return_pct": _round_or_none(report_return_pct, 6),
            "recommendation_confidence_score_percent": _round_or_none(confidence_score_percent, 6),
            "decision": decision_value if isinstance(decision_value, str) else None,
        },
        "sources": {
            "trades": trades_source,
            "rejections": rejections_source,
            "alpha": alpha_source,
            "report_present": report_payload is not None,
            "decision_present": decision_payload is not None,
            "recommendation_confidence_present": confidence_payload is not None,
            "category_resolution_order": [
                "explicit_category_fields",
                "market_title_keyword_detection",
                "uncategorized_fallback",
            ],
        },
        "heatmap": {
            "columns": heatmap_columns,
            "rows": heatmap_rows,
        },
        "categories": row_payloads,
        "warnings": warnings,
    }

    out_file = pathlib.Path(args.out_file).resolve() if args.out_file else (out_dir / "market-category-heatmap.json")
    out_file.write_text(json.dumps(result, indent=2, sort_keys=True), encoding="utf-8")
    print(f"Wrote {out_file}")
    print(
        "Market category heatmap: {} categories | trades={} | rejections={} | alpha_signals={}".format(
            result["summary"]["categories_total"],
            result["summary"]["trades_total"],
            result["summary"]["rejections_total"],
            result["summary"]["alpha_signals_total"],
        )
    )
    if warnings:
        print(f"Warnings: {len(warnings)} (missing metadata handled with deterministic fallback)")
    return 0


def _resolve_gate_weight(gate_name: str, weights: dict[str, float]) -> tuple[str, float] | None:
    normalized = gate_name.strip().lower()
    aliases = {
        "kill_switch": ("kill_switch", "trading_enabled", "kill switch"),
        "circuit_breaker": ("circuit_breaker", "circuit breaker", "daily_loss"),
        "daily_loss_limit": ("daily_loss_limit", "max_daily_loss", "daily loss"),
        "var_limit": ("var_limit", "var", "value_at_risk"),
        "rate_limit": ("rate_limit", "max_orders_per_second", "rate"),
        "failsafe_abort": ("failsafe_abort", "drift_abort", "in_play_failsafe"),
    }
    for key, candidates in aliases.items():
        if any(candidate in normalized for candidate in candidates):
            return key, weights.get(key, 0.0)
    return None


def _evaluate_band(
    *,
    value: float | None,
    go_threshold: float,
    tune_threshold: float,
    higher_is_better: bool,
) -> tuple[str, str]:
    if value is None:
        return "MISSING", "Metric unavailable"
    if higher_is_better:
        if value >= go_threshold:
            return "GO", f"{value:.4f} >= go threshold {go_threshold:.4f}"
        if value >= tune_threshold:
            return "TUNE", f"{value:.4f} between tune {tune_threshold:.4f} and go {go_threshold:.4f}"
        return "ROLLBACK", f"{value:.4f} < tune threshold {tune_threshold:.4f}"
    if value <= go_threshold:
        return "GO", f"{value:.4f} <= go threshold {go_threshold:.4f}"
    if value <= tune_threshold:
        return "TUNE", f"{value:.4f} between go {go_threshold:.4f} and tune {tune_threshold:.4f}"
    return "ROLLBACK", f"{value:.4f} > tune threshold {tune_threshold:.4f}"


def _extract_report_decision_metrics(report: dict[str, Any]) -> dict[str, Any]:
    pnl_return_pct = _pick_nested_scalar(report, ("pnl_return_pct", "return_pct", "net_return_pct", "roi_pct"))
    fee_drag_pct = _pick_nested_scalar(report, ("fee_drag_pct", "fee_drag", "fee_drag_percent"))

    risk_components: dict[str, float] = {}
    window = report.get("window", {})
    if isinstance(window, dict):
        funnel = window.get("funnel_delta", {})
        if isinstance(funnel, dict):
            aborts = _coerce_float(funnel.get("aborts"))
            if aborts is not None and aborts > 0:
                risk_components["failsafe_abort"] = aborts
            rejections = _coerce_float(funnel.get("rejections"))
            if rejections is not None and rejections > 0:
                risk_components["total_rejections"] = rejections

    gate_rows = report.get("gate_pressure_top5_run_window")
    if isinstance(gate_rows, list):
        for row in gate_rows:
            if not isinstance(row, dict):
                continue
            gate_name = row.get("gate")
            gate_count = _coerce_float(row.get("rejections_delta"))
            if isinstance(gate_name, str) and gate_count is not None and gate_count > 0:
                risk_components[f"gate:{gate_name}"] = gate_count

    return {
        "pnl_return_pct": pnl_return_pct,
        "fee_drag_pct": fee_drag_pct,
        "risk_components": risk_components,
    }


def _extract_drift_decision_metrics(drift: dict[str, Any], normalizers: dict[str, float], weights: dict[str, float]) -> dict[str, Any]:
    delta = drift.get("delta", {})
    if not isinstance(delta, dict):
        delta = {}

    fill_rate_delta = _coerce_float(delta.get("fill_rate_pct_points")) or 0.0
    slippage_delta = _coerce_float(delta.get("avg_slippage_bps")) or 0.0

    reject_l1 = _coerce_float(_pick_nested_map(delta, "reject_mix").get("l1_distance")) or 0.0
    notional_l1 = _coerce_float(_pick_nested_map(delta, "notional_distribution").get("l1_distance")) or 0.0
    category_l1 = _coerce_float(_pick_nested_map(delta, "category_mix").get("l1_distance")) or 0.0

    penalties = {
        "fill_rate_pp": max(0.0, -fill_rate_delta),
        "slippage_bps": max(0.0, slippage_delta),
        "reject_mix_l1": max(0.0, reject_l1),
        "notional_l1": max(0.0, notional_l1),
        "category_l1": max(0.0, category_l1),
    }

    normalized: dict[str, float] = {}
    weighted_score = 0.0
    total_weight = 0.0
    for key, raw_value in penalties.items():
        norm = normalizers.get(key, 1.0)
        weight = weights.get(key, 0.0)
        if norm <= 0:
            norm = 1.0
        normalized_value = raw_value / norm
        normalized[key] = normalized_value
        if weight > 0:
            weighted_score += normalized_value * weight
            total_weight += weight

    return {
        "penalties": penalties,
        "normalized_penalties": normalized,
        "severity_score": (weighted_score / total_weight) if total_weight > 0 else 0.0,
    }


def _extract_conformal_miscoverage_pct(conformal: dict[str, Any]) -> float | None:
    direct = _pick_nested_scalar(
        conformal,
        (
            "miscoverage_pct",
            "empirical_miscoverage_pct",
            "miscoverage_rate_pct",
            "error_rate_pct",
            "alpha_realized_pct",
        ),
    )
    if direct is not None:
        return direct

    coverage = _pick_nested_scalar(conformal, ("coverage_pct", "empirical_coverage_pct"))
    if coverage is not None:
        return max(0.0, 100.0 - coverage)
    return None


def _compute_threshold_policy_fingerprint(thresholds_payload: dict[str, Any]) -> str:
    normalized = json.loads(json.dumps(thresholds_payload))
    if isinstance(normalized, dict):
        policy = normalized.get("policy")
        if isinstance(policy, dict):
            policy.pop("fingerprint", None)
    return _stable_hash_any(normalized)


def _resolve_threshold_policy_metadata(
    thresholds_payload: dict[str, Any],
    *,
    fallback_schema_version: Any = None,
) -> tuple[dict[str, Any], list[str]]:
    warnings: list[str] = []
    policy = thresholds_payload.get("policy")
    if not isinstance(policy, dict):
        policy = {}

    policy_version = str(policy.get("version", "")).strip()
    schema_version = str(fallback_schema_version if fallback_schema_version is not None else thresholds_payload.get("schema_version", "")).strip()
    if not policy_version:
        policy_version = schema_version or "legacy-unversioned"
        warnings.append("policy.version missing in thresholds schema; falling back to schema_version/legacy-unversioned")

    computed_fingerprint = _compute_threshold_policy_fingerprint(thresholds_payload)
    declared_fingerprint = str(policy.get("fingerprint", "")).strip()
    if declared_fingerprint and declared_fingerprint != computed_fingerprint:
        warnings.append("policy.fingerprint does not match computed canonical hash; decision artifact records computed fingerprint")

    notes = policy.get("notes") if isinstance(policy.get("notes"), str) else None
    changelog = policy.get("changelog")
    if changelog is not None and not isinstance(changelog, (str, list)):
        warnings.append("policy.changelog should be a string or array when present")
        changelog = None

    return (
        {
            "version": policy_version,
            "fingerprint": computed_fingerprint,
            "declared_fingerprint": declared_fingerprint or None,
            "notes": notes,
            "changelog": changelog,
        },
        warnings,
    )


def _validate_threshold_policy_payload(
    thresholds_payload: dict[str, Any],
    *,
    require_stamped: bool,
    strict_policy_version: bool,
) -> tuple[list[str], list[str], dict[str, Any]]:
    errors: list[str] = []
    warnings: list[str] = []

    dimensions = thresholds_payload.get("dimensions")
    if not isinstance(dimensions, dict):
        errors.append("thresholds: top-level 'dimensions' object is required")
        dimensions = {}

    required_dimensions: dict[str, tuple[str, str]] = {
        "pnl_return_pct": ("go_min", "tune_min"),
        "fee_drag_pct": ("go_max", "tune_max"),
        "drift_severity": ("go_max", "tune_max"),
        "risk_events": ("go_max", "tune_max"),
    }
    for dimension, keys in required_dimensions.items():
        payload = dimensions.get(dimension)
        if not isinstance(payload, dict):
            errors.append(f"thresholds: missing required dimension '{dimension}'")
            continue
        for key in keys:
            if _coerce_float(payload.get(key)) is None:
                errors.append(f"thresholds: dimension '{dimension}' missing numeric '{key}'")

    conformal_payload = dimensions.get("conformal_miscoverage_pct")
    if conformal_payload is not None:
        if not isinstance(conformal_payload, dict):
            errors.append("thresholds: optional 'conformal_miscoverage_pct' must be an object when present")
        else:
            for key in ("go_max", "tune_max"):
                if _coerce_float(conformal_payload.get(key)) is None:
                    errors.append(f"thresholds: dimension 'conformal_miscoverage_pct' missing numeric '{key}'")

    metadata, metadata_warnings = _resolve_threshold_policy_metadata(thresholds_payload)
    warnings.extend(metadata_warnings)
    if strict_policy_version and metadata["version"] == "legacy-unversioned":
        errors.append("thresholds: policy.version is required in strict mode")

    if require_stamped:
        declared_fingerprint = metadata.get("declared_fingerprint")
        if not isinstance(declared_fingerprint, str) or not declared_fingerprint:
            errors.append("thresholds: policy.fingerprint is required when --require-stamped is set")
        elif declared_fingerprint != metadata["fingerprint"]:
            errors.append("thresholds: policy.fingerprint does not match computed canonical hash")

    return errors, warnings, metadata


def _clamp01(value: float) -> float:
    return max(0.0, min(1.0, value))


def _score_from_decision_status(status: Any) -> float:
    mapping = {
        "GO": 1.0,
        "TUNE": 0.6,
        "ROLLBACK": 0.1,
        "MISSING": 0.35,
    }
    if isinstance(status, str):
        return mapping.get(status.strip().upper(), 0.35)
    return 0.35


def _extract_conformal_eval_coverage(conformal: dict[str, Any]) -> float | None:
    metrics = conformal.get("metrics")
    if not isinstance(metrics, dict):
        return None
    coverages: list[float] = []
    for metric_payload in metrics.values():
        if not isinstance(metric_payload, dict):
            continue
        bands = metric_payload.get("bands")
        if not isinstance(bands, list):
            continue
        for band in bands:
            if not isinstance(band, dict):
                continue
            coverage_eval = _coerce_float(band.get("coverage_eval"))
            if coverage_eval is None:
                continue
            coverages.append(coverage_eval if coverage_eval <= 1.0 else (coverage_eval / 100.0))
    if not coverages:
        return None
    return min(coverages)


def _extract_conformal_sample_count(conformal: dict[str, Any]) -> int | None:
    metrics = conformal.get("metrics")
    if not isinstance(metrics, dict):
        return None
    sample_counts: list[int] = []
    for metric_payload in metrics.values():
        if not isinstance(metric_payload, dict):
            continue
        count = _coerce_float(metric_payload.get("sample_count"))
        if count is None or count <= 0:
            continue
        sample_counts.append(int(round(count)))
    if not sample_counts:
        return None
    return max(sample_counts)


def _extract_walkforward_regime_stability(walkforward: dict[str, Any]) -> tuple[float | None, dict[str, Any]]:
    aggregate = walkforward.get("aggregate")
    if not isinstance(aggregate, dict):
        return None, {"fold_count": None, "test_count_mean": None, "signal_count": 0}
    metrics_by_fold = aggregate.get("metrics_by_fold")
    fold_count = _coerce_float(aggregate.get("fold_count"))
    test_count_mean = _coerce_float(aggregate.get("test_count_mean"))
    if not isinstance(metrics_by_fold, dict):
        return None, {
            "fold_count": int(round(fold_count)) if fold_count is not None else None,
            "test_count_mean": test_count_mean,
            "signal_count": 0,
        }

    dispersion_scores: list[float] = []
    for key in sorted(metrics_by_fold.keys()):
        payload = metrics_by_fold.get(key)
        if not isinstance(payload, dict):
            continue
        mean_value = _coerce_float(payload.get("mean"))
        min_value = _coerce_float(payload.get("min"))
        max_value = _coerce_float(payload.get("max"))
        if mean_value is None or min_value is None or max_value is None:
            continue
        if any(token in key.lower() for token in ("paired_observations", "train_count", "test_count")):
            continue
        spread = abs(max_value - min_value)
        scale = max(abs(mean_value), 1e-6)
        instability = spread / scale
        dispersion_scores.append(1.0 / (1.0 + instability))

    if not dispersion_scores:
        return None, {
            "fold_count": int(round(fold_count)) if fold_count is not None else None,
            "test_count_mean": test_count_mean,
            "signal_count": 0,
        }

    stability_score = statistics.median(dispersion_scores)
    return _clamp01(stability_score), {
        "fold_count": int(round(fold_count)) if fold_count is not None else None,
        "test_count_mean": test_count_mean,
        "signal_count": len(dispersion_scores),
    }


def command_recommendation_confidence_v2(args: argparse.Namespace) -> int:
    if not args.run_id:
        raise ValueError("--run-id is required for recommendation-confidence-v2")

    out_dir = run_output_dir(args.output_dir, args.run_id)
    ensure_dir(out_dir)

    decision_path = pathlib.Path(args.decision_file).resolve() if args.decision_file else (out_dir / "decision.json")
    report_path = pathlib.Path(args.report_file).resolve() if args.report_file else (out_dir / "report.json")
    drift_path = pathlib.Path(args.drift_file).resolve() if args.drift_file else (out_dir / "drift-matrix.json")
    conformal_path = pathlib.Path(args.conformal_file).resolve() if args.conformal_file else (out_dir / "conformal-summary.json")
    walkforward_path = (
        pathlib.Path(args.walkforward_file).resolve() if args.walkforward_file else (out_dir / "purged-walkforward.json")
    )

    decision_payload = optional_json_object(decision_path)
    report_payload = optional_json_object(report_path)
    drift_payload = optional_json_object(drift_path)
    conformal_payload = optional_json_object(conformal_path)
    walkforward_payload = optional_json_object(walkforward_path)

    warnings: list[str] = []
    for label, path, payload in (
        ("decision", decision_path, decision_payload),
        ("report", report_path, report_payload),
        ("drift-matrix", drift_path, drift_payload),
        ("conformal-summary", conformal_path, conformal_payload),
        ("purged-walkforward", walkforward_path, walkforward_payload),
    ):
        if payload is None:
            warnings.append(f"{label} artifact missing: {path}")

    decision_dimensions = decision_payload.get("dimensions", {}) if isinstance(decision_payload, dict) else {}
    if not isinstance(decision_dimensions, dict):
        decision_dimensions = {}

    pnl_return_pct = _pick_nested_scalar(report_payload or {}, ("pnl_return_pct", "return_pct", "net_return_pct", "roi_pct"))
    risk_adjusted = (report_payload or {}).get("risk_adjusted", {})
    if not isinstance(risk_adjusted, dict):
        risk_adjusted = {}
    psr = _coerce_float(risk_adjusted.get("psr_probability_sharpe_gt_0"))
    dsr = _coerce_float(risk_adjusted.get("dsr_probability"))
    returns_count = _coerce_float(risk_adjusted.get("returns_count"))

    walkforward_regime_stability, walkforward_meta = _extract_walkforward_regime_stability(walkforward_payload or {})
    if walkforward_regime_stability is None:
        walkforward_regime_stability = 0.45
        warnings.append("regime stability unavailable from walkforward artifact; using conservative fallback")

    pnl_score = _clamp01(((pnl_return_pct if pnl_return_pct is not None else -1.0) + 2.0) / 8.0)
    psr_score = _clamp01(psr if psr is not None else 0.45)
    dsr_score = _clamp01(dsr if dsr is not None else 0.45)
    pnl_decision_status = (decision_dimensions.get("pnl_return_pct") or {}).get("status")
    pnl_decision_score = _score_from_decision_status(pnl_decision_status)
    profitability_stability = _clamp01(
        (0.30 * pnl_score)
        + (0.25 * psr_score)
        + (0.20 * dsr_score)
        + (0.15 * pnl_decision_score)
        + (0.10 * walkforward_regime_stability)
    )

    drift_metrics = _extract_drift_decision_metrics(
        drift_payload or {},
        normalizers={
            "fill_rate_pp": 10.0,
            "slippage_bps": 75.0,
            "reject_mix_l1": 0.35,
            "notional_l1": 0.30,
            "category_l1": 0.30,
        },
        weights={
            "fill_rate_pp": 0.25,
            "slippage_bps": 0.20,
            "reject_mix_l1": 0.25,
            "notional_l1": 0.15,
            "category_l1": 0.15,
        },
    )
    drift_severity = _coerce_float(drift_metrics.get("severity_score"))
    drift_delta = (drift_payload or {}).get("delta", {})
    if not isinstance(drift_delta, dict):
        drift_delta = {}
    fill_rate_delta = _coerce_float(drift_delta.get("fill_rate_pct_points")) or 0.0
    slippage_delta = _coerce_float(drift_delta.get("avg_slippage_bps")) or 0.0
    drift_decision_status = (decision_dimensions.get("drift_severity") or {}).get("status")
    drift_decision_score = _score_from_decision_status(drift_decision_status)
    drift_risk = _clamp01(
        (0.45 * (1.0 - _clamp01((drift_severity if drift_severity is not None else 0.8) / 1.2)))
        + (0.20 * (1.0 - _clamp01(max(0.0, -fill_rate_delta) / 8.0)))
        + (0.20 * (1.0 - _clamp01(max(0.0, slippage_delta) / 80.0)))
        + (0.15 * drift_decision_score)
    )

    miscoverage_pct = _extract_conformal_miscoverage_pct(conformal_payload or {})
    coverage_eval_min = _extract_conformal_eval_coverage(conformal_payload or {})
    conformal_decision_status = (decision_dimensions.get("conformal_miscoverage_pct") or {}).get("status")
    coverage_decision_score = _score_from_decision_status(conformal_decision_status)
    coverage_reliability = _clamp01(
        (0.50 * (1.0 - _clamp01((miscoverage_pct if miscoverage_pct is not None else 12.0) / 20.0)))
        + (0.30 * _clamp01(((coverage_eval_min if coverage_eval_min is not None else 0.82) - 0.70) / 0.25))
        + (0.20 * coverage_decision_score)
    )

    conformal_sample_count = _extract_conformal_sample_count(conformal_payload or {})
    walkforward_test_count_mean = _coerce_float(walkforward_meta.get("test_count_mean"))
    walkforward_fold_count = _coerce_float(walkforward_meta.get("fold_count"))
    walkforward_effective_samples = (
        walkforward_test_count_mean * walkforward_fold_count
        if walkforward_test_count_mean is not None and walkforward_fold_count is not None
        else None
    )
    sample_sufficiency = _clamp01(
        (0.40 * _clamp01((returns_count if returns_count is not None else 12.0) / 80.0))
        + (0.35 * _clamp01((conformal_sample_count if conformal_sample_count is not None else 15.0) / 120.0))
        + (0.25 * _clamp01((walkforward_effective_samples if walkforward_effective_samples is not None else 8.0) / 40.0))
    )

    component_weights = {
        "profitability_stability": 0.35,
        "drift_risk": 0.25,
        "coverage_reliability": 0.25,
        "sample_sufficiency": 0.15,
    }
    component_scores = {
        "profitability_stability": profitability_stability,
        "drift_risk": drift_risk,
        "coverage_reliability": coverage_reliability,
        "sample_sufficiency": sample_sufficiency,
    }

    availability_weights = {
        "decision": 0.15,
        "report": 0.35,
        "drift": 0.20,
        "conformal": 0.20,
        "walkforward": 0.10,
    }
    availability_flags = {
        "decision": decision_payload is not None,
        "report": report_payload is not None,
        "drift": drift_payload is not None,
        "conformal": conformal_payload is not None,
        "walkforward": walkforward_payload is not None,
    }
    weighted_presence = sum(
        availability_weights[key] * (1.0 if availability_flags.get(key) else 0.0) for key in sorted(availability_weights.keys())
    )
    artifact_availability_factor = 0.70 + (0.30 * weighted_presence)
    score_before_availability = sum(
        component_weights[key] * component_scores[key] for key in sorted(component_weights.keys())
    )
    confidence_score_normalized = _clamp01(score_before_availability * artifact_availability_factor)

    confidence_level = "LOW"
    operator_guidance = "Do not sign off for promotion; collect more evidence and tune before next cycle."
    if confidence_score_normalized >= 0.75:
        confidence_level = "HIGH"
        operator_guidance = "Eligible for operator signoff review if hard-stop criteria are clear."
    elif confidence_score_normalized >= 0.55:
        confidence_level = "MEDIUM"
        operator_guidance = "Conditional signoff only; tune and re-run if risk dimensions are unstable."

    components_payload: dict[str, Any] = {}
    for key in sorted(component_weights.keys()):
        score_value = component_scores[key]
        components_payload[key] = {
            "weight": round(component_weights[key], 6),
            "score": round(score_value, 6),
            "weighted_contribution": round(component_weights[key] * score_value * artifact_availability_factor, 6),
        }

    result = {
        "schema_version": "2.0.0",
        "generated_at_utc": now_utc_iso(),
        "run_id": args.run_id,
        "confidence_score_normalized": round(confidence_score_normalized, 6),
        "confidence_score_percent": round(confidence_score_normalized * 100.0, 2),
        "confidence_level": confidence_level,
        "operator_guidance": operator_guidance,
        "score_before_availability": round(score_before_availability, 6),
        "artifact_availability_factor": round(artifact_availability_factor, 6),
        "artifacts": {
            "decision": {"path": str(decision_path), "present": decision_payload is not None},
            "report": {"path": str(report_path), "present": report_payload is not None},
            "drift_matrix": {"path": str(drift_path), "present": drift_payload is not None},
            "conformal_summary": {"path": str(conformal_path), "present": conformal_payload is not None},
            "purged_walkforward": {"path": str(walkforward_path), "present": walkforward_payload is not None},
        },
        "components": components_payload,
        "signals": {
            "profitability": {
                "pnl_return_pct": pnl_return_pct,
                "psr_probability_sharpe_gt_0": psr,
                "dsr_probability": dsr,
                "decision_status": pnl_decision_status,
                "walkforward_regime_stability": round(walkforward_regime_stability, 6),
            },
            "drift": {
                "severity_score": drift_severity,
                "fill_rate_pct_points": fill_rate_delta,
                "avg_slippage_bps_delta": slippage_delta,
                "decision_status": drift_decision_status,
            },
            "coverage": {
                "miscoverage_pct": miscoverage_pct,
                "min_eval_coverage": coverage_eval_min,
                "decision_status": conformal_decision_status,
            },
            "sample_sufficiency": {
                "returns_count": int(round(returns_count)) if returns_count is not None else None,
                "conformal_sample_count": conformal_sample_count,
                "walkforward_effective_samples": round(walkforward_effective_samples, 6)
                if walkforward_effective_samples is not None
                else None,
                "walkforward_fold_count": int(round(walkforward_fold_count)) if walkforward_fold_count is not None else None,
                "walkforward_test_count_mean": round(walkforward_test_count_mean, 6)
                if walkforward_test_count_mean is not None
                else None,
            },
        },
        "warnings": warnings,
    }

    out_file = pathlib.Path(args.out_file).resolve() if args.out_file else (out_dir / "recommendation-confidence-v2.json")
    out_file.write_text(json.dumps(result, indent=2, sort_keys=True), encoding="utf-8")
    print(f"Wrote {out_file}")
    print(
        f"Confidence v2: {result['confidence_score_percent']:.2f}% ({result['confidence_level']}) "
        f"| availability_factor={result['artifact_availability_factor']:.3f}"
    )
    if warnings:
        print(f"Warnings: {len(warnings)} (missing artifacts handled with conservative fallback)")
    return 0


def _is_non_empty_string(value: Any) -> bool:
    return isinstance(value, str) and bool(value.strip())


def _validate_required_object_keys(payload: dict[str, Any], required_keys: tuple[str, ...], *, label: str) -> list[str]:
    errors: list[str] = []
    for key in required_keys:
        if key not in payload:
            errors.append(f"{label}: missing key '{key}'")
    return errors


def _validate_fingerprint_payload(payload: dict[str, Any], run_id: str) -> list[str]:
    errors = _validate_required_object_keys(
        payload,
        ("captured_at_utc", "run_id", "base_url", "git_sha", "env_path", "env_sha256"),
        label="fingerprint",
    )
    for key in ("captured_at_utc", "run_id", "base_url", "git_sha", "env_path", "env_sha256"):
        if key in payload and not _is_non_empty_string(payload.get(key)):
            errors.append(f"fingerprint: key '{key}' must be a non-empty string")
    if payload.get("run_id") != run_id:
        errors.append(f"fingerprint: run_id mismatch (expected '{run_id}')")
    return errors


def _validate_snapshot_payload(payload: dict[str, Any], run_id: str, index: int) -> list[str]:
    errors = _validate_required_object_keys(
        payload,
        ("captured_at_utc", "run_id", "base_url", "data", "derived"),
        label=f"snapshot[{index}]",
    )
    if payload.get("run_id") != run_id:
        errors.append(f"snapshot[{index}]: run_id mismatch (expected '{run_id}')")
    if not _is_non_empty_string(payload.get("captured_at_utc")):
        errors.append(f"snapshot[{index}]: captured_at_utc must be a non-empty string")
    if not _is_non_empty_string(payload.get("base_url")):
        errors.append(f"snapshot[{index}]: base_url must be a non-empty string")

    data = payload.get("data")
    if not isinstance(data, dict) or not data:
        errors.append(f"snapshot[{index}]: data must be a non-empty object")
    derived = payload.get("derived")
    if not isinstance(derived, dict):
        errors.append(f"snapshot[{index}]: derived must be an object")
        return errors

    funnel = derived.get("funnel")
    if not isinstance(funnel, dict):
        errors.append(f"snapshot[{index}]: derived.funnel must be an object")
    else:
        for key in ("signals", "accepted", "fills", "aborts", "rejections"):
            value = funnel.get(key)
            if not isinstance(value, int) or value < 0:
                errors.append(f"snapshot[{index}]: derived.funnel.{key} must be a non-negative integer")

    gate_totals = derived.get("gate_rejections_total")
    if not isinstance(gate_totals, dict):
        errors.append(f"snapshot[{index}]: derived.gate_rejections_total must be an object")
    return errors


def _validate_funnel_rollup_payload(payload: dict[str, Any], run_id: str, snapshot_count: int) -> list[str]:
    errors = _validate_required_object_keys(
        payload,
        ("available", "window_start_utc", "window_end_utc", "snapshot_count", "funnel_delta", "gate_rejections_delta"),
        label="funnel-rollup",
    )
    if payload.get("available") is not True:
        errors.append("funnel-rollup: available must be true")
    if not _is_non_empty_string(payload.get("window_start_utc")):
        errors.append("funnel-rollup: window_start_utc must be a non-empty string")
    if not _is_non_empty_string(payload.get("window_end_utc")):
        errors.append("funnel-rollup: window_end_utc must be a non-empty string")
    count = payload.get("snapshot_count")
    if not isinstance(count, int) or count < 1:
        errors.append("funnel-rollup: snapshot_count must be a positive integer")
    elif snapshot_count > 0 and count < snapshot_count:
        errors.append("funnel-rollup: snapshot_count is lower than discovered snapshot files")

    funnel_delta = payload.get("funnel_delta")
    if not isinstance(funnel_delta, dict):
        errors.append("funnel-rollup: funnel_delta must be an object")
    else:
        for key in ("signals", "accepted", "fills", "aborts", "rejections"):
            value = funnel_delta.get(key)
            if value is not None and (not isinstance(value, int) or value < 0):
                errors.append(f"funnel-rollup: funnel_delta.{key} must be a non-negative integer when present")

    if not isinstance(payload.get("gate_rejections_delta"), dict):
        errors.append("funnel-rollup: gate_rejections_delta must be an object")

    if payload.get("run_id") is not None and payload.get("run_id") != run_id:
        errors.append(f"funnel-rollup: run_id mismatch (expected '{run_id}')")
    return errors


def _validate_report_payload(payload: dict[str, Any], run_id: str, snapshot_count: int) -> list[str]:
    errors = _validate_required_object_keys(
        payload,
        (
            "generated_at_utc",
            "run_id",
            "snapshot_count",
            "window",
            "capital",
            "health",
            "gate_pressure_top5_run_window",
        ),
        label="report",
    )
    if payload.get("run_id") != run_id:
        errors.append(f"report: run_id mismatch (expected '{run_id}')")
    if not _is_non_empty_string(payload.get("generated_at_utc")):
        errors.append("report: generated_at_utc must be a non-empty string")

    count = payload.get("snapshot_count")
    if not isinstance(count, int) or count < 1:
        errors.append("report: snapshot_count must be a positive integer")
    elif snapshot_count > 0 and count < snapshot_count:
        errors.append("report: snapshot_count is lower than discovered snapshot files")

    window = payload.get("window")
    if not isinstance(window, dict):
        errors.append("report: window must be an object")
    else:
        if not isinstance(window.get("funnel_delta"), dict):
            errors.append("report: window.funnel_delta must be an object")
        artifacts_dir = window.get("artifacts_dir")
        if artifacts_dir is not None and not _is_non_empty_string(artifacts_dir):
            errors.append("report: window.artifacts_dir must be a non-empty string when present")

    if not isinstance(payload.get("capital"), dict):
        errors.append("report: capital must be an object")
    if not isinstance(payload.get("health"), dict):
        errors.append("report: health must be an object")
    if not isinstance(payload.get("gate_pressure_top5_run_window"), list):
        errors.append("report: gate_pressure_top5_run_window must be an array")
    return errors


def _validate_optional_artifact(payload: dict[str, Any], run_id: str, label: str) -> list[str]:
    errors = _validate_required_object_keys(payload, ("run_id",), label=label)
    if payload.get("run_id") != run_id:
        errors.append(f"{label}: run_id mismatch (expected '{run_id}')")
    return errors


def command_artifact_integrity(args: argparse.Namespace) -> int:
    if not args.run_id:
        raise ValueError("--run-id is required for artifact-integrity")

    out_dir = run_output_dir(args.output_dir, args.run_id)
    ensure_dir(out_dir)

    checks: list[dict[str, Any]] = []

    def add_check(
        *,
        artifact: str,
        path: pathlib.Path,
        required: bool,
        validator: Any,
    ) -> None:
        record: dict[str, Any] = {
            "artifact": artifact,
            "path": str(path),
            "required": required,
            "present": path.exists(),
            "valid": False,
            "errors": [],
        }
        if not path.exists():
            if required:
                record["errors"].append("artifact missing")
            else:
                record["valid"] = True
            checks.append(record)
            return
        try:
            payload = read_json_object(path)
        except Exception as exc:
            record["errors"].append(f"invalid JSON object: {exc}")
            checks.append(record)
            return
        errors = validator(payload)
        record["errors"] = errors
        record["valid"] = len(errors) == 0
        checks.append(record)

    fingerprint_path = out_dir / "fingerprint.json"
    add_check(
        artifact="fingerprint",
        path=fingerprint_path,
        required=True,
        validator=lambda payload: _validate_fingerprint_payload(payload, args.run_id),
    )

    snapshots = sorted(out_dir.glob("snapshot-*.json"))
    snapshot_errors: list[str] = []
    if not snapshots:
        snapshot_errors.append("no snapshot-*.json files found")
    else:
        for idx, snapshot_path in enumerate(snapshots):
            try:
                snapshot_payload = read_json_object(snapshot_path)
            except Exception as exc:
                snapshot_errors.append(f"{snapshot_path.name}: invalid JSON object: {exc}")
                continue
            per_snapshot = _validate_snapshot_payload(snapshot_payload, args.run_id, idx)
            snapshot_errors.extend([f"{snapshot_path.name}: {error}" for error in per_snapshot])
    checks.append(
        {
            "artifact": "snapshots",
            "path": str(out_dir),
            "required": True,
            "present": bool(snapshots),
            "valid": len(snapshot_errors) == 0,
            "errors": snapshot_errors,
            "details": {"snapshot_count": len(snapshots)},
        }
    )

    funnel_rollup_path = out_dir / "funnel-rollup.json"
    add_check(
        artifact="funnel-rollup",
        path=funnel_rollup_path,
        required=True,
        validator=lambda payload: _validate_funnel_rollup_payload(payload, args.run_id, len(snapshots)),
    )

    report_path = out_dir / "report.json"
    add_check(
        artifact="report",
        path=report_path,
        required=True,
        validator=lambda payload: _validate_report_payload(payload, args.run_id, len(snapshots)),
    )

    drift_path = out_dir / "drift-matrix.json"
    add_check(
        artifact="drift-matrix",
        path=drift_path,
        required=args.require_drift_matrix,
        validator=lambda payload: _validate_optional_artifact(payload, args.run_id, "drift-matrix"),
    )

    conformal_path = out_dir / "conformal-summary.json"
    add_check(
        artifact="conformal-summary",
        path=conformal_path,
        required=args.require_conformal_summary,
        validator=lambda payload: _validate_optional_artifact(payload, args.run_id, "conformal-summary"),
    )

    failed_checks = [check for check in checks if not check.get("valid")]
    required_failed = [check for check in failed_checks if check.get("required")]
    overall_status = "PASS" if not required_failed else "FAIL"

    result = {
        "generated_at_utc": now_utc_iso(),
        "run_id": args.run_id,
        "artifacts_dir": str(out_dir),
        "overall_status": overall_status,
        "summary": {
            "checks_total": len(checks),
            "checks_passed": len(checks) - len(failed_checks),
            "checks_failed": len(failed_checks),
            "required_failed": len(required_failed),
        },
        "checks": checks,
    }

    out_file = pathlib.Path(args.out_file).resolve() if args.out_file else (out_dir / "artifact-integrity.json")
    out_file.write_text(json.dumps(result, indent=2, sort_keys=True), encoding="utf-8")

    print(f"Wrote {out_file}")
    print(
        "Artifact integrity: {} ({}/{} checks passed, required_failed={})".format(
            overall_status,
            result["summary"]["checks_passed"],
            result["summary"]["checks_total"],
            result["summary"]["required_failed"],
        )
    )
    for failed in failed_checks:
        artifact = failed.get("artifact", "unknown")
        first_error = failed.get("errors", ["validation failed"])[0]
        print(f"- {artifact}: {first_error}")

    return 0 if overall_status == "PASS" else 1


def command_decision_eval(args: argparse.Namespace) -> int:
    if not args.run_id:
        raise ValueError("--run-id is required for decision-eval")

    out_dir = run_output_dir(args.output_dir, args.run_id)
    ensure_dir(out_dir)

    thresholds_path = pathlib.Path(args.thresholds_json).resolve()
    thresholds = read_json_object(thresholds_path)
    dimensions = thresholds.get("dimensions")
    if not isinstance(dimensions, dict):
        raise ValueError("Threshold schema must include a top-level 'dimensions' object")
    policy_metadata, policy_warnings = _resolve_threshold_policy_metadata(
        thresholds,
        fallback_schema_version=thresholds.get("schema_version"),
    )

    report_path = pathlib.Path(args.report_file).resolve() if args.report_file else (out_dir / "report.json")
    drift_path = pathlib.Path(args.drift_file).resolve() if args.drift_file else (out_dir / "drift-matrix.json")
    conformal_path = pathlib.Path(args.conformal_file).resolve() if args.conformal_file else (out_dir / "conformal-summary.json")

    report_payload = optional_json_object(report_path)
    drift_payload = optional_json_object(drift_path)
    conformal_payload = optional_json_object(conformal_path)

    warnings: list[str] = []
    warnings.extend(policy_warnings)
    if report_payload is None:
        warnings.append(f"report artifact missing: {report_path}")
    if drift_payload is None:
        warnings.append(f"drift artifact missing: {drift_path}")
    if conformal_payload is None:
        warnings.append(f"conformal artifact missing: {conformal_path}")

    decisions: dict[str, Any] = {}

    report_metrics = _extract_report_decision_metrics(report_payload or {})
    pnl_cfg = dimensions.get("pnl_return_pct", {})
    fee_cfg = dimensions.get("fee_drag_pct", {})
    risk_cfg = dimensions.get("risk_events", {})
    drift_cfg = dimensions.get("drift_severity", {})
    conformal_cfg = dimensions.get("conformal_miscoverage_pct", {})

    pnl_value = report_metrics.get("pnl_return_pct")
    pnl_status, pnl_reason = _evaluate_band(
        value=pnl_value if isinstance(pnl_value, float) else None,
        go_threshold=float(pnl_cfg.get("go_min", 0.0)),
        tune_threshold=float(pnl_cfg.get("tune_min", -5.0)),
        higher_is_better=True,
    )
    decisions["pnl_return_pct"] = {"status": pnl_status, "value": pnl_value, "reason": pnl_reason}

    fee_value = report_metrics.get("fee_drag_pct")
    fee_status, fee_reason = _evaluate_band(
        value=fee_value if isinstance(fee_value, float) else None,
        go_threshold=float(fee_cfg.get("go_max", 20.0)),
        tune_threshold=float(fee_cfg.get("tune_max", 35.0)),
        higher_is_better=False,
    )
    decisions["fee_drag_pct"] = {"status": fee_status, "value": fee_value, "reason": fee_reason}

    drift_metrics = _extract_drift_decision_metrics(
        drift_payload or {},
        normalizers={
            key: float(value)
            for key, value in ((drift_cfg.get("normalizers") or {}).items() if isinstance(drift_cfg.get("normalizers"), dict) else [])
            if _coerce_float(value) is not None
        },
        weights={
            key: float(value)
            for key, value in ((drift_cfg.get("weights") or {}).items() if isinstance(drift_cfg.get("weights"), dict) else [])
            if _coerce_float(value) is not None
        },
    )
    drift_value = drift_metrics.get("severity_score")
    drift_status, drift_reason = _evaluate_band(
        value=drift_value if isinstance(drift_value, float) else None,
        go_threshold=float(drift_cfg.get("go_max", 0.25)),
        tune_threshold=float(drift_cfg.get("tune_max", 0.5)),
        higher_is_better=False,
    )
    decisions["drift_severity"] = {
        "status": drift_status,
        "value": drift_value,
        "reason": drift_reason,
        "penalties": drift_metrics.get("penalties"),
    }

    risk_weights = {
        key: float(value)
        for key, value in ((risk_cfg.get("weights") or {}).items() if isinstance(risk_cfg.get("weights"), dict) else [])
        if _coerce_float(value) is not None
    }
    risk_components = report_metrics.get("risk_components", {})
    weighted_risk = 0.0
    risk_breakdown: dict[str, float] = {}
    if isinstance(risk_components, dict):
        for key, raw in risk_components.items():
            if not isinstance(key, str):
                continue
            amount = _coerce_float(raw)
            if amount is None or amount <= 0:
                continue
            if key.startswith("gate:"):
                resolved = _resolve_gate_weight(key[5:], risk_weights)
                if resolved is not None:
                    category, weight = resolved
                    contribution = amount * weight
                    weighted_risk += contribution
                    risk_breakdown[category] = risk_breakdown.get(category, 0.0) + contribution
            else:
                weight = risk_weights.get(key, 1.0 if key == "failsafe_abort" else 0.0)
                contribution = amount * weight
                weighted_risk += contribution
                risk_breakdown[key] = risk_breakdown.get(key, 0.0) + contribution

    risk_status, risk_reason = _evaluate_band(
        value=weighted_risk,
        go_threshold=float(risk_cfg.get("go_max", 2.0)),
        tune_threshold=float(risk_cfg.get("tune_max", 5.0)),
        higher_is_better=False,
    )
    decisions["risk_events"] = {
        "status": risk_status,
        "value": weighted_risk,
        "reason": risk_reason,
        "weighted_components": risk_breakdown,
    }

    conformal_value = _extract_conformal_miscoverage_pct(conformal_payload or {})
    conformal_status, conformal_reason = _evaluate_band(
        value=conformal_value,
        go_threshold=float(conformal_cfg.get("go_max", 5.0)),
        tune_threshold=float(conformal_cfg.get("tune_max", 10.0)),
        higher_is_better=False,
    )
    decisions["conformal_miscoverage_pct"] = {
        "status": conformal_status,
        "value": conformal_value,
        "reason": conformal_reason,
    }

    statuses = [row.get("status") for row in decisions.values() if isinstance(row, dict)]
    final_decision = "TUNE"
    if "ROLLBACK" in statuses:
        final_decision = "ROLLBACK"
    elif "TUNE" in statuses:
        final_decision = "TUNE"
    elif statuses and all(status == "GO" for status in statuses):
        final_decision = "GO"

    if all(status == "MISSING" for status in statuses):
        final_decision = "TUNE"
        warnings.append("all decision dimensions are missing; defaulting to TUNE")

    result = {
        "generated_at_utc": now_utc_iso(),
        "run_id": args.run_id,
        "policy_version": policy_metadata["version"],
        "policy_fingerprint": policy_metadata["fingerprint"],
        "thresholds_schema": {
            "path": str(thresholds_path),
            "schema_version": thresholds.get("schema_version"),
            "decision_levels": thresholds.get("decision_levels"),
            "policy": {
                "version": policy_metadata["version"],
                "fingerprint": policy_metadata["fingerprint"],
                "declared_fingerprint": policy_metadata.get("declared_fingerprint"),
                "notes": policy_metadata.get("notes"),
                "changelog": policy_metadata.get("changelog"),
            },
        },
        "artifacts": {
            "report": str(report_path),
            "drift_matrix": str(drift_path),
            "conformal": str(conformal_path),
            "report_present": report_payload is not None,
            "drift_present": drift_payload is not None,
            "conformal_present": conformal_payload is not None,
        },
        "decision": final_decision,
        "dimensions": decisions,
        "warnings": warnings,
    }

    out_file = pathlib.Path(args.out_file).resolve() if args.out_file else (out_dir / "decision.json")
    out_file.write_text(json.dumps(result, indent=2, sort_keys=True), encoding="utf-8")
    print(f"Wrote {out_file}")
    print(f"Decision: {final_decision}")
    if warnings:
        print(f"Warnings: {len(warnings)} (missing artifacts handled gracefully)")
    return 0


def command_threshold_policy_validate(args: argparse.Namespace) -> int:
    thresholds_path = pathlib.Path(args.thresholds_json).resolve()
    thresholds = read_json_object(thresholds_path)
    errors, warnings, metadata = _validate_threshold_policy_payload(
        thresholds,
        require_stamped=bool(args.require_stamped),
        strict_policy_version=bool(args.strict_policy_version),
    )

    print(f"Policy file: {thresholds_path}")
    print(f"Policy version: {metadata.get('version')}")
    print(f"Computed fingerprint: {metadata.get('fingerprint')}")
    declared = metadata.get("declared_fingerprint")
    print(f"Declared fingerprint: {declared or 'missing'}")
    print(f"Validation status: {'PASS' if not errors else 'FAIL'}")
    if warnings:
        print(f"Warnings ({len(warnings)}):")
        for warning in warnings:
            print(f"- {warning}")
    if errors:
        print(f"Errors ({len(errors)}):")
        for error in errors:
            print(f"- {error}")
        return 1
    return 0


def command_threshold_policy_stamp(args: argparse.Namespace) -> int:
    thresholds_path = pathlib.Path(args.thresholds_json).resolve()
    thresholds = read_json_object(thresholds_path)
    policy = thresholds.get("policy")
    if not isinstance(policy, dict):
        policy = {}
    else:
        policy = dict(policy)

    if args.policy_version:
        policy["version"] = str(args.policy_version).strip()
    elif not str(policy.get("version", "")).strip():
        schema_version = str(thresholds.get("schema_version", "")).strip()
        policy["version"] = schema_version or "1.0.0"

    if args.notes is not None:
        policy["notes"] = str(args.notes)
    if args.changelog is not None:
        policy["changelog"] = str(args.changelog)

    thresholds["policy"] = policy
    fingerprint = _compute_threshold_policy_fingerprint(thresholds)
    thresholds["policy"]["fingerprint"] = fingerprint

    out_file = pathlib.Path(args.out_file).resolve() if args.out_file else thresholds_path
    ensure_dir(out_file.parent)
    out_file.write_text(json.dumps(thresholds, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    print(f"Wrote {out_file}")
    print(f"Policy version: {thresholds['policy'].get('version')}")
    print(f"Policy fingerprint: {fingerprint}")
    return 0


def command_report(args: argparse.Namespace) -> int:
    if not args.run_id:
        raise ValueError("--run-id is required for report")
    out_dir = run_output_dir(args.output_dir, args.run_id)
    snapshots = _collect_snapshots(out_dir)
    if not snapshots:
        raise RuntimeError(f"No snapshot files found in {out_dir}")

    invested: list[float] = []
    cash: list[float] = []
    open_positions: list[int] = []
    messages_total: list[int] = []
    ws_connected_true = 0
    gate_peak_totals: dict[str, int] = {}

    for snap in snapshots:
        data = snap.get("data", {})
        portfolio = data.get("/api/portfolio", {})
        status = data.get("/api/status", {})
        gates = data.get("/api/gates", {})

        if isinstance(portfolio.get("invested_usdc"), (int, float)):
            invested.append(float(portfolio["invested_usdc"]))
        if isinstance(portfolio.get("cash_usdc"), (int, float)):
            cash.append(float(portfolio["cash_usdc"]))
        if isinstance(portfolio.get("open_positions"), list):
            open_positions.append(len(portfolio["open_positions"]))
        if isinstance(status.get("messages_total"), int):
            messages_total.append(int(status["messages_total"]))
        if status.get("ws_connected") is True:
            ws_connected_true += 1
        if isinstance(gates.get("gates"), list):
            for gate in gates["gates"]:
                gate_name = gate.get("gate")
                gate_count = gate.get("rejections_total")
                if isinstance(gate_name, str) and isinstance(gate_count, int):
                    gate_peak_totals[gate_name] = max(gate_peak_totals.get(gate_name, 0), gate_count)

    rollup = _compute_window_rollup(snapshots)
    probabilistic_metrics = _compute_probabilistic_metrics(snapshots)
    gate_rejections_delta = rollup.get("gate_rejections_delta", {}) if isinstance(rollup, dict) else {}
    top_gates = sorted(gate_rejections_delta.items(), key=lambda x: x[1], reverse=True)[:5]
    if not top_gates:
        top_gates = sorted(gate_peak_totals.items(), key=lambda x: x[1], reverse=True)[:5]

    _persist_window_rollup(out_dir, snapshots)
    report = {
        "generated_at_utc": now_utc_iso(),
        "run_id": args.run_id,
        "snapshot_count": len(snapshots),
        "window": {
            "artifacts_dir": str(out_dir),
            "window_start_utc": rollup.get("window_start_utc"),
            "window_end_utc": rollup.get("window_end_utc"),
            "funnel_delta": rollup.get("funnel_delta", {}),
        },
        "capital": {
            "avg_invested_usdc": round(statistics.fmean(invested), 4) if invested else None,
            "peak_invested_usdc": round(max(invested), 4) if invested else None,
            "avg_cash_usdc": round(statistics.fmean(cash), 4) if cash else None,
            "avg_open_positions": round(statistics.fmean(open_positions), 4) if open_positions else None,
            "peak_open_positions": max(open_positions) if open_positions else None,
        },
        "health": {
            "ws_connected_ratio": round(ws_connected_true / len(snapshots), 4),
            "messages_total_start": messages_total[0] if messages_total else None,
            "messages_total_end": messages_total[-1] if messages_total else None,
        },
        "risk_adjusted": probabilistic_metrics,
        "gate_pressure_top5_run_window": [{"gate": gate, "rejections_delta": count} for gate, count in top_gates],
    }
    out_file = out_dir / "report.json"
    out_file.write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(f"Wrote {out_file}")

    if not args.no_registry_update:
        registry_args = argparse.Namespace(
            run_id=args.run_id,
            output_dir=args.output_dir,
            repo_root=args.repo_root,
            registry_dir=args.registry_dir,
            thresholds_file=args.thresholds_file,
            decision_file=args.decision_file,
            decision_tag=args.decision_tag,
        )
        command_registry_upsert(registry_args)
    return 0


def command_walkforward(args: argparse.Namespace) -> int:
    from purged_walkforward import run_walkforward

    return int(run_walkforward(args))


def command_regime_conditional_recommender(args: argparse.Namespace) -> int:
    from regime_conditional_recommender import run_recommender

    return int(run_recommender(args))


def command_bandit_allocation(args: argparse.Namespace) -> int:
    from contextual_bandit_allocation import run_allocation

    return int(run_allocation(args))


def _normalize_gate_status(passed: bool, *, missing: bool = False) -> str:
    if missing:
        return "MISSING"
    return "PASS" if passed else "FAIL"


def _warnings_include_missing_decision_artifacts(warnings: Any) -> bool:
    if not isinstance(warnings, list):
        return False
    for warning in warnings:
        if not isinstance(warning, str):
            continue
        lowered = warning.lower()
        if "missing" in lowered and any(token in lowered for token in ("report", "drift", "conformal")):
            return True
    return False


def _artifact_timestamp_candidates(payload: dict[str, Any]) -> list[str]:
    candidates: list[str] = []
    for key in ("generated_at_utc", "recorded_at_utc", "captured_at_utc"):
        value = payload.get(key)
        if isinstance(value, str) and value.strip():
            candidates.append(value.strip())
    return candidates


def command_production_readiness_gate(args: argparse.Namespace) -> int:
    if not args.run_id:
        raise ValueError("--run-id is required for production-readiness-gate")

    repo_root = pathlib.Path(args.repo_root).resolve()
    out_dir = run_output_dir(args.output_dir, args.run_id)
    ensure_dir(out_dir)

    decision_path = pathlib.Path(args.decision_file).resolve() if args.decision_file else (out_dir / "decision.json")
    integrity_path = pathlib.Path(args.integrity_file).resolve() if args.integrity_file else (out_dir / "artifact-integrity.json")
    confidence_path = pathlib.Path(args.confidence_file).resolve() if args.confidence_file else (out_dir / "recommendation-confidence-v2.json")
    signoff_path = pathlib.Path(args.signoff_file).resolve() if args.signoff_file else (out_dir / "signoff-packet.json")
    rollback_packet_path = (
        pathlib.Path(args.rollback_packet_file).resolve() if args.rollback_packet_file else (out_dir / "rollback-packet.json")
    )
    anomaly_response_path = (
        pathlib.Path(args.anomaly_response_file).resolve()
        if args.anomaly_response_file
        else (out_dir / "anomaly-response-plan.json")
    )
    thresholds_path = pathlib.Path(args.thresholds_json).resolve()

    artifact_records: list[dict[str, Any]] = []
    payloads: dict[str, dict[str, Any]] = {}

    def load_artifact(*, name: str, path: pathlib.Path, required: bool, expect_json_object: bool) -> None:
        record: dict[str, Any] = {
            "artifact": name,
            "path": str(path),
            "path_relative_to_repo": to_relative_path(path, repo_root),
            "required": required,
            "present": path.exists(),
            "valid": False,
            "sha256": file_sha256(path),
            "errors": [],
        }
        if not path.exists():
            if required:
                record["errors"].append("artifact missing")
            artifact_records.append(record)
            return
        if not expect_json_object:
            record["valid"] = True
            artifact_records.append(record)
            return
        try:
            payload = read_json_object(path)
        except Exception as exc:
            record["errors"].append(f"invalid JSON object: {exc}")
            artifact_records.append(record)
            return
        record["valid"] = True
        payloads[name] = payload
        artifact_records.append(record)

    load_artifact(name="decision", path=decision_path, required=True, expect_json_object=True)
    load_artifact(name="integrity", path=integrity_path, required=True, expect_json_object=True)
    load_artifact(name="confidence", path=confidence_path, required=True, expect_json_object=True)
    load_artifact(name="signoff", path=signoff_path, required=True, expect_json_object=True)
    load_artifact(name="rollback_packet", path=rollback_packet_path, required=True, expect_json_object=True)
    load_artifact(name="anomaly_response", path=anomaly_response_path, required=True, expect_json_object=True)
    load_artifact(name="thresholds", path=thresholds_path, required=True, expect_json_object=True)

    artifact_records.sort(key=lambda item: str(item.get("artifact", "")))
    missing_artifact_diagnostics = [
        {
            "artifact": record["artifact"],
            "path": record["path"],
            "path_relative_to_repo": record.get("path_relative_to_repo"),
            "required": True,
            "present": bool(record.get("present")),
            "valid": bool(record.get("valid")),
            "errors": list(record.get("errors", [])),
        }
        for record in artifact_records
        if bool(record.get("required")) and (not record.get("present") or not record.get("valid"))
    ]

    decision_payload = payloads.get("decision", {})
    integrity_payload = payloads.get("integrity", {})
    confidence_payload = payloads.get("confidence", {})
    signoff_payload = payloads.get("signoff", {})
    rollback_payload = payloads.get("rollback_packet", {})
    anomaly_payload = payloads.get("anomaly_response", {})
    thresholds_payload = payloads.get("thresholds", {})

    def gate_entry(gate_type: str, status: str, reason: str, details: dict[str, Any] | None = None) -> dict[str, Any]:
        return {
            "gate_type": gate_type,
            "status": status,
            "reason": reason,
            "details": details or {},
        }

    decision_status = str(decision_payload.get("decision", "")).strip().upper() if decision_payload else ""
    decision_dimensions = decision_payload.get("dimensions", {}) if isinstance(decision_payload, dict) else {}
    if not isinstance(decision_dimensions, dict):
        decision_dimensions = {}
    rollback_dimensions = sorted(
        key
        for key, value in decision_dimensions.items()
        if isinstance(value, dict) and str(value.get("status", "")).strip().upper() == "ROLLBACK"
    )

    integrity_status = str(integrity_payload.get("overall_status", "")).strip().upper() if integrity_payload else ""
    integrity_required_failed = None
    integrity_summary = integrity_payload.get("summary", {}) if isinstance(integrity_payload, dict) else {}
    if isinstance(integrity_summary, dict):
        integrity_required_failed = _coerce_float(integrity_summary.get("required_failed"))
    integrity_gate_pass = integrity_status == "PASS"

    confidence_floor = float(args.confidence_gate_min)
    confidence_score = _coerce_float(confidence_payload.get("confidence_score_normalized"))
    confidence_level = str(confidence_payload.get("confidence_level", "")).strip().upper() if confidence_payload else ""
    confidence_gate_pass = False
    if confidence_score is not None:
        confidence_gate_pass = confidence_score >= confidence_floor
    elif confidence_level:
        confidence_gate_pass = confidence_level in {"MEDIUM", "HIGH"}

    signoff_promotion_allowed = signoff_payload.get("promotion_allowed") is True if signoff_payload else False
    signoff_pre_hard_stop = bool(((signoff_payload.get("pre_run", {}) or {}).get("hard_stop_triggered")) if signoff_payload else False)
    signoff_post_hard_stop = bool(
        ((signoff_payload.get("post_run", {}) or {}).get("hard_stop_triggered")) if signoff_payload else False
    )
    signoff_gate_statuses = signoff_payload.get("gate_statuses", {}) if isinstance(signoff_payload, dict) else {}
    signoff_failed_gates = sorted(
        key
        for key, value in (signoff_gate_statuses.items() if isinstance(signoff_gate_statuses, dict) else [])
        if isinstance(value, dict) and str(value.get("status", "")).strip().upper() not in {"", "PASS"}
    )
    signoff_gate_pass = signoff_promotion_allowed and not signoff_pre_hard_stop and not signoff_post_hard_stop

    rollback_readiness = rollback_payload.get("readiness", {}) if isinstance(rollback_payload, dict) else {}
    rollback_checks = rollback_readiness.get("checks", {}) if isinstance(rollback_readiness, dict) else {}
    rollback_packet_ready_for_review = (
        bool(rollback_checks.get("packet_ready_for_review")) if isinstance(rollback_checks, dict) else False
    )
    rollback_recommended = bool(rollback_checks.get("rollback_recommended")) if isinstance(rollback_checks, dict) else False
    rollback_trigger_count = (
        len((rollback_payload.get("rollback_context", {}) or {}).get("triggers", []))
        if isinstance((rollback_payload.get("rollback_context", {}) or {}).get("triggers"), list)
        else 0
    )
    rollback_failed_gate_count = (
        len((rollback_payload.get("rollback_context", {}) or {}).get("failed_gates", []))
        if isinstance((rollback_payload.get("rollback_context", {}) or {}).get("failed_gates"), list)
        else 0
    )

    anomaly_summary = anomaly_payload.get("summary", {}) if isinstance(anomaly_payload, dict) else {}
    if not isinstance(anomaly_summary, dict):
        anomaly_summary = {}
    anomaly_severity = str(anomaly_payload.get("severity_tier", "")).strip().upper() if anomaly_payload else ""
    if not anomaly_severity:
        anomaly_severity = str(anomaly_summary.get("highest_severity", "")).strip().upper()
    anomaly_signoff_status = str(anomaly_payload.get("operator_signoff_status", "")).strip().upper() if anomaly_payload else ""
    if not anomaly_signoff_status:
        if anomaly_severity in {"CRITICAL", "HIGH"}:
            anomaly_signoff_status = "HOLD"
        elif anomaly_severity == "MEDIUM":
            anomaly_signoff_status = "CONDITIONAL"
        else:
            anomaly_signoff_status = "PASS"
    anomaly_actions = 0
    if isinstance((anomaly_payload.get("guardrail_recommendations", {}) or {}).get("actions"), list):
        anomaly_actions = len((anomaly_payload.get("guardrail_recommendations", {}) or {}).get("actions", []))
    elif isinstance(anomaly_payload.get("automation_safe_action_list"), list):
        anomaly_actions = len(anomaly_payload.get("automation_safe_action_list", []))

    threshold_policy_errors: list[str] = []
    threshold_policy_warnings: list[str] = []
    threshold_policy_metadata: dict[str, Any] = {}
    policy_checks_pass = False
    if thresholds_payload:
        threshold_policy_errors, threshold_policy_warnings, threshold_policy_metadata = _validate_threshold_policy_payload(
            thresholds_payload,
            require_stamped=bool(args.require_stamped_policy),
            strict_policy_version=bool(args.strict_policy_version),
        )
        decision_policy_fingerprint = str(decision_payload.get("policy_fingerprint", "")).strip() if decision_payload else ""
        expected_fingerprint = str(threshold_policy_metadata.get("fingerprint", "")).strip()
        fingerprint_match = bool(decision_policy_fingerprint) and bool(expected_fingerprint) and decision_policy_fingerprint == expected_fingerprint
        policy_checks_pass = len(threshold_policy_errors) == 0 and fingerprint_match
    else:
        decision_policy_fingerprint = ""
        expected_fingerprint = ""
        fingerprint_match = False

    hard_gates: dict[str, dict[str, Any]] = {}
    soft_gates: dict[str, dict[str, Any]] = {}

    artifact_gate_pass = len(missing_artifact_diagnostics) == 0
    hard_gates["artifact_completeness"] = gate_entry(
        "hard",
        _normalize_gate_status(artifact_gate_pass),
        "all required readiness artifacts are present and valid" if artifact_gate_pass else "required readiness artifacts missing or invalid",
        {"missing_required_artifacts": len(missing_artifact_diagnostics)},
    )
    hard_gates["integrity"] = gate_entry(
        "hard",
        _normalize_gate_status(integrity_gate_pass, missing=not bool(integrity_payload)),
        "artifact-integrity overall_status is PASS" if integrity_gate_pass else "artifact-integrity gate failed or missing",
        {
            "overall_status": integrity_status or "MISSING",
            "required_failed": int(integrity_required_failed) if integrity_required_failed is not None else None,
        },
    )
    hard_gates["signoff"] = gate_entry(
        "hard",
        _normalize_gate_status(signoff_gate_pass, missing=not bool(signoff_payload)),
        "promotion_allowed=true and no signoff hard-stop triggered"
        if signoff_gate_pass
        else "signoff indicates promotion not allowed or hard-stop triggered",
        {
            "promotion_allowed": signoff_promotion_allowed,
            "pre_run_hard_stop": signoff_pre_hard_stop,
            "post_run_hard_stop": signoff_post_hard_stop,
            "failed_signoff_gates": signoff_failed_gates,
        },
    )
    hard_gates["rollback_packet_quality"] = gate_entry(
        "hard",
        _normalize_gate_status(rollback_packet_ready_for_review, missing=not bool(rollback_payload)),
        "rollback packet is present and review-ready" if rollback_packet_ready_for_review else "rollback packet missing or not review-ready",
        {
            "packet_ready_for_review": rollback_packet_ready_for_review,
            "trigger_count": rollback_trigger_count,
            "failed_gate_count": rollback_failed_gate_count,
        },
    )
    hard_gates["policy"] = gate_entry(
        "hard",
        _normalize_gate_status(policy_checks_pass, missing=not bool(thresholds_payload)),
        "threshold policy checks passed and decision policy fingerprint matches thresholds"
        if policy_checks_pass
        else "threshold policy checks failed or decision policy fingerprint mismatch",
        {
            "threshold_policy_errors": threshold_policy_errors,
            "threshold_policy_warnings": threshold_policy_warnings,
            "decision_policy_fingerprint": decision_policy_fingerprint,
            "expected_policy_fingerprint": expected_fingerprint,
            "fingerprint_match": fingerprint_match,
            "policy_version": threshold_policy_metadata.get("version"),
        },
    )
    hard_gates["anomaly_response_hard_stop"] = gate_entry(
        "hard",
        _normalize_gate_status(anomaly_signoff_status != "HOLD", missing=not bool(anomaly_payload)),
        "anomaly response did not request HOLD" if anomaly_signoff_status != "HOLD" else "anomaly response requested HOLD",
        {
            "severity_tier": anomaly_severity or "MISSING",
            "operator_signoff_status": anomaly_signoff_status or "MISSING",
            "recommended_actions_count": anomaly_actions,
        },
    )

    soft_gates["decision_alignment"] = gate_entry(
        "soft",
        _normalize_gate_status(decision_status == "GO", missing=not bool(decision_payload)),
        "decision artifact indicates GO"
        if decision_status == "GO"
        else "decision artifact indicates TUNE/ROLLBACK or is missing",
        {
            "decision": decision_status or "MISSING",
            "rollback_dimensions": rollback_dimensions,
        },
    )
    soft_gates["confidence"] = gate_entry(
        "soft",
        _normalize_gate_status(confidence_gate_pass, missing=not bool(confidence_payload)),
        f"confidence_score_normalized >= {confidence_floor:.2f}" if confidence_gate_pass else "confidence gate below floor or missing",
        {
            "confidence_score_normalized": confidence_score,
            "confidence_level": confidence_level or None,
            "confidence_floor": round(confidence_floor, 6),
        },
    )
    soft_gates["anomaly_response"] = gate_entry(
        "soft",
        _normalize_gate_status(anomaly_signoff_status == "PASS", missing=not bool(anomaly_payload)),
        "anomaly response signoff status is PASS"
        if anomaly_signoff_status == "PASS"
        else "anomaly response indicates CONDITIONAL/HOLD or missing",
        {
            "severity_tier": anomaly_severity or "MISSING",
            "operator_signoff_status": anomaly_signoff_status or "MISSING",
        },
    )
    soft_gates["rollback_signal"] = gate_entry(
        "soft",
        _normalize_gate_status(not rollback_recommended, missing=not bool(rollback_payload)),
        "rollback packet has no active rollback recommendation" if not rollback_recommended else "rollback packet recommends rollback",
        {
            "rollback_recommended": rollback_recommended,
            "trigger_count": rollback_trigger_count,
            "failed_gate_count": rollback_failed_gate_count,
        },
    )

    critical_rollback_reasons: list[str] = []
    if decision_status == "ROLLBACK":
        critical_rollback_reasons.append("decision artifact verdict is ROLLBACK")
    if rollback_recommended:
        critical_rollback_reasons.append("rollback packet recommends rollback")
    if anomaly_signoff_status == "HOLD":
        critical_rollback_reasons.append("anomaly response requires HOLD")

    hard_failures = sorted(key for key, gate in hard_gates.items() if gate.get("status") != "PASS")
    soft_failures = sorted(key for key, gate in soft_gates.items() if gate.get("status") != "PASS")

    if critical_rollback_reasons:
        verdict = "ROLLBACK"
    elif hard_failures or soft_failures:
        verdict = "TUNE"
    else:
        verdict = "GO"

    generated_candidates: list[str] = []
    for payload in payloads.values():
        generated_candidates.extend(_artifact_timestamp_candidates(payload))
    generated_at_utc = args.generated_at_utc.strip() or (sorted(generated_candidates)[-1] if generated_candidates else "unknown")

    packet = {
        "schema_version": "1.0.0",
        "run_id": args.run_id,
        "generated_at_utc": generated_at_utc,
        "deterministic_inputs": {
            "confidence_gate_min": round(confidence_floor, 6),
            "require_stamped_policy": bool(args.require_stamped_policy),
            "strict_policy_version": bool(args.strict_policy_version),
            "repo_root": str(repo_root),
            "output_dir": str(out_dir),
        },
        "artifacts": {
            "decision_file_path": to_relative_path(decision_path, repo_root),
            "integrity_file_path": to_relative_path(integrity_path, repo_root),
            "confidence_file_path": to_relative_path(confidence_path, repo_root),
            "signoff_file_path": to_relative_path(signoff_path, repo_root),
            "rollback_packet_file_path": to_relative_path(rollback_packet_path, repo_root),
            "anomaly_response_file_path": to_relative_path(anomaly_response_path, repo_root),
            "thresholds_json_path": to_relative_path(thresholds_path, repo_root),
        },
        "missing_artifact_diagnostics": missing_artifact_diagnostics,
        "gate_statuses": {
            "hard": hard_gates,
            "soft": soft_gates,
        },
        "readiness_verdict": {
            "status": verdict,
            "critical_rollback_reasons": critical_rollback_reasons,
            "hard_gate_failures": hard_failures,
            "soft_gate_failures": soft_failures,
            "summary": (
                "all hard/soft gates passed"
                if verdict == "GO"
                else ("critical rollback conditions detected" if verdict == "ROLLBACK" else "tuning required before promotion")
            ),
        },
        "policy_checks": {
            "threshold_policy_errors": threshold_policy_errors,
            "threshold_policy_warnings": threshold_policy_warnings,
            "decision_policy_fingerprint": decision_policy_fingerprint,
            "expected_policy_fingerprint": expected_fingerprint,
            "fingerprint_match": fingerprint_match,
            "policy_version": threshold_policy_metadata.get("version"),
        },
        "diagnostics": {
            "decision_status": decision_status or "MISSING",
            "rollback_dimensions": rollback_dimensions,
            "rollback_packet_recommended": rollback_recommended,
            "anomaly_severity_tier": anomaly_severity or "MISSING",
            "anomaly_signoff_status": anomaly_signoff_status or "MISSING",
        },
    }
    packet["deterministic_fingerprint"] = stable_hash(packet)

    out_file = pathlib.Path(args.out_file).resolve() if args.out_file else (out_dir / "production-readiness-gate.json")
    ensure_dir(out_file.parent)
    out_file.write_text(json.dumps(packet, indent=2, sort_keys=True), encoding="utf-8")

    print(f"Wrote {out_file}")
    print(
        "Production readiness verdict={} hard_failures={} soft_failures={} missing_required_artifacts={}".format(
            verdict,
            len(hard_failures),
            len(soft_failures),
            len(missing_artifact_diagnostics),
        )
    )
    if missing_artifact_diagnostics:
        print(f"Missing/invalid required artifacts: {len(missing_artifact_diagnostics)}")

    if bool(args.fail_on_non_go) and verdict != "GO":
        return 1
    return 0


def command_signoff_packet(args: argparse.Namespace) -> int:
    if not args.run_id:
        raise ValueError("--run-id is required for signoff-packet")

    repo_root = pathlib.Path(args.repo_root).resolve()
    out_dir = run_output_dir(args.output_dir, args.run_id)
    ensure_dir(out_dir)

    decision_path = pathlib.Path(args.decision_file).resolve() if args.decision_file else (out_dir / "decision.json")
    recommendation_path = (
        pathlib.Path(args.recommendation_file).resolve()
        if args.recommendation_file
        else (out_dir / "regime-conditional-recommendations.json")
    )
    confidence_path = (
        pathlib.Path(args.confidence_file).resolve()
        if args.confidence_file
        else (out_dir / "recommendation-confidence-v2.json")
    )
    integrity_path = pathlib.Path(args.integrity_file).resolve() if args.integrity_file else (out_dir / "artifact-integrity.json")
    thresholds_path = resolve_input_path(args.thresholds_schema_path, repo_root=repo_root)
    rollback_playbook_path = resolve_input_path(args.rollback_playbook_path, repo_root=repo_root)
    rollback_helper_path = resolve_input_path(args.rollback_helper_path, repo_root=repo_root)

    artifact_records: list[dict[str, Any]] = []

    def load_artifact(
        *,
        name: str,
        path: pathlib.Path,
        required: bool,
        expect_json_object: bool,
    ) -> dict[str, Any] | None:
        record: dict[str, Any] = {
            "artifact": name,
            "path": str(path),
            "required": required,
            "present": path.exists(),
            "valid": False,
            "errors": [],
        }
        if not path.exists():
            if required:
                record["errors"].append("artifact missing")
            artifact_records.append(record)
            return None
        if not expect_json_object:
            record["valid"] = True
            artifact_records.append(record)
            return None
        try:
            payload = read_json_object(path)
        except Exception as exc:
            record["errors"].append(f"invalid JSON object: {exc}")
            artifact_records.append(record)
            return None
        record["valid"] = True
        artifact_records.append(record)
        return payload

    decision_payload = load_artifact(name="decision", path=decision_path, required=True, expect_json_object=True)
    recommendation_payload = load_artifact(
        name="recommendation",
        path=recommendation_path,
        required=False,
        expect_json_object=True,
    )
    confidence_payload = load_artifact(name="confidence", path=confidence_path, required=True, expect_json_object=True)
    integrity_payload = load_artifact(name="integrity", path=integrity_path, required=True, expect_json_object=True)
    _ = load_artifact(name="thresholds_schema", path=thresholds_path, required=True, expect_json_object=True)
    _ = load_artifact(name="rollback_playbook", path=rollback_playbook_path, required=True, expect_json_object=False)
    _ = load_artifact(name="rollback_helper", path=rollback_helper_path, required=True, expect_json_object=False)

    missing_artifact_diagnostics = [
        record for record in artifact_records if (record.get("required") and (not record.get("present") or not record.get("valid")))
    ]

    decision_warnings = (decision_payload or {}).get("warnings", []) if isinstance(decision_payload, dict) else []
    decision_missing_artifact_warning = _warnings_include_missing_decision_artifacts(decision_warnings)
    decision_dimensions = (decision_payload or {}).get("dimensions", {}) if isinstance(decision_payload, dict) else {}
    if not isinstance(decision_dimensions, dict):
        decision_dimensions = {}
    decision_status = str((decision_payload or {}).get("decision", "")).strip().upper()
    dimension_rollbacks = sorted(
        key
        for key, payload in decision_dimensions.items()
        if isinstance(payload, dict) and str(payload.get("status", "")).strip().upper() == "ROLLBACK"
    )
    risk_events_status = str((decision_dimensions.get("risk_events", {}) or {}).get("status", "")).strip().upper()
    drift_status = str((decision_dimensions.get("drift_severity", {}) or {}).get("status", "")).strip().upper()

    confidence_score = _coerce_float((confidence_payload or {}).get("confidence_score_normalized"))
    confidence_level = str((confidence_payload or {}).get("confidence_level", "")).strip().upper()
    confidence_gate_pass = False
    if confidence_score is not None:
        confidence_gate_pass = confidence_score >= float(args.confidence_gate_min)
    elif confidence_level:
        confidence_gate_pass = confidence_level in ("MEDIUM", "HIGH")

    integrity_status = str((integrity_payload or {}).get("overall_status", "")).strip().upper()
    integrity_gate_pass = integrity_status == "PASS"

    artifact_gate_pass = len(missing_artifact_diagnostics) == 0
    decision_gate_pass = bool(decision_payload) and decision_status != "ROLLBACK" and len(dimension_rollbacks) == 0
    risk_critical_pass = bool(decision_payload) and risk_events_status != "ROLLBACK" and drift_status != "ROLLBACK"

    pre_hard_stop_reasons: list[str] = []
    primary_name = args.primary_operator_name.strip()
    primary_signed_at = args.primary_operator_signed_at_utc.strip()
    secondary_name = args.secondary_reviewer_name.strip()
    secondary_signed_at = args.secondary_reviewer_signed_at_utc.strip()
    if not primary_name or not primary_signed_at:
        pre_hard_stop_reasons.append("missing primary signer")
    if not secondary_name or not secondary_signed_at:
        pre_hard_stop_reasons.append("missing secondary signer")
    if not args.rollback_preview_verified:
        pre_hard_stop_reasons.append("rollback preview not verified")
    if not args.target_mode_verified:
        pre_hard_stop_reasons.append("target mode controls not verified")
    if not args.prior_decision_reviewed:
        pre_hard_stop_reasons.append("prior decision not marked as reviewed")
    if decision_payload is None:
        pre_hard_stop_reasons.append("missing prior decision artifact")
    elif decision_status == "ROLLBACK":
        pre_hard_stop_reasons.append("prior decision is ROLLBACK")
    if decision_missing_artifact_warning:
        pre_hard_stop_reasons.append("prior decision contains missing-artifact warning")
    pre_hard_stop_triggered = len(pre_hard_stop_reasons) > 0

    post_hard_stop_reasons: list[str] = []
    if decision_payload is None:
        post_hard_stop_reasons.append("decision.json missing or unreadable")
    elif decision_status == "ROLLBACK":
        post_hard_stop_reasons.append("decision is ROLLBACK")
    if dimension_rollbacks:
        post_hard_stop_reasons.append(f"ROLLBACK dimensions: {', '.join(dimension_rollbacks)}")
    if decision_missing_artifact_warning:
        post_hard_stop_reasons.append("decision warnings include missing report/drift/conformal artifacts")
    if risk_events_status == "ROLLBACK":
        post_hard_stop_reasons.append("risk-critical dimension rollback: risk_events")
    if drift_status == "ROLLBACK":
        post_hard_stop_reasons.append("risk-critical dimension rollback: drift_severity")
    post_hard_stop_triggered = len(post_hard_stop_reasons) > 0

    pre_run_gate_pass = not pre_hard_stop_triggered
    post_run_gate_pass = not post_hard_stop_triggered

    gate_statuses = {
        "artifact_gate": {
            "status": _normalize_gate_status(artifact_gate_pass),
            "reason": "required artifacts present and valid" if artifact_gate_pass else "required artifacts missing or invalid",
        },
        "integrity_gate": {
            "status": _normalize_gate_status(integrity_gate_pass, missing=integrity_payload is None),
            "reason": "artifact-integrity overall_status is PASS" if integrity_gate_pass else "artifact-integrity gate failed or missing",
        },
        "decision_gate": {
            "status": _normalize_gate_status(decision_gate_pass, missing=decision_payload is None),
            "reason": "decision is promotable and no dimension is ROLLBACK"
            if decision_gate_pass
            else "decision is missing/ROLLBACK or includes ROLLBACK dimensions",
        },
        "risk_critical_gate": {
            "status": _normalize_gate_status(risk_critical_pass, missing=decision_payload is None),
            "reason": "risk_events and drift_severity are not ROLLBACK"
            if risk_critical_pass
            else "risk-critical dimension rollback detected or decision missing",
        },
        "pre_run_gate": {
            "status": _normalize_gate_status(pre_run_gate_pass),
            "reason": "pre-run checklist satisfied" if pre_run_gate_pass else "pre-run hard-stop criteria triggered",
        },
        "post_run_gate": {
            "status": _normalize_gate_status(post_run_gate_pass),
            "reason": "post-run checklist satisfied" if post_run_gate_pass else "post-run hard-stop criteria triggered",
        },
        "confidence_gate": {
            "status": _normalize_gate_status(confidence_gate_pass, missing=confidence_payload is None),
            "reason": f"confidence_score_normalized >= {float(args.confidence_gate_min):.2f}"
            if confidence_gate_pass
            else "confidence artifact missing or below confidence floor",
        },
    }

    promotion_allowed = all(
        gate_statuses[key]["status"] == "PASS"
        for key in ("artifact_gate", "integrity_gate", "decision_gate", "risk_critical_gate", "pre_run_gate", "post_run_gate")
    )
    if bool(args.enforce_confidence_gate) and gate_statuses["confidence_gate"]["status"] != "PASS":
        promotion_allowed = False

    signoff_timestamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%d-%H%M%S")
    default_out_file = repo_root / "deploy" / "signoffs" / f"{safe_run_id_filename(args.run_id)}-{signoff_timestamp}.json"
    out_file = pathlib.Path(args.out_file).resolve() if args.out_file else default_out_file
    ensure_dir(out_file.parent)

    checklist_missing_fields: list[str] = []
    if not primary_name:
        checklist_missing_fields.append("signers.primary_operator.name")
    if not primary_signed_at:
        checklist_missing_fields.append("signers.primary_operator.signed_at_utc")
    if not secondary_name:
        checklist_missing_fields.append("signers.secondary_reviewer.name")
    if not secondary_signed_at:
        checklist_missing_fields.append("signers.secondary_reviewer.signed_at_utc")

    packet = {
        "schema_version": "1.1.0",
        "generated_at_utc": now_utc_iso(),
        "run_id": args.run_id,
        "recorded_at_utc": args.recorded_at_utc.strip() or now_utc_iso(),
        "environment": args.environment.strip(),
        "signers": {
            "primary_operator": {
                "name": primary_name,
                "signed_at_utc": primary_signed_at,
            },
            "secondary_reviewer": {
                "name": secondary_name,
                "signed_at_utc": secondary_signed_at,
            },
        },
        "artifacts": {
            "thresholds_schema_path": to_relative_path(thresholds_path, repo_root),
            "decision_file_path": to_relative_path(decision_path, repo_root),
            "recommendation_file_path": to_relative_path(recommendation_path, repo_root),
            "confidence_file_path": to_relative_path(confidence_path, repo_root),
            "integrity_file_path": to_relative_path(integrity_path, repo_root),
            "rollback_playbook_path": to_relative_path(rollback_playbook_path, repo_root),
            "rollback_helper_path": to_relative_path(rollback_helper_path, repo_root),
        },
        "pre_run": {
            "rollback_preview_verified": bool(args.rollback_preview_verified),
            "target_mode_verified": bool(args.target_mode_verified),
            "prior_decision_reviewed": bool(args.prior_decision_reviewed),
            "hard_stop_triggered": pre_hard_stop_triggered,
            "hard_stop_reasons": pre_hard_stop_reasons,
        },
        "post_run": {
            "decision": decision_status or "MISSING",
            "decision_dimensions_reviewed": bool(args.decision_dimensions_reviewed),
            "warnings_present": bool(decision_warnings),
            "hard_stop_triggered": post_hard_stop_triggered,
            "hard_stop_reasons": post_hard_stop_reasons,
            "rollback_executed": bool(args.rollback_executed),
            "rollback_reference": args.rollback_reference.strip(),
        },
        "gate_statuses": gate_statuses,
        "missing_artifact_diagnostics": missing_artifact_diagnostics,
        "artifact_diagnostics": artifact_records,
        "checklist_missing_fields": checklist_missing_fields,
        "source_artifact_summary": {
            "decision_warnings": decision_warnings,
            "rollback_dimensions": dimension_rollbacks,
            "confidence": {
                "score_normalized": confidence_score,
                "level": confidence_level or None,
            },
            "integrity": {
                "overall_status": integrity_status or None,
                "required_failed": ((integrity_payload or {}).get("summary", {}) or {}).get("required_failed")
                if isinstance((integrity_payload or {}).get("summary"), dict)
                else None,
            },
            "recommendation": {
                "present": recommendation_payload is not None,
                "keys": sorted(recommendation_payload.keys()) if isinstance(recommendation_payload, dict) else [],
            },
        },
        "promotion_allowed": promotion_allowed,
        "notes": args.notes,
    }

    out_file.write_text(json.dumps(packet, indent=2, sort_keys=True), encoding="utf-8")
    print(f"Wrote {out_file}")
    print(
        "Signoff gate summary: artifact={} integrity={} decision={} risk={} pre={} post={} confidence={} promotion_allowed={}".format(
            gate_statuses["artifact_gate"]["status"],
            gate_statuses["integrity_gate"]["status"],
            gate_statuses["decision_gate"]["status"],
            gate_statuses["risk_critical_gate"]["status"],
            gate_statuses["pre_run_gate"]["status"],
            gate_statuses["post_run_gate"]["status"],
            gate_statuses["confidence_gate"]["status"],
            promotion_allowed,
        )
    )
    if missing_artifact_diagnostics:
        print(f"Missing/invalid required artifacts: {len(missing_artifact_diagnostics)}")
    return 0 if promotion_allowed else 1


def command_rollback_packet(args: argparse.Namespace) -> int:
    if not args.run_id:
        raise ValueError("--run-id is required for rollback-packet")

    repo_root = pathlib.Path(args.repo_root).resolve()
    out_dir = run_output_dir(args.output_dir, args.run_id)
    ensure_dir(out_dir)

    decision_path = pathlib.Path(args.decision_file).resolve() if args.decision_file else (out_dir / "decision.json")
    integrity_path = pathlib.Path(args.integrity_file).resolve() if args.integrity_file else (out_dir / "artifact-integrity.json")
    confidence_path = pathlib.Path(args.confidence_file).resolve() if args.confidence_file else (out_dir / "recommendation-confidence-v2.json")
    toxic_flow_path = pathlib.Path(args.toxic_flow_file).resolve() if args.toxic_flow_file else (out_dir / "toxic-flow-advisor.json")
    signoff_path = pathlib.Path(args.signoff_file).resolve() if args.signoff_file else (out_dir / "signoff-packet.json")
    report_path = pathlib.Path(args.report_file).resolve() if args.report_file else (out_dir / "report.json")
    rollback_playbook_path = resolve_input_path(args.rollback_playbook_path, repo_root=repo_root)
    rollback_helper_path = resolve_input_path(args.rollback_helper_path, repo_root=repo_root)

    artifact_records: list[dict[str, Any]] = []
    payloads: dict[str, dict[str, Any]] = {}

    def load_artifact(
        *,
        name: str,
        path: pathlib.Path,
        required: bool,
        expect_json_object: bool,
    ) -> None:
        record: dict[str, Any] = {
            "artifact": name,
            "path": str(path),
            "path_relative_to_repo": to_relative_path(path, repo_root),
            "required": required,
            "present": path.exists(),
            "valid": False,
            "sha256": file_sha256(path),
            "errors": [],
        }
        if not path.exists():
            if required:
                record["errors"].append("artifact missing")
            artifact_records.append(record)
            return
        if not expect_json_object:
            record["valid"] = True
            artifact_records.append(record)
            return
        try:
            payload = read_json_object(path)
        except Exception as exc:
            record["errors"].append(f"invalid JSON object: {exc}")
            artifact_records.append(record)
            return
        record["valid"] = True
        payloads[name] = payload
        artifact_records.append(record)

    load_artifact(name="decision", path=decision_path, required=True, expect_json_object=True)
    load_artifact(name="integrity", path=integrity_path, required=True, expect_json_object=True)
    load_artifact(name="confidence", path=confidence_path, required=False, expect_json_object=True)
    load_artifact(name="toxic_flow", path=toxic_flow_path, required=False, expect_json_object=True)
    load_artifact(name="signoff", path=signoff_path, required=False, expect_json_object=True)
    load_artifact(name="report", path=report_path, required=False, expect_json_object=True)
    load_artifact(name="rollback_playbook", path=rollback_playbook_path, required=True, expect_json_object=False)
    load_artifact(name="rollback_helper", path=rollback_helper_path, required=True, expect_json_object=False)

    artifact_records.sort(key=lambda item: str(item.get("artifact", "")))
    missing_artifact_diagnostics = [
        {
            "artifact": record["artifact"],
            "path": record["path"],
            "path_relative_to_repo": record.get("path_relative_to_repo"),
            "required": True,
            "present": bool(record.get("present")),
            "valid": bool(record.get("valid")),
            "errors": list(record.get("errors", [])),
        }
        for record in artifact_records
        if bool(record.get("required")) and (not record.get("present") or not record.get("valid"))
    ]

    decision_payload = payloads.get("decision", {})
    integrity_payload = payloads.get("integrity", {})
    confidence_payload = payloads.get("confidence", {})
    toxic_flow_payload = payloads.get("toxic_flow", {})
    signoff_payload = payloads.get("signoff", {})
    report_payload = payloads.get("report", {})

    decision_status = str(decision_payload.get("decision", "")).strip().upper()
    decision_dimensions = decision_payload.get("dimensions", {})
    if not isinstance(decision_dimensions, dict):
        decision_dimensions = {}
    rollback_dimensions = sorted(
        key
        for key, value in decision_dimensions.items()
        if isinstance(value, dict) and str(value.get("status", "")).strip().upper() == "ROLLBACK"
    )

    failed_gates: list[dict[str, Any]] = []

    def add_failed_gate(gate: str, reason: str, source_artifact: str) -> None:
        failed_gates.append({"gate": gate, "status": "FAIL", "reason": reason, "source_artifact": source_artifact})

    if not decision_payload:
        add_failed_gate("decision_gate", "decision artifact missing or invalid", "decision")
    elif decision_status == "ROLLBACK" or rollback_dimensions:
        reason = "decision is ROLLBACK"
        if rollback_dimensions:
            reason = f"ROLLBACK dimensions: {', '.join(rollback_dimensions)}"
        add_failed_gate("decision_gate", reason, "decision")

    integrity_status = str(integrity_payload.get("overall_status", "")).strip().upper()
    integrity_summary = integrity_payload.get("summary", {})
    required_failed = None
    if isinstance(integrity_summary, dict):
        required_failed = _coerce_float(integrity_summary.get("required_failed"))
    if not integrity_payload:
        add_failed_gate("integrity_gate", "artifact-integrity missing or invalid", "integrity")
    elif integrity_status != "PASS":
        detail = f"overall_status={integrity_status or 'unknown'}"
        if required_failed is not None:
            detail += f", required_failed={int(round(required_failed))}"
        add_failed_gate("integrity_gate", detail, "integrity")

    confidence_score = _coerce_float(confidence_payload.get("confidence_score_normalized"))
    confidence_level = str(confidence_payload.get("confidence_level", "")).strip().upper()
    confidence_gate_min = float(args.confidence_gate_min)
    confidence_gate_pass = False
    if confidence_score is not None:
        confidence_gate_pass = confidence_score >= confidence_gate_min
    elif confidence_level:
        confidence_gate_pass = confidence_level in {"MEDIUM", "HIGH"}
    if confidence_payload and not confidence_gate_pass:
        add_failed_gate(
            "confidence_gate",
            f"confidence below floor {confidence_gate_min:.2f}",
            "confidence",
        )

    toxic_flow_tier = str(toxic_flow_payload.get("severity_tier", "")).strip().upper()
    if toxic_flow_tier in {"HIGH", "SEVERE"}:
        add_failed_gate("toxic_flow_gate", f"toxic flow severity tier {toxic_flow_tier}", "toxic_flow")

    signoff_gate_statuses = signoff_payload.get("gate_statuses", {})
    if isinstance(signoff_gate_statuses, dict):
        for gate_name in sorted(signoff_gate_statuses.keys()):
            payload = signoff_gate_statuses.get(gate_name)
            if not isinstance(payload, dict):
                continue
            status = str(payload.get("status", "")).strip().upper()
            if status and status != "PASS":
                reason = str(payload.get("reason", "")).strip() or f"status={status}"
                add_failed_gate(f"signoff:{gate_name}", reason, "signoff")

    failed_gates.sort(key=lambda item: str(item.get("gate", "")))

    trigger_rows: list[dict[str, Any]] = []

    def add_trigger(trigger_id: str, source_artifact: str, reason: str, severity: str = "HIGH") -> None:
        trigger_rows.append(
            {
                "trigger_id": trigger_id,
                "severity": severity,
                "source_artifact": source_artifact,
                "reason": reason,
            }
        )

    if decision_status == "ROLLBACK":
        add_trigger("decision-rollback", "decision", "decision.json computed final decision ROLLBACK", "CRITICAL")
    for dimension in rollback_dimensions:
        add_trigger(
            f"dimension-rollback:{dimension}",
            "decision",
            f"decision dimension '{dimension}' evaluated to ROLLBACK",
            "HIGH",
        )
    if integrity_payload and integrity_status != "PASS":
        add_trigger("artifact-integrity-failed", "integrity", f"artifact integrity status {integrity_status or 'unknown'}", "HIGH")
    if confidence_payload and not confidence_gate_pass:
        add_trigger("confidence-below-floor", "confidence", f"confidence below {confidence_gate_min:.2f}", "MEDIUM")
    if toxic_flow_tier in {"HIGH", "SEVERE"}:
        add_trigger(
            f"toxic-flow-{toxic_flow_tier.lower()}",
            "toxic_flow",
            f"toxic flow severity tier {toxic_flow_tier}",
            "HIGH" if toxic_flow_tier == "HIGH" else "CRITICAL",
        )

    post_run = signoff_payload.get("post_run", {})
    if isinstance(post_run, dict) and post_run.get("hard_stop_triggered") is True:
        reasons = post_run.get("hard_stop_reasons")
        if isinstance(reasons, list) and reasons:
            add_trigger(
                "signoff-post-run-hard-stop",
                "signoff",
                "; ".join(str(reason) for reason in reasons if isinstance(reason, str)),
                "CRITICAL",
            )
        else:
            add_trigger("signoff-post-run-hard-stop", "signoff", "signoff post-run hard stop triggered", "CRITICAL")

    unique_trigger_map: dict[str, dict[str, Any]] = {}
    for trigger in trigger_rows:
        trigger_id = str(trigger.get("trigger_id", "")).strip()
        if not trigger_id:
            continue
        unique_trigger_map[trigger_id] = trigger
    triggers = [unique_trigger_map[key] for key in sorted(unique_trigger_map.keys())]

    impacted_keys: set[str] = {
        "TRADING_ENABLED",
        "LIVE_TRADING",
        "PAPER_TRADING",
        "ALPHA_TRADING_ENABLED",
    }
    dimension_to_env = {
        "pnl_return_pct": ("MAX_SINGLE_ORDER_USDC", "MIN_SIGNAL_NOTIONAL_USD"),
        "fee_drag_pct": ("MAX_SINGLE_ORDER_USDC", "MIN_SIGNAL_NOTIONAL_USD"),
        "drift_severity": ("DRIFT_ABORT_COOLDOWN_SECS", "MAX_ORDERS_PER_SECOND", "PAPER_ADVERSE_FILL_BPS"),
        "risk_events": ("MAX_SINGLE_ORDER_USDC", "MAX_ORDERS_PER_SECOND", "TRADING_ENABLED"),
        "conformal_miscoverage_pct": ("ALPHA_TRADING_ENABLED",),
    }
    for dimension in rollback_dimensions:
        for key in dimension_to_env.get(dimension, ()):
            impacted_keys.add(key)

    toxic_guardrails = toxic_flow_payload.get("guardrail_recommendations", {})
    if isinstance(toxic_guardrails, dict):
        env_overrides = toxic_guardrails.get("env_overrides")
        if isinstance(env_overrides, dict):
            for key in env_overrides.keys():
                if isinstance(key, str) and key.strip():
                    impacted_keys.add(key.strip())

    rollback_steps = [
        {
            "step_id": "backup-current-env",
            "description": "Backup /opt/blink/.env before making rollback edits.",
            "commands": ["sudo cp /opt/blink/.env /opt/blink/.env.rollback.$(date +%Y%m%d-%H%M%S).bak"],
        },
        {
            "step_id": "lock-runtime-switches",
            "description": "Apply rollback target mode flags in /opt/blink/.env.",
            "commands": [
                "sudo sed -i 's/^TRADING_ENABLED=.*/TRADING_ENABLED=false/' /opt/blink/.env",
                "sudo sed -i 's/^LIVE_TRADING=.*/LIVE_TRADING=false/' /opt/blink/.env",
                "sudo sed -i 's/^PAPER_TRADING=.*/PAPER_TRADING=true/' /opt/blink/.env",
                "sudo sed -i 's/^ALPHA_TRADING_ENABLED=.*/ALPHA_TRADING_ENABLED=false/' /opt/blink/.env",
            ],
        },
        {
            "step_id": "append-missing-runtime-switches",
            "description": "Append rollback flags if missing from /opt/blink/.env.",
            "commands": [
                "grep -q '^TRADING_ENABLED=' /opt/blink/.env || echo 'TRADING_ENABLED=false' | sudo tee -a /opt/blink/.env >/dev/null",
                "grep -q '^LIVE_TRADING=' /opt/blink/.env || echo 'LIVE_TRADING=false' | sudo tee -a /opt/blink/.env >/dev/null",
                "grep -q '^PAPER_TRADING=' /opt/blink/.env || echo 'PAPER_TRADING=true' | sudo tee -a /opt/blink/.env >/dev/null",
                "grep -q '^ALPHA_TRADING_ENABLED=' /opt/blink/.env || echo 'ALPHA_TRADING_ENABLED=false' | sudo tee -a /opt/blink/.env >/dev/null",
            ],
        },
        {
            "step_id": "restart-services",
            "description": "Restart blink-engine and optional blink-sidecar services.",
            "commands": [
                "sudo systemctl restart blink-engine",
                "if systemctl list-unit-files | grep -q '^blink-sidecar\\.service'; then sudo systemctl restart blink-sidecar; fi",
            ],
        },
        {
            "step_id": "verify-rollback-state",
            "description": "Run verification checks and ensure all rollback targets are active.",
            "commands": [
                "systemctl is-active blink-engine",
                "if systemctl list-unit-files | grep -q '^blink-sidecar\\.service'; then systemctl is-active blink-sidecar; fi",
                "grep -E '^(TRADING_ENABLED|LIVE_TRADING|PAPER_TRADING|ALPHA_TRADING_ENABLED)=' /opt/blink/.env",
                "curl -sf http://127.0.0.1:3030/api/status",
                "journalctl -u blink-engine -n 30 --no-pager",
            ],
        },
    ]

    verification_checklist = [
        "blink-engine service is active",
        "blink-sidecar is active if installed",
        "TRADING_ENABLED=false and LIVE_TRADING=false",
        "PAPER_TRADING=true and ALPHA_TRADING_ENABLED=false",
        "http://127.0.0.1:3030/api/status returns HTTP 200",
    ]

    evidence_artifacts = [
        {
            "artifact": record["artifact"],
            "path": record["path"],
            "path_relative_to_repo": record.get("path_relative_to_repo"),
            "present": bool(record.get("present")),
            "valid": bool(record.get("valid")),
            "required": bool(record.get("required")),
            "sha256": record.get("sha256"),
        }
        for record in artifact_records
    ]

    generated_candidates: list[str] = []
    for payload in payloads.values():
        generated_candidates.extend(_artifact_timestamp_candidates(payload))
    generated_at_utc = args.generated_at_utc.strip() or (sorted(generated_candidates)[-1] if generated_candidates else "unknown")

    artifact_gate_pass = len(missing_artifact_diagnostics) == 0
    rollback_paths_present = all(
        any(
            record["artifact"] == required_artifact and record.get("present") and record.get("valid")
            for record in artifact_records
        )
        for required_artifact in ("rollback_playbook", "rollback_helper")
    )
    trigger_detected = len(triggers) > 0
    failed_gate_detected = len(failed_gates) > 0
    packet_ready_for_review = artifact_gate_pass and rollback_paths_present
    rollback_recommended = trigger_detected or failed_gate_detected
    readiness_pass = packet_ready_for_review and rollback_recommended

    readiness_reasons: list[str] = []
    if not artifact_gate_pass:
        readiness_reasons.append("required artifacts are missing or invalid")
    if not rollback_paths_present:
        readiness_reasons.append("rollback playbook/helper references are missing")
    if not rollback_recommended:
        readiness_reasons.append("no rollback triggers or failed gates detected")
    if not readiness_reasons:
        readiness_reasons.append("rollback packet is ready for operator execution review")

    packet = {
        "schema_version": "1.0.0",
        "run_id": args.run_id,
        "generated_at_utc": generated_at_utc,
        "deterministic_inputs": {
            "confidence_gate_min": round(confidence_gate_min, 6),
            "repo_root": str(repo_root),
            "output_dir": str(out_dir),
        },
        "artifacts": {
            "decision_file_path": to_relative_path(decision_path, repo_root),
            "integrity_file_path": to_relative_path(integrity_path, repo_root),
            "confidence_file_path": to_relative_path(confidence_path, repo_root),
            "toxic_flow_file_path": to_relative_path(toxic_flow_path, repo_root),
            "signoff_file_path": to_relative_path(signoff_path, repo_root),
            "report_file_path": to_relative_path(report_path, repo_root),
            "rollback_playbook_path": to_relative_path(rollback_playbook_path, repo_root),
            "rollback_helper_path": to_relative_path(rollback_helper_path, repo_root),
        },
        "missing_artifact_diagnostics": missing_artifact_diagnostics,
        "rollback_context": {
            "decision": decision_status or "MISSING",
            "rollback_dimensions": rollback_dimensions,
            "triggers": triggers,
            "failed_gates": failed_gates,
            "impacted_configs": {
                "rollback_targets": {
                    "TRADING_ENABLED": "false",
                    "LIVE_TRADING": "false",
                    "PAPER_TRADING": "true",
                    "ALPHA_TRADING_ENABLED": "false",
                },
                "likely_impacted_env_keys": sorted(impacted_keys),
                "services": ["blink-engine", "blink-sidecar (optional)"],
            },
            "rollback_steps": rollback_steps,
            "verification_checklist": verification_checklist,
            "evidence_artifacts": evidence_artifacts,
            "playbook_alignment": {
                "playbook_path": to_relative_path(rollback_playbook_path, repo_root),
                "helper_path": to_relative_path(rollback_helper_path, repo_root),
                "playbook_scope": "Immediate rollback to locked-down paper mode",
            },
        },
        "readiness": {
            "status": "PASS" if readiness_pass else "FAIL",
            "pass": readiness_pass,
            "checks": {
                "artifacts_complete": artifact_gate_pass,
                "rollback_paths_present": rollback_paths_present,
                "trigger_detected": trigger_detected,
                "failed_gate_detected": failed_gate_detected,
                "packet_ready_for_review": packet_ready_for_review,
                "rollback_recommended": rollback_recommended,
            },
            "reasons": readiness_reasons,
        },
        "automation_safety": {
            "deterministic_output": True,
            "sorted_trigger_ids": [row["trigger_id"] for row in triggers],
            "safe_for_operator_review": packet_ready_for_review,
            "safe_for_automation": packet_ready_for_review,
        },
        "report_snapshot": {
            "pnl_return_pct": _pick_nested_scalar(report_payload, ("pnl_return_pct", "return_pct", "net_return_pct", "roi_pct"))
            if report_payload
            else None,
            "confidence_score_normalized": confidence_score,
            "toxic_flow_severity_tier": toxic_flow_tier or None,
        },
    }

    packet["deterministic_fingerprint"] = stable_hash(packet)

    out_file = pathlib.Path(args.out_file).resolve() if args.out_file else (out_dir / "rollback-packet.json")
    ensure_dir(out_file.parent)
    out_file.write_text(json.dumps(packet, indent=2, sort_keys=True), encoding="utf-8")

    print(f"Wrote {out_file}")
    print(
        "Rollback readiness={} triggers={} failed_gates={} missing_required_artifacts={}".format(
            packet["readiness"]["status"],
            len(triggers),
            len(failed_gates),
            len(missing_artifact_diagnostics),
        )
    )
    if missing_artifact_diagnostics:
        print(f"Missing/invalid required artifacts: {len(missing_artifact_diagnostics)}")

    if bool(args.fail_on_not_ready) and not readiness_pass:
        return 1
    return 0


_ANOMALY_SEVERITY_ORDER = {"LOW": 1, "MEDIUM": 2, "HIGH": 3, "CRITICAL": 4}


def _normalize_anomaly_severity(value: Any) -> str:
    candidate = str(value or "").strip().upper()
    if candidate in _ANOMALY_SEVERITY_ORDER:
        return candidate
    return "LOW"


def _max_anomaly_severity(values: list[str]) -> str:
    best = "LOW"
    for value in values:
        normalized = _normalize_anomaly_severity(value)
        if _ANOMALY_SEVERITY_ORDER[normalized] > _ANOMALY_SEVERITY_ORDER[best]:
            best = normalized
    return best


def _severity_meets_gate(severity: str, gate: str) -> bool:
    normalized_gate = str(gate or "none").strip().lower()
    if normalized_gate == "none":
        return False
    normalized = _normalize_anomaly_severity(severity)
    if normalized_gate == "critical":
        return normalized == "CRITICAL"
    if normalized_gate == "high":
        return _ANOMALY_SEVERITY_ORDER[normalized] >= _ANOMALY_SEVERITY_ORDER["HIGH"]
    return False


def _escalation_channel_for_severity(severity: str) -> str:
    normalized = _normalize_anomaly_severity(severity)
    if normalized == "CRITICAL":
        return "pagerduty://blink-control-plane-primary"
    if normalized == "HIGH":
        return "slack://#blink-evals-oncall"
    if normalized == "MEDIUM":
        return "slack://#blink-evals-monitoring"
    return "slack://#blink-evals-monitoring"


def command_anomaly_response_automation(args: argparse.Namespace) -> int:
    if not args.run_id:
        raise ValueError("--run-id is required for anomaly-response-automation")

    out_dir = run_output_dir(args.output_dir, args.run_id)
    ensure_dir(out_dir)

    drift_path = pathlib.Path(args.drift_file).resolve() if args.drift_file else (out_dir / "drift-matrix.json")
    toxic_flow_path = pathlib.Path(args.toxic_flow_file).resolve() if args.toxic_flow_file else (out_dir / "toxic-flow-advisor.json")
    integrity_path = pathlib.Path(args.integrity_file).resolve() if args.integrity_file else (out_dir / "artifact-integrity.json")
    confidence_path = pathlib.Path(args.confidence_file).resolve() if args.confidence_file else (out_dir / "recommendation-confidence-v2.json")
    decision_path = pathlib.Path(args.decision_file).resolve() if args.decision_file else (out_dir / "decision.json")

    drift_payload = optional_json_object(drift_path)
    toxic_flow_payload = optional_json_object(toxic_flow_path)
    integrity_payload = optional_json_object(integrity_path)
    confidence_payload = optional_json_object(confidence_path)
    decision_payload = optional_json_object(decision_path)

    warnings: list[str] = []
    for label, path, payload in (
        ("drift-matrix", drift_path, drift_payload),
        ("toxic-flow-advisor", toxic_flow_path, toxic_flow_payload),
        ("artifact-integrity", integrity_path, integrity_payload),
        ("recommendation-confidence-v2", confidence_path, confidence_payload),
        ("decision", decision_path, decision_payload),
    ):
        if payload is None:
            warnings.append(f"{label} artifact missing: {path} (graceful fallback active)")

    conditions: list[dict[str, Any]] = []
    mitigations: list[dict[str, Any]] = []
    action_map: dict[str, dict[str, Any]] = {}

    def add_mitigation(mitigation_id: str, *, priority: int, description: str, applies_to: str) -> None:
        mitigations.append(
            {
                "priority": priority,
                "mitigation_id": mitigation_id,
                "applies_to": applies_to,
                "description": description,
            }
        )

    def add_action(
        action_id: str,
        *,
        priority: int,
        category: str,
        description: str,
        command_hint: str,
    ) -> None:
        existing = action_map.get(action_id)
        record = {
            "action_id": action_id,
            "priority": priority,
            "category": category,
            "description": description,
            "command_hint": command_hint,
            "automation_safe": True,
            "requires_human_confirmation": False,
            "live_trade_mutation": False,
        }
        if existing is None or int(record["priority"]) < int(existing.get("priority", 999)):
            action_map[action_id] = record

    drift_delta = drift_payload.get("delta", {}) if isinstance(drift_payload, dict) else {}
    if not isinstance(drift_delta, dict):
        drift_delta = {}
    fill_rate_delta = _coerce_float(drift_delta.get("fill_rate_pct_points"))
    slippage_delta = _coerce_float(drift_delta.get("avg_slippage_bps"))
    reject_mix_l1 = _coerce_float(((drift_delta.get("reject_mix") or {}) if isinstance(drift_delta.get("reject_mix"), dict) else {}).get("l1_distance"))
    notional_l1 = _coerce_float(
        ((drift_delta.get("notional_distribution") or {}) if isinstance(drift_delta.get("notional_distribution"), dict) else {}).get("l1_distance")
    )
    category_l1 = _coerce_float(
        ((drift_delta.get("category_mix") or {}) if isinstance(drift_delta.get("category_mix"), dict) else {}).get("l1_distance")
    )
    drift_l1_candidates = [value for value in (reject_mix_l1, notional_l1, category_l1) if value is not None]
    max_l1_drift = max(drift_l1_candidates) if drift_l1_candidates else None
    severe_drift_triggered = bool(
        (fill_rate_delta is not None and fill_rate_delta <= float(args.drift_fill_rate_critical_pp))
        or (slippage_delta is not None and slippage_delta >= float(args.drift_slippage_critical_bps))
        or (max_l1_drift is not None and max_l1_drift >= float(args.drift_distribution_critical_l1))
    )
    drift_severity = "CRITICAL" if severe_drift_triggered else "LOW"
    conditions.append(
        {
            "condition_id": "severe_drift",
            "triggered": severe_drift_triggered,
            "severity": drift_severity,
            "summary": "Drift exceeds configured critical thresholds" if severe_drift_triggered else "Drift remains below critical threshold gates",
            "evidence": {
                "fill_rate_pct_points": fill_rate_delta,
                "avg_slippage_bps_delta": slippage_delta,
                "reject_mix_l1": reject_mix_l1,
                "notional_distribution_l1": notional_l1,
                "category_mix_l1": category_l1,
                "max_distribution_l1": max_l1_drift,
            },
            "thresholds": {
                "fill_rate_pct_points_min": float(args.drift_fill_rate_critical_pp),
                "avg_slippage_bps_max": float(args.drift_slippage_critical_bps),
                "distribution_l1_max": float(args.drift_distribution_critical_l1),
            },
        }
    )
    if severe_drift_triggered:
        add_mitigation(
            "drift-rebaseline-required",
            priority=1,
            applies_to="severe_drift",
            description="Pause promotion and regenerate drift/context artifacts before next decision.",
        )
        add_action(
            "refresh-drift-artifacts",
            priority=1,
            category="artifact-refresh",
            description="Re-run drift-matrix, artifact-integrity, decision-eval, and confidence synthesis.",
            command_hint="python tools\\run_eval_cycle.py --run-id <run-id> drift-matrix --baseline-json <baseline.json>",
        )

    toxic_tier = str((toxic_flow_payload or {}).get("severity_tier", "")).strip().upper()
    toxic_score = _coerce_float((toxic_flow_payload or {}).get("toxic_flow_score_normalized"))
    severe_toxic_triggered = toxic_tier in {"HIGH", "SEVERE"}
    toxic_severity = "CRITICAL" if toxic_tier == "SEVERE" else ("HIGH" if toxic_tier == "HIGH" else "LOW")
    conditions.append(
        {
            "condition_id": "severe_toxic_flow",
            "triggered": severe_toxic_triggered,
            "severity": toxic_severity,
            "summary": f"Toxic-flow tier is {toxic_tier or 'UNKNOWN'}",
            "evidence": {
                "severity_tier": toxic_tier or None,
                "toxic_flow_score_normalized": toxic_score,
            },
            "thresholds": {"trigger_tiers": ["HIGH", "SEVERE"]},
        }
    )
    if severe_toxic_triggered:
        add_mitigation(
            "tighten-eval-guardrails",
            priority=2,
            applies_to="severe_toxic_flow",
            description="Escalate control-plane review and require integrity + confidence refresh before signoff.",
        )
        add_action(
            "force-control-plane-review",
            priority=2,
            category="escalation",
            description="Open immediate eval incident and require operator review packet before any promotion decision.",
            command_hint="python tools\\run_eval_cycle.py --run-id <run-id> signoff-packet --enforce-confidence-gate",
        )

    integrity_summary = (integrity_payload or {}).get("summary", {})
    if not isinstance(integrity_summary, dict):
        integrity_summary = {}
    required_failed = _coerce_float(integrity_summary.get("required_failed")) or 0.0
    checks_failed = _coerce_float(integrity_summary.get("checks_failed")) or 0.0
    overall_integrity_status = str((integrity_payload or {}).get("overall_status", "")).strip().upper()
    repeated_integrity_triggered = bool(
        required_failed >= float(args.integrity_required_failure_threshold)
        or checks_failed >= float(args.integrity_total_failure_threshold)
    )
    integrity_severity = "CRITICAL" if required_failed >= float(args.integrity_required_failure_threshold) else ("HIGH" if repeated_integrity_triggered else "LOW")
    conditions.append(
        {
            "condition_id": "repeated_integrity_failures",
            "triggered": repeated_integrity_triggered,
            "severity": integrity_severity,
            "summary": (
                "Integrity failures crossed repeated-failure thresholds"
                if repeated_integrity_triggered
                else "Integrity checks did not cross repeated-failure thresholds"
            ),
            "evidence": {
                "overall_status": overall_integrity_status or None,
                "required_failed": int(round(required_failed)),
                "checks_failed": int(round(checks_failed)),
            },
            "thresholds": {
                "required_failed_min": int(round(float(args.integrity_required_failure_threshold))),
                "checks_failed_min": int(round(float(args.integrity_total_failure_threshold))),
            },
        }
    )
    if repeated_integrity_triggered:
        add_mitigation(
            "integrity-repair-loop",
            priority=1,
            applies_to="repeated_integrity_failures",
            description="Run deterministic integrity repair loop until required artifacts validate cleanly.",
        )
        add_action(
            "rerun-integrity-gate",
            priority=1,
            category="integrity",
            description="Rebuild required artifacts then rerun artifact-integrity checks before decision steps.",
            command_hint="python tools\\run_eval_cycle.py --run-id <run-id> artifact-integrity --require-drift-matrix",
        )

    confidence_score = _coerce_float((confidence_payload or {}).get("confidence_score_normalized"))
    confidence_level = str((confidence_payload or {}).get("confidence_level", "")).strip().upper()
    confidence_collapse_triggered = bool(
        (confidence_score is not None and confidence_score <= float(args.confidence_collapse_threshold))
        or (confidence_level == "LOW" and (confidence_score is None or confidence_score <= 0.45))
    )
    confidence_severity = "CRITICAL" if (confidence_score is not None and confidence_score <= 0.25) else ("HIGH" if confidence_collapse_triggered else "LOW")
    conditions.append(
        {
            "condition_id": "confidence_collapse",
            "triggered": confidence_collapse_triggered,
            "severity": confidence_severity,
            "summary": "Confidence score collapsed below control-plane floor" if confidence_collapse_triggered else "Confidence remains above collapse threshold",
            "evidence": {
                "confidence_score_normalized": confidence_score,
                "confidence_level": confidence_level or None,
            },
            "thresholds": {"confidence_score_max": float(args.confidence_collapse_threshold)},
        }
    )
    if confidence_collapse_triggered:
        add_mitigation(
            "confidence-rebuild",
            priority=2,
            applies_to="confidence_collapse",
            description="Require additional snapshots/walkforward evidence before considering signoff.",
        )
        add_action(
            "capture-extra-snapshots",
            priority=2,
            category="data-quality",
            description="Collect extra snapshots and rerun recommendation-confidence-v2 with refreshed artifacts.",
            command_hint="python tools\\run_eval_cycle.py --run-id <run-id> snapshot",
        )

    triggered_rows = [row for row in conditions if row.get("triggered") is True]
    if triggered_rows:
        add_action(
            "freeze-promotion",
            priority=1,
            category="gating",
            description="Freeze promotion and require operator review until anomalies clear.",
            command_hint="python tools\\run_eval_cycle.py --run-id <run-id> signoff-packet --enforce-confidence-gate",
        )

    highest_severity = _max_anomaly_severity([str(row.get("severity", "LOW")) for row in triggered_rows] or ["LOW"])
    escalation_channel = _escalation_channel_for_severity(highest_severity)

    sorted_actions = sorted(action_map.values(), key=lambda item: (int(item.get("priority", 999)), str(item.get("action_id", ""))))
    sorted_mitigations = sorted(
        mitigations,
        key=lambda item: (int(item.get("priority", 999)), str(item.get("mitigation_id", ""))),
    )
    for index, item in enumerate(sorted_actions, start=1):
        item["priority_rank"] = index

    response = {
        "schema_version": "1.0.0",
        "generated_at_utc": now_utc_iso(),
        "run_id": args.run_id,
        "inputs": {
            "drift_file": {"path": str(drift_path), "present": drift_payload is not None},
            "toxic_flow_file": {"path": str(toxic_flow_path), "present": toxic_flow_payload is not None},
            "integrity_file": {"path": str(integrity_path), "present": integrity_payload is not None},
            "confidence_file": {"path": str(confidence_path), "present": confidence_payload is not None},
            "decision_file": {"path": str(decision_path), "present": decision_payload is not None},
        },
        "conditions": conditions,
        "summary": {
            "triggered_conditions": len(triggered_rows),
            "highest_severity": highest_severity,
            "escalation_channel": escalation_channel,
            "requires_escalation": highest_severity in {"HIGH", "CRITICAL"},
            "control_plane_only": True,
        },
        "recommended_mitigations": sorted_mitigations,
        "automation_safe_action_list": sorted_actions,
        "escalation": {
            "channel": escalation_channel,
            "routing_key": "blink-eval-anomaly",
            "target_sla_minutes": 10 if highest_severity == "CRITICAL" else (30 if highest_severity == "HIGH" else 120),
            "requires_operator_ack": highest_severity in {"HIGH", "CRITICAL"},
        },
        "constraints": {
            "live_trade_mutation_allowed": False,
            "safe_for_control_plane_automation": True,
        },
        "warnings": sorted(set(warnings)),
    }
    response["deterministic_fingerprint"] = stable_hash(response)

    out_file = pathlib.Path(args.out_file).resolve() if args.out_file else (out_dir / "anomaly-response-plan.json")
    ensure_dir(out_file.parent)
    out_file.write_text(json.dumps(response, indent=2, sort_keys=True), encoding="utf-8")

    print(f"Wrote {out_file}")
    print(
        "Anomaly response severity={} triggered_conditions={} actions={} escalation={}".format(
            highest_severity,
            len(triggered_rows),
            len(sorted_actions),
            escalation_channel,
        )
    )
    if warnings:
        print(f"Warnings: {len(warnings)} (missing artifacts handled gracefully)")

    if _severity_meets_gate(highest_severity, str(args.fail_on_severity)):
        return 1
    return 0


def command_monthly_strategy_review(args: argparse.Namespace) -> int:
    from monthly_strategy_review import run_monthly_strategy_review

    return int(run_monthly_strategy_review(args))


def command_full_cycle(args: argparse.Namespace) -> int:
    if not args.run_id:
        raise ValueError("--run-id is required for full-cycle")
    if args.require_drift_matrix and not args.baseline_json:
        raise ValueError("--require-drift-matrix requires --baseline-json in full-cycle")

    out_dir = run_output_dir(args.output_dir, args.run_id)
    ensure_dir(out_dir)
    repo_root = pathlib.Path(args.repo_root).resolve()
    decision_out_file = pathlib.Path(args.decision_out_file).resolve() if args.decision_out_file else (out_dir / "decision.json")
    artifact_integrity_out_file = (
        pathlib.Path(args.artifact_integrity_out_file).resolve()
        if args.artifact_integrity_out_file
        else (out_dir / "artifact-integrity.json")
    )
    drift_out_file = pathlib.Path(args.drift_out_file).resolve() if args.drift_out_file else (out_dir / "drift-matrix.json")
    walkforward_out_file = (
        pathlib.Path(args.walkforward_out_file).resolve()
        if args.walkforward_out_file
        else (out_dir / "purged-walkforward.json")
    )
    microstructure_out_file = (
        pathlib.Path(args.microstructure_out_file).resolve()
        if args.microstructure_out_file
        else (out_dir / "microstructure-imbalance.json")
    )
    toxic_flow_out_file = (
        pathlib.Path(args.toxic_flow_out_file).resolve() if args.toxic_flow_out_file else (out_dir / "toxic-flow-advisor.json")
    )
    market_category_heatmap_out_file = (
        pathlib.Path(args.market_category_heatmap_out_file).resolve()
        if args.market_category_heatmap_out_file
        else (out_dir / "market-category-heatmap.json")
    )
    confidence_out_file = (
        pathlib.Path(args.confidence_out_file).resolve()
        if args.confidence_out_file
        else (out_dir / "recommendation-confidence-v2.json")
    )
    anomaly_response_out_file = (
        pathlib.Path(args.anomaly_response_out_file).resolve()
        if args.anomaly_response_out_file
        else (out_dir / "anomaly-response-plan.json")
    )
    signoff_packet_out_file = (
        pathlib.Path(args.signoff_packet_out_file).resolve()
        if args.signoff_packet_out_file
        else (repo_root / "deploy" / "signoffs" / f"{safe_run_id_filename(args.run_id)}-auto-signoff.json")
    )
    rollback_packet_out_file = (
        pathlib.Path(args.rollback_packet_out_file).resolve()
        if args.rollback_packet_out_file
        else (out_dir / "rollback-packet.json")
    )
    production_readiness_out_file = (
        pathlib.Path(args.production_readiness_out_file).resolve()
        if args.production_readiness_out_file
        else (out_dir / "production-readiness-gate.json")
    )
    execution_drag_out_file = (
        pathlib.Path(args.execution_drag_out_file).resolve()
        if args.execution_drag_out_file
        else (out_dir / "execution-drag-attribution.json")
    )
    bandit_out_dir = (
        pathlib.Path(args.bandit_out_dir).resolve()
        if args.bandit_out_dir
        else (out_dir / "bandit-allocation")
    )
    regime_recommendation_out_file = (
        pathlib.Path(args.regime_recommendation_out_file).resolve()
        if args.regime_recommendation_out_file
        else (out_dir / "regime-conditional-recommendations.json")
    )
    thresholds_path = pathlib.Path(args.thresholds_json).resolve()
    conformal_path = pathlib.Path(args.conformal_file).resolve() if args.conformal_file else (out_dir / "conformal-summary.json")

    steps: list[dict[str, Any]] = []
    warnings: list[str] = []

    def run_step(
        label: str,
        func: Any,
        step_args: argparse.Namespace,
        *,
        optional: bool = False,
    ) -> bool:
        try:
            rc = int(func(step_args))
            if rc != 0:
                raise RuntimeError(f"exit_code={rc}")
            steps.append({"step": label, "status": "ok"})
            return True
        except Exception as exc:
            steps.append({"step": label, "status": "error", "error": str(exc)})
            if optional and not args.strict:
                warnings.append(f"{label} failed but strict mode is disabled: {exc}")
                return False
            raise

    collect_mode = args.collect_mode
    if collect_mode == "auto":
        collect_mode = "snapshot" if (out_dir / "fingerprint.json").exists() else "start"

    collect_args = argparse.Namespace(
        run_id=args.run_id,
        output_dir=args.output_dir,
        base_url=args.base_url,
        repo_root=args.repo_root,
        env_path=args.env_path,
        notes=args.notes,
        strategy_mode_hint=args.strategy_mode_hint,
    )
    if collect_mode == "start":
        run_step("collect:start+snapshot", command_start, collect_args)
    else:
        run_step("collect:snapshot", command_snapshot, collect_args)

    report_args = argparse.Namespace(
        run_id=args.run_id,
        output_dir=args.output_dir,
        repo_root=args.repo_root,
        registry_dir=args.registry_dir,
        thresholds_file="",
        decision_file="",
        decision_tag=[],
        no_registry_update=True,
    )
    run_step("analyze:report", command_report, report_args)

    if args.run_execution_drag_attribution:
        execution_drag_args = argparse.Namespace(
            run_id=args.run_id,
            output_dir=args.output_dir,
            out_file=str(execution_drag_out_file),
            spread_bps=args.execution_drag_spread_bps,
            entry_delay_secs=args.execution_drag_entry_delay_secs,
            delay_bps_per_second=args.execution_drag_delay_bps_per_second,
            rejection_edge_bps=args.execution_drag_rejection_edge_bps,
        )
        run_step(
            "analyze:execution-drag-attribution",
            command_execution_drag_attribution,
            execution_drag_args,
            optional=True,
        )
    else:
        steps.append(
            {
                "step": "analyze:execution-drag-attribution",
                "status": "skipped",
                "reason": "flag not enabled",
            }
        )

    microstructure_args = argparse.Namespace(
        run_id=args.run_id,
        output_dir=args.output_dir,
        snapshots_dir=str(out_dir),
        out_file=str(microstructure_out_file),
    )
    run_step("analyze:microstructure-imbalance-scorer", command_microstructure_imbalance_scorer, microstructure_args, optional=True)

    toxic_flow_args = argparse.Namespace(
        run_id=args.run_id,
        output_dir=args.output_dir,
        snapshots_dir=str(out_dir),
        report_file=str(out_dir / "report.json"),
        rejections_file=args.rejections_file,
        microstructure_file=str(microstructure_out_file),
        execution_drag_file=str(execution_drag_out_file),
        out_file=str(toxic_flow_out_file),
    )
    run_step("analyze:toxic-flow-advisor", command_toxic_flow_advisor, toxic_flow_args, optional=True)

    if args.run_market_category_heatmap:
        market_category_heatmap_args = argparse.Namespace(
            run_id=args.run_id,
            output_dir=args.output_dir,
            out_file=str(market_category_heatmap_out_file),
        )
        run_step(
            "analyze:market-category-heatmap",
            command_market_category_heatmap,
            market_category_heatmap_args,
            optional=True,
        )
    else:
        steps.append(
            {
                "step": "analyze:market-category-heatmap",
                "status": "skipped",
                "reason": "flag not enabled",
            }
        )

    drift_required = bool(args.baseline_json) or args.require_drift_matrix
    if args.baseline_json:
        drift_args = argparse.Namespace(
            run_id=args.run_id,
            output_dir=args.output_dir,
            baseline_json=args.baseline_json,
            out_file=str(drift_out_file),
        )
        run_step("analyze:drift-matrix", command_drift_matrix, drift_args, optional=not drift_required)
    else:
        steps.append({"step": "analyze:drift-matrix", "status": "skipped", "reason": "no baseline provided"})

    if args.walkforward_input:
        walkforward_args = argparse.Namespace(
            input=args.walkforward_input,
            output=str(walkforward_out_file),
            input_format=args.walkforward_input_format,
            timestamp_col=args.walkforward_timestamp_col,
            id_col=args.walkforward_id_col,
            n_splits=args.walkforward_n_splits,
            test_size=args.walkforward_test_size,
            min_train_size=args.walkforward_min_train_size,
            purge_size=args.walkforward_purge_size,
            embargo_size=args.walkforward_embargo_size,
            train_policy=args.walkforward_train_policy,
            train_window_size=args.walkforward_train_window_size,
            metric_col=args.walkforward_metric_col,
            target_col=args.walkforward_target_col,
            prediction_col=args.walkforward_prediction_col,
            include_indices=bool(args.walkforward_include_indices),
        )
        run_step("analyze:purged-walkforward", command_walkforward, walkforward_args, optional=not args.strict)
    else:
        steps.append({"step": "analyze:purged-walkforward", "status": "skipped", "reason": "no walkforward input"})

    integrity_args = argparse.Namespace(
        run_id=args.run_id,
        output_dir=args.output_dir,
        out_file=str(artifact_integrity_out_file),
        require_drift_matrix=drift_required,
        require_conformal_summary=args.require_conformal_summary,
    )
    run_step("analyze:artifact-integrity", command_artifact_integrity, integrity_args)

    decision_args = argparse.Namespace(
        run_id=args.run_id,
        output_dir=args.output_dir,
        thresholds_json=str(thresholds_path),
        report_file=str(out_dir / "report.json"),
        drift_file=str(drift_out_file),
        conformal_file=str(conformal_path),
        out_file=str(decision_out_file),
    )
    run_step("decide:decision-eval", command_decision_eval, decision_args)

    confidence_args = argparse.Namespace(
        run_id=args.run_id,
        output_dir=args.output_dir,
        decision_file=str(decision_out_file),
        report_file=str(out_dir / "report.json"),
        drift_file=str(drift_out_file),
        conformal_file=str(conformal_path),
        walkforward_file=str(walkforward_out_file),
        out_file=str(confidence_out_file),
    )
    run_step("decide:recommendation-confidence-v2", command_recommendation_confidence_v2, confidence_args, optional=True)

    if args.run_anomaly_response_automation:
        anomaly_response_args = argparse.Namespace(
            run_id=args.run_id,
            output_dir=args.output_dir,
            drift_file=str(drift_out_file),
            toxic_flow_file=str(toxic_flow_out_file),
            integrity_file=str(artifact_integrity_out_file),
            confidence_file=str(confidence_out_file),
            decision_file=str(decision_out_file),
            out_file=str(anomaly_response_out_file),
            confidence_collapse_threshold=args.anomaly_confidence_collapse_threshold,
            drift_fill_rate_critical_pp=args.anomaly_drift_fill_rate_critical_pp,
            drift_slippage_critical_bps=args.anomaly_drift_slippage_critical_bps,
            drift_distribution_critical_l1=args.anomaly_drift_distribution_critical_l1,
            integrity_required_failure_threshold=args.anomaly_integrity_required_failure_threshold,
            integrity_total_failure_threshold=args.anomaly_integrity_total_failure_threshold,
            fail_on_severity=args.anomaly_fail_on_severity,
        )
        run_step(
            "analyze:anomaly-response-automation",
            command_anomaly_response_automation,
            anomaly_response_args,
            optional=not args.strict,
        )
    else:
        steps.append(
            {
                "step": "analyze:anomaly-response-automation",
                "status": "skipped",
                "reason": "flag not enabled",
            }
        )

    if args.run_bandit_allocation:
        bandit_report_files = list(args.bandit_report_file)
        bandit_report_files.append(str(out_dir / "report.json"))
        bandit_args = argparse.Namespace(
            eval_root=args.bandit_eval_root or args.output_dir,
            report_file=bandit_report_files,
            algorithm=args.bandit_algorithm,
            alpha=args.bandit_alpha,
            epsilon=args.bandit_epsilon,
            ridge_lambda=args.bandit_ridge_lambda,
            allocation_temperature=args.bandit_allocation_temperature,
            min_arm_prob=args.bandit_min_arm_prob,
            max_arm_prob=args.bandit_max_arm_prob,
            max_aggressive_prob=args.bandit_max_aggressive_prob,
            quality_floor=args.bandit_quality_floor,
            synthetic_steps=args.bandit_synthetic_steps,
            no_fallback_synthetic=bool(args.bandit_no_fallback_synthetic),
            seed=args.bandit_seed,
            ips_weight_cap=args.bandit_ips_weight_cap,
            logging_propensity_floor=args.bandit_logging_propensity_floor,
            out_dir=str(bandit_out_dir),
            summary_file=args.bandit_summary_file,
            history_json=args.bandit_history_json,
            history_csv=args.bandit_history_csv,
        )
        run_step("decide:bandit-allocation", command_bandit_allocation, bandit_args, optional=True)
    else:
        steps.append({"step": "decide:bandit-allocation", "status": "skipped", "reason": "flag not enabled"})

    if args.run_regime_conditional_recommender:
        regime_file = pathlib.Path(args.regime_file).resolve() if args.regime_file else (out_dir / "regimes" / "regime-summary.json")
        if regime_file.exists():
            regime_recommender_args = argparse.Namespace(
                run_id=args.run_id,
                output_dir=args.output_dir,
                regime_file=str(regime_file),
                report_file=str(out_dir / "report.json"),
                drift_file=str(drift_out_file),
                rejections_file=args.rejections_file,
                snapshots_dir=args.regime_snapshots_dir or str(out_dir),
                out_json=str(regime_recommendation_out_file),
            )
            run_step(
                "decide:regime-conditional-recommender",
                command_regime_conditional_recommender,
                regime_recommender_args,
                optional=True,
            )
        else:
            steps.append(
                {
                    "step": "decide:regime-conditional-recommender",
                    "status": "skipped",
                    "reason": f"regime file not found: {regime_file}",
                }
            )
    else:
        steps.append(
            {
                "step": "decide:regime-conditional-recommender",
                "status": "skipped",
                "reason": "flag not enabled",
            }
        )

    registry_record_path: pathlib.Path | None = None
    if args.registry_upsert:
        registry_args = argparse.Namespace(
            run_id=args.run_id,
            output_dir=args.output_dir,
            repo_root=args.repo_root,
            registry_dir=args.registry_dir,
            thresholds_file=str(thresholds_path),
            decision_file=str(decision_out_file),
            decision_tag=args.decision_tag,
        )
        run_step("packet:registry-upsert", command_registry_upsert, registry_args, optional=False)
        registry_record_path = pathlib.Path(args.registry_dir).resolve() / "runs" / f"{safe_run_id_filename(args.run_id)}.json"
    else:
        steps.append({"step": "packet:registry-upsert", "status": "skipped", "reason": "flag not enabled"})

    if args.generate_signoff_packet:
        signoff_args = argparse.Namespace(
            run_id=args.run_id,
            output_dir=args.output_dir,
            repo_root=args.repo_root,
            decision_file=str(decision_out_file),
            recommendation_file=str(regime_recommendation_out_file),
            confidence_file=str(confidence_out_file),
            integrity_file=str(artifact_integrity_out_file),
            thresholds_schema_path=args.signoff_thresholds_schema_path,
            rollback_playbook_path=args.signoff_rollback_playbook_path,
            rollback_helper_path=args.signoff_rollback_helper_path,
            out_file=str(signoff_packet_out_file),
            environment=args.signoff_environment,
            recorded_at_utc=args.signoff_recorded_at_utc,
            primary_operator_name=args.signoff_primary_operator_name,
            primary_operator_signed_at_utc=args.signoff_primary_operator_signed_at_utc,
            secondary_reviewer_name=args.signoff_secondary_reviewer_name,
            secondary_reviewer_signed_at_utc=args.signoff_secondary_reviewer_signed_at_utc,
            rollback_preview_verified=bool(args.signoff_rollback_preview_verified),
            target_mode_verified=bool(args.signoff_target_mode_verified),
            prior_decision_reviewed=bool(args.signoff_prior_decision_reviewed),
            decision_dimensions_reviewed=bool(args.signoff_decision_dimensions_reviewed),
            rollback_executed=bool(args.signoff_rollback_executed),
            rollback_reference=args.signoff_rollback_reference,
            confidence_gate_min=args.signoff_confidence_gate_min,
            enforce_confidence_gate=bool(args.signoff_enforce_confidence_gate),
            notes=args.signoff_notes,
        )
        run_step("packet:signoff-packet", command_signoff_packet, signoff_args, optional=False)
    else:
        steps.append({"step": "packet:signoff-packet", "status": "skipped", "reason": "flag not enabled"})

    if args.generate_rollback_packet:
        rollback_args = argparse.Namespace(
            run_id=args.run_id,
            output_dir=args.output_dir,
            repo_root=args.repo_root,
            decision_file=str(decision_out_file),
            integrity_file=str(artifact_integrity_out_file),
            confidence_file=str(confidence_out_file),
            toxic_flow_file=str(toxic_flow_out_file),
            signoff_file=str(signoff_packet_out_file),
            report_file=str(out_dir / "report.json"),
            rollback_playbook_path=args.rollback_packet_playbook_path,
            rollback_helper_path=args.rollback_packet_helper_path,
            out_file=str(rollback_packet_out_file),
            generated_at_utc=args.rollback_packet_generated_at_utc,
            confidence_gate_min=args.rollback_packet_confidence_gate_min,
            fail_on_not_ready=bool(args.rollback_packet_fail_on_not_ready),
        )
        run_step("packet:rollback-packet", command_rollback_packet, rollback_args, optional=False)
    else:
        steps.append({"step": "packet:rollback-packet", "status": "skipped", "reason": "flag not enabled"})

    if args.generate_production_readiness_gate:
        production_readiness_args = argparse.Namespace(
            run_id=args.run_id,
            output_dir=args.output_dir,
            repo_root=args.repo_root,
            decision_file=str(decision_out_file),
            integrity_file=str(artifact_integrity_out_file),
            confidence_file=str(confidence_out_file),
            signoff_file=str(signoff_packet_out_file),
            rollback_packet_file=str(rollback_packet_out_file),
            anomaly_response_file=str(anomaly_response_out_file),
            thresholds_json=str(thresholds_path),
            confidence_gate_min=args.production_readiness_confidence_gate_min,
            generated_at_utc=args.production_readiness_generated_at_utc,
            require_stamped_policy=bool(args.production_readiness_require_stamped_policy),
            strict_policy_version=bool(args.production_readiness_strict_policy_version),
            fail_on_non_go=bool(args.production_readiness_fail_on_non_go),
            out_file=str(production_readiness_out_file),
        )
        run_step(
            "packet:production-readiness-gate",
            command_production_readiness_gate,
            production_readiness_args,
            optional=False,
        )
    else:
        steps.append({"step": "packet:production-readiness-gate", "status": "skipped", "reason": "flag not enabled"})

    decision_payload = optional_json_object(decision_out_file) or {}
    confidence_payload = optional_json_object(confidence_out_file) or {}
    execution_drag_payload = optional_json_object(execution_drag_out_file) or {}
    integrity_payload = optional_json_object(artifact_integrity_out_file) or {}
    signoff_payload = optional_json_object(signoff_packet_out_file) or {}
    rollback_packet_payload = optional_json_object(rollback_packet_out_file) or {}
    production_readiness_payload = optional_json_object(production_readiness_out_file) or {}
    microstructure_payload = optional_json_object(microstructure_out_file) or {}
    toxic_flow_payload = optional_json_object(toxic_flow_out_file) or {}
    anomaly_response_payload = optional_json_object(anomaly_response_out_file) or {}
    market_category_heatmap_payload = optional_json_object(market_category_heatmap_out_file) or {}
    latest_snapshot = latest_snapshot_path(out_dir)

    outputs = {
        "output_dir": to_relative_path(out_dir, repo_root),
        "fingerprint": to_relative_path(out_dir / "fingerprint.json", repo_root),
        "latest_snapshot": to_relative_path(latest_snapshot, repo_root) if latest_snapshot else None,
        "funnel_rollup": to_relative_path(out_dir / "funnel-rollup.json", repo_root),
        "report": to_relative_path(out_dir / "report.json", repo_root),
        "microstructure_imbalance": to_relative_path(microstructure_out_file, repo_root)
        if microstructure_out_file.exists()
        else None,
        "toxic_flow_advisor": to_relative_path(toxic_flow_out_file, repo_root) if toxic_flow_out_file.exists() else None,
        "anomaly_response_plan": to_relative_path(anomaly_response_out_file, repo_root)
        if anomaly_response_out_file.exists()
        else None,
        "market_category_heatmap": to_relative_path(market_category_heatmap_out_file, repo_root)
        if market_category_heatmap_out_file.exists()
        else None,
        "drift_matrix": to_relative_path(drift_out_file, repo_root) if drift_out_file.exists() else None,
        "purged_walkforward": to_relative_path(walkforward_out_file, repo_root) if walkforward_out_file.exists() else None,
        "artifact_integrity": to_relative_path(artifact_integrity_out_file, repo_root),
        "decision": to_relative_path(decision_out_file, repo_root),
        "recommendation_confidence_v2": to_relative_path(confidence_out_file, repo_root)
        if confidence_out_file.exists()
        else None,
        "signoff_packet": to_relative_path(signoff_packet_out_file, repo_root) if signoff_packet_out_file.exists() else None,
        "rollback_packet": to_relative_path(rollback_packet_out_file, repo_root) if rollback_packet_out_file.exists() else None,
        "production_readiness_gate": to_relative_path(production_readiness_out_file, repo_root)
        if production_readiness_out_file.exists()
        else None,
        "execution_drag_attribution": to_relative_path(execution_drag_out_file, repo_root)
        if execution_drag_out_file.exists()
        else None,
        "bandit_allocation_summary": to_relative_path(bandit_out_dir / args.bandit_summary_file, repo_root)
        if (bandit_out_dir / args.bandit_summary_file).exists()
        else None,
        "bandit_allocation_history_json": to_relative_path(bandit_out_dir / args.bandit_history_json, repo_root)
        if (bandit_out_dir / args.bandit_history_json).exists()
        else None,
        "bandit_allocation_history_csv": to_relative_path(bandit_out_dir / args.bandit_history_csv, repo_root)
        if (bandit_out_dir / args.bandit_history_csv).exists()
        else None,
        "regime_conditional_recommendations": to_relative_path(regime_recommendation_out_file, repo_root)
        if regime_recommendation_out_file.exists()
        else None,
        "registry_record": to_relative_path(registry_record_path, repo_root)
        if registry_record_path and registry_record_path.exists()
        else None,
    }

    summary = {
        "run_id": args.run_id,
        "status": "ok",
        "strict": bool(args.strict),
        "decision": decision_payload.get("decision"),
        "recommendation_confidence_v2": {
            "score_normalized": confidence_payload.get("confidence_score_normalized"),
            "score_percent": confidence_payload.get("confidence_score_percent"),
            "level": confidence_payload.get("confidence_level"),
            "availability_factor": confidence_payload.get("artifact_availability_factor"),
        },
        "execution_drag_attribution": {
            "enabled": bool(args.run_execution_drag_attribution),
            "total_drag_usdc": execution_drag_payload.get("aggregate", {}).get("total_drag_usdc")
            if isinstance(execution_drag_payload.get("aggregate"), dict)
            else None,
        },
        "microstructure_imbalance": {
            "token_count": (microstructure_payload.get("summary") or {}).get("token_count")
            if isinstance(microstructure_payload.get("summary"), dict)
            else None,
            "high_risk_tokens": (microstructure_payload.get("summary") or {}).get("high_risk_tokens")
            if isinstance(microstructure_payload.get("summary"), dict)
            else None,
            "max_risk_score": (microstructure_payload.get("summary") or {}).get("max_risk_score")
            if isinstance(microstructure_payload.get("summary"), dict)
            else None,
        },
        "toxic_flow_advisor": {
            "score_normalized": toxic_flow_payload.get("toxic_flow_score_normalized"),
            "score_percent": toxic_flow_payload.get("toxic_flow_score_percent"),
            "severity_tier": toxic_flow_payload.get("severity_tier"),
            "operator_signoff_status": toxic_flow_payload.get("operator_signoff_status"),
            "recommended_actions": len(toxic_flow_payload.get("guardrail_recommendations", {}).get("actions", []))
            if isinstance(toxic_flow_payload.get("guardrail_recommendations"), dict)
            and isinstance(toxic_flow_payload.get("guardrail_recommendations", {}).get("actions"), list)
            else None,
        },
        "anomaly_response_automation": {
            "enabled": bool(args.run_anomaly_response_automation),
            "highest_severity": (anomaly_response_payload.get("summary") or {}).get("highest_severity")
            if isinstance(anomaly_response_payload.get("summary"), dict)
            else None,
            "escalation_channel": (anomaly_response_payload.get("summary") or {}).get("escalation_channel")
            if isinstance(anomaly_response_payload.get("summary"), dict)
            else None,
            "triggered_conditions": (anomaly_response_payload.get("summary") or {}).get("triggered_conditions")
            if isinstance(anomaly_response_payload.get("summary"), dict)
            else None,
            "actions": len(anomaly_response_payload.get("automation_safe_action_list", []))
            if isinstance(anomaly_response_payload.get("automation_safe_action_list"), list)
            else None,
        },
        "market_category_heatmap": {
            "enabled": bool(args.run_market_category_heatmap),
            "categories_total": (market_category_heatmap_payload.get("summary") or {}).get("categories_total")
            if isinstance(market_category_heatmap_payload.get("summary"), dict)
            else None,
            "trades_total": (market_category_heatmap_payload.get("summary") or {}).get("trades_total")
            if isinstance(market_category_heatmap_payload.get("summary"), dict)
            else None,
            "rejections_total": (market_category_heatmap_payload.get("summary") or {}).get("rejections_total")
            if isinstance(market_category_heatmap_payload.get("summary"), dict)
            else None,
        },
        "regime_conditional_recommender": {
            "enabled": bool(args.run_regime_conditional_recommender),
            "regime_count": (optional_json_object(regime_recommendation_out_file) or {}).get("regime_count"),
        },
        "bandit_allocation": {
            "enabled": bool(args.run_bandit_allocation),
            "summary_path": outputs.get("bandit_allocation_summary"),
        },
        "artifact_integrity": integrity_payload.get("overall_status"),
        "signoff_packet": {
            "enabled": bool(args.generate_signoff_packet),
            "promotion_allowed": signoff_payload.get("promotion_allowed"),
            "artifact_gate": ((signoff_payload.get("gate_statuses", {}) or {}).get("artifact_gate", {}) or {}).get("status")
            if isinstance(signoff_payload.get("gate_statuses"), dict)
            else None,
            "decision_gate": ((signoff_payload.get("gate_statuses", {}) or {}).get("decision_gate", {}) or {}).get("status")
            if isinstance(signoff_payload.get("gate_statuses"), dict)
            else None,
            "post_run_gate": ((signoff_payload.get("gate_statuses", {}) or {}).get("post_run_gate", {}) or {}).get("status")
            if isinstance(signoff_payload.get("gate_statuses"), dict)
            else None,
        },
        "rollback_packet": {
            "enabled": bool(args.generate_rollback_packet),
            "readiness_status": (rollback_packet_payload.get("readiness", {}) or {}).get("status")
            if isinstance(rollback_packet_payload.get("readiness"), dict)
            else None,
            "trigger_count": len(rollback_packet_payload.get("rollback_context", {}).get("triggers", []))
            if isinstance(rollback_packet_payload.get("rollback_context"), dict)
            and isinstance((rollback_packet_payload.get("rollback_context", {}) or {}).get("triggers"), list)
            else None,
            "failed_gate_count": len(rollback_packet_payload.get("rollback_context", {}).get("failed_gates", []))
            if isinstance(rollback_packet_payload.get("rollback_context"), dict)
            and isinstance((rollback_packet_payload.get("rollback_context", {}) or {}).get("failed_gates"), list)
            else None,
            "missing_required_artifacts": len(rollback_packet_payload.get("missing_artifact_diagnostics", []))
            if isinstance(rollback_packet_payload.get("missing_artifact_diagnostics"), list)
            else None,
        },
        "production_readiness_gate": {
            "enabled": bool(args.generate_production_readiness_gate),
            "verdict": (production_readiness_payload.get("readiness_verdict", {}) or {}).get("status")
            if isinstance(production_readiness_payload.get("readiness_verdict"), dict)
            else None,
            "hard_failures": len((production_readiness_payload.get("readiness_verdict", {}) or {}).get("hard_gate_failures", []))
            if isinstance((production_readiness_payload.get("readiness_verdict", {}) or {}).get("hard_gate_failures"), list)
            else None,
            "soft_failures": len((production_readiness_payload.get("readiness_verdict", {}) or {}).get("soft_gate_failures", []))
            if isinstance((production_readiness_payload.get("readiness_verdict", {}) or {}).get("soft_gate_failures"), list)
            else None,
            "missing_required_artifacts": len(production_readiness_payload.get("missing_artifact_diagnostics", []))
            if isinstance(production_readiness_payload.get("missing_artifact_diagnostics"), list)
            else None,
        },
        "outputs": outputs,
        "steps": steps,
        "warnings": warnings,
    }
    print(json.dumps(summary, sort_keys=True))
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Blink 24h evaluation helper: fingerprint, snapshots, and report."
    )
    parser.add_argument("--run-id", default="", help="Unique run identifier, e.g. 2026-04-19-paper-a")
    parser.add_argument(
        "--base-url",
        default="http://127.0.0.1:3030",
        help="Blink API base URL (default: http://127.0.0.1:3030)",
    )
    parser.add_argument(
        "--output-dir",
        default="logs\\eval-cycle",
        help="Artifact root; run files go to <output-dir>\\\\<run-id> (default: logs\\\\eval-cycle)",
    )

    sub = parser.add_subparsers(dest="command", required=True)

    start = sub.add_parser("start", help="Create run fingerprint and capture first snapshot")
    start.add_argument("--repo-root", default=".", help="Repo root for git SHA lookup")
    start.add_argument("--env-path", default=".env", help="Path to env file used for run")
    start.add_argument("--notes", default="", help="Optional free-text notes")
    start.add_argument("--strategy-mode-hint", default="", help="Expected strategy mode for this run")
    start.set_defaults(func=command_start)

    snap = sub.add_parser("snapshot", help="Capture one API snapshot")
    snap.set_defaults(func=command_snapshot)

    report = sub.add_parser("report", help="Aggregate snapshots into report.json")
    report.add_argument("--repo-root", default=".", help="Repo root used for relative artifact links")
    report.add_argument(
        "--registry-dir",
        default="artifacts\\experiment-registry",
        help="Registry directory (default: artifacts\\\\experiment-registry)",
    )
    report.add_argument("--thresholds-file", default="", help="Optional thresholds file path")
    report.add_argument("--decision-file", default="", help="Optional decision output file path")
    report.add_argument(
        "--decision-tag",
        action="append",
        default=[],
        help="Decision tag to attach (repeatable)",
    )
    report.add_argument(
        "--no-registry-update",
        action="store_true",
        help="Do not upsert into artifact registry after generating report",
    )
    report.set_defaults(func=command_report)

    registry_upsert = sub.add_parser(
        "registry-upsert",
        help="Upsert run artifacts into JSONL-backed experiment registry",
    )
    registry_upsert.add_argument("--repo-root", default=".", help="Repo root used for relative artifact links")
    registry_upsert.add_argument(
        "--registry-dir",
        default="artifacts\\experiment-registry",
        help="Registry directory (default: artifacts\\\\experiment-registry)",
    )
    registry_upsert.add_argument("--thresholds-file", default="", help="Optional thresholds file path")
    registry_upsert.add_argument("--decision-file", default="", help="Optional decision output file path")
    registry_upsert.add_argument(
        "--decision-tag",
        action="append",
        default=[],
        help="Decision tag to attach (repeatable)",
    )
    registry_upsert.set_defaults(func=command_registry_upsert)

    registry_query = sub.add_parser(
        "registry-query",
        help="Query latest records from experiment registry",
    )
    registry_query.add_argument(
        "--registry-dir",
        default="artifacts\\experiment-registry",
        help="Registry directory (default: artifacts\\\\experiment-registry)",
    )
    registry_query.add_argument("--tag", action="append", default=[], help="Filter by decision tag (repeatable)")
    registry_query.add_argument("--limit", type=int, default=20, help="Max records to return (default: 20)")
    registry_query.set_defaults(func=command_registry_query)

    drift = sub.add_parser(
        "drift-matrix",
        help="Compare run artifacts against backtest baseline JSON and write drift-matrix.json",
    )
    drift.add_argument(
        "--baseline-json",
        required=True,
        help="Path to baseline JSON containing fill/slippage/reject/notional/category metrics",
    )
    drift.add_argument(
        "--out-file",
        default="",
        help="Optional explicit output file path (default: <output-dir>\\<run-id>\\drift-matrix.json)",
    )
    drift.set_defaults(func=command_drift_matrix)

    execution_drag = sub.add_parser(
        "execution-drag-attribution",
        help="Decompose execution drag into spread/slippage/fees/delay/rejections components",
    )
    execution_drag.add_argument(
        "--out-file",
        default="",
        help="Optional explicit output file path (default: <output-dir>\\<run-id>\\execution-drag-attribution.json)",
    )
    execution_drag.add_argument(
        "--spread-bps",
        type=float,
        default=None,
        help="Optional spread proxy override in bps (default from PAPER_ADVERSE_FILL_BPS or 10)",
    )
    execution_drag.add_argument(
        "--entry-delay-secs",
        type=float,
        default=None,
        help="Optional entry delay override in seconds (default from ENTRY_DELAY_SECS or 0)",
    )
    execution_drag.add_argument(
        "--delay-bps-per-second",
        type=float,
        default=2.0,
        help="Opportunity-cost bps applied per second of configured entry delay (default: 2.0)",
    )
    execution_drag.add_argument(
        "--rejection-edge-bps",
        type=float,
        default=50.0,
        help="Opportunity-cost edge assumption for rejected notional (default: 50bps)",
    )
    execution_drag.set_defaults(func=command_execution_drag_attribution)

    microstructure = sub.add_parser(
        "microstructure-imbalance-scorer",
        help="Score token/market microstructure imbalance risk from eval snapshots",
    )
    microstructure.add_argument(
        "--snapshots-dir",
        default="",
        help="Optional snapshots directory override (default: <output-dir>\\<run-id>)",
    )
    microstructure.add_argument(
        "--out-file",
        default="",
        help="Optional explicit output file path (default: <output-dir>\\<run-id>\\microstructure-imbalance.json)",
    )
    microstructure.set_defaults(func=command_microstructure_imbalance_scorer)

    toxic_flow = sub.add_parser(
        "toxic-flow-advisor",
        help="Score adverse-selection/toxic-flow risk and emit guardrail recommendations",
    )
    toxic_flow.add_argument(
        "--snapshots-dir",
        default="",
        help="Optional snapshots directory override (default: <output-dir>\\<run-id>)",
    )
    toxic_flow.add_argument(
        "--report-file",
        default="",
        help="Optional report artifact path (default: <output-dir>\\<run-id>\\report.json)",
    )
    toxic_flow.add_argument(
        "--rejections-file",
        default="",
        help="Optional rejections artifact path (default: <output-dir>\\<run-id>\\rejections.json)",
    )
    toxic_flow.add_argument(
        "--microstructure-file",
        default="",
        help="Optional microstructure artifact path (default: <output-dir>\\<run-id>\\microstructure-imbalance.json)",
    )
    toxic_flow.add_argument(
        "--execution-drag-file",
        default="",
        help="Optional execution drag artifact path (default: <output-dir>\\<run-id>\\execution-drag-attribution.json)",
    )
    toxic_flow.add_argument(
        "--out-file",
        default="",
        help="Optional explicit output file path (default: <output-dir>\\<run-id>\\toxic-flow-advisor.json)",
    )
    toxic_flow.set_defaults(func=command_toxic_flow_advisor)

    anomaly_response = sub.add_parser(
        "anomaly-response-automation",
        help="Detect eval anomaly conditions and emit deterministic response plans",
    )
    anomaly_response.add_argument(
        "--drift-file",
        default="",
        help="Optional drift artifact path (default: <output-dir>\\<run-id>\\drift-matrix.json)",
    )
    anomaly_response.add_argument(
        "--toxic-flow-file",
        default="",
        help="Optional toxic-flow-advisor path (default: <output-dir>\\<run-id>\\toxic-flow-advisor.json)",
    )
    anomaly_response.add_argument(
        "--integrity-file",
        default="",
        help="Optional artifact-integrity path (default: <output-dir>\\<run-id>\\artifact-integrity.json)",
    )
    anomaly_response.add_argument(
        "--confidence-file",
        default="",
        help="Optional recommendation-confidence-v2 path (default: <output-dir>\\<run-id>\\recommendation-confidence-v2.json)",
    )
    anomaly_response.add_argument(
        "--decision-file",
        default="",
        help="Optional decision path (default: <output-dir>\\<run-id>\\decision.json)",
    )
    anomaly_response.add_argument(
        "--out-file",
        default="",
        help="Optional explicit output file path (default: <output-dir>\\<run-id>\\anomaly-response-plan.json)",
    )
    anomaly_response.add_argument(
        "--confidence-collapse-threshold",
        type=float,
        default=0.35,
        help="Trigger confidence-collapse condition when confidence score is at or below this value (default: 0.35)",
    )
    anomaly_response.add_argument(
        "--drift-fill-rate-critical-pp",
        type=float,
        default=-15.0,
        help="Critical fill-rate drift threshold in percentage points (default: -15.0)",
    )
    anomaly_response.add_argument(
        "--drift-slippage-critical-bps",
        type=float,
        default=120.0,
        help="Critical slippage drift threshold in bps (default: 120.0)",
    )
    anomaly_response.add_argument(
        "--drift-distribution-critical-l1",
        type=float,
        default=0.75,
        help="Critical L1 drift threshold for reject/notional/category distributions (default: 0.75)",
    )
    anomaly_response.add_argument(
        "--integrity-required-failure-threshold",
        type=int,
        default=2,
        help="Required-failure count threshold for repeated integrity failures (default: 2)",
    )
    anomaly_response.add_argument(
        "--integrity-total-failure-threshold",
        type=int,
        default=4,
        help="Total check-failure threshold for repeated integrity failures (default: 4)",
    )
    anomaly_response.add_argument(
        "--fail-on-severity",
        choices=("none", "high", "critical"),
        default="none",
        help="Optional non-zero exit gate when computed highest severity meets/exceeds threshold",
    )
    anomaly_response.set_defaults(func=command_anomaly_response_automation)

    market_category_heatmap = sub.add_parser(
        "market-category-heatmap",
        help="Aggregate deterministic market-category heatmap analytics from eval artifacts",
    )
    market_category_heatmap.add_argument(
        "--out-file",
        default="",
        help="Optional explicit output file path (default: <output-dir>\\<run-id>\\market-category-heatmap.json)",
    )
    market_category_heatmap.set_defaults(func=command_market_category_heatmap)

    walkforward = sub.add_parser(
        "walkforward",
        help="Run deterministic purged walk-forward splitting and emit fold metrics JSON",
    )
    walkforward.add_argument("--input", required=True, help="Input dataset path (csv/json/jsonl)")
    walkforward.add_argument(
        "--output",
        default="logs\\eval-cycle\\purged-walkforward.json",
        help="Output JSON path",
    )
    walkforward.add_argument(
        "--input-format",
        choices=("auto", "csv", "json", "jsonl"),
        default="auto",
        help="Input format (default: auto by extension)",
    )
    walkforward.add_argument("--timestamp-col", default="timestamp", help="Timestamp column")
    walkforward.add_argument("--id-col", default="", help="Optional deterministic tiebreak column")
    walkforward.add_argument("--n-splits", type=int, default=5, help="Number of folds")
    walkforward.add_argument("--test-size", type=int, default=None, help="Rows per test fold")
    walkforward.add_argument("--min-train-size", type=int, default=20, help="Minimum rows in each train fold")
    walkforward.add_argument("--purge-size", type=int, default=0, help="Rows purged around test interval")
    walkforward.add_argument("--embargo-size", type=int, default=0, help="Rows embargoed after test interval")
    walkforward.add_argument(
        "--train-policy",
        choices=("expanding", "rolling", "both-sides"),
        default="expanding",
        help="Train policy (default: expanding)",
    )
    walkforward.add_argument("--train-window-size", type=int, default=None, help="Required for rolling policy")
    walkforward.add_argument(
        "--metric-col",
        action="append",
        default=[],
        help="Repeatable numeric column for fold means",
    )
    walkforward.add_argument("--target-col", default="", help="Optional target column for fold quality metrics")
    walkforward.add_argument("--prediction-col", default="", help="Optional prediction column for fold quality metrics")
    walkforward.add_argument("--include-indices", action="store_true", help="Include full index arrays in output")
    walkforward.set_defaults(func=command_walkforward)

    artifact_integrity = sub.add_parser(
        "artifact-integrity",
        help="Validate run artifacts completeness/schema before decision synthesis",
    )
    artifact_integrity.add_argument(
        "--out-file",
        default="",
        help="Optional explicit output file path (default: <output-dir>\\<run-id>\\artifact-integrity.json)",
    )
    artifact_integrity.add_argument(
        "--require-drift-matrix",
        action="store_true",
        help="Treat drift-matrix.json as a required artifact",
    )
    artifact_integrity.add_argument(
        "--require-conformal-summary",
        action="store_true",
        help="Treat conformal-summary.json as a required artifact",
    )
    artifact_integrity.set_defaults(func=command_artifact_integrity)

    decision_eval = sub.add_parser(
        "decision-eval",
        help="Evaluate GO/TUNE/ROLLBACK using report/drift/conformal artifacts",
    )
    decision_eval.add_argument(
        "--thresholds-json",
        default="tools\\examples\\decision-thresholds.json",
        help="Path to decision threshold schema JSON",
    )
    decision_eval.add_argument(
        "--report-file",
        default="",
        help="Optional report artifact path (default: <output-dir>\\<run-id>\\report.json)",
    )
    decision_eval.add_argument(
        "--drift-file",
        default="",
        help="Optional drift artifact path (default: <output-dir>\\<run-id>\\drift-matrix.json)",
    )
    decision_eval.add_argument(
        "--conformal-file",
        default="",
        help="Optional conformal artifact path (default: <output-dir>\\<run-id>\\conformal-summary.json)",
    )
    decision_eval.add_argument(
        "--out-file",
        default="",
        help="Optional explicit output file path (default: <output-dir>\\<run-id>\\decision.json)",
    )
    decision_eval.set_defaults(func=command_decision_eval)

    threshold_policy_validate = sub.add_parser(
        "threshold-policy-validate",
        help="Validate threshold policy schema/metadata and print canonical fingerprint",
    )
    threshold_policy_validate.add_argument(
        "--thresholds-json",
        default="tools\\examples\\decision-thresholds.json",
        help="Path to decision threshold schema JSON",
    )
    threshold_policy_validate.add_argument(
        "--require-stamped",
        action="store_true",
        help="Require policy.fingerprint to be present and matching computed canonical hash",
    )
    threshold_policy_validate.add_argument(
        "--strict-policy-version",
        action="store_true",
        help="Require explicit policy.version metadata (no legacy fallback)",
    )
    threshold_policy_validate.set_defaults(func=command_threshold_policy_validate)

    threshold_policy_stamp = sub.add_parser(
        "threshold-policy-stamp",
        help="Stamp threshold policy file with canonical policy fingerprint",
    )
    threshold_policy_stamp.add_argument(
        "--thresholds-json",
        default="tools\\examples\\decision-thresholds.json",
        help="Path to decision threshold schema JSON",
    )
    threshold_policy_stamp.add_argument(
        "--out-file",
        default="",
        help="Optional explicit output path (default: overwrite --thresholds-json)",
    )
    threshold_policy_stamp.add_argument(
        "--policy-version",
        default="",
        help="Optional explicit policy.version override before stamping",
    )
    threshold_policy_stamp.add_argument(
        "--notes",
        default=None,
        help="Optional policy.notes text override",
    )
    threshold_policy_stamp.add_argument(
        "--changelog",
        default=None,
        help="Optional policy.changelog text override",
    )
    threshold_policy_stamp.set_defaults(func=command_threshold_policy_stamp)

    confidence_v2 = sub.add_parser(
        "recommendation-confidence-v2",
        help="Compute confidence v2 from decision/report/conformal/drift/walkforward artifacts",
    )
    confidence_v2.add_argument(
        "--decision-file",
        default="",
        help="Optional decision artifact path (default: <output-dir>\\<run-id>\\decision.json)",
    )
    confidence_v2.add_argument(
        "--report-file",
        default="",
        help="Optional report artifact path (default: <output-dir>\\<run-id>\\report.json)",
    )
    confidence_v2.add_argument(
        "--drift-file",
        default="",
        help="Optional drift artifact path (default: <output-dir>\\<run-id>\\drift-matrix.json)",
    )
    confidence_v2.add_argument(
        "--conformal-file",
        default="",
        help="Optional conformal artifact path (default: <output-dir>\\<run-id>\\conformal-summary.json)",
    )
    confidence_v2.add_argument(
        "--walkforward-file",
        default="",
        help="Optional walkforward artifact path (default: <output-dir>\\<run-id>\\purged-walkforward.json)",
    )
    confidence_v2.add_argument(
        "--out-file",
        default="",
        help="Optional explicit output file path (default: <output-dir>\\<run-id>\\recommendation-confidence-v2.json)",
    )
    confidence_v2.set_defaults(func=command_recommendation_confidence_v2)

    signoff_packet = sub.add_parser(
        "signoff-packet",
        help="Generate operator signoff packet from decision/recommendation/confidence/integrity artifacts",
    )
    signoff_packet.add_argument("--repo-root", default=".", help="Repo root used for relative output links")
    signoff_packet.add_argument(
        "--decision-file",
        default="",
        help="Optional decision artifact path (default: <output-dir>\\<run-id>\\decision.json)",
    )
    signoff_packet.add_argument(
        "--recommendation-file",
        default="",
        help="Optional recommendation artifact path (default: <output-dir>\\<run-id>\\regime-conditional-recommendations.json)",
    )
    signoff_packet.add_argument(
        "--confidence-file",
        default="",
        help="Optional confidence artifact path (default: <output-dir>\\<run-id>\\recommendation-confidence-v2.json)",
    )
    signoff_packet.add_argument(
        "--integrity-file",
        default="",
        help="Optional integrity artifact path (default: <output-dir>\\<run-id>\\artifact-integrity.json)",
    )
    signoff_packet.add_argument(
        "--thresholds-schema-path",
        default="tools\\examples\\decision-thresholds.json",
        help="Decision thresholds schema path included in signoff packet",
    )
    signoff_packet.add_argument(
        "--rollback-playbook-path",
        default="..\\deploy\\ROLLBACK-PLAYBOOK.md",
        help="Rollback playbook path included in signoff packet",
    )
    signoff_packet.add_argument(
        "--rollback-helper-path",
        default="..\\deploy\\rollback-hetzner.ps1",
        help="Rollback helper path included in signoff packet",
    )
    signoff_packet.add_argument(
        "--out-file",
        default="",
        help="Optional explicit output file path (default: deploy\\signoffs\\<run-id>-<utc>.json)",
    )
    signoff_packet.add_argument("--environment", default="", help="Target environment label")
    signoff_packet.add_argument("--recorded-at-utc", default="", help="Optional recorded_at_utc override")
    signoff_packet.add_argument("--primary-operator-name", default="", help="Primary signer name")
    signoff_packet.add_argument("--primary-operator-signed-at-utc", default="", help="Primary signer timestamp")
    signoff_packet.add_argument("--secondary-reviewer-name", default="", help="Secondary signer name")
    signoff_packet.add_argument("--secondary-reviewer-signed-at-utc", default="", help="Secondary signer timestamp")
    signoff_packet.add_argument(
        "--rollback-preview-verified",
        action="store_true",
        help="Mark rollback helper/playbook preview as verified for pre-run gate",
    )
    signoff_packet.add_argument(
        "--target-mode-verified",
        action="store_true",
        help="Mark target mode env controls as verified for pre-run checklist",
    )
    signoff_packet.add_argument(
        "--prior-decision-reviewed",
        action="store_true",
        help="Mark prior decision review completed for pre-run checklist",
    )
    signoff_packet.add_argument(
        "--decision-dimensions-reviewed",
        action="store_true",
        help="Mark decision dimensions reviewed for post-run checklist",
    )
    signoff_packet.add_argument(
        "--rollback-executed",
        action="store_true",
        help="Mark rollback as executed (use with --rollback-reference)",
    )
    signoff_packet.add_argument("--rollback-reference", default="", help="Rollback evidence/reference link")
    signoff_packet.add_argument(
        "--confidence-gate-min",
        type=float,
        default=0.55,
        help="Confidence gate floor for gate status reporting (default: 0.55)",
    )
    signoff_packet.add_argument(
        "--enforce-confidence-gate",
        action="store_true",
        help="Require confidence gate PASS for promotion_allowed=true",
    )
    signoff_packet.add_argument("--notes", default="", help="Optional operator notes")
    signoff_packet.set_defaults(func=command_signoff_packet)

    rollback_packet = sub.add_parser(
        "rollback-packet",
        help="Generate deterministic rollback packet from eval artifacts and rollout gates",
    )
    rollback_packet.add_argument("--repo-root", default=".", help="Repo root used for relative output links")
    rollback_packet.add_argument(
        "--decision-file",
        default="",
        help="Optional decision artifact path (default: <output-dir>\\<run-id>\\decision.json)",
    )
    rollback_packet.add_argument(
        "--integrity-file",
        default="",
        help="Optional artifact-integrity path (default: <output-dir>\\<run-id>\\artifact-integrity.json)",
    )
    rollback_packet.add_argument(
        "--confidence-file",
        default="",
        help="Optional recommendation-confidence-v2 path (default: <output-dir>\\<run-id>\\recommendation-confidence-v2.json)",
    )
    rollback_packet.add_argument(
        "--toxic-flow-file",
        default="",
        help="Optional toxic-flow-advisor path (default: <output-dir>\\<run-id>\\toxic-flow-advisor.json)",
    )
    rollback_packet.add_argument(
        "--signoff-file",
        default="",
        help="Optional signoff packet path (default: <output-dir>\\<run-id>\\signoff-packet.json)",
    )
    rollback_packet.add_argument(
        "--report-file",
        default="",
        help="Optional report artifact path (default: <output-dir>\\<run-id>\\report.json)",
    )
    rollback_packet.add_argument(
        "--rollback-playbook-path",
        default="deploy\\ROLLBACK-PLAYBOOK.md",
        help="Rollback playbook path used for packet step/checklist alignment",
    )
    rollback_packet.add_argument(
        "--rollback-helper-path",
        default="deploy\\rollback-hetzner.ps1",
        help="Rollback helper script path used for packet step/checklist alignment",
    )
    rollback_packet.add_argument(
        "--out-file",
        default="",
        help="Optional explicit output file path (default: <output-dir>\\<run-id>\\rollback-packet.json)",
    )
    rollback_packet.add_argument(
        "--generated-at-utc",
        default="",
        help="Optional deterministic generated_at_utc override (defaults to max artifact timestamp)",
    )
    rollback_packet.add_argument(
        "--confidence-gate-min",
        type=float,
        default=0.55,
        help="Confidence floor for rollback packet readiness checks (default: 0.55)",
    )
    rollback_packet.add_argument(
        "--fail-on-not-ready",
        action="store_true",
        help="Exit non-zero when readiness.pass=false",
    )
    rollback_packet.set_defaults(func=command_rollback_packet)

    production_readiness = sub.add_parser(
        "production-readiness-gate",
        help="Synthesize integrity/decision/confidence/signoff/rollback/anomaly into GO/TUNE/ROLLBACK readiness",
    )
    production_readiness.add_argument("--repo-root", default=".", help="Repo root used for relative output links")
    production_readiness.add_argument(
        "--decision-file",
        default="",
        help="Optional decision artifact path (default: <output-dir>\\<run-id>\\decision.json)",
    )
    production_readiness.add_argument(
        "--integrity-file",
        default="",
        help="Optional artifact-integrity path (default: <output-dir>\\<run-id>\\artifact-integrity.json)",
    )
    production_readiness.add_argument(
        "--confidence-file",
        default="",
        help="Optional recommendation-confidence-v2 path (default: <output-dir>\\<run-id>\\recommendation-confidence-v2.json)",
    )
    production_readiness.add_argument(
        "--signoff-file",
        default="",
        help="Optional signoff packet path (default: <output-dir>\\<run-id>\\signoff-packet.json)",
    )
    production_readiness.add_argument(
        "--rollback-packet-file",
        default="",
        help="Optional rollback packet path (default: <output-dir>\\<run-id>\\rollback-packet.json)",
    )
    production_readiness.add_argument(
        "--anomaly-response-file",
        default="",
        help="Optional anomaly response plan path (default: <output-dir>\\<run-id>\\anomaly-response-plan.json)",
    )
    production_readiness.add_argument(
        "--thresholds-json",
        default="tools\\examples\\decision-thresholds.json",
        help="Threshold policy schema path used for policy checks",
    )
    production_readiness.add_argument(
        "--confidence-gate-min",
        type=float,
        default=0.55,
        help="Confidence floor used by readiness confidence soft-gate (default: 0.55)",
    )
    production_readiness.add_argument(
        "--generated-at-utc",
        default="",
        help="Optional deterministic generated_at_utc override (defaults to max artifact timestamp)",
    )
    production_readiness.add_argument(
        "--require-stamped-policy",
        action="store_true",
        help="Require policy.fingerprint to be declared and valid in thresholds schema",
    )
    production_readiness.add_argument(
        "--strict-policy-version",
        action="store_true",
        help="Require non-legacy policy.version in thresholds schema",
    )
    production_readiness.add_argument(
        "--fail-on-non-go",
        action="store_true",
        help="Exit non-zero when readiness verdict is not GO",
    )
    production_readiness.add_argument(
        "--out-file",
        default="",
        help="Optional explicit output file path (default: <output-dir>\\<run-id>\\production-readiness-gate.json)",
    )
    production_readiness.set_defaults(func=command_production_readiness_gate)

    monthly_review = sub.add_parser(
        "monthly-strategy-review",
        help="Compile monthly eval outputs into deterministic JSON + markdown review packet artifacts",
    )
    monthly_review.add_argument("--month", default="", help="Month window in YYYY-MM format (default: previous month)")
    monthly_review.add_argument(
        "--eval-root",
        default="logs\\eval-cycle",
        help="Root directory containing eval run folders (default: logs\\\\eval-cycle)",
    )
    monthly_review.add_argument(
        "--out-dir",
        default="logs\\eval-cycle\\monthly-review",
        help="Output root for monthly packet artifacts",
    )
    monthly_review.add_argument("--out-json", default="", help="Optional explicit summary JSON path")
    monthly_review.add_argument("--out-md", default="", help="Optional explicit markdown packet path")
    monthly_review.set_defaults(func=command_monthly_strategy_review)

    regime_recommender = sub.add_parser(
        "regime-conditional-recommender",
        help="Generate deterministic per-regime parameter recommendations from regime/report/drift artifacts",
    )
    regime_recommender.add_argument(
        "--regime-file",
        default="",
        help="Optional regime summary path (default: <output-dir>\\<run-id>\\regimes\\regime-summary.json)",
    )
    regime_recommender.add_argument(
        "--report-file",
        default="",
        help="Optional report path (default: <output-dir>\\<run-id>\\report.json)",
    )
    regime_recommender.add_argument(
        "--drift-file",
        default="",
        help="Optional drift path (default: <output-dir>\\<run-id>\\drift-matrix.json)",
    )
    regime_recommender.add_argument(
        "--rejections-file",
        default="",
        help="Optional rejections artifact path",
    )
    regime_recommender.add_argument(
        "--snapshots-dir",
        default="",
        help="Optional snapshots dir fallback for rejection extraction",
    )
    regime_recommender.add_argument(
        "--out-json",
        default="",
        help="Optional explicit output file path (default: <output-dir>\\<run-id>\\regime-conditional-recommendations.json)",
    )
    regime_recommender.set_defaults(func=command_regime_conditional_recommender)

    bandit_allocation = sub.add_parser(
        "bandit-allocation",
        help="Run contextual bandit allocation replay with safe policy + off-policy evaluation outputs",
    )
    bandit_allocation.add_argument(
        "--eval-root",
        default="logs\\eval-cycle",
        help="Root directory containing eval run subfolders with report.json/fingerprint.json",
    )
    bandit_allocation.add_argument("--report-file", action="append", default=[], help="Optional explicit report.json path (repeatable)")
    bandit_allocation.add_argument(
        "--algorithm",
        choices=("linucb", "linucb-safe", "epsilon-greedy"),
        default="linucb-safe",
        help="Bandit algorithm variant",
    )
    bandit_allocation.add_argument("--alpha", type=float, default=0.6, help="Exploration multiplier for LinUCB")
    bandit_allocation.add_argument("--epsilon", type=float, default=0.12, help="Exploration rate for epsilon-greedy")
    bandit_allocation.add_argument("--ridge-lambda", type=float, default=1.0, help="Ridge regularization lambda")
    bandit_allocation.add_argument("--allocation-temperature", type=float, default=0.75, help="Softmax temperature")
    bandit_allocation.add_argument("--min-arm-prob", type=float, default=0.05, help="Safe allocation floor per arm")
    bandit_allocation.add_argument("--max-arm-prob", type=float, default=0.80, help="Safe allocation cap per arm")
    bandit_allocation.add_argument("--max-aggressive-prob", type=float, default=0.55, help="Safe allocation cap for aggressive arm")
    bandit_allocation.add_argument("--quality-floor", type=float, default=0.55, help="Quality floor for safe policy controls")
    bandit_allocation.add_argument("--synthetic-steps", type=int, default=0, help="Force synthetic replay sample count")
    bandit_allocation.add_argument(
        "--no-fallback-synthetic",
        action="store_true",
        help="Fail instead of synthetic fallback when no compatible artifacts are available",
    )
    bandit_allocation.add_argument("--seed", type=int, default=7, help="Deterministic RNG seed")
    bandit_allocation.add_argument("--ips-weight-cap", type=float, default=8.0, help="IPS clipping cap")
    bandit_allocation.add_argument("--logging-propensity-floor", type=float, default=0.05, help="Minimum logging propensity estimate")
    bandit_allocation.add_argument("--out-dir", default="logs\\bandit-allocation-sim", help="Output directory for artifacts")
    bandit_allocation.add_argument("--summary-file", default="summary.json", help="Summary JSON filename under --out-dir")
    bandit_allocation.add_argument(
        "--history-json",
        default="allocation-history.json",
        help="History JSON filename under --out-dir",
    )
    bandit_allocation.add_argument(
        "--history-csv",
        default="allocation-history.csv",
        help="History CSV filename under --out-dir",
    )
    bandit_allocation.set_defaults(func=command_bandit_allocation)

    full_cycle = sub.add_parser(
        "full-cycle",
        help="Run collect/analyze/decide/packet flow for one eval run",
    )
    full_cycle.add_argument("--repo-root", default=".", help="Repo root used for relative artifact links")
    full_cycle.add_argument("--env-path", default=".env", help="Path to env file used when collect-mode=start")
    full_cycle.add_argument("--notes", default="", help="Optional free-text notes when collect-mode=start")
    full_cycle.add_argument("--strategy-mode-hint", default="", help="Expected strategy mode when collect-mode=start")
    full_cycle.add_argument(
        "--collect-mode",
        choices=("auto", "start", "snapshot"),
        default="auto",
        help="Collect mode: auto picks start if fingerprint is missing, otherwise snapshot",
    )
    full_cycle.add_argument(
        "--baseline-json",
        default="",
        help="Optional baseline JSON path; when provided drift-matrix is generated",
    )
    full_cycle.add_argument(
        "--drift-out-file",
        default="",
        help="Optional explicit drift output path (default: <output-dir>\\<run-id>\\drift-matrix.json)",
    )
    full_cycle.add_argument(
        "--walkforward-input",
        default="",
        help="Optional input dataset path to run purged walk-forward during full-cycle",
    )
    full_cycle.add_argument(
        "--walkforward-out-file",
        default="",
        help="Optional output path for purged walk-forward artifact",
    )
    full_cycle.add_argument(
        "--microstructure-out-file",
        default="",
        help="Optional explicit output path for microstructure-imbalance artifact",
    )
    full_cycle.add_argument(
        "--toxic-flow-out-file",
        default="",
        help="Optional explicit output path for toxic-flow-advisor artifact",
    )
    full_cycle.add_argument(
        "--run-market-category-heatmap",
        action="store_true",
        help="Run market-category heatmap synthesis after report generation",
    )
    full_cycle.add_argument(
        "--market-category-heatmap-out-file",
        default="",
        help="Optional explicit output path for market-category-heatmap artifact",
    )
    full_cycle.add_argument(
        "--walkforward-input-format",
        choices=("auto", "csv", "json", "jsonl"),
        default="auto",
        help="Walk-forward input format (default: auto by extension)",
    )
    full_cycle.add_argument(
        "--walkforward-timestamp-col",
        default="timestamp",
        help="Walk-forward timestamp column",
    )
    full_cycle.add_argument("--walkforward-id-col", default="", help="Optional walk-forward tiebreak column")
    full_cycle.add_argument("--walkforward-n-splits", type=int, default=5, help="Walk-forward fold count")
    full_cycle.add_argument("--walkforward-test-size", type=int, default=None, help="Walk-forward test size rows")
    full_cycle.add_argument("--walkforward-min-train-size", type=int, default=20, help="Walk-forward min train rows")
    full_cycle.add_argument("--walkforward-purge-size", type=int, default=0, help="Walk-forward purge rows")
    full_cycle.add_argument("--walkforward-embargo-size", type=int, default=0, help="Walk-forward embargo rows")
    full_cycle.add_argument(
        "--walkforward-train-policy",
        choices=("expanding", "rolling", "both-sides"),
        default="expanding",
        help="Walk-forward train policy",
    )
    full_cycle.add_argument(
        "--walkforward-train-window-size",
        type=int,
        default=None,
        help="Walk-forward rolling window size when train policy is rolling",
    )
    full_cycle.add_argument(
        "--walkforward-metric-col",
        action="append",
        default=[],
        help="Walk-forward repeatable metric column",
    )
    full_cycle.add_argument("--walkforward-target-col", default="", help="Walk-forward target column")
    full_cycle.add_argument("--walkforward-prediction-col", default="", help="Walk-forward prediction column")
    full_cycle.add_argument(
        "--walkforward-include-indices",
        action="store_true",
        help="Include walk-forward full index arrays in output",
    )
    full_cycle.add_argument(
        "--require-drift-matrix",
        action="store_true",
        help="Require drift-matrix in artifact-integrity (requires --baseline-json)",
    )
    full_cycle.add_argument(
        "--require-conformal-summary",
        action="store_true",
        help="Require conformal-summary.json in artifact-integrity",
    )
    full_cycle.add_argument(
        "--conformal-file",
        default="",
        help="Optional conformal summary path for decision-eval",
    )
    full_cycle.add_argument(
        "--thresholds-json",
        default="tools\\examples\\decision-thresholds.json",
        help="Path to decision threshold schema JSON",
    )
    full_cycle.add_argument(
        "--decision-out-file",
        default="",
        help="Optional explicit decision output path (default: <output-dir>\\<run-id>\\decision.json)",
    )
    full_cycle.add_argument(
        "--confidence-out-file",
        default="",
        help="Optional explicit recommendation confidence v2 output path",
    )
    full_cycle.add_argument(
        "--run-anomaly-response-automation",
        action="store_true",
        help="Run anomaly-response-automation after confidence synthesis",
    )
    full_cycle.add_argument(
        "--anomaly-response-out-file",
        default="",
        help="Optional explicit anomaly response plan output path",
    )
    full_cycle.add_argument(
        "--anomaly-confidence-collapse-threshold",
        type=float,
        default=0.35,
        help="Confidence-collapse threshold used by anomaly response hook",
    )
    full_cycle.add_argument(
        "--anomaly-drift-fill-rate-critical-pp",
        type=float,
        default=-15.0,
        help="Critical fill-rate drift threshold (percentage points) used by anomaly response hook",
    )
    full_cycle.add_argument(
        "--anomaly-drift-slippage-critical-bps",
        type=float,
        default=120.0,
        help="Critical slippage drift threshold (bps) used by anomaly response hook",
    )
    full_cycle.add_argument(
        "--anomaly-drift-distribution-critical-l1",
        type=float,
        default=0.75,
        help="Critical L1 distribution drift threshold used by anomaly response hook",
    )
    full_cycle.add_argument(
        "--anomaly-integrity-required-failure-threshold",
        type=int,
        default=2,
        help="Required-failure threshold used by anomaly response hook",
    )
    full_cycle.add_argument(
        "--anomaly-integrity-total-failure-threshold",
        type=int,
        default=4,
        help="Total-failure threshold used by anomaly response hook",
    )
    full_cycle.add_argument(
        "--anomaly-fail-on-severity",
        choices=("none", "high", "critical"),
        default="none",
        help="Optional non-zero gate from anomaly response hook severity",
    )
    full_cycle.add_argument(
        "--generate-signoff-packet",
        action="store_true",
        help="Generate deploy signoff packet after decision/confidence/integrity synthesis",
    )
    full_cycle.add_argument(
        "--signoff-packet-out-file",
        default="",
        help="Optional explicit signoff packet output path",
    )
    full_cycle.add_argument(
        "--signoff-thresholds-schema-path",
        default="tools\\examples\\decision-thresholds.json",
        help="Threshold schema path used by full-cycle signoff packet",
    )
    full_cycle.add_argument(
        "--signoff-rollback-playbook-path",
        default="..\\deploy\\ROLLBACK-PLAYBOOK.md",
        help="Rollback playbook path used by full-cycle signoff packet",
    )
    full_cycle.add_argument(
        "--signoff-rollback-helper-path",
        default="..\\deploy\\rollback-hetzner.ps1",
        help="Rollback helper path used by full-cycle signoff packet",
    )
    full_cycle.add_argument("--signoff-environment", default="", help="Environment label for signoff packet")
    full_cycle.add_argument("--signoff-recorded-at-utc", default="", help="Optional recorded_at_utc override for signoff packet")
    full_cycle.add_argument("--signoff-primary-operator-name", default="", help="Primary signer name for signoff packet")
    full_cycle.add_argument(
        "--signoff-primary-operator-signed-at-utc",
        default="",
        help="Primary signer timestamp for signoff packet",
    )
    full_cycle.add_argument("--signoff-secondary-reviewer-name", default="", help="Secondary signer name for signoff packet")
    full_cycle.add_argument(
        "--signoff-secondary-reviewer-signed-at-utc",
        default="",
        help="Secondary signer timestamp for signoff packet",
    )
    full_cycle.add_argument(
        "--signoff-rollback-preview-verified",
        action="store_true",
        help="Mark rollback preview checklist as verified in signoff packet",
    )
    full_cycle.add_argument(
        "--signoff-target-mode-verified",
        action="store_true",
        help="Mark target mode checklist as verified in signoff packet",
    )
    full_cycle.add_argument(
        "--signoff-prior-decision-reviewed",
        action="store_true",
        help="Mark prior decision checklist as verified in signoff packet",
    )
    full_cycle.add_argument(
        "--signoff-decision-dimensions-reviewed",
        action="store_true",
        help="Mark decision dimensions checklist as verified in signoff packet",
    )
    full_cycle.add_argument(
        "--signoff-rollback-executed",
        action="store_true",
        help="Mark rollback executed in signoff packet",
    )
    full_cycle.add_argument("--signoff-rollback-reference", default="", help="Rollback reference for signoff packet")
    full_cycle.add_argument(
        "--signoff-confidence-gate-min",
        type=float,
        default=0.55,
        help="Confidence floor for signoff confidence gate (default: 0.55)",
    )
    full_cycle.add_argument(
        "--signoff-enforce-confidence-gate",
        action="store_true",
        help="Require confidence gate PASS to allow promotion in signoff packet",
    )
    full_cycle.add_argument("--signoff-notes", default="", help="Optional signoff packet notes")
    full_cycle.add_argument(
        "--generate-rollback-packet",
        action="store_true",
        help="Generate rollback packet from eval artifacts + rollback playbook alignment",
    )
    full_cycle.add_argument(
        "--rollback-packet-out-file",
        default="",
        help="Optional explicit rollback packet output path",
    )
    full_cycle.add_argument(
        "--rollback-packet-playbook-path",
        default="deploy\\ROLLBACK-PLAYBOOK.md",
        help="Rollback playbook path used by full-cycle rollback packet step",
    )
    full_cycle.add_argument(
        "--rollback-packet-helper-path",
        default="deploy\\rollback-hetzner.ps1",
        help="Rollback helper path used by full-cycle rollback packet step",
    )
    full_cycle.add_argument(
        "--rollback-packet-confidence-gate-min",
        type=float,
        default=0.55,
        help="Confidence floor for rollback packet readiness checks (default: 0.55)",
    )
    full_cycle.add_argument(
        "--rollback-packet-generated-at-utc",
        default="",
        help="Optional generated_at_utc override for deterministic rollback packet generation",
    )
    full_cycle.add_argument(
        "--rollback-packet-fail-on-not-ready",
        action="store_true",
        help="Fail full-cycle if generated rollback packet readiness is FAIL",
    )
    full_cycle.add_argument(
        "--generate-production-readiness-gate",
        action="store_true",
        help="Generate production readiness gate artifact after signoff/rollback/anomaly synthesis",
    )
    full_cycle.add_argument(
        "--production-readiness-out-file",
        default="",
        help="Optional explicit production readiness gate output path",
    )
    full_cycle.add_argument(
        "--production-readiness-confidence-gate-min",
        type=float,
        default=0.55,
        help="Confidence floor used by production readiness gate (default: 0.55)",
    )
    full_cycle.add_argument(
        "--production-readiness-generated-at-utc",
        default="",
        help="Optional generated_at_utc override for deterministic production readiness artifact generation",
    )
    full_cycle.add_argument(
        "--production-readiness-require-stamped-policy",
        action="store_true",
        help="Require thresholds policy fingerprint stamping in production readiness checks",
    )
    full_cycle.add_argument(
        "--production-readiness-strict-policy-version",
        action="store_true",
        help="Require non-legacy thresholds policy.version in production readiness checks",
    )
    full_cycle.add_argument(
        "--production-readiness-fail-on-non-go",
        action="store_true",
        help="Fail full-cycle when production readiness verdict is not GO",
    )
    full_cycle.add_argument(
        "--execution-drag-out-file",
        default="",
        help="Optional explicit output path for execution-drag-attribution artifact",
    )
    full_cycle.add_argument(
        "--run-execution-drag-attribution",
        action="store_true",
        help="Run execution drag attribution synthesis after report generation",
    )
    full_cycle.add_argument(
        "--execution-drag-spread-bps",
        type=float,
        default=None,
        help="Optional spread proxy override for full-cycle execution drag step",
    )
    full_cycle.add_argument(
        "--execution-drag-entry-delay-secs",
        type=float,
        default=None,
        help="Optional entry delay override (seconds) for full-cycle execution drag step",
    )
    full_cycle.add_argument(
        "--execution-drag-delay-bps-per-second",
        type=float,
        default=2.0,
        help="Delay opportunity-cost bps per second for full-cycle execution drag step",
    )
    full_cycle.add_argument(
        "--execution-drag-rejection-edge-bps",
        type=float,
        default=50.0,
        help="Rejected notional opportunity-cost edge assumption (bps) for full-cycle drag step",
    )
    full_cycle.add_argument(
        "--run-regime-conditional-recommender",
        action="store_true",
        help="Run regime-conditional recommender if regime artifact is available",
    )
    full_cycle.add_argument(
        "--run-bandit-allocation",
        action="store_true",
        help="Run contextual bandit allocation replay + OPE artifacts",
    )
    full_cycle.add_argument("--bandit-eval-root", default="", help="Optional eval root override for bandit allocation")
    full_cycle.add_argument("--bandit-report-file", action="append", default=[], help="Optional explicit report path for bandit replay")
    full_cycle.add_argument(
        "--bandit-algorithm",
        choices=("linucb", "linucb-safe", "epsilon-greedy"),
        default="linucb-safe",
        help="Bandit algorithm variant for full-cycle hook",
    )
    full_cycle.add_argument("--bandit-alpha", type=float, default=0.6, help="Bandit LinUCB alpha")
    full_cycle.add_argument("--bandit-epsilon", type=float, default=0.12, help="Bandit epsilon-greedy epsilon")
    full_cycle.add_argument("--bandit-ridge-lambda", type=float, default=1.0, help="Bandit ridge regularization")
    full_cycle.add_argument("--bandit-allocation-temperature", type=float, default=0.75, help="Bandit softmax temperature")
    full_cycle.add_argument("--bandit-min-arm-prob", type=float, default=0.05, help="Bandit safe floor per arm")
    full_cycle.add_argument("--bandit-max-arm-prob", type=float, default=0.80, help="Bandit safe cap per arm")
    full_cycle.add_argument("--bandit-max-aggressive-prob", type=float, default=0.55, help="Bandit cap for aggressive arm")
    full_cycle.add_argument("--bandit-quality-floor", type=float, default=0.55, help="Bandit quality floor")
    full_cycle.add_argument("--bandit-synthetic-steps", type=int, default=0, help="Bandit forced synthetic steps")
    full_cycle.add_argument(
        "--bandit-no-fallback-synthetic",
        action="store_true",
        help="Bandit hook fails if no compatible artifacts and synthetic fallback is disabled",
    )
    full_cycle.add_argument("--bandit-seed", type=int, default=7, help="Bandit deterministic RNG seed")
    full_cycle.add_argument("--bandit-ips-weight-cap", type=float, default=8.0, help="Bandit IPS clipping cap")
    full_cycle.add_argument(
        "--bandit-logging-propensity-floor",
        type=float,
        default=0.05,
        help="Bandit minimum logging propensity estimate",
    )
    full_cycle.add_argument(
        "--bandit-out-dir",
        default="",
        help="Optional explicit output directory for bandit artifacts (default: <output-dir>\\<run-id>\\bandit-allocation)",
    )
    full_cycle.add_argument(
        "--bandit-summary-file",
        default="summary.json",
        help="Bandit summary filename under --bandit-out-dir",
    )
    full_cycle.add_argument(
        "--bandit-history-json",
        default="allocation-history.json",
        help="Bandit history JSON filename under --bandit-out-dir",
    )
    full_cycle.add_argument(
        "--bandit-history-csv",
        default="allocation-history.csv",
        help="Bandit history CSV filename under --bandit-out-dir",
    )
    full_cycle.add_argument(
        "--regime-file",
        default="",
        help="Optional regime summary file path for full-cycle recommender hook",
    )
    full_cycle.add_argument(
        "--rejections-file",
        default="",
        help="Optional rejections artifact path for full-cycle recommender hook",
    )
    full_cycle.add_argument(
        "--regime-snapshots-dir",
        default="",
        help="Optional snapshots directory fallback used by full-cycle recommender hook",
    )
    full_cycle.add_argument(
        "--regime-recommendation-out-file",
        default="",
        help="Optional explicit regime-conditional recommendations output path",
    )
    full_cycle.add_argument(
        "--artifact-integrity-out-file",
        default="",
        help="Optional explicit artifact integrity output path",
    )
    full_cycle.add_argument(
        "--registry-upsert",
        action="store_true",
        help="Upsert run artifacts into experiment registry after decision synthesis",
    )
    full_cycle.add_argument(
        "--registry-dir",
        default="artifacts\\experiment-registry",
        help="Registry directory (default: artifacts\\\\experiment-registry)",
    )
    full_cycle.add_argument(
        "--decision-tag",
        action="append",
        default=[],
        help="Decision tag to attach on optional registry-upsert (repeatable)",
    )
    full_cycle.add_argument(
        "--strict",
        action="store_true",
        help="Fail immediately if any optional full-cycle step fails",
    )
    full_cycle.set_defaults(func=command_full_cycle)
    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
