"""Category-specific prompts for the Alpha AI reasoning pipeline.

Detects market category from question/description text and provides tailored
analytical guidance based on Philip Tetlock's superforecasting research.

Two prompt types:
  - Deep Analysis (Call 1): structured Bayesian reasoning with category guidance
  - Devil's Advocate (Call 2): adversarial critique of Call 1's output
"""

from __future__ import annotations

# ─── Category detection ───────────────────────────────────────────────────────

_CATEGORY_KEYWORDS: dict[str, list[str]] = {
    "politics": [
        "president", "election", "congress", "senate", "democrat", "republican",
        "trump", "biden", "poll", "vote", "governor", "nominee", "primary",
        "cabinet", "impeach", "veto", "legislation", "ballot", "electoral",
        "party", "gop", "dnc", "rnc", "harris", "desantis", "newsom",
        "speaker", "majority", "minority leader",
    ],
    "sports": [
        "vs ", " vs.", "nba", "nfl", "mlb", "nhl", "soccer", "tennis", "f1 ",
        "premier league", "champions league", "world cup", "super bowl",
        "playoff", "finals", "championship", "match", "game ", "season",
        "win ", "beat", "tournament", "seed", "standings",
        "grand prix", "open ", "ufc", "boxing",
    ],
    "crypto": [
        "bitcoin", "btc", "ethereum", "eth ", "solana", "sol ", "defi",
        "nft", "crypto", "blockchain", "token", "halving", "mining",
        "exchange", "binance", "coinbase", "altcoin", "stablecoin",
        "smart contract", "layer 2", "rollup",
    ],
    "geopolitics": [
        "war ", "invasion", "sanction", "nato", "military", "treaty",
        "united nations", "diplomacy", "regime", "nuclear", "missile",
        "ceasefire", "conflict", "territory", "occupation", "annexation",
        "china invade", "russia", "iran", "taiwan", "ukraine", "north korea",
        "embargo", "coup", "greenland",
    ],
}


def detect_category(question: str, description: str = "") -> str:
    """Detect market category from question and description text.

    Returns the category with the most keyword hits (min 2), or 'default'.
    """
    text = (question + " " + description).lower()
    scores: dict[str, int] = {}
    for cat, keywords in _CATEGORY_KEYWORDS.items():
        scores[cat] = sum(1 for kw in keywords if kw in text)
    best = max(scores, key=lambda k: scores[k])
    return best if scores[best] >= 2 else "default"


# ─── Category-specific guidance blocks ────────────────────────────────────────

_CATEGORY_GUIDANCE: dict[str, str] = {
    "politics": """\
CATEGORY-SPECIFIC GUIDANCE (Political Markets):
- Start with POLLING AVERAGES as your base rate, not gut feeling or media narrative.
- Consider: incumbency advantage (+3-5pp historically), economic indicators (GDP, inflation).
- Primary factors: fundraising totals, key endorsements, party unity/division.
- Historical precedent: How often do candidates in similar polling positions win?
- Beware: media narratives != voter behavior. Aggregated polls > individual pundits.
- Time horizon matters: polls are less predictive >6 months before an election.
""",
    "sports": """\
CATEGORY-SPECIFIC GUIDANCE (Sports Markets):
- Use team/player FORM over the last 5-10 matches as your base rate.
- Key factors: head-to-head record, home/away advantage, injuries, rest days, travel.
- Consider ELO ratings or power rankings if you know them.
- RECENCY BIAS is extremely common — a team's last game matters less than their season average.
- Tournament dynamics: upsets are more common in single elimination than best-of-7.
- Weather, altitude, and surface can matter significantly in some sports.
""",
    "crypto": """\
CATEGORY-SPECIFIC GUIDANCE (Crypto Markets):
- Base rate: How often do similar crypto milestones actually get reached by a deadline?
- Key factors: Bitcoin halving cycle timing, regulatory environment, macro liquidity (Fed rates).
- On-chain signals: exchange flows, whale accumulation, funding rates, open interest.
- Crypto markets are highly REFLEXIVE — price affects narrative affects price.
- Beware extreme predictions: crypto is volatile but often mean-reverting long-term.
- Regulatory risk is asymmetric — negative news has larger impact than positive.
""",
    "geopolitics": """\
CATEGORY-SPECIFIC GUIDANCE (Geopolitics Markets):
- Historical BASE RATES are critical: wars, regime changes, and treaties have LOW base rates.
- Key factors: diplomatic channels (open/closed), military deployments, economic sanctions.
- Intelligence assessments often OVERESTIMATE dramatic outcomes (invasion, regime collapse).
- Consider: economic interdependence, nuclear deterrence, domestic political incentives.
- Geopolitical events CLUSTER: one event can cascade into many related outcomes.
- STATUS QUO BIAS is often correct in geopolitics — dramatic change is rare.
""",
    "default": """\
CATEGORY-SPECIFIC GUIDANCE:
- Use REFERENCE CLASS FORECASTING: what is the base rate for similar events?
- Consider both insider information the market may have AND public misconceptions.
- Markets with LOW volume may be less efficient — your edge may be real.
- Markets with HIGH volume are likely well-calibrated — be conservative.
- Think about: what would need to be true for this market to be mispriced?
""",
}


# ─── Call 1: Deep Analysis prompt ─────────────────────────────────────────────


def get_deep_analysis_prompt(
    question: str,
    description: str,
    yes_price: float,
    no_price: float,
    volume: float,
    end_date: str,
    category: str,
    *,
    clob_best_bid: float | None = None,
    clob_best_ask: float | None = None,
    clob_spread_bps: float | None = None,
    clob_bid_depth: float | None = None,
    clob_ask_depth: float | None = None,
    price_change_1h: str | None = None,
    news_context: str | None = None,
) -> str:
    """Build the deep analysis prompt (Call 1 of the reasoning chain)."""
    guidance = _CATEGORY_GUIDANCE.get(category, _CATEGORY_GUIDANCE["default"])

    prompt = f"""\
You are a SUPERFORECASTER trained in Philip Tetlock's methods for accurate
probability estimation. You combine reference class forecasting, Bayesian
reasoning, and careful evidence evaluation.

Market: {question}
Description: {(description or 'No description provided.')[:500]}
Current YES price: {yes_price:.2%}   (market-implied probability)
Current NO price:  {no_price:.2%}
24h Volume: ${volume:,.0f}
Closes: {end_date or 'unknown'}
Category: {category}

{guidance}"""

    if clob_best_bid is not None:
        prompt += f"""
Live Orderbook (CLOB data):
  Best Bid:     {clob_best_bid:.4f}
  Best Ask:     {clob_best_ask:.4f}
  Spread:       {clob_spread_bps:.0f}bps
  Bid Depth:    ${clob_bid_depth:,.0f} USDC (top 5 levels)
  Ask Depth:    ${clob_ask_depth:,.0f} USDC (top 5 levels)
  1h Price chg: {price_change_1h or 'n/a'}
"""

    if news_context:
        prompt += f"""
{news_context}
USE THIS NEWS to inform your analysis — it contains information the market may not have priced in yet.
"""

    prompt += """
ANALYSIS INSTRUCTIONS — Follow these steps precisely:
1. BASE RATE: Identify the reference class and historical base rate for this event type.
2. EVIDENCE FOR: List 3-5 specific pieces of evidence supporting YES.
3. EVIDENCE AGAINST: List 3-5 specific pieces of evidence supporting NO.
4. BAYESIAN UPDATE: Starting from your base rate, explain how each evidence updates it.
5. MARKET EFFICIENCY: Is this market likely efficient, or is there genuine mispricing?
6. FINAL ESTIMATE: Your probability estimate and confidence level.

Output ONLY valid JSON with this exact schema:
{
  "base_rate": "<reference class and historical rate as a sentence>",
  "evidence_for": ["<specific evidence 1>", "<evidence 2>", "<evidence 3>"],
  "evidence_against": ["<specific evidence 1>", "<evidence 2>", "<evidence 3>"],
  "bayesian_reasoning": "<Starting from X% base rate, I update because...>",
  "market_efficiency": "<Is this market likely efficient? Why or why not?>",
  "probability": <float 0.0-1.0>,
  "confidence": <float 0.0-1.0>,
  "recommended_action": "BUY" | "SELL" | "PASS"
}

IMPORTANT: Only output "PASS" if you genuinely cannot form ANY directional view.
If you have even a slight lean, output "BUY" or "SELL" with appropriate confidence.
Low confidence (0.4-0.6) is fine — the sizing algorithm scales accordingly."""

    return prompt


# ─── Call 2: Devil's Advocate prompt ──────────────────────────────────────────


def get_devils_advocate_prompt(
    question: str,
    analysis: dict,
    market_price: float,
) -> str:
    """Build the devil's advocate prompt (Call 2 of the reasoning chain)."""
    evidence_for = ", ".join(analysis.get("evidence_for", []))
    evidence_against = ", ".join(analysis.get("evidence_against", []))

    return f"""\
You are a CONTRARIAN ANALYST reviewing a prediction market forecast. Your job is
to find FLAWS, BIASES, and MISSED EVIDENCE. Be aggressive — the original analyst
is probably overconfident.

Market: {question}
Current market price (YES): {market_price:.2%}

ORIGINAL ANALYSIS:
- Base rate: {analysis.get('base_rate', 'Not provided')}
- Evidence FOR: {evidence_for or 'None listed'}
- Evidence AGAINST: {evidence_against or 'None listed'}
- Bayesian reasoning: {analysis.get('bayesian_reasoning', 'Not provided')}
- Market efficiency: {analysis.get('market_efficiency', 'Not assessed')}
- Probability estimate: {analysis.get('probability', 'N/A')}
- Confidence: {analysis.get('confidence', 'N/A')}

YOUR TASK — be genuinely adversarial:
1. CRITIQUE: What is the biggest flaw in this analysis?
2. MISSED EVIDENCE: What 2-3 pieces of evidence were overlooked?
3. COGNITIVE BIASES: Which biases might affect the estimate?
   (anchoring, availability, overconfidence, narrative fallacy, base rate neglect, etc.)
4. REVISED ESTIMATE: Given your critique, provide your revised probability.

Output ONLY valid JSON:
{{
  "critique": "<The main flaw in this analysis is...>",
  "missed_evidence": ["<missed item 1>", "<missed item 2>"],
  "cognitive_biases": ["<bias 1>", "<bias 2>"],
  "revised_probability": <float 0.0-1.0>,
  "revised_confidence": <float 0.0-1.0>
}}

If the market is efficient and the original analysis adds no edge, bring the
probability CLOSER to the market price. If the analysis is overconfident,
reduce the confidence."""
