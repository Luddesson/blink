"""SQLite-backed prediction store for Alpha AI.

Records every LLM prediction (submitted, filtered, or abstained) along with
market context and CLOB data. Outcome fields are filled later by the
outcome tracker once a market resolves.

Uses aiosqlite for non-blocking access from the async event loop.
"""

from __future__ import annotations

import logging
from datetime import datetime, timedelta, timezone
from pathlib import Path

import aiosqlite

logger = logging.getLogger(__name__)

DB_PATH_DEFAULT = Path("data/alpha_predictions.db")

_SCHEMA = """\
CREATE TABLE IF NOT EXISTS predictions (
    -- Identity
    analysis_id     TEXT PRIMARY KEY,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),

    -- Market
    market_id       TEXT,
    condition_id    TEXT NOT NULL,
    token_id        TEXT NOT NULL,
    question        TEXT NOT NULL,
    category        TEXT,
    end_date        TEXT,

    -- LLM output
    predicted_prob  REAL,
    market_price    REAL NOT NULL,
    confidence      REAL,
    edge_bps        REAL,
    reasoning       TEXT,
    model_used      TEXT,
    prompt_version  TEXT DEFAULT 'v1',

    -- Status (split per rubber-duck advice)
    model_action    TEXT NOT NULL,   -- BUY / SELL / PASS
    filter_status   TEXT NOT NULL,   -- submitted / low_edge / low_confidence / pass / api_error / parse_error / engine_rejected

    -- Sizing
    recommended_size_usdc REAL,
    side            TEXT,            -- BUY / SELL

    -- Reasoning chain (Phase 2)
    reasoning_chain_json TEXT,       -- JSON blob from ReasoningChain.to_dict()

    -- CLOB snapshot at prediction time
    clob_best_bid   REAL,
    clob_best_ask   REAL,
    clob_spread_pct REAL,
    clob_bid_depth  REAL,
    clob_ask_depth  REAL,
    price_change_1h REAL,

    -- Outcome (filled by outcome_tracker)
    resolved        INTEGER NOT NULL DEFAULT 0,
    resolved_at     TEXT,
    actual_outcome  REAL,           -- 1.0 = YES, 0.0 = NO
    brier_score     REAL,           -- (predicted_prob - actual)^2
    was_correct     INTEGER,        -- 1 if directional call was right
    pnl_usdc        REAL,

    -- Outcome tracking lifecycle
    next_check_at   TEXT,
    check_count     INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_pred_unresolved
    ON predictions (resolved, condition_id) WHERE resolved = 0;
CREATE INDEX IF NOT EXISTS idx_pred_category
    ON predictions (category, resolved);
CREATE INDEX IF NOT EXISTS idx_pred_model
    ON predictions (model_used, resolved);
CREATE INDEX IF NOT EXISTS idx_pred_created
    ON predictions (created_at);
"""


class PredictionRecord:
    """Flat record for insertion into the predictions table."""

    __slots__ = (
        "analysis_id", "condition_id", "token_id", "question",
        "market_price", "model_action", "filter_status",
        "market_id", "category", "end_date",
        "predicted_prob", "confidence", "edge_bps",
        "reasoning", "model_used", "prompt_version",
        "recommended_size_usdc", "side",
        "reasoning_chain_json",
        "clob_best_bid", "clob_best_ask", "clob_spread_pct",
        "clob_bid_depth", "clob_ask_depth", "price_change_1h",
    )

    def __init__(
        self,
        *,
        analysis_id: str,
        condition_id: str,
        token_id: str,
        question: str,
        market_price: float,
        model_action: str,
        filter_status: str,
        market_id: str | None = None,
        category: str | None = None,
        end_date: str | None = None,
        predicted_prob: float | None = None,
        confidence: float | None = None,
        edge_bps: float | None = None,
        reasoning: str | None = None,
        model_used: str | None = None,
        prompt_version: str = "v1",
        recommended_size_usdc: float | None = None,
        side: str | None = None,
        reasoning_chain_json: str | None = None,
        clob_best_bid: float | None = None,
        clob_best_ask: float | None = None,
        clob_spread_pct: float | None = None,
        clob_bid_depth: float | None = None,
        clob_ask_depth: float | None = None,
        price_change_1h: float | None = None,
    ) -> None:
        self.analysis_id = analysis_id
        self.condition_id = condition_id
        self.token_id = token_id
        self.question = question
        self.market_price = market_price
        self.model_action = model_action
        self.filter_status = filter_status
        self.market_id = market_id
        self.category = category
        self.end_date = end_date
        self.predicted_prob = predicted_prob
        self.confidence = confidence
        self.edge_bps = edge_bps
        self.reasoning = reasoning
        self.model_used = model_used
        self.prompt_version = prompt_version
        self.recommended_size_usdc = recommended_size_usdc
        self.side = side
        self.reasoning_chain_json = reasoning_chain_json
        self.clob_best_bid = clob_best_bid
        self.clob_best_ask = clob_best_ask
        self.clob_spread_pct = clob_spread_pct
        self.clob_bid_depth = clob_bid_depth
        self.clob_ask_depth = clob_ask_depth
        self.price_change_1h = price_change_1h


class PredictionStore:
    """Async SQLite store for Alpha AI predictions."""

    def __init__(self, db_path: Path | str = DB_PATH_DEFAULT) -> None:
        self._db_path = Path(db_path)
        self._db: aiosqlite.Connection | None = None

    async def open(self) -> None:
        """Open DB connection, create tables if needed."""
        self._db_path.parent.mkdir(parents=True, exist_ok=True)
        self._db = await aiosqlite.connect(str(self._db_path))
        await self._db.execute("PRAGMA journal_mode=WAL")
        await self._db.execute("PRAGMA busy_timeout=5000")
        await self._db.executescript(_SCHEMA)
        # Migrate existing DBs: add new columns that didn't exist in Phase 1
        for col, coltype in [("reasoning_chain_json", "TEXT")]:
            try:
                await self._db.execute(
                    f"ALTER TABLE predictions ADD COLUMN {col} {coltype}"
                )
            except Exception:
                pass  # column already exists
        await self._db.commit()
        logger.info("Prediction store opened: %s", self._db_path)

    async def close(self) -> None:
        if self._db:
            await self._db.close()
            self._db = None

    async def record_prediction(self, rec: PredictionRecord) -> None:
        """Insert a new prediction. Silently skips duplicates."""
        if not self._db:
            return
        next_check = _compute_next_check(rec.end_date)
        try:
            await self._db.execute(
                """INSERT OR IGNORE INTO predictions (
                    analysis_id, condition_id, token_id, question,
                    market_price, model_action, filter_status,
                    market_id, category, end_date,
                    predicted_prob, confidence, edge_bps,
                    reasoning, model_used, prompt_version,
                    recommended_size_usdc, side,
                    reasoning_chain_json,
                    clob_best_bid, clob_best_ask, clob_spread_pct,
                    clob_bid_depth, clob_ask_depth, price_change_1h,
                    next_check_at
                ) VALUES (
                    ?, ?, ?, ?,
                    ?, ?, ?,
                    ?, ?, ?,
                    ?, ?, ?,
                    ?, ?, ?,
                    ?, ?,
                    ?,
                    ?, ?, ?,
                    ?, ?, ?,
                    ?
                )""",
                (
                    rec.analysis_id, rec.condition_id, rec.token_id, rec.question,
                    rec.market_price, rec.model_action, rec.filter_status,
                    rec.market_id, rec.category, rec.end_date,
                    rec.predicted_prob, rec.confidence, rec.edge_bps,
                    rec.reasoning, rec.model_used, rec.prompt_version,
                    rec.recommended_size_usdc, rec.side,
                    rec.reasoning_chain_json,
                    rec.clob_best_bid, rec.clob_best_ask, rec.clob_spread_pct,
                    rec.clob_bid_depth, rec.clob_ask_depth, rec.price_change_1h,
                    next_check,
                ),
            )
            await self._db.commit()
        except Exception as e:
            logger.warning("Failed to record prediction %s: %s", rec.analysis_id, e)

    async def get_unresolved(self, limit: int = 50) -> list[dict]:
        """Get unresolved predictions that are due for a check."""
        if not self._db:
            return []
        now_iso = datetime.now(timezone.utc).isoformat()
        cursor = await self._db.execute(
            """SELECT analysis_id, condition_id, token_id, question, category,
                      predicted_prob, market_price, model_action, filter_status,
                      side, end_date, check_count
               FROM predictions
               WHERE resolved = 0
                 AND filter_status IN ('submitted', 'low_edge')
                 AND (next_check_at IS NULL OR next_check_at <= ?)
               ORDER BY created_at ASC
               LIMIT ?""",
            (now_iso, limit),
        )
        rows = await cursor.fetchall()
        cols = [d[0] for d in cursor.description]
        return [dict(zip(cols, row)) for row in rows]

    async def mark_resolved(
        self,
        analysis_id: str,
        actual_outcome: float,
        brier_score: float,
        was_correct: bool,
        pnl_usdc: float | None = None,
    ) -> None:
        """Mark a prediction as resolved with its outcome."""
        if not self._db:
            return
        now_iso = datetime.now(timezone.utc).isoformat()
        await self._db.execute(
            """UPDATE predictions
               SET resolved = 1,
                   resolved_at = ?,
                   actual_outcome = ?,
                   brier_score = ?,
                   was_correct = ?,
                   pnl_usdc = ?
               WHERE analysis_id = ?""",
            (now_iso, actual_outcome, brier_score, int(was_correct), pnl_usdc, analysis_id),
        )
        await self._db.commit()

    async def bump_next_check(self, analysis_id: str) -> None:
        """Push next_check_at forward with exponential backoff."""
        if not self._db:
            return
        cursor = await self._db.execute(
            "SELECT check_count, end_date FROM predictions WHERE analysis_id = ?",
            (analysis_id,),
        )
        row = await cursor.fetchone()
        if not row:
            return
        check_count = row[0] + 1
        next_check = _compute_next_check_backoff(row[1], check_count)
        await self._db.execute(
            """UPDATE predictions
               SET check_count = ?, next_check_at = ?
               WHERE analysis_id = ?""",
            (check_count, next_check, analysis_id),
        )
        await self._db.commit()

    async def get_resolved(self, limit: int = 200, category: str | None = None) -> list[dict]:
        """Get resolved predictions for calibration analysis."""
        if not self._db:
            return []
        if category:
            cursor = await self._db.execute(
                """SELECT * FROM predictions
                   WHERE resolved = 1 AND category = ?
                   ORDER BY resolved_at DESC LIMIT ?""",
                (category, limit),
            )
        else:
            cursor = await self._db.execute(
                """SELECT * FROM predictions
                   WHERE resolved = 1
                   ORDER BY resolved_at DESC LIMIT ?""",
                (limit,),
            )
        rows = await cursor.fetchall()
        cols = [d[0] for d in cursor.description]
        return [dict(zip(cols, row)) for row in rows]

    async def get_stats(self) -> dict:
        """Get aggregate prediction statistics."""
        if not self._db:
            return {}
        cursor = await self._db.execute(
            """SELECT
                COUNT(*) AS total,
                SUM(CASE WHEN resolved = 1 THEN 1 ELSE 0 END) AS resolved,
                SUM(CASE WHEN resolved = 0 THEN 1 ELSE 0 END) AS unresolved,
                SUM(CASE WHEN filter_status = 'submitted' THEN 1 ELSE 0 END) AS submitted,
                SUM(CASE WHEN filter_status = 'low_edge' THEN 1 ELSE 0 END) AS low_edge,
                SUM(CASE WHEN filter_status = 'pass' THEN 1 ELSE 0 END) AS passed,
                AVG(CASE WHEN resolved = 1 THEN brier_score END) AS avg_brier,
                SUM(CASE WHEN resolved = 1 THEN pnl_usdc ELSE 0 END) AS total_pnl
            FROM predictions"""
        )
        row = await cursor.fetchone()
        if not row:
            return {}
        cols = [d[0] for d in cursor.description]
        return dict(zip(cols, row))

    async def count(self) -> int:
        """Total number of predictions stored."""
        if not self._db:
            return 0
        cursor = await self._db.execute("SELECT COUNT(*) FROM predictions")
        row = await cursor.fetchone()
        return row[0] if row else 0

    async def get_all_predictions(self, limit: int = 100) -> list[dict]:
        """Get recent predictions for dashboard display."""
        if not self._db:
            return []
        cursor = await self._db.execute(
            """SELECT analysis_id, created_at, question, category,
                      predicted_prob, market_price, confidence, edge_bps,
                      model_action, filter_status, side, model_used,
                      resolved, actual_outcome, brier_score, was_correct, pnl_usdc
               FROM predictions
               ORDER BY created_at DESC
               LIMIT ?""",
            (limit,),
        )
        rows = await cursor.fetchall()
        cols = [d[0] for d in cursor.description]
        return [dict(zip(cols, row)) for row in rows]


def _compute_next_check(end_date: str | None) -> str:
    """Initial next_check_at based on market end date."""
    now = datetime.now(timezone.utc)

    if end_date:
        try:
            end = datetime.fromisoformat(end_date.replace("Z", "+00:00"))
            time_to_end = (end - now).total_seconds()
            if time_to_end > 86400 * 7:
                return (now + timedelta(hours=24)).isoformat()
            elif time_to_end > 86400:
                return (now + timedelta(hours=6)).isoformat()
            elif time_to_end > 3600:
                return (now + timedelta(hours=1)).isoformat()
            else:
                return (now + timedelta(minutes=5)).isoformat()
        except (ValueError, TypeError):
            pass

    return (now + timedelta(hours=1)).isoformat()


def _compute_next_check_backoff(end_date: str | None, check_count: int) -> str:
    """Exponential backoff for repeated checks."""
    now = datetime.now(timezone.utc)

    base_minutes = 60
    if end_date:
        try:
            end = datetime.fromisoformat(end_date.replace("Z", "+00:00"))
            time_to_end = (end - now).total_seconds()
            if time_to_end < 0:
                base_minutes = 10
            elif time_to_end < 3600:
                base_minutes = 5
            elif time_to_end < 86400:
                base_minutes = 60
            else:
                base_minutes = 360
        except (ValueError, TypeError):
            pass

    backoff_minutes = min(base_minutes * (2 ** min(check_count, 8)), 1440)
    return (now + timedelta(minutes=backoff_minutes)).isoformat()
