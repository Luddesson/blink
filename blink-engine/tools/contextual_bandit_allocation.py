#!/usr/bin/env python3
from __future__ import annotations

import argparse
import csv
import datetime as dt
import json
import math
import random
from dataclasses import dataclass
from pathlib import Path
from typing import Any


ARMS = ("mirror", "conservative", "aggressive")


@dataclass(frozen=True)
class BanditExample:
    run_id: str
    timestamp_utc: str
    logged_arm: str
    reward: float
    context: tuple[float, ...]
    quality_score: float
    source: str


@dataclass(frozen=True)
class PolicyDecision:
    chosen_arm: str
    scores: dict[str, float]
    means: dict[str, float]
    uncertainties: dict[str, float]
    probabilities: dict[str, float]
    effective_alpha: float
    guardrail_applied: bool
    exploration_mode: str


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Offline contextual bandit simulation for strategy allocation across mirror/conservative/aggressive."
    )
    parser.add_argument(
        "--eval-root",
        default="logs\\eval-cycle",
        help="Root directory containing eval run subfolders with report.json/fingerprint.json (default: logs\\eval-cycle).",
    )
    parser.add_argument(
        "--report-file",
        action="append",
        default=[],
        help="Optional explicit report.json path (repeatable).",
    )
    parser.add_argument(
        "--algorithm",
        choices=("linucb", "linucb-safe", "epsilon-greedy"),
        default="linucb-safe",
        help="Bandit algorithm variant (default: linucb-safe).",
    )
    parser.add_argument(
        "--alpha",
        type=float,
        default=0.6,
        help="Exploration multiplier for LinUCB variants (default: 0.6).",
    )
    parser.add_argument(
        "--epsilon",
        type=float,
        default=0.12,
        help="Exploration rate for epsilon-greedy (default: 0.12).",
    )
    parser.add_argument(
        "--ridge-lambda",
        type=float,
        default=1.0,
        help="Ridge regularization lambda for linear models (default: 1.0).",
    )
    parser.add_argument(
        "--allocation-temperature",
        type=float,
        default=0.75,
        help="Softmax temperature for allocation probabilities (default: 0.75).",
    )
    parser.add_argument(
        "--min-arm-prob",
        type=float,
        default=0.05,
        help="Safety floor per arm probability for safe policy (default: 0.05).",
    )
    parser.add_argument(
        "--max-arm-prob",
        type=float,
        default=0.80,
        help="Safety cap per arm probability for safe policy (default: 0.80).",
    )
    parser.add_argument(
        "--max-aggressive-prob",
        type=float,
        default=0.55,
        help="Safety cap for aggressive arm probability in safe policy (default: 0.55).",
    )
    parser.add_argument(
        "--quality-floor",
        type=float,
        default=0.55,
        help="Quality threshold used by safe policy risk controls (default: 0.55).",
    )
    parser.add_argument(
        "--synthetic-steps",
        type=int,
        default=0,
        help="Force synthetic replay sample count (default: 0, meaning use artifacts if found).",
    )
    parser.add_argument(
        "--no-fallback-synthetic",
        action="store_true",
        help="Fail if no compatible artifacts are found (instead of auto-fallback to synthetic).",
    )
    parser.add_argument(
        "--seed",
        type=int,
        default=7,
        help="RNG seed for reproducible exploration/synthetic generation (default: 7).",
    )
    parser.add_argument(
        "--ips-weight-cap",
        type=float,
        default=8.0,
        help="Clip cap for IPS weights in off-policy evaluation (default: 8.0).",
    )
    parser.add_argument(
        "--logging-propensity-floor",
        type=float,
        default=0.05,
        help="Minimum estimated logging propensity per arm (default: 0.05).",
    )
    parser.add_argument(
        "--out-dir",
        default="logs\\bandit-allocation-sim",
        help="Output directory for summary + history artifacts (default: logs\\bandit-allocation-sim).",
    )
    parser.add_argument(
        "--summary-file",
        default="summary.json",
        help="Summary JSON filename under --out-dir (default: summary.json).",
    )
    parser.add_argument(
        "--history-json",
        default="allocation-history.json",
        help="History JSON filename under --out-dir (default: allocation-history.json).",
    )
    parser.add_argument(
        "--history-csv",
        default="allocation-history.csv",
        help="History CSV filename under --out-dir (default: allocation-history.csv).",
    )
    return parser.parse_args()


def _safe_float(value: Any) -> float | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return float(value)
    if isinstance(value, str):
        text = value.strip()
        if not text:
            return None
        try:
            return float(text)
        except ValueError:
            return None
    return None


def _clamp(value: float, lo: float, hi: float) -> float:
    return max(lo, min(hi, value))


def _pick_nested_scalar(payload: dict[str, Any], keys: tuple[str, ...]) -> float | None:
    stack: list[Any] = [payload]
    while stack:
        current = stack.pop()
        if isinstance(current, dict):
            for key in keys:
                val = _safe_float(current.get(key))
                if val is not None:
                    return val
            stack.extend(current.values())
        elif isinstance(current, list):
            stack.extend(current)
    return None


def _load_json_obj(path: Path) -> dict[str, Any]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise ValueError(f"Expected JSON object in {path}")
    return payload


def _infer_mode(report: dict[str, Any], fingerprint: dict[str, Any] | None) -> str | None:
    if fingerprint is not None:
        mode = str(fingerprint.get("strategy_mode_hint", "")).strip().lower()
        if mode in ARMS:
            return mode

    tags = report.get("decision_tags")
    if isinstance(tags, list):
        for tag in tags:
            if isinstance(tag, str) and tag.lower().startswith("strategy:"):
                mode = tag.split(":", 1)[1].strip().lower()
                if mode in ARMS:
                    return mode
    return None


def _feature_reward_from_report(report: dict[str, Any]) -> tuple[tuple[float, ...], float, float]:
    window = report.get("window", {})
    funnel = window.get("funnel_delta", {}) if isinstance(window, dict) else {}

    signals = _safe_float(funnel.get("signals")) or 0.0
    accepted = _safe_float(funnel.get("accepted")) or 0.0
    fills = _safe_float(funnel.get("fills")) or 0.0
    aborts = _safe_float(funnel.get("aborts")) or 0.0
    rejections = _safe_float(funnel.get("rejections")) or 0.0

    signals_safe = max(signals, 1.0)
    accepted_safe = max(accepted, 1.0)

    accept_rate = _clamp(accepted / signals_safe, 0.0, 1.0)
    fill_rate = _clamp(fills / accepted_safe, 0.0, 1.0)
    abort_rate = _clamp(aborts / signals_safe, 0.0, 1.0)
    rejection_rate = _clamp(rejections / signals_safe, 0.0, 1.0)

    health = report.get("health", {})
    ws_connected_ratio = _clamp(_safe_float(health.get("ws_connected_ratio")) or 0.0, 0.0, 1.0)

    gate_rows = report.get("gate_pressure_top5_run_window")
    gate_pressure = 0.0
    if isinstance(gate_rows, list):
        gate_pressure = sum(
            _safe_float(row.get("rejections_delta")) or 0.0
            for row in gate_rows
            if isinstance(row, dict)
        )
    gate_pressure_norm = _clamp(gate_pressure / signals_safe, 0.0, 1.0)

    quality_score = _clamp(
        (0.35 * accept_rate)
        + (0.35 * fill_rate)
        + (0.20 * ws_connected_ratio)
        - (0.05 * rejection_rate)
        - (0.05 * abort_rate),
        -1.0,
        1.0,
    )

    pnl_return_pct = _pick_nested_scalar(report, ("pnl_return_pct", "return_pct", "net_return_pct", "roi_pct"))
    fee_drag_pct = _pick_nested_scalar(report, ("fee_drag_pct", "fee_drag", "fee_drag_percent"))
    pnl_component = math.tanh((pnl_return_pct or 0.0) / 5.0)
    fee_penalty = math.tanh((fee_drag_pct or 0.0) / 100.0)

    reward = _clamp((0.70 * quality_score) + (0.35 * pnl_component) - (0.20 * fee_penalty), -1.0, 1.0)
    context = (
        1.0,
        quality_score,
        accept_rate,
        fill_rate,
        ws_connected_ratio,
        1.0 - rejection_rate,
        1.0 - gate_pressure_norm,
    )
    return context, quality_score, reward


def _collect_examples(eval_root: Path, report_files: list[str]) -> tuple[list[BanditExample], list[str]]:
    warnings: list[str] = []
    paths: list[Path] = []

    for explicit in report_files:
        p = Path(explicit).resolve()
        if p.exists():
            paths.append(p)
        else:
            warnings.append(f"explicit report missing: {p}")

    if eval_root.exists():
        paths.extend(sorted(eval_root.rglob("report.json")))
    else:
        warnings.append(f"eval root missing: {eval_root}")

    dedup: dict[str, Path] = {str(path.resolve()): path for path in paths}
    examples: list[BanditExample] = []

    for report_path in sorted(dedup.values()):
        try:
            report = _load_json_obj(report_path)
        except (ValueError, json.JSONDecodeError) as exc:
            warnings.append(f"invalid report skipped: {report_path} ({exc})")
            continue

        fingerprint_path = report_path.parent / "fingerprint.json"
        fingerprint: dict[str, Any] | None = None
        if fingerprint_path.exists():
            try:
                fingerprint = _load_json_obj(fingerprint_path)
            except (ValueError, json.JSONDecodeError):
                warnings.append(f"invalid fingerprint ignored: {fingerprint_path}")

        mode = _infer_mode(report, fingerprint)
        if mode not in ARMS:
            warnings.append(f"strategy mode missing/unsupported in {report_path}; skipping")
            continue

        context, quality, reward = _feature_reward_from_report(report)
        run_id = str(report.get("run_id") or report_path.parent.name)
        timestamp = str(report.get("generated_at_utc") or dt.datetime.now(dt.timezone.utc).isoformat())
        examples.append(
            BanditExample(
                run_id=run_id,
                timestamp_utc=timestamp,
                logged_arm=mode,
                reward=reward,
                context=context,
                quality_score=quality,
                source=str(report_path),
            )
        )

    examples.sort(key=lambda ex: ex.timestamp_utc)
    return examples, warnings


def _synthetic_examples(steps: int, rng: random.Random) -> list[BanditExample]:
    examples: list[BanditExample] = []
    start = dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc)
    for idx in range(max(steps, 1)):
        t = idx / max(steps - 1, 1)
        cyc = 0.5 + 0.5 * math.sin(t * math.pi * 4.0)
        quality = _clamp(cyc + rng.uniform(-0.12, 0.12), 0.0, 1.0)
        accept_rate = _clamp(0.45 + 0.4 * quality + rng.uniform(-0.08, 0.08), 0.0, 1.0)
        fill_rate = _clamp(0.40 + 0.45 * quality + rng.uniform(-0.08, 0.08), 0.0, 1.0)
        ws_ratio = _clamp(0.94 + rng.uniform(-0.05, 0.03), 0.0, 1.0)
        rejection_rate = _clamp(1.0 - accept_rate + rng.uniform(-0.05, 0.05), 0.0, 1.0)
        gate_inv = _clamp(0.9 - rejection_rate, 0.0, 1.0)
        context = (1.0, quality, accept_rate, fill_rate, ws_ratio, 1.0 - rejection_rate, gate_inv)

        rewards = {
            "conservative": _clamp(0.70 * (1.0 - quality) - 0.18 + rng.uniform(-0.08, 0.08), -1.0, 1.0),
            "mirror": _clamp(0.70 - abs(quality - 0.5) * 1.35 + rng.uniform(-0.08, 0.08), -1.0, 1.0),
            "aggressive": _clamp(0.70 * quality - 0.18 + rng.uniform(-0.08, 0.08), -1.0, 1.0),
        }
        logged_arm = max(rewards, key=rewards.get) if rng.random() > 0.30 else rng.choice(list(ARMS))
        reward = rewards[logged_arm]
        ts = (start + dt.timedelta(minutes=idx * 15)).isoformat()
        examples.append(
            BanditExample(
                run_id=f"synthetic-{idx:04d}",
                timestamp_utc=ts,
                logged_arm=logged_arm,
                reward=reward,
                context=context,
                quality_score=quality,
                source="synthetic",
            )
        )
    return examples


def _mat_identity(n: int, diagonal: float = 1.0) -> list[list[float]]:
    return [[diagonal if i == j else 0.0 for j in range(n)] for i in range(n)]


def _dot(a: tuple[float, ...], b: list[float]) -> float:
    return sum(x * y for x, y in zip(a, b))


def _mat_vec_mul(m: list[list[float]], v: tuple[float, ...]) -> list[float]:
    return [sum(row[i] * v[i] for i in range(len(v))) for row in m]


def _mat_inv(matrix: list[list[float]]) -> list[list[float]]:
    n = len(matrix)
    aug = [row[:] + ident for row, ident in zip(matrix, _mat_identity(n))]
    for i in range(n):
        pivot = max(range(i, n), key=lambda r: abs(aug[r][i]))
        if abs(aug[pivot][i]) < 1e-12:
            return _mat_identity(n)
        if pivot != i:
            aug[i], aug[pivot] = aug[pivot], aug[i]
        factor = aug[i][i]
        aug[i] = [x / factor for x in aug[i]]
        for r in range(n):
            if r == i:
                continue
            coeff = aug[r][i]
            if coeff == 0.0:
                continue
            aug[r] = [rv - coeff * iv for rv, iv in zip(aug[r], aug[i])]
    return [row[n:] for row in aug]


def _softmax(scores: dict[str, float], temperature: float) -> dict[str, float]:
    temp = max(temperature, 1e-6)
    max_score = max(scores.values())
    exps = {arm: math.exp((score - max_score) / temp) for arm, score in scores.items()}
    total = sum(exps.values())
    if total <= 0.0:
        return {arm: 1.0 / len(ARMS) for arm in ARMS}
    return {arm: exps[arm] / total for arm in ARMS}


def _project_probabilities(
    raw: dict[str, float],
    min_probs: dict[str, float],
    max_probs: dict[str, float],
) -> tuple[dict[str, float], bool]:
    projected = {arm: max(0.0, raw.get(arm, 0.0)) for arm in ARMS}
    total = sum(projected.values())
    if total <= 0.0:
        projected = {arm: 1.0 / len(ARMS) for arm in ARMS}
    else:
        projected = {arm: projected[arm] / total for arm in ARMS}

    result: dict[str, float] = {}
    free = set(ARMS)
    remaining_mass = 1.0
    changed = False

    while free:
        free_mass = sum(projected[arm] for arm in free)
        if free_mass <= 0.0:
            for arm in sorted(free):
                projected[arm] = 1.0
            free_mass = float(len(free))

        violated = False
        for arm in sorted(free):
            proposal = remaining_mass * (projected[arm] / free_mass)
            floor = min_probs[arm]
            cap = max_probs[arm]
            if proposal < floor - 1e-12:
                result[arm] = floor
                remaining_mass -= floor
                free.remove(arm)
                violated = True
                changed = True
                break
            if proposal > cap + 1e-12:
                result[arm] = cap
                remaining_mass -= cap
                free.remove(arm)
                violated = True
                changed = True
                break
        if not violated:
            for arm in sorted(free):
                result[arm] = remaining_mass * (projected[arm] / free_mass)
            break

    normalized_total = sum(result.values())
    if normalized_total <= 0.0:
        result = {arm: 1.0 / len(ARMS) for arm in ARMS}
    else:
        result = {arm: _clamp(result[arm] / normalized_total, 0.0, 1.0) for arm in ARMS}
    return result, changed


class LinearPolicy:
    def __init__(self, dim: int, ridge_lambda: float) -> None:
        self.dim = dim
        self.A = {arm: _mat_identity(dim, diagonal=ridge_lambda) for arm in ARMS}
        self.b = {arm: [0.0 for _ in range(dim)] for arm in ARMS}

    def _theta(self, arm: str) -> tuple[list[list[float]], list[float]]:
        inv = _mat_inv(self.A[arm])
        return inv, _mat_vec_mul(inv, tuple(self.b[arm]))

    def update(self, arm: str, context: tuple[float, ...], reward: float) -> None:
        mat = self.A[arm]
        for i in range(self.dim):
            for j in range(self.dim):
                mat[i][j] += context[i] * context[j]
            self.b[arm][i] += reward * context[i]

    def predict_means(self, context: tuple[float, ...]) -> dict[str, float]:
        means: dict[str, float] = {}
        for arm in ARMS:
            _, theta = self._theta(arm)
            means[arm] = _dot(context, theta)
        return means


class LinUCBPolicy(LinearPolicy):
    def __init__(self, dim: int, ridge_lambda: float, alpha: float, allocation_temperature: float) -> None:
        super().__init__(dim=dim, ridge_lambda=ridge_lambda)
        self.alpha = alpha
        self.allocation_temperature = allocation_temperature

    def _effective_alpha(self, quality: float) -> float:
        return self.alpha

    def select(self, context: tuple[float, ...], quality: float) -> PolicyDecision:
        means: dict[str, float] = {}
        uncertainties: dict[str, float] = {}
        alpha = self._effective_alpha(quality)
        scores: dict[str, float] = {}
        for arm in ARMS:
            inv, theta = self._theta(arm)
            mean = _dot(context, theta)
            variance = max(_dot(context, _mat_vec_mul(inv, context)), 0.0)
            uncertainty = math.sqrt(variance)
            means[arm] = mean
            uncertainties[arm] = uncertainty
            scores[arm] = mean + (alpha * uncertainty)
        probabilities = _softmax(scores, self.allocation_temperature)
        chosen = max(ARMS, key=lambda arm: (probabilities[arm], scores[arm], arm))
        return PolicyDecision(
            chosen_arm=chosen,
            scores=scores,
            means=means,
            uncertainties=uncertainties,
            probabilities=probabilities,
            effective_alpha=alpha,
            guardrail_applied=False,
            exploration_mode="linucb",
        )


class SafeLinUCBPolicy(LinUCBPolicy):
    def __init__(
        self,
        dim: int,
        ridge_lambda: float,
        alpha: float,
        allocation_temperature: float,
        min_arm_prob: float,
        max_arm_prob: float,
        max_aggressive_prob: float,
        quality_floor: float,
    ) -> None:
        super().__init__(dim=dim, ridge_lambda=ridge_lambda, alpha=alpha, allocation_temperature=allocation_temperature)
        self.min_arm_prob = min_arm_prob
        self.max_arm_prob = max_arm_prob
        self.max_aggressive_prob = max_aggressive_prob
        self.quality_floor = quality_floor

    def _effective_alpha(self, quality: float) -> float:
        quality_factor = _clamp(0.35 + _clamp(quality, 0.0, 1.0), 0.25, 1.35)
        return self.alpha * quality_factor

    def select(self, context: tuple[float, ...], quality: float) -> PolicyDecision:
        baseline = super().select(context=context, quality=quality)
        quality_gap = max(self.quality_floor - _clamp(quality, 0.0, 1.0), 0.0)
        aggressive_cap = _clamp(
            self.max_aggressive_prob - (0.25 * quality_gap),
            self.min_arm_prob,
            self.max_arm_prob,
        )
        min_probs = {arm: self.min_arm_prob for arm in ARMS}
        max_probs = {arm: self.max_arm_prob for arm in ARMS}
        max_probs["aggressive"] = min(max_probs["aggressive"], aggressive_cap)

        projected, changed = _project_probabilities(
            raw=baseline.probabilities,
            min_probs=min_probs,
            max_probs=max_probs,
        )
        chosen = max(ARMS, key=lambda arm: (projected[arm], baseline.scores[arm], arm))
        return PolicyDecision(
            chosen_arm=chosen,
            scores=baseline.scores,
            means=baseline.means,
            uncertainties=baseline.uncertainties,
            probabilities=projected,
            effective_alpha=baseline.effective_alpha,
            guardrail_applied=changed,
            exploration_mode="linucb-safe",
        )


class EpsilonGreedyPolicy(LinearPolicy):
    def __init__(self, dim: int, ridge_lambda: float, epsilon: float, rng: random.Random) -> None:
        super().__init__(dim=dim, ridge_lambda=ridge_lambda)
        self.epsilon = epsilon
        self.rng = rng

    def select(self, context: tuple[float, ...], quality: float) -> PolicyDecision:
        means = self.predict_means(context)
        best_arm = max(ARMS, key=lambda arm: (means[arm], arm))
        base = self.epsilon / len(ARMS)
        probs = {arm: base for arm in ARMS}
        probs[best_arm] += max(0.0, 1.0 - self.epsilon)
        if self.rng.random() < self.epsilon:
            chosen = self.rng.choice(list(ARMS))
            mode = "epsilon-explore"
        else:
            chosen = best_arm
            mode = "epsilon-greedy"
        return PolicyDecision(
            chosen_arm=chosen,
            scores=means,
            means=means,
            uncertainties={arm: 0.0 for arm in ARMS},
            probabilities=probs,
            effective_alpha=0.0,
            guardrail_applied=False,
            exploration_mode=mode,
        )


class RewardModel(LinearPolicy):
    pass


def _estimate_logging_propensity(examples: list[BanditExample], floor: float) -> dict[str, float]:
    counts = {arm: 0.0 for arm in ARMS}
    for ex in examples:
        counts[ex.logged_arm] += 1.0
    total = sum(counts.values())
    if total <= 0:
        return {arm: 1.0 / len(ARMS) for arm in ARMS}

    floored = {arm: max(counts[arm] / total, floor) for arm in ARMS}
    norm = sum(floored.values())
    if norm <= 0.0:
        return {arm: 1.0 / len(ARMS) for arm in ARMS}
    return {arm: floored[arm] / norm for arm in ARMS}


def _simulate(
    *,
    examples: list[BanditExample],
    algorithm: str,
    alpha: float,
    epsilon: float,
    ridge_lambda: float,
    allocation_temperature: float,
    min_arm_prob: float,
    max_arm_prob: float,
    max_aggressive_prob: float,
    quality_floor: float,
    ips_weight_cap: float,
    logging_propensity_floor: float,
    rng: random.Random,
) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    if not examples:
        raise RuntimeError("No examples available for simulation.")
    dim = len(examples[0].context)
    if algorithm == "linucb":
        policy: LinUCBPolicy | EpsilonGreedyPolicy = LinUCBPolicy(
            dim=dim,
            ridge_lambda=ridge_lambda,
            alpha=alpha,
            allocation_temperature=allocation_temperature,
        )
    elif algorithm == "linucb-safe":
        policy = SafeLinUCBPolicy(
            dim=dim,
            ridge_lambda=ridge_lambda,
            alpha=alpha,
            allocation_temperature=allocation_temperature,
            min_arm_prob=min_arm_prob,
            max_arm_prob=max_arm_prob,
            max_aggressive_prob=max_aggressive_prob,
            quality_floor=quality_floor,
        )
    else:
        policy = EpsilonGreedyPolicy(dim=dim, ridge_lambda=ridge_lambda, epsilon=epsilon, rng=rng)

    reward_model = RewardModel(dim=dim, ridge_lambda=ridge_lambda)
    logging_propensity = _estimate_logging_propensity(examples, logging_propensity_floor)

    chosen_counts = {arm: 0 for arm in ARMS}
    logged_counts = {arm: 0 for arm in ARMS}
    matched_counts = {arm: 0 for arm in ARMS}
    avg_prob = {arm: 0.0 for arm in ARMS}

    cumulative_reward = 0.0
    matched_reward = 0.0
    matched_events = 0
    guardrail_hits = 0
    clipped_weight_events = 0
    total_weight = 0.0
    total_weight_sq = 0.0

    ips_sum = 0.0
    dm_sum = 0.0
    dr_sum = 0.0
    history: list[dict[str, Any]] = []

    for idx, ex in enumerate(examples):
        decision = policy.select(ex.context, ex.quality_score)
        chosen = decision.chosen_arm
        matched = chosen == ex.logged_arm
        reward_applied = ex.reward if matched else 0.0

        if matched:
            policy.update(chosen, ex.context, ex.reward)
            matched_events += 1
            matched_counts[chosen] += 1
            matched_reward += ex.reward
        cumulative_reward += reward_applied
        chosen_counts[chosen] += 1
        logged_counts[ex.logged_arm] += 1
        for arm in ARMS:
            avg_prob[arm] += decision.probabilities[arm]
        if decision.guardrail_applied:
            guardrail_hits += 1

        q_values = reward_model.predict_means(ex.context)
        dm_value = sum(decision.probabilities[arm] * q_values[arm] for arm in ARMS)
        logged_q = q_values[ex.logged_arm]
        target_prob = decision.probabilities[ex.logged_arm]
        logging_prob = max(logging_propensity[ex.logged_arm], 1e-9)
        raw_weight = target_prob / logging_prob
        clipped_weight = min(raw_weight, ips_weight_cap)
        if clipped_weight < raw_weight:
            clipped_weight_events += 1

        ips_contrib = clipped_weight * ex.reward
        dr_contrib = dm_value + (clipped_weight * (ex.reward - logged_q))
        ips_sum += ips_contrib
        dm_sum += dm_value
        dr_sum += dr_contrib
        total_weight += clipped_weight
        total_weight_sq += clipped_weight * clipped_weight

        reward_model.update(ex.logged_arm, ex.context, ex.reward)

        history.append(
            {
                "step": idx,
                "run_id": ex.run_id,
                "timestamp_utc": ex.timestamp_utc,
                "source": ex.source,
                "logged_arm": ex.logged_arm,
                "chosen_arm": chosen,
                "matched_replay": matched,
                "quality_score": round(ex.quality_score, 6),
                "reward_observed": round(ex.reward, 6),
                "reward_applied": round(reward_applied, 6),
                "score_mirror": round(decision.scores["mirror"], 6),
                "score_conservative": round(decision.scores["conservative"], 6),
                "score_aggressive": round(decision.scores["aggressive"], 6),
                "prob_mirror": round(decision.probabilities["mirror"], 6),
                "prob_conservative": round(decision.probabilities["conservative"], 6),
                "prob_aggressive": round(decision.probabilities["aggressive"], 6),
                "effective_alpha": round(decision.effective_alpha, 6),
                "guardrail_applied": decision.guardrail_applied,
                "exploration_mode": decision.exploration_mode,
                "logged_propensity": round(logging_prob, 6),
                "target_prob_logged_arm": round(target_prob, 6),
                "ips_weight": round(clipped_weight, 6),
                "ips_contribution": round(ips_contrib, 6),
                "dm_contribution": round(dm_value, 6),
                "dr_contribution": round(dr_contrib, 6),
            }
        )

    n = len(examples)
    avg_prob = {arm: avg_prob[arm] / n for arm in ARMS}
    snips = (ips_sum / total_weight) if total_weight > 0 else None
    ess = ((total_weight * total_weight) / total_weight_sq) if total_weight_sq > 0 else 0.0

    summary = {
        "schema_version": "2.0.0",
        "generated_at_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "algorithm": algorithm,
        "params": {
            "alpha": alpha,
            "epsilon": epsilon,
            "ridge_lambda": ridge_lambda,
            "allocation_temperature": allocation_temperature,
            "min_arm_prob": min_arm_prob,
            "max_arm_prob": max_arm_prob,
            "max_aggressive_prob": max_aggressive_prob,
            "quality_floor": quality_floor,
            "ips_weight_cap": ips_weight_cap,
            "logging_propensity_floor": logging_propensity_floor,
            "replay_mode": "logged-bandit-replay",
        },
        "examples_total": n,
        "matched_events": matched_events,
        "matched_coverage_pct": round((matched_events / n) * 100.0, 4),
        "cumulative_reward": round(cumulative_reward, 6),
        "avg_reward_all_steps": round(cumulative_reward / n, 6),
        "avg_reward_matched_only": round((matched_reward / matched_events), 6) if matched_events > 0 else None,
        "chosen_counts": chosen_counts,
        "logged_counts": logged_counts,
        "matched_counts": matched_counts,
        "avg_quality_score": round(sum(ex.quality_score for ex in examples) / n, 6),
        "avg_allocation_probability": {arm: round(avg_prob[arm], 6) for arm in ARMS},
        "guardrail_trigger_count": guardrail_hits,
        "guardrail_trigger_rate_pct": round((guardrail_hits / n) * 100.0, 4),
        "logging_propensity_estimate": {arm: round(logging_propensity[arm], 6) for arm in ARMS},
        "off_policy_evaluation": {
            "ips_reward_estimate": round(ips_sum / n, 6),
            "snips_reward_estimate": round(snips, 6) if snips is not None else None,
            "direct_method_reward_estimate": round(dm_sum / n, 6),
            "doubly_robust_reward_estimate": round(dr_sum / n, 6),
            "effective_sample_size": round(ess, 4),
            "weight_clipping_events": clipped_weight_events,
            "weight_clipping_rate_pct": round((clipped_weight_events / n) * 100.0, 4),
        },
    }
    return summary, history


def _write_outputs(
    out_dir: Path,
    summary_file: str,
    history_json: str,
    history_csv: str,
    summary: dict[str, Any],
    history: list[dict[str, Any]],
) -> None:
    out_dir.mkdir(parents=True, exist_ok=True)
    summary_path = out_dir / summary_file
    history_json_path = out_dir / history_json
    history_csv_path = out_dir / history_csv

    summary_path.write_text(json.dumps(summary, indent=2, sort_keys=True), encoding="utf-8")
    history_json_path.write_text(json.dumps(history, indent=2, sort_keys=True), encoding="utf-8")

    fieldnames = [
        "step",
        "run_id",
        "timestamp_utc",
        "source",
        "logged_arm",
        "chosen_arm",
        "matched_replay",
        "quality_score",
        "reward_observed",
        "reward_applied",
        "score_mirror",
        "score_conservative",
        "score_aggressive",
        "prob_mirror",
        "prob_conservative",
        "prob_aggressive",
        "effective_alpha",
        "guardrail_applied",
        "exploration_mode",
        "logged_propensity",
        "target_prob_logged_arm",
        "ips_weight",
        "ips_contribution",
        "dm_contribution",
        "dr_contribution",
    ]
    with history_csv_path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=fieldnames)
        writer.writeheader()
        for row in history:
            writer.writerow(row)

    print(f"Wrote {summary_path}")
    print(f"Wrote {history_json_path}")
    print(f"Wrote {history_csv_path}")


def run_allocation(args: argparse.Namespace) -> int:
    rng = random.Random(args.seed)
    warnings: list[str] = []
    eval_root = Path(args.eval_root).resolve()

    examples, collect_warnings = _collect_examples(eval_root, list(args.report_file))
    warnings.extend(collect_warnings)

    synthetic_used = False
    if args.synthetic_steps > 0:
        examples = _synthetic_examples(args.synthetic_steps, rng)
        synthetic_used = True
        warnings.append("forced synthetic replay via --synthetic-steps")
    elif not examples:
        if args.no_fallback_synthetic:
            raise RuntimeError("No compatible eval artifacts found and --no-fallback-synthetic is set.")
        examples = _synthetic_examples(90, rng)
        synthetic_used = True
        warnings.append("no compatible eval artifacts found; using synthetic fallback dataset")

    min_arm_prob = _clamp(args.min_arm_prob, 0.0, 1.0)
    max_arm_prob = _clamp(args.max_arm_prob, 0.0, 1.0)
    max_aggressive_prob = _clamp(args.max_aggressive_prob, 0.0, 1.0)
    if min_arm_prob * len(ARMS) > 1.0 + 1e-9:
        raise ValueError("Invalid guardrails: min-arm-prob is too large for 3 arms.")
    if max_arm_prob <= 0.0:
        raise ValueError("Invalid guardrails: max-arm-prob must be > 0.")
    if max_arm_prob < min_arm_prob:
        raise ValueError("Invalid guardrails: max-arm-prob must be >= min-arm-prob.")
    if max_aggressive_prob < min_arm_prob:
        raise ValueError("Invalid guardrails: max-aggressive-prob must be >= min-arm-prob.")

    summary, history = _simulate(
        examples=examples,
        algorithm=args.algorithm,
        alpha=max(args.alpha, 0.0),
        epsilon=_clamp(args.epsilon, 0.0, 1.0),
        ridge_lambda=max(args.ridge_lambda, 1e-6),
        allocation_temperature=max(args.allocation_temperature, 1e-6),
        min_arm_prob=min_arm_prob,
        max_arm_prob=max_arm_prob,
        max_aggressive_prob=max_aggressive_prob,
        quality_floor=_clamp(args.quality_floor, 0.0, 1.0),
        ips_weight_cap=max(args.ips_weight_cap, 1e-6),
        logging_propensity_floor=_clamp(args.logging_propensity_floor, 1e-6, 0.99),
        rng=rng,
    )
    summary["synthetic_data"] = synthetic_used
    summary["sources_seen"] = sorted({ex.source for ex in examples})
    summary["warnings"] = sorted(set(warnings))
    summary["warning_count"] = len(summary["warnings"])

    _write_outputs(
        out_dir=Path(args.out_dir),
        summary_file=args.summary_file,
        history_json=args.history_json,
        history_csv=args.history_csv,
        summary=summary,
        history=history,
    )
    if summary["warnings"]:
        print(f"Warnings: {summary['warning_count']} (see summary.json warnings)")
    return 0


def main() -> int:
    return run_allocation(parse_args())


if __name__ == "__main__":
    raise SystemExit(main())
