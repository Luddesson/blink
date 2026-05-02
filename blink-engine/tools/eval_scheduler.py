#!/usr/bin/env python3
from __future__ import annotations

import argparse
import datetime as dt
import json
import pathlib
import subprocess
import sys
import time
from dataclasses import asdict, dataclass
from typing import Any


@dataclass(frozen=True)
class CadenceTemplate:
    name: str
    window_seconds: int
    snapshot_every_seconds: int


CADENCE_TEMPLATES: dict[str, CadenceTemplate] = {
    "15m": CadenceTemplate(name="15m", window_seconds=15 * 60, snapshot_every_seconds=60),
    "1h": CadenceTemplate(name="1h", window_seconds=60 * 60, snapshot_every_seconds=5 * 60),
    "24h": CadenceTemplate(name="24h", window_seconds=24 * 60 * 60, snapshot_every_seconds=15 * 60),
}


@dataclass
class SchedulerCheckpoint:
    schema_version: int
    cadence: str
    run_id: str
    window_start_utc: str
    window_end_utc: str
    started: bool
    reported: bool
    snapshot_count: int
    last_snapshot_utc: str | None
    next_snapshot_due_utc: str | None


def now_utc() -> dt.datetime:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0)


def to_iso_utc(value: dt.datetime | None) -> str | None:
    if value is None:
        return None
    return value.astimezone(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def parse_iso_utc(value: str) -> dt.datetime:
    return dt.datetime.fromisoformat(value.replace("Z", "+00:00")).astimezone(dt.timezone.utc)


def window_start_for(timestamp: dt.datetime, window_seconds: int) -> dt.datetime:
    epoch = int(timestamp.timestamp())
    floored = epoch - (epoch % window_seconds)
    return dt.datetime.fromtimestamp(floored, tz=dt.timezone.utc)


def run_id_for_window(prefix: str, cadence: str, window_start: dt.datetime) -> str:
    return f"{prefix}-{cadence}-{window_start.strftime('%Y%m%dT%H%M%SZ')}"


def checkpoint_name(cadence: str, window_start: dt.datetime) -> str:
    return f"checkpoint-{cadence}-{window_start.strftime('%Y%m%dT%H%M%SZ')}.json"


def load_checkpoint(path: pathlib.Path) -> SchedulerCheckpoint:
    payload = json.loads(path.read_text(encoding="utf-8"))
    return SchedulerCheckpoint(
        schema_version=int(payload.get("schema_version", 1)),
        cadence=str(payload["cadence"]),
        run_id=str(payload["run_id"]),
        window_start_utc=str(payload["window_start_utc"]),
        window_end_utc=str(payload["window_end_utc"]),
        started=bool(payload.get("started", False)),
        reported=bool(payload.get("reported", False)),
        snapshot_count=int(payload.get("snapshot_count", 0)),
        last_snapshot_utc=payload.get("last_snapshot_utc"),
        next_snapshot_due_utc=payload.get("next_snapshot_due_utc"),
    )


def save_checkpoint(path: pathlib.Path, checkpoint: SchedulerCheckpoint) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = path.with_suffix(path.suffix + ".tmp")
    tmp_path.write_text(json.dumps(asdict(checkpoint), indent=2, sort_keys=True), encoding="utf-8")
    tmp_path.replace(path)


def persist_checkpoint(path: pathlib.Path, checkpoint: SchedulerCheckpoint, dry_run: bool) -> None:
    if dry_run:
        print(f"[eval-scheduler] DRY-RUN checkpoint update skipped: {path}")
        return
    save_checkpoint(path, checkpoint)


def run_eval_command(run_eval_cycle: pathlib.Path, args: list[str], dry_run: bool) -> None:
    command = [sys.executable, str(run_eval_cycle), *args]
    print("[eval-scheduler]", " ".join(command))
    if dry_run:
        return
    subprocess.run(command, check=True)


def completed_month_for_window_end(window_end: dt.datetime) -> str | None:
    if (
        window_end.day == 1
        and window_end.hour == 0
        and window_end.minute == 0
        and window_end.second == 0
    ):
        previous_month_day = window_end - dt.timedelta(days=1)
        return previous_month_day.strftime("%Y-%m")
    return None


def maybe_run_monthly_review(
    *,
    args: argparse.Namespace,
    run_eval_cycle: pathlib.Path,
    window_end: dt.datetime,
) -> None:
    if not args.monthly_review_on_report:
        return
    month = completed_month_for_window_end(window_end)
    if month is None:
        print(
            "[eval-scheduler] monthly review hook skipped: window does not close a full month "
            f"({to_iso_utc(window_end)})"
        )
        return
    eval_root = args.monthly_review_eval_root or args.output_dir
    monthly_out_dir = args.monthly_review_out_dir or "logs\\eval-cycle\\monthly-review"
    monthly_args = [
        "monthly-strategy-review",
        "--month",
        month,
        "--eval-root",
        eval_root,
        "--out-dir",
        monthly_out_dir,
    ]
    run_eval_command(run_eval_cycle, monthly_args, args.dry_run)


def build_base_eval_args(args: argparse.Namespace, run_id: str) -> list[str]:
    return [
        "--run-id",
        run_id,
        "--base-url",
        args.base_url,
        "--output-dir",
        args.output_dir,
    ]


def refresh_from_existing_artifacts(
    checkpoint: SchedulerCheckpoint,
    output_dir: pathlib.Path,
) -> SchedulerCheckpoint:
    run_dir = output_dir / checkpoint.run_id
    fingerprint_exists = (run_dir / "fingerprint.json").exists()
    report_exists = (run_dir / "report.json").exists()
    snapshot_count = len(list(run_dir.glob("snapshot-*.json")))
    return SchedulerCheckpoint(
        schema_version=checkpoint.schema_version,
        cadence=checkpoint.cadence,
        run_id=checkpoint.run_id,
        window_start_utc=checkpoint.window_start_utc,
        window_end_utc=checkpoint.window_end_utc,
        started=checkpoint.started or fingerprint_exists,
        reported=checkpoint.reported or report_exists,
        snapshot_count=max(checkpoint.snapshot_count, snapshot_count),
        last_snapshot_utc=checkpoint.last_snapshot_utc,
        next_snapshot_due_utc=checkpoint.next_snapshot_due_utc,
    )


def build_new_checkpoint(args: argparse.Namespace, template: CadenceTemplate, current_time: dt.datetime) -> SchedulerCheckpoint:
    start = window_start_for(current_time, template.window_seconds)
    end = start + dt.timedelta(seconds=template.window_seconds)
    run_id = run_id_for_window(args.run_id_prefix, template.name, start)
    return SchedulerCheckpoint(
        schema_version=1,
        cadence=template.name,
        run_id=run_id,
        window_start_utc=to_iso_utc(start) or "",
        window_end_utc=to_iso_utc(end) or "",
        started=False,
        reported=False,
        snapshot_count=0,
        last_snapshot_utc=None,
        next_snapshot_due_utc=None,
    )


def finalize_pending_checkpoints(args: argparse.Namespace, template: CadenceTemplate, run_eval_cycle: pathlib.Path) -> None:
    checkpoint_dir = pathlib.Path(args.checkpoint_dir)
    if not checkpoint_dir.exists():
        return
    for checkpoint_path in sorted(checkpoint_dir.glob(f"checkpoint-{template.name}-*.json")):
        checkpoint = load_checkpoint(checkpoint_path)
        checkpoint = refresh_from_existing_artifacts(checkpoint, pathlib.Path(args.output_dir))
        if checkpoint.reported:
            continue
        window_end = parse_iso_utc(checkpoint.window_end_utc)
        if now_utc() < window_end:
            continue
        eval_args = build_base_eval_args(args, checkpoint.run_id)
        eval_args.extend(["report", "--repo-root", args.repo_root])
        run_eval_command(run_eval_cycle, eval_args, args.dry_run)
        maybe_run_monthly_review(args=args, run_eval_cycle=run_eval_cycle, window_end=window_end)
        checkpoint.reported = True
        persist_checkpoint(checkpoint_path, checkpoint, args.dry_run)


def process_current_window(args: argparse.Namespace, template: CadenceTemplate, run_eval_cycle: pathlib.Path) -> None:
    current_time = now_utc()
    checkpoint_root = pathlib.Path(args.checkpoint_dir)
    checkpoint_root.mkdir(parents=True, exist_ok=True)

    new_checkpoint = build_new_checkpoint(args, template, current_time)
    checkpoint_path = checkpoint_root / checkpoint_name(template.name, parse_iso_utc(new_checkpoint.window_start_utc))
    checkpoint = load_checkpoint(checkpoint_path) if checkpoint_path.exists() else new_checkpoint
    checkpoint = refresh_from_existing_artifacts(checkpoint, pathlib.Path(args.output_dir))

    window_end = parse_iso_utc(checkpoint.window_end_utc)
    base_eval_args = build_base_eval_args(args, checkpoint.run_id)

    if not checkpoint.started and current_time < window_end:
        start_args = base_eval_args + [
            "start",
            "--repo-root",
            args.repo_root,
            "--env-path",
            args.env_path,
            "--notes",
            args.notes,
            "--strategy-mode-hint",
            args.strategy_mode_hint,
        ]
        run_eval_command(run_eval_cycle, start_args, args.dry_run)
        checkpoint.started = True
        checkpoint.snapshot_count = max(checkpoint.snapshot_count, 1)
        checkpoint.last_snapshot_utc = to_iso_utc(current_time)
        checkpoint.next_snapshot_due_utc = to_iso_utc(current_time + dt.timedelta(seconds=template.snapshot_every_seconds))
        persist_checkpoint(checkpoint_path, checkpoint, args.dry_run)

    if checkpoint.started and not checkpoint.reported and current_time < window_end:
        next_due = parse_iso_utc(checkpoint.next_snapshot_due_utc) if checkpoint.next_snapshot_due_utc else current_time
        while next_due <= current_time and next_due < window_end:
            snapshot_args = base_eval_args + ["snapshot"]
            run_eval_command(run_eval_cycle, snapshot_args, args.dry_run)
            checkpoint.snapshot_count += 1
            checkpoint.last_snapshot_utc = to_iso_utc(current_time)
            next_due = next_due + dt.timedelta(seconds=template.snapshot_every_seconds)
            checkpoint.next_snapshot_due_utc = to_iso_utc(next_due)
            persist_checkpoint(checkpoint_path, checkpoint, args.dry_run)

    if checkpoint.started and not checkpoint.reported and current_time >= window_end:
        report_args = base_eval_args + ["report", "--repo-root", args.repo_root]
        run_eval_command(run_eval_cycle, report_args, args.dry_run)
        maybe_run_monthly_review(args=args, run_eval_cycle=run_eval_cycle, window_end=window_end)
        checkpoint.reported = True
        persist_checkpoint(checkpoint_path, checkpoint, args.dry_run)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Schedule recurring eval-cycle sessions with deterministic run IDs and resume-safe checkpoints."
    )
    parser.add_argument("--cadence", choices=sorted(CADENCE_TEMPLATES.keys()), required=True, help="Session template")
    parser.add_argument("--run-id-prefix", default="eval", help="Run ID prefix (default: eval)")
    parser.add_argument("--base-url", default="http://127.0.0.1:3030", help="Blink API base URL")
    parser.add_argument("--repo-root", default=".", help="Repo root used by run_eval_cycle start/report")
    parser.add_argument("--env-path", default=".env", help="Env file path recorded in fingerprint")
    parser.add_argument("--output-dir", default="logs\\eval-cycle", help="Eval artifact root directory")
    parser.add_argument(
        "--checkpoint-dir",
        default="logs\\eval-cycle\\checkpoints",
        help="Scheduler checkpoint directory",
    )
    parser.add_argument("--strategy-mode-hint", default="", help="Optional strategy mode hint")
    parser.add_argument("--notes", default="", help="Optional run notes")
    parser.add_argument("--sleep-seconds", type=int, default=5, help="Loop sleep for daemon mode")
    parser.add_argument("--once", action="store_true", help="Process due work once and exit")
    parser.add_argument("--dry-run", action="store_true", help="Print commands/checkpoint flow without executing")
    parser.add_argument(
        "--monthly-review-on-report",
        action="store_true",
        help="Run monthly-strategy-review when a checkpoint closes a full month window",
    )
    parser.add_argument(
        "--monthly-review-eval-root",
        default="",
        help="Optional eval root override for monthly review hook (default: --output-dir)",
    )
    parser.add_argument(
        "--monthly-review-out-dir",
        default="logs\\eval-cycle\\monthly-review",
        help="Output root for monthly review artifacts generated by scheduler hook",
    )
    return parser


def main() -> int:
    args = build_parser().parse_args()
    template = CADENCE_TEMPLATES[args.cadence]
    run_eval_cycle = pathlib.Path(__file__).resolve().parent / "run_eval_cycle.py"

    if not run_eval_cycle.exists():
        raise FileNotFoundError(f"Missing run_eval_cycle.py at {run_eval_cycle}")

    finalize_pending_checkpoints(args, template, run_eval_cycle)
    process_current_window(args, template, run_eval_cycle)
    if args.once:
        return 0

    while True:
        time.sleep(max(args.sleep_seconds, 1))
        finalize_pending_checkpoints(args, template, run_eval_cycle)
        process_current_window(args, template, run_eval_cycle)


if __name__ == "__main__":
    raise SystemExit(main())
