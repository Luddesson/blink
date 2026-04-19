#!/usr/bin/env python3
from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import time
from dataclasses import asdict, dataclass, field
from typing import Any


QUEUE_STATUSES = ("pending", "running", "done", "blocked", "cancelled")


@dataclass
class QueueEvent:
    at_utc: str
    event: str
    worker: str
    note: str
    status: str


@dataclass
class QueueJob:
    id: str
    run_id: str
    eval_command: str
    eval_args: list[str]
    status: str
    created_seq: int
    created_at_utc: str
    updated_at_utc: str
    priority: int
    max_retries: int
    attempts: int
    description: str
    tags: list[str]
    claim_owner: str | None
    last_error: str | None
    history: list[dict[str, Any]] = field(default_factory=list)


@dataclass
class QueueState:
    schema_version: int
    next_created_seq: int
    updated_at_utc: str
    jobs: list[dict[str, Any]]


def now_utc_iso() -> str:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def stable_hash(payload: dict[str, Any]) -> str:
    canonical = json.dumps(payload, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


def normalize_job_id(raw: str) -> str:
    allowed = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-")
    normalized = "".join(ch for ch in raw.strip() if ch in allowed)
    if not normalized:
        raise ValueError("Job ID is empty after normalization.")
    return normalized


def normalize_tags(tags: list[str]) -> list[str]:
    cleaned = {tag.strip() for tag in tags if tag.strip()}
    return sorted(cleaned)


def ensure_parent(path: pathlib.Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)


def queue_sort_key(job: QueueJob) -> tuple[int, int, str]:
    return (-int(job.priority), int(job.created_seq), job.id)


def load_state(path: pathlib.Path) -> QueueState:
    if not path.exists():
        return QueueState(schema_version=1, next_created_seq=1, updated_at_utc=now_utc_iso(), jobs=[])

    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise ValueError(f"Expected JSON object in queue state file: {path}")

    jobs = payload.get("jobs", [])
    if not isinstance(jobs, list):
        raise ValueError(f"'jobs' must be an array in queue state file: {path}")

    return QueueState(
        schema_version=int(payload.get("schema_version", 1)),
        next_created_seq=int(payload.get("next_created_seq", 1)),
        updated_at_utc=str(payload.get("updated_at_utc", now_utc_iso())),
        jobs=jobs,
    )


def write_state(path: pathlib.Path, state: QueueState) -> None:
    ensure_parent(path)
    sorted_jobs = sorted(
        state.jobs,
        key=lambda item: (
            -int(item.get("priority", 0)),
            int(item.get("created_seq", 0)),
            str(item.get("id", "")),
        ),
    )
    payload = {
        "schema_version": int(state.schema_version),
        "next_created_seq": int(state.next_created_seq),
        "updated_at_utc": state.updated_at_utc,
        "jobs": sorted_jobs,
    }
    tmp_path = path.with_suffix(path.suffix + ".tmp")
    tmp_path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    tmp_path.replace(path)


class FileLock:
    def __init__(self, lock_path: pathlib.Path, timeout_seconds: float) -> None:
        self.lock_path = lock_path
        self.timeout_seconds = timeout_seconds
        self.fd: int | None = None

    def __enter__(self) -> FileLock:
        ensure_parent(self.lock_path)
        deadline = time.monotonic() + max(self.timeout_seconds, 0.1)
        while True:
            try:
                self.fd = os.open(str(self.lock_path), os.O_CREAT | os.O_EXCL | os.O_WRONLY)
                lock_payload = {
                    "pid": os.getpid(),
                    "created_at_utc": now_utc_iso(),
                }
                os.write(self.fd, (json.dumps(lock_payload, sort_keys=True) + "\n").encode("utf-8"))
                return self
            except FileExistsError:
                if time.monotonic() >= deadline:
                    raise TimeoutError(f"Timed out waiting for lock: {self.lock_path}")
                time.sleep(0.1)

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        if self.fd is not None:
            os.close(self.fd)
            self.fd = None
        if self.lock_path.exists():
            self.lock_path.unlink()


def parse_jobs(state: QueueState) -> list[QueueJob]:
    parsed: list[QueueJob] = []
    for item in state.jobs:
        if not isinstance(item, dict):
            raise ValueError("Each job entry must be a JSON object.")
        job = QueueJob(
            id=str(item["id"]),
            run_id=str(item.get("run_id", "")),
            eval_command=str(item.get("eval_command", "")),
            eval_args=[str(v) for v in item.get("eval_args", [])],
            status=str(item.get("status", "pending")),
            created_seq=int(item.get("created_seq", 0)),
            created_at_utc=str(item.get("created_at_utc", "")),
            updated_at_utc=str(item.get("updated_at_utc", "")),
            priority=int(item.get("priority", 0)),
            max_retries=int(item.get("max_retries", 3)),
            attempts=int(item.get("attempts", 0)),
            description=str(item.get("description", "")),
            tags=normalize_tags([str(tag) for tag in item.get("tags", [])]),
            claim_owner=str(item["claim_owner"]) if item.get("claim_owner") is not None else None,
            last_error=str(item["last_error"]) if item.get("last_error") is not None else None,
            history=[dict(event) for event in item.get("history", []) if isinstance(event, dict)],
        )
        if job.status not in QUEUE_STATUSES:
            raise ValueError(f"Invalid status '{job.status}' for job {job.id}")
        parsed.append(job)
    return parsed


def jobs_to_payload(jobs: list[QueueJob]) -> list[dict[str, Any]]:
    ordered = sorted(jobs, key=queue_sort_key)
    return [asdict(job) for job in ordered]


def append_event(job: QueueJob, event: str, worker: str, note: str) -> None:
    job.history.append(
        asdict(
            QueueEvent(
                at_utc=now_utc_iso(),
                event=event,
                worker=worker,
                note=note,
                status=job.status,
            )
        )
    )


def pick_pending_job(jobs: list[QueueJob], explicit_job_id: str | None) -> QueueJob | None:
    if explicit_job_id:
        for job in jobs:
            if job.id == explicit_job_id and job.status == "pending":
                return job
        return None
    pending = [job for job in jobs if job.status == "pending"]
    if not pending:
        return None
    return sorted(pending, key=queue_sort_key)[0]


def build_job_id(args: argparse.Namespace) -> str:
    if args.job_id:
        return normalize_job_id(args.job_id)
    fingerprint = stable_hash(
        {
            "run_id": args.run_id,
            "eval_command": args.eval_command,
            "eval_args": args.eval_arg,
            "priority": args.priority,
            "max_retries": args.max_retries,
        }
    )
    return f"eval-{fingerprint[:12]}"


def format_output(payload: dict[str, Any]) -> None:
    print(json.dumps(payload, indent=2, sort_keys=True))


def command_enqueue(args: argparse.Namespace) -> int:
    state_path = pathlib.Path(args.state_file).resolve()
    lock_path = state_path.with_suffix(state_path.suffix + ".lock")
    with FileLock(lock_path=lock_path, timeout_seconds=args.lock_timeout_seconds):
        state = load_state(state_path)
        jobs = parse_jobs(state)

        job_id = build_job_id(args)
        if any(job.id == job_id for job in jobs):
            raise ValueError(f"Job ID already exists: {job_id}")

        now = now_utc_iso()
        eval_args = [str(v) for v in args.eval_arg]
        job = QueueJob(
            id=job_id,
            run_id=args.run_id,
            eval_command=args.eval_command,
            eval_args=eval_args,
            status="pending",
            created_seq=state.next_created_seq,
            created_at_utc=now,
            updated_at_utc=now,
            priority=args.priority,
            max_retries=max(args.max_retries, 1),
            attempts=0,
            description=args.description.strip(),
            tags=normalize_tags(args.tag),
            claim_owner=None,
            last_error=None,
            history=[],
        )
        append_event(job, event="enqueue", worker=args.worker, note=args.note.strip())

        jobs.append(job)
        state.next_created_seq += 1
        state.updated_at_utc = now_utc_iso()
        state.jobs = jobs_to_payload(jobs)
        write_state(state_path, state)
        format_output({"status": "ok", "action": "enqueue", "job": asdict(job), "state_file": str(state_path)})
    return 0


def command_list(args: argparse.Namespace) -> int:
    state_path = pathlib.Path(args.state_file).resolve()
    if not state_path.exists():
        format_output({"status": "ok", "jobs": [], "state_file": str(state_path), "total": 0})
        return 0

    state = load_state(state_path)
    jobs = parse_jobs(state)
    if args.status:
        wanted = set(args.status)
        jobs = [job for job in jobs if job.status in wanted]

    jobs = sorted(jobs, key=queue_sort_key)
    if args.limit is not None and args.limit >= 0:
        jobs = jobs[: args.limit]

    payload = {
        "status": "ok",
        "state_file": str(state_path),
        "schema_version": state.schema_version,
        "updated_at_utc": state.updated_at_utc,
        "total": len(jobs),
        "jobs": [asdict(job) for job in jobs],
    }
    format_output(payload)
    return 0


def command_claim(args: argparse.Namespace) -> int:
    state_path = pathlib.Path(args.state_file).resolve()
    lock_path = state_path.with_suffix(state_path.suffix + ".lock")
    with FileLock(lock_path=lock_path, timeout_seconds=args.lock_timeout_seconds):
        state = load_state(state_path)
        jobs = parse_jobs(state)
        selected = pick_pending_job(jobs, explicit_job_id=args.job_id)
        if selected is None:
            format_output({"status": "empty", "action": "claim", "state_file": str(state_path)})
            return 2

        selected.status = "running"
        selected.attempts += 1
        selected.claim_owner = args.worker
        selected.updated_at_utc = now_utc_iso()
        append_event(selected, event="claim", worker=args.worker, note=args.note.strip())

        state.updated_at_utc = now_utc_iso()
        state.jobs = jobs_to_payload(jobs)
        write_state(state_path, state)
        format_output({"status": "ok", "action": "claim", "job": asdict(selected), "state_file": str(state_path)})
    return 0


def find_job(jobs: list[QueueJob], job_id: str) -> QueueJob:
    for job in jobs:
        if job.id == job_id:
            return job
    raise ValueError(f"Unknown job id: {job_id}")


def command_complete(args: argparse.Namespace) -> int:
    state_path = pathlib.Path(args.state_file).resolve()
    lock_path = state_path.with_suffix(state_path.suffix + ".lock")
    with FileLock(lock_path=lock_path, timeout_seconds=args.lock_timeout_seconds):
        state = load_state(state_path)
        jobs = parse_jobs(state)
        job = find_job(jobs, args.job_id)
        if job.status not in ("running", "pending"):
            raise ValueError(f"Cannot complete job in status '{job.status}'")
        job.status = "done"
        job.claim_owner = args.worker
        job.last_error = None
        job.updated_at_utc = now_utc_iso()
        append_event(job, event="complete", worker=args.worker, note=args.note.strip())

        state.updated_at_utc = now_utc_iso()
        state.jobs = jobs_to_payload(jobs)
        write_state(state_path, state)
        format_output({"status": "ok", "action": "complete", "job": asdict(job), "state_file": str(state_path)})
    return 0


def command_fail(args: argparse.Namespace) -> int:
    state_path = pathlib.Path(args.state_file).resolve()
    lock_path = state_path.with_suffix(state_path.suffix + ".lock")
    with FileLock(lock_path=lock_path, timeout_seconds=args.lock_timeout_seconds):
        state = load_state(state_path)
        jobs = parse_jobs(state)
        job = find_job(jobs, args.job_id)
        if job.status not in ("running", "pending"):
            raise ValueError(f"Cannot fail job in status '{job.status}'")

        job.last_error = args.error.strip()
        should_block = bool(args.blocked or job.attempts >= job.max_retries)
        job.status = "blocked" if should_block else "pending"
        job.claim_owner = args.worker if job.status == "blocked" else None
        job.updated_at_utc = now_utc_iso()
        append_event(job, event="fail", worker=args.worker, note=args.note.strip() or job.last_error)

        state.updated_at_utc = now_utc_iso()
        state.jobs = jobs_to_payload(jobs)
        write_state(state_path, state)
        format_output(
            {
                "status": "ok",
                "action": "fail",
                "job": asdict(job),
                "state_file": str(state_path),
                "requeued": job.status == "pending",
            }
        )
    return 0


def command_retry(args: argparse.Namespace) -> int:
    state_path = pathlib.Path(args.state_file).resolve()
    lock_path = state_path.with_suffix(state_path.suffix + ".lock")
    with FileLock(lock_path=lock_path, timeout_seconds=args.lock_timeout_seconds):
        state = load_state(state_path)
        jobs = parse_jobs(state)
        job = find_job(jobs, args.job_id)
        if job.status not in ("blocked", "cancelled"):
            raise ValueError(f"Cannot retry job in status '{job.status}'")
        job.status = "pending"
        job.claim_owner = None
        if args.reset_attempts:
            job.attempts = 0
        job.last_error = None
        job.updated_at_utc = now_utc_iso()
        append_event(job, event="retry", worker=args.worker, note=args.note.strip())

        state.updated_at_utc = now_utc_iso()
        state.jobs = jobs_to_payload(jobs)
        write_state(state_path, state)
        format_output({"status": "ok", "action": "retry", "job": asdict(job), "state_file": str(state_path)})
    return 0


def command_cancel(args: argparse.Namespace) -> int:
    state_path = pathlib.Path(args.state_file).resolve()
    lock_path = state_path.with_suffix(state_path.suffix + ".lock")
    with FileLock(lock_path=lock_path, timeout_seconds=args.lock_timeout_seconds):
        state = load_state(state_path)
        jobs = parse_jobs(state)
        job = find_job(jobs, args.job_id)
        if job.status in ("done", "cancelled"):
            raise ValueError(f"Cannot cancel job in status '{job.status}'")
        job.status = "cancelled"
        job.claim_owner = args.worker
        job.updated_at_utc = now_utc_iso()
        append_event(job, event="cancel", worker=args.worker, note=args.note.strip())

        state.updated_at_utc = now_utc_iso()
        state.jobs = jobs_to_payload(jobs)
        write_state(state_path, state)
        format_output({"status": "ok", "action": "cancel", "job": asdict(job), "state_file": str(state_path)})
    return 0


def command_run_next(args: argparse.Namespace) -> int:
    state_path = pathlib.Path(args.state_file).resolve()
    lock_path = state_path.with_suffix(state_path.suffix + ".lock")
    run_eval_cycle_path = pathlib.Path(args.run_eval_cycle).resolve()
    if not run_eval_cycle_path.exists():
        raise FileNotFoundError(f"run_eval_cycle.py not found: {run_eval_cycle_path}")

    with FileLock(lock_path=lock_path, timeout_seconds=args.lock_timeout_seconds):
        state = load_state(state_path)
        jobs = parse_jobs(state)
        selected = pick_pending_job(jobs, explicit_job_id=args.job_id)
        if selected is None:
            format_output({"status": "empty", "action": "run-next", "state_file": str(state_path)})
            return 2

        preview_command = [
            args.python_executable,
            str(run_eval_cycle_path),
            "--run-id",
            selected.run_id,
            selected.eval_command,
            *selected.eval_args,
        ]
        if args.dry_run:
            format_output(
                {
                    "status": "ok",
                    "action": "run-next",
                    "dry_run": True,
                    "job_id": selected.id,
                    "command": preview_command,
                    "state_file": str(state_path),
                }
            )
            return 0

        selected.status = "running"
        selected.attempts += 1
        selected.claim_owner = args.worker
        selected.updated_at_utc = now_utc_iso()
        append_event(selected, event="claim", worker=args.worker, note="run-next claim")
        state.updated_at_utc = now_utc_iso()
        state.jobs = jobs_to_payload(jobs)
        write_state(state_path, state)
        selected_id = selected.id
        command = preview_command

    completed = subprocess.run(command, check=False)
    with FileLock(lock_path=lock_path, timeout_seconds=args.lock_timeout_seconds):
        state = load_state(state_path)
        jobs = parse_jobs(state)
        job = find_job(jobs, selected_id)

        if completed.returncode == 0:
            job.status = "done"
            job.last_error = None
            job.updated_at_utc = now_utc_iso()
            append_event(job, event="complete", worker=args.worker, note="run-next success")
        else:
            should_block = job.attempts >= job.max_retries
            job.status = "blocked" if should_block else "pending"
            job.last_error = f"run_eval_cycle exit code {completed.returncode}"
            job.claim_owner = args.worker if should_block else None
            job.updated_at_utc = now_utc_iso()
            append_event(job, event="fail", worker=args.worker, note=job.last_error)

        state.updated_at_utc = now_utc_iso()
        state.jobs = jobs_to_payload(jobs)
        write_state(state_path, state)
        format_output(
            {
                "status": "ok" if completed.returncode == 0 else "error",
                "action": "run-next",
                "job": asdict(job),
                "command": command,
                "exit_code": completed.returncode,
                "state_file": str(state_path),
            }
        )
    return completed.returncode


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Deterministic file-backed queue manager for recurring run_eval_cycle experiments."
    )
    parser.add_argument(
        "--state-file",
        default="logs\\eval-cycle\\experiment-queue.json",
        help="Queue state file path (default: logs\\eval-cycle\\experiment-queue.json)",
    )
    parser.add_argument(
        "--lock-timeout-seconds",
        type=float,
        default=10.0,
        help="Max time to wait for lock acquisition (default: 10.0)",
    )
    parser.add_argument("--worker", default="queue-worker", help="Worker/operator identity for lifecycle events")
    sub = parser.add_subparsers(dest="command", required=True)

    enqueue = sub.add_parser("enqueue", help="Add a pending experiment job")
    enqueue.add_argument("--job-id", default="", help="Optional explicit stable job ID")
    enqueue.add_argument("--run-id", required=True, help="run_eval_cycle --run-id value for this job")
    enqueue.add_argument("--eval-command", default="full-cycle", help="run_eval_cycle command to run")
    enqueue.add_argument("--eval-arg", action="append", default=[], help="Repeatable arg for run_eval_cycle command")
    enqueue.add_argument("--description", default="", help="Optional free-text job description")
    enqueue.add_argument("--tag", action="append", default=[], help="Repeatable job tag")
    enqueue.add_argument("--priority", type=int, default=0, help="Higher priority is claimed first (default: 0)")
    enqueue.add_argument("--max-retries", type=int, default=3, help="Max attempts before fail transitions to blocked")
    enqueue.add_argument("--note", default="", help="Optional lifecycle note")
    enqueue.set_defaults(func=command_enqueue)

    ls = sub.add_parser("list", help="List jobs in deterministic execution order")
    ls.add_argument("--status", action="append", choices=QUEUE_STATUSES, help="Optional status filter (repeatable)")
    ls.add_argument("--limit", type=int, default=None, help="Optional max returned jobs")
    ls.set_defaults(func=command_list)

    claim = sub.add_parser("claim", help="Claim next pending job (or explicit --job-id)")
    claim.add_argument("--job-id", default="", help="Optional explicit pending job ID")
    claim.add_argument("--note", default="", help="Optional lifecycle note")
    claim.set_defaults(func=command_claim)

    complete = sub.add_parser("complete", help="Mark a running/pending job as done")
    complete.add_argument("--job-id", required=True, help="Job ID")
    complete.add_argument("--note", default="", help="Optional lifecycle note")
    complete.set_defaults(func=command_complete)

    fail = sub.add_parser("fail", help="Mark a running/pending job as failed")
    fail.add_argument("--job-id", required=True, help="Job ID")
    fail.add_argument("--error", required=True, help="Failure reason")
    fail.add_argument("--blocked", action="store_true", help="Force transition to blocked")
    fail.add_argument("--note", default="", help="Optional lifecycle note")
    fail.set_defaults(func=command_fail)

    retry = sub.add_parser("retry", help="Move blocked/cancelled job back to pending")
    retry.add_argument("--job-id", required=True, help="Job ID")
    retry.add_argument("--reset-attempts", action="store_true", help="Reset attempts back to zero")
    retry.add_argument("--note", default="", help="Optional lifecycle note")
    retry.set_defaults(func=command_retry)

    cancel = sub.add_parser("cancel", help="Cancel a non-terminal job")
    cancel.add_argument("--job-id", required=True, help="Job ID")
    cancel.add_argument("--note", default="", help="Optional lifecycle note")
    cancel.set_defaults(func=command_cancel)

    run_next = sub.add_parser(
        "run-next",
        help="Claim next pending job, execute run_eval_cycle, and auto-complete or fail/requeue",
    )
    run_next.add_argument("--job-id", default="", help="Optional explicit pending job ID")
    run_next.add_argument(
        "--run-eval-cycle",
        default=str(pathlib.Path(__file__).resolve().parent / "run_eval_cycle.py"),
        help="Path to run_eval_cycle.py",
    )
    run_next.add_argument("--python-executable", default=sys.executable, help="Python executable for subprocess")
    run_next.add_argument("--dry-run", action="store_true", help="Claim and print command without executing")
    run_next.set_defaults(func=command_run_next)
    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    job_id = getattr(args, "job_id", "")
    if isinstance(job_id, str) and job_id.strip():
        args.job_id = normalize_job_id(job_id)
    return int(args.func(args))


if __name__ == "__main__":
    raise SystemExit(main())
