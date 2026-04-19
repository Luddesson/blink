#!/usr/bin/env python3
from __future__ import annotations

import argparse
import csv
import datetime as dt
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable


TIMESTAMP_KEYS = ("timestamp_ms", "timestamp", "ts", "captured_at_utc", "time")
TOKEN_KEYS = ("token_id", "asset_id", "market_id")
SIGNAL_SIDE_KEYS = ("side", "signal_side", "direction")
SIGNAL_PRICE_KEYS = ("entry_price_scaled", "entry_price", "price_scaled", "price")
PRICE_KEYS = ("price_scaled", "yes_price_scaled", "mid_price_scaled", "price", "yes_price")


@dataclass(frozen=True)
class SignalRow:
    signal_id: str
    timestamp_ms: int
    token_id: str
    side: str
    entry_price_scaled: float


@dataclass(frozen=True)
class PriceRow:
    timestamp_ms: int
    token_id: str
    price_scaled: float


@dataclass(frozen=True)
class LabelResult:
    signal_id: str
    token_id: str
    side: str
    signal_timestamp_ms: int
    entry_price_scaled: float
    horizon_ms: int
    stop_loss_bps: int
    take_profit_bps: int
    hit_timestamp_ms: int | None
    hit_type: str
    triple_label: int
    meta_label: int
    realized_return_bps: float | None
    bars_seen: int


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Offline triple-barrier meta-labeling for Blink signal datasets."
    )
    parser.add_argument("--signals", required=True, help="Path to signal events (.csv, .json, .jsonl).")
    parser.add_argument(
        "--prices",
        default="",
        help="Path to price events (.csv, .json, .jsonl). Defaults to --signals when omitted.",
    )
    parser.add_argument(
        "--horizons-minutes",
        default="5,15,30",
        help="Comma-separated time horizons in minutes (example: 5,15,30).",
    )
    parser.add_argument(
        "--take-profit-bps",
        type=int,
        default=150,
        help="Upper barrier in bps along signal direction (default: 150).",
    )
    parser.add_argument(
        "--stop-loss-bps",
        type=int,
        default=100,
        help="Lower barrier in bps along signal direction (default: 100).",
    )
    parser.add_argument(
        "--min-hold-bars",
        type=int,
        default=1,
        help="Require at least this many forward price bars before labeling (default: 1).",
    )
    parser.add_argument(
        "--out-dir",
        default="logs\\meta-labeling",
        help="Output directory for labels and summary (default: logs\\meta-labeling).",
    )
    parser.add_argument(
        "--labels-file",
        default="labels.csv",
        help="Output CSV filename under --out-dir (default: labels.csv).",
    )
    parser.add_argument(
        "--summary-file",
        default="summary.json",
        help="Output summary filename under --out-dir (default: summary.json).",
    )
    return parser.parse_args()


def _choose_key(row: dict[str, Any], keys: Iterable[str]) -> str | None:
    for key in keys:
        if key in row:
            return key
    return None


def _parse_timestamp_ms(value: Any) -> int | None:
    if isinstance(value, (int, float)):
        v = int(value)
        if v <= 0:
            return None
        if v < 10_000_000_000:
            return v * 1000
        return v
    if isinstance(value, str):
        s = value.strip()
        if not s:
            return None
        if s.isdigit():
            return _parse_timestamp_ms(int(s))
        try:
            normalized = s.replace("Z", "+00:00")
            parsed = dt.datetime.fromisoformat(normalized)
            if parsed.tzinfo is None:
                parsed = parsed.replace(tzinfo=dt.timezone.utc)
            return int(parsed.timestamp() * 1000)
        except ValueError:
            return None
    return None


def _parse_price_scaled(value: Any) -> float | None:
    if isinstance(value, (int, float)):
        raw = float(value)
    elif isinstance(value, str):
        s = value.strip()
        if not s:
            return None
        try:
            raw = float(s)
        except ValueError:
            return None
    else:
        return None

    if raw <= 0:
        return None
    return raw if raw > 1.0 else raw * 1000.0


def _load_rows(path: Path) -> list[dict[str, Any]]:
    suffix = path.suffix.lower()
    if suffix == ".csv":
        with path.open("r", encoding="utf-8", newline="") as handle:
            return [dict(row) for row in csv.DictReader(handle)]

    if suffix in {".jsonl", ".ndjson"}:
        rows: list[dict[str, Any]] = []
        with path.open("r", encoding="utf-8") as handle:
            for line in handle:
                line = line.strip()
                if not line:
                    continue
                payload = json.loads(line)
                if isinstance(payload, dict):
                    rows.append(payload)
        return rows

    payload = json.loads(path.read_text(encoding="utf-8"))
    if isinstance(payload, list):
        return [row for row in payload if isinstance(row, dict)]
    if isinstance(payload, dict):
        for key in ("rows", "events", "signals", "prices", "data"):
            value = payload.get(key)
            if isinstance(value, list):
                return [row for row in value if isinstance(row, dict)]
    raise ValueError(f"Unsupported JSON shape in {path}")


def _normalize_side(side_raw: str) -> str | None:
    s = side_raw.strip().upper()
    if s in {"YES", "BUY", "LONG"}:
        return "YES"
    if s in {"NO", "SELL", "SHORT"}:
        return "NO"
    return None


def _extract_signals(rows: list[dict[str, Any]]) -> list[SignalRow]:
    signals: list[SignalRow] = []
    for idx, row in enumerate(rows):
        ts_key = _choose_key(row, TIMESTAMP_KEYS)
        token_key = _choose_key(row, TOKEN_KEYS)
        side_key = _choose_key(row, SIGNAL_SIDE_KEYS)
        price_key = _choose_key(row, SIGNAL_PRICE_KEYS)
        if not all([ts_key, token_key, side_key, price_key]):
            continue

        ts_ms = _parse_timestamp_ms(row.get(ts_key))
        token_id = str(row.get(token_key, "")).strip()
        side = _normalize_side(str(row.get(side_key, "")))
        price_scaled = _parse_price_scaled(row.get(price_key))
        if ts_ms is None or not token_id or side is None or price_scaled is None:
            continue

        provided_id = str(row.get("signal_id", "")).strip()
        signal_id = provided_id if provided_id else f"{token_id}-{ts_ms}-{idx}"
        signals.append(
            SignalRow(
                signal_id=signal_id,
                timestamp_ms=ts_ms,
                token_id=token_id,
                side=side,
                entry_price_scaled=price_scaled,
            )
        )
    signals.sort(key=lambda s: s.timestamp_ms)
    return signals


def _extract_prices(rows: list[dict[str, Any]]) -> list[PriceRow]:
    prices: list[PriceRow] = []
    for row in rows:
        ts_key = _choose_key(row, TIMESTAMP_KEYS)
        token_key = _choose_key(row, TOKEN_KEYS)
        price_key = _choose_key(row, PRICE_KEYS)
        if not all([ts_key, token_key, price_key]):
            continue

        ts_ms = _parse_timestamp_ms(row.get(ts_key))
        token_id = str(row.get(token_key, "")).strip()
        price_scaled = _parse_price_scaled(row.get(price_key))
        if ts_ms is None or not token_id or price_scaled is None:
            continue
        prices.append(PriceRow(timestamp_ms=ts_ms, token_id=token_id, price_scaled=price_scaled))

    prices.sort(key=lambda p: (p.token_id, p.timestamp_ms))
    return prices


def _label_signal(
    signal: SignalRow,
    price_rows: list[PriceRow],
    horizon_ms: int,
    take_profit_bps: int,
    stop_loss_bps: int,
    min_hold_bars: int,
) -> LabelResult:
    upper = take_profit_bps / 10_000.0
    lower = -stop_loss_bps / 10_000.0
    side_mult = 1.0 if signal.side == "YES" else -1.0

    end_ms = signal.timestamp_ms + horizon_ms
    forward = [p for p in price_rows if signal.timestamp_ms < p.timestamp_ms <= end_ms]
    if len(forward) < min_hold_bars:
        return LabelResult(
            signal_id=signal.signal_id,
            token_id=signal.token_id,
            side=signal.side,
            signal_timestamp_ms=signal.timestamp_ms,
            entry_price_scaled=signal.entry_price_scaled,
            horizon_ms=horizon_ms,
            stop_loss_bps=stop_loss_bps,
            take_profit_bps=take_profit_bps,
            hit_timestamp_ms=None,
            hit_type="insufficient_data",
            triple_label=0,
            meta_label=0,
            realized_return_bps=None,
            bars_seen=len(forward),
        )

    for bar in forward:
        ret = side_mult * ((bar.price_scaled - signal.entry_price_scaled) / signal.entry_price_scaled)
        if ret >= upper:
            return LabelResult(
                signal_id=signal.signal_id,
                token_id=signal.token_id,
                side=signal.side,
                signal_timestamp_ms=signal.timestamp_ms,
                entry_price_scaled=signal.entry_price_scaled,
                horizon_ms=horizon_ms,
                stop_loss_bps=stop_loss_bps,
                take_profit_bps=take_profit_bps,
                hit_timestamp_ms=bar.timestamp_ms,
                hit_type="take_profit",
                triple_label=1,
                meta_label=1,
                realized_return_bps=ret * 10_000.0,
                bars_seen=len(forward),
            )
        if ret <= lower:
            return LabelResult(
                signal_id=signal.signal_id,
                token_id=signal.token_id,
                side=signal.side,
                signal_timestamp_ms=signal.timestamp_ms,
                entry_price_scaled=signal.entry_price_scaled,
                horizon_ms=horizon_ms,
                stop_loss_bps=stop_loss_bps,
                take_profit_bps=take_profit_bps,
                hit_timestamp_ms=bar.timestamp_ms,
                hit_type="stop_loss",
                triple_label=-1,
                meta_label=0,
                realized_return_bps=ret * 10_000.0,
                bars_seen=len(forward),
            )

    terminal = forward[-1]
    terminal_ret = side_mult * ((terminal.price_scaled - signal.entry_price_scaled) / signal.entry_price_scaled)
    label = 1 if terminal_ret > 0 else (-1 if terminal_ret < 0 else 0)
    return LabelResult(
        signal_id=signal.signal_id,
        token_id=signal.token_id,
        side=signal.side,
        signal_timestamp_ms=signal.timestamp_ms,
        entry_price_scaled=signal.entry_price_scaled,
        horizon_ms=horizon_ms,
        stop_loss_bps=stop_loss_bps,
        take_profit_bps=take_profit_bps,
        hit_timestamp_ms=terminal.timestamp_ms,
        hit_type="vertical_barrier",
        triple_label=label,
        meta_label=1 if label == 1 else 0,
        realized_return_bps=terminal_ret * 10_000.0,
        bars_seen=len(forward),
    )


def _write_labels_csv(path: Path, labels: list[LabelResult]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fieldnames = list(LabelResult.__dataclass_fields__.keys())
    with path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=fieldnames)
        writer.writeheader()
        for row in labels:
            writer.writerow(row.__dict__)


def _build_summary(labels: list[LabelResult], signals_total: int) -> dict[str, Any]:
    horizons: dict[str, Any] = {}
    for label in labels:
        key = str(label.horizon_ms)
        bucket = horizons.setdefault(
            key,
            {
                "count": 0,
                "meta_positive": 0,
                "take_profit_hits": 0,
                "stop_loss_hits": 0,
                "vertical_barrier_hits": 0,
                "insufficient_data": 0,
                "avg_return_bps": 0.0,
            },
        )
        bucket["count"] += 1
        bucket["meta_positive"] += label.meta_label
        if label.hit_type == "take_profit":
            bucket["take_profit_hits"] += 1
        elif label.hit_type == "stop_loss":
            bucket["stop_loss_hits"] += 1
        elif label.hit_type == "vertical_barrier":
            bucket["vertical_barrier_hits"] += 1
        elif label.hit_type == "insufficient_data":
            bucket["insufficient_data"] += 1
        if label.realized_return_bps is not None:
            bucket["avg_return_bps"] += label.realized_return_bps

    for bucket in horizons.values():
        denom = max(bucket["count"] - bucket["insufficient_data"], 1)
        bucket["meta_positive_rate"] = round(bucket["meta_positive"] / max(bucket["count"], 1), 6)
        bucket["avg_return_bps"] = round(bucket["avg_return_bps"] / denom, 4)

    return {
        "generated_at_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "signals_total": signals_total,
        "labels_total": len(labels),
        "horizons": horizons,
    }


def _write_summary(path: Path, summary: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(summary, indent=2, sort_keys=True), encoding="utf-8")


def main() -> int:
    args = parse_args()
    horizons_minutes = [int(part.strip()) for part in args.horizons_minutes.split(",") if part.strip()]
    if not horizons_minutes:
        raise ValueError("At least one horizon is required in --horizons-minutes.")
    if args.take_profit_bps <= 0 or args.stop_loss_bps <= 0:
        raise ValueError("--take-profit-bps and --stop-loss-bps must be positive.")
    if args.min_hold_bars <= 0:
        raise ValueError("--min-hold-bars must be positive.")

    signals_path = Path(args.signals)
    prices_path = Path(args.prices) if args.prices else signals_path
    signals = _extract_signals(_load_rows(signals_path))
    prices = _extract_prices(_load_rows(prices_path))

    if not signals:
        raise RuntimeError("No usable signal rows found.")
    if not prices:
        raise RuntimeError("No usable price rows found.")

    prices_by_token: dict[str, list[PriceRow]] = {}
    for row in prices:
        prices_by_token.setdefault(row.token_id, []).append(row)

    labels: list[LabelResult] = []
    for signal in signals:
        token_prices = prices_by_token.get(signal.token_id, [])
        for horizon_min in horizons_minutes:
            labels.append(
                _label_signal(
                    signal=signal,
                    price_rows=token_prices,
                    horizon_ms=horizon_min * 60_000,
                    take_profit_bps=args.take_profit_bps,
                    stop_loss_bps=args.stop_loss_bps,
                    min_hold_bars=args.min_hold_bars,
                )
            )

    out_dir = Path(args.out_dir)
    labels_path = out_dir / args.labels_file
    summary_path = out_dir / args.summary_file

    _write_labels_csv(labels_path, labels)
    summary = _build_summary(labels, signals_total=len(signals))
    _write_summary(summary_path, summary)

    print(f"Wrote labels: {labels_path}")
    print(f"Wrote summary: {summary_path}")
    print(f"Labeled {len(signals)} signals across {len(horizons_minutes)} horizons.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
