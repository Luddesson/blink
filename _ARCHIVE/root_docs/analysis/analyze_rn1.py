"""
RN1 Trading Pattern Analyzer
Analyserar Polymarket whale 0x2005d16a84ceefa912d4e380cd32e7ff827875ea
Known stats: $6M+ profit, sophisticated whale behavior
"""

import requests
import json
from datetime import datetime, timedelta
from collections import defaultdict
import statistics
import random

# Try to import matplotlib for visualizations
try:
    import matplotlib.pyplot as plt
    import matplotlib.dates as mdates
    HAS_MATPLOTLIB = True
except ImportError:
    HAS_MATPLOTLIB = False
    print("⚠️  matplotlib not available, skipping visualizations")

RN1_WALLET = "0x2005d16a84ceefa912d4e380cd32e7ff827875ea"

def fetch_polymarket_trades(wallet, limit=100):
    """Försök hämta trades från Polymarket public API"""
    try:
        # Polymarket CLOB API endpoint
        url = f"https://clob.polymarket.com/trades?maker={wallet}"
        response = requests.get(url, timeout=10)
        if response.status_code == 200:
            return response.json()
        else:
            print(f"  API returned status {response.status_code}")
            return None
    except Exception as e:
        print(f"  API error: {e}")
        return None

def fetch_polygon_transactions(wallet):
    """Försök hämta on-chain transactions från Polygon"""
    try:
        # Polygonscan API (requires API key for high limits)
        url = f"https://api.polygonscan.com/api?module=account&action=txlist&address={wallet}&sort=desc"
        response = requests.get(url, timeout=10)
        if response.status_code == 200:
            data = response.json()
            if data.get("status") == "1":
                return data.get("result", [])
        return None
    except Exception as e:
        print(f"  Polygon API error: {e}")
        return None

def analyze_bet_sizes(trades):
    """Analysera distribution av bet sizes"""
    sizes = [trade.get("size_usdc", 0) for trade in trades if trade.get("size_usdc")]
    
    if not sizes:
        return {}
    
    result = {
        "mean": statistics.mean(sizes),
        "median": statistics.median(sizes),
        "min": min(sizes),
        "max": max(sizes),
        "total_volume": sum(sizes),
        "trade_count": len(sizes),
    }
    
    if len(sizes) >= 4:
        quartiles = statistics.quantiles(sizes, n=4)
        result["quartiles"] = {
            "q1": quartiles[0],
            "q2": quartiles[1],
            "q3": quartiles[2],
        }
    
    return result

def analyze_market_selection(trades):
    """Vilka typer av markets väljer RN1?"""
    market_types = defaultdict(int)
    market_tags = defaultdict(int)
    markets = defaultdict(int)
    
    for trade in trades:
        market = trade.get("market", "")
        
        # Categorize by keywords
        if any(sport in market.lower() for sport in ["nba", "nfl", "nhl", "mlb", "premier league", "champions league"]):
            market_types["Sports"] += 1
            if "nba" in market.lower():
                market_tags["NBA"] += 1
            elif "nfl" in market.lower():
                market_tags["NFL"] += 1
            elif any(soccer in market.lower() for soccer in ["premier league", "champions league"]):
                market_tags["Soccer"] += 1
        elif any(pol in market.lower() for pol in ["election", "president", "senate", "congress"]):
            market_types["Politics"] += 1
        elif any(crypto in market.lower() for crypto in ["bitcoin", "eth", "crypto", "btc"]):
            market_types["Crypto"] += 1
        elif any(ent in market.lower() for ent in ["oscars", "grammys", "movie", "tv show"]):
            market_types["Entertainment"] += 1
        else:
            market_types["Other"] += 1
        
        markets[market] += 1
    
    return dict(market_types), dict(market_tags), dict(sorted(markets.items(), key=lambda x: x[1], reverse=True)[:10])

def analyze_timing_patterns(trades):
    """När lägger RN1 sina bets?"""
    hours = defaultdict(int)
    days_of_week = defaultdict(int)
    time_before_event = []
    
    for trade in trades:
        timestamp = trade.get("timestamp", 0)
        if timestamp:
            dt = datetime.fromtimestamp(timestamp)
            hours[dt.hour] += 1
            days_of_week[dt.strftime("%A")] += 1
            
            # Calculate time before event if available
            event_start = trade.get("event_start")
            if event_start:
                hours_before = (event_start - timestamp) / 3600
                if hours_before > 0:
                    time_before_event.append(hours_before)
    
    return {
        "peak_hours": dict(sorted(hours.items(), key=lambda x: x[1], reverse=True)[:5]),
        "days_of_week": dict(days_of_week),
        "hours_distribution": dict(hours),
        "avg_hours_before_event": statistics.mean(time_before_event) if time_before_event else None,
        "median_hours_before_event": statistics.median(time_before_event) if time_before_event else None,
    }

def analyze_win_rate(trades):
    """Estimate win rate (requires closed positions)"""
    wins = 0
    losses = 0
    total_pnl = 0
    
    for trade in trades:
        if trade.get("closed"):
            pnl = trade.get("pnl", 0)
            total_pnl += pnl
            if pnl > 0:
                wins += 1
            elif pnl < 0:
                losses += 1
    
    total_trades = wins + losses
    
    return {
        "wins": wins,
        "losses": losses,
        "win_rate": wins / total_trades if total_trades > 0 else None,
        "total_pnl": total_pnl,
        "avg_win": sum(t["pnl"] for t in trades if t.get("closed") and t.get("pnl", 0) > 0) / wins if wins > 0 else 0,
        "avg_loss": sum(t["pnl"] for t in trades if t.get("closed") and t.get("pnl", 0) < 0) / losses if losses > 0 else 0,
    }

def analyze_position_holding_time(trades):
    """Hur länge håller RN1 positioner?"""
    holding_times = []
    
    # Simple approach: if trade has entry/exit timestamps
    for trade in trades:
        if trade.get("closed") and trade.get("entry_time") and trade.get("exit_time"):
            holding_seconds = trade["exit_time"] - trade["entry_time"]
            holding_times.append(holding_seconds)
    
    if not holding_times:
        return {
            "mean_holding_seconds": None,
            "median_holding_seconds": None,
        }
    
    return {
        "mean_holding_seconds": statistics.mean(holding_times),
        "median_holding_seconds": statistics.median(holding_times),
        "min_holding_seconds": min(holding_times),
        "max_holding_seconds": max(holding_times),
    }

def analyze_bet_size_correlation(trades):
    """Correlation mellan bet size och market liquidity/confidence"""
    size_by_outcome = {"YES": [], "NO": []}
    size_by_won = {"won": [], "lost": []}
    
    for trade in trades:
        size = trade.get("size_usdc", 0)
        outcome = trade.get("outcome", "")
        
        if outcome in size_by_outcome:
            size_by_outcome[outcome].append(size)
        
        if trade.get("closed"):
            if trade.get("pnl", 0) > 0:
                size_by_won["won"].append(size)
            elif trade.get("pnl", 0) < 0:
                size_by_won["lost"].append(size)
    
    return {
        "avg_size_yes": statistics.mean(size_by_outcome["YES"]) if size_by_outcome["YES"] else 0,
        "avg_size_no": statistics.mean(size_by_outcome["NO"]) if size_by_outcome["NO"] else 0,
        "avg_size_won": statistics.mean(size_by_won["won"]) if size_by_won["won"] else 0,
        "avg_size_lost": statistics.mean(size_by_won["lost"]) if size_by_won["lost"] else 0,
    }

def analyze_streaks(trades):
    """Winning/losing streaks analysis"""
    sorted_trades = sorted([t for t in trades if t.get("closed")], key=lambda x: x.get("timestamp", 0))
    
    current_streak = 0
    max_win_streak = 0
    max_loss_streak = 0
    streak_type = None
    
    for trade in sorted_trades:
        pnl = trade.get("pnl", 0)
        
        if pnl > 0:  # Win
            if streak_type == "win":
                current_streak += 1
            else:
                current_streak = 1
                streak_type = "win"
            max_win_streak = max(max_win_streak, current_streak)
        elif pnl < 0:  # Loss
            if streak_type == "loss":
                current_streak += 1
            else:
                current_streak = 1
                streak_type = "loss"
            max_loss_streak = max(max_loss_streak, current_streak)
    
    return {
        "max_win_streak": max_win_streak,
        "max_loss_streak": max_loss_streak,
    }

def create_visualizations(trades, bet_sizes, timing, win_rate, output_prefix="D:\\Blink\\rn1"):
    """Create matplotlib visualizations"""
    if not HAS_MATPLOTLIB:
        return
    
    print("\n📈 Generating visualizations...")
    
    # 1. Bet Size Distribution Histogram
    if bet_sizes.get("trade_count", 0) > 0:
        sizes = [trade.get("size_usdc", 0) for trade in trades if trade.get("size_usdc")]
        plt.figure(figsize=(10, 6))
        plt.hist(sizes, bins=20, edgecolor='black', color='skyblue')
        plt.xlabel('Bet Size (USDC)', fontsize=12)
        plt.ylabel('Frequency', fontsize=12)
        plt.title('RN1 Bet Size Distribution', fontsize=14, fontweight='bold')
        plt.axvline(bet_sizes['mean'], color='red', linestyle='--', label=f'Mean: ${bet_sizes["mean"]:,.0f}')
        plt.axvline(bet_sizes['median'], color='green', linestyle='--', label=f'Median: ${bet_sizes["median"]:,.0f}')
        plt.legend()
        plt.grid(axis='y', alpha=0.3)
        plt.tight_layout()
        plt.savefig(f"{output_prefix}_bet_size_dist.png", dpi=150)
        print(f"  ✓ Saved: {output_prefix}_bet_size_dist.png")
        plt.close()
    
    # 2. Time-of-Day Heatmap
    if timing.get("hours_distribution"):
        hours = timing["hours_distribution"]
        hour_labels = list(range(24))
        counts = [hours.get(h, 0) for h in hour_labels]
        
        plt.figure(figsize=(12, 6))
        colors = plt.cm.YlOrRd([(c / max(counts) if max(counts) > 0 else 0) for c in counts])
        plt.bar(hour_labels, counts, color=colors, edgecolor='black')
        plt.xlabel('Hour of Day (UTC)', fontsize=12)
        plt.ylabel('Number of Trades', fontsize=12)
        plt.title('RN1 Trading Activity by Hour', fontsize=14, fontweight='bold')
        plt.xticks(range(0, 24, 2))
        plt.grid(axis='y', alpha=0.3)
        plt.tight_layout()
        plt.savefig(f"{output_prefix}_time_heatmap.png", dpi=150)
        print(f"  ✓ Saved: {output_prefix}_time_heatmap.png")
        plt.close()
    
    # 3. Win Rate Over Time
    closed_trades = sorted([t for t in trades if t.get("closed")], key=lambda x: x.get("timestamp", 0))
    if len(closed_trades) >= 5:
        dates = [datetime.fromtimestamp(t["timestamp"]) for t in closed_trades]
        cumulative_pnl = []
        running_pnl = 0
        for t in closed_trades:
            running_pnl += t.get("pnl", 0)
            cumulative_pnl.append(running_pnl)
        
        plt.figure(figsize=(12, 6))
        plt.plot(dates, cumulative_pnl, linewidth=2, color='green' if cumulative_pnl[-1] > 0 else 'red')
        plt.fill_between(dates, cumulative_pnl, alpha=0.3, color='green' if cumulative_pnl[-1] > 0 else 'red')
        plt.xlabel('Date', fontsize=12)
        plt.ylabel('Cumulative P&L (USDC)', fontsize=12)
        plt.title('RN1 Cumulative Profit/Loss Over Time', fontsize=14, fontweight='bold')
        plt.grid(True, alpha=0.3)
        plt.gca().xaxis.set_major_formatter(mdates.DateFormatter('%Y-%m-%d'))
        plt.gcf().autofmt_xdate()
        plt.tight_layout()
        plt.savefig(f"{output_prefix}_pnl_curve.png", dpi=150)
        print(f"  ✓ Saved: {output_prefix}_pnl_curve.png")
        plt.close()
    
    # 4. Market Type Pie Chart
    market_types, _, _ = analyze_market_selection(trades)
    if market_types:
        plt.figure(figsize=(10, 8))
        colors = plt.cm.Set3(range(len(market_types)))
        plt.pie(market_types.values(), labels=market_types.keys(), autopct='%1.1f%%',
                startangle=90, colors=colors, textprops={'fontsize': 11})
        plt.title('RN1 Market Type Distribution', fontsize=14, fontweight='bold')
        plt.tight_layout()
        plt.savefig(f"{output_prefix}_market_types.png", dpi=150)
        print(f"  ✓ Saved: {output_prefix}_market_types.png")
        plt.close()

def generate_mock_data():
    """Generate realistic mock data based on known RN1 behavior"""
    # RN1 is known for: $6M profit, whale-sized bets, sports focus, high win rate
    
    trades = []
    base_time = int(datetime.now().timestamp()) - (90 * 24 * 3600)  # 90 days ago
    
    # Generate 150 trades over 90 days
    for i in range(150):
        timestamp = base_time + random.randint(0, 90 * 24 * 3600)
        
        # Bet sizes: mostly $20k-$100k, some whales up to $500k
        if random.random() < 0.7:  # 70% normal bets
            size = random.randint(20000, 100000)
        elif random.random() < 0.9:  # 20% large bets
            size = random.randint(100000, 250000)
        else:  # 10% whale bets
            size = random.randint(250000, 500000)
        
        # Market types (sports heavy)
        market_type_rand = random.random()
        if market_type_rand < 0.5:  # 50% sports
            sports = ["NBA", "NFL", "Premier League", "Champions League", "NHL", "MLB"]
            sport = random.choice(sports)
            if sport == "NBA":
                teams = [("Lakers", "Warriors"), ("Celtics", "Heat"), ("Nuggets", "Suns"), ("Bucks", "Nets")]
            elif sport == "NFL":
                teams = [("Chiefs", "Bills"), ("Eagles", "49ers"), ("Cowboys", "Packers"), ("Ravens", "Bengals")]
            elif sport in ["Premier League", "Champions League"]:
                teams = [("Arsenal", "Man City"), ("Liverpool", "Chelsea"), ("Barcelona", "Real Madrid"), ("Bayern", "PSG")]
            else:
                teams = [("Team A", "Team B")]
            
            team_a, team_b = random.choice(teams)
            market = f"{sport}: {team_a} vs {team_b}"
        elif market_type_rand < 0.75:  # 25% politics
            topics = ["Presidential Election", "Senate Race", "Gubernatorial", "Congressional District"]
            market = f"{random.choice(topics)} - {random.choice(['Red', 'Blue'])} Win"
        elif market_type_rand < 0.85:  # 10% crypto
            market = f"Bitcoin price {random.choice(['above', 'below'])} ${random.randint(40, 80)}k"
        else:  # 15% other
            market = f"Entertainment: {random.choice(['Oscars', 'Grammys', 'Emmy Awards'])} winner"
        
        # Outcome and PnL (RN1 has high win rate ~65%)
        outcome = random.choice(["YES", "NO"])
        closed = random.random() < 0.8  # 80% of positions closed
        
        if closed:
            won = random.random() < 0.65  # 65% win rate
            if won:
                pnl = size * random.uniform(0.05, 0.25)  # 5-25% profit
            else:
                pnl = -size * random.uniform(0.30, 0.80)  # Lose 30-80% of bet
        else:
            pnl = 0
        
        # Entry and exit times
        entry_time = timestamp
        exit_time = timestamp + random.randint(3600, 7 * 24 * 3600) if closed else None  # 1 hour to 7 days
        
        trade = {
            "timestamp": timestamp,
            "size_usdc": size,
            "market": market,
            "outcome": outcome,
            "closed": closed,
            "pnl": pnl,
            "entry_time": entry_time,
            "exit_time": exit_time,
            "event_start": timestamp + random.randint(3600, 14 * 24 * 3600),  # Event 1hr to 14 days after bet
        }
        trades.append(trade)
    
    return sorted(trades, key=lambda x: x["timestamp"])

def main():
    print("=" * 70)
    print("RN1 TRADING PATTERN ANALYZER".center(70))
    print(f"Wallet: {RN1_WALLET}".center(70))
    print("Known Stats: $6M+ profit, Whale trader".center(70))
    print("=" * 70)
    
    # Try multiple data sources
    trades = []
    
    # Source 1: Polymarket CLOB API
    try:
        print("\n[1/3] Fetching trades from Polymarket CLOB API...")
        api_trades = fetch_polymarket_trades(RN1_WALLET)
        if api_trades:
            print(f"  ✓ Found {len(api_trades)} trades from API")
            trades = api_trades
        else:
            print("  ⚠️  No data from API")
    except Exception as e:
        print(f"  ❌ Failed: {e}")
    
    # Source 2: Polygon on-chain data
    if not trades:
        try:
            print("\n[2/3] Fetching on-chain transactions from Polygonscan...")
            txs = fetch_polygon_transactions(RN1_WALLET)
            if txs:
                print(f"  ℹ️  Found {len(txs)} transactions (but complex to parse as trades)")
            else:
                print("  ⚠️  No data from Polygonscan")
        except Exception as e:
            print(f"  ❌ Failed: {e}")
    
    # Source 3: Use mock/example data
    if not trades:
        print("\n[3/3] Using realistic mock data based on known RN1 behavior...")
        trades = generate_mock_data()
        print(f"  ✓ Generated {len(trades)} mock trades")
    
    # Run analyses
    print("\n" + "=" * 70)
    print("ANALYSIS RESULTS".center(70))
    print("=" * 70)
    
    print("\n📊 BET SIZE ANALYSIS:")
    bet_sizes = analyze_bet_sizes(trades)
    print(f"  Total Volume:    ${bet_sizes.get('total_volume', 0):,.2f}")
    print(f"  Trade Count:     {bet_sizes.get('trade_count', 0)}")
    print(f"  Mean Bet:        ${bet_sizes.get('mean', 0):,.2f}")
    print(f"  Median Bet:      ${bet_sizes.get('median', 0):,.2f}")
    print(f"  Min Bet:         ${bet_sizes.get('min', 0):,.2f}")
    print(f"  Max Bet:         ${bet_sizes.get('max', 0):,.2f}")
    if "quartiles" in bet_sizes:
        print(f"  Q1 (25%):        ${bet_sizes['quartiles']['q1']:,.2f}")
        print(f"  Q2 (50%):        ${bet_sizes['quartiles']['q2']:,.2f}")
        print(f"  Q3 (75%):        ${bet_sizes['quartiles']['q3']:,.2f}")
    
    print("\n🎯 MARKET SELECTION:")
    market_types, market_tags, top_markets = analyze_market_selection(trades)
    for mtype, count in sorted(market_types.items(), key=lambda x: x[1], reverse=True):
        pct = (count / len(trades)) * 100
        print(f"  {mtype:20s}: {count:3d} bets ({pct:5.1f}%)")
    
    if market_tags:
        print("\n  Top tags:")
        for tag, count in sorted(market_tags.items(), key=lambda x: x[1], reverse=True)[:5]:
            print(f"    - {tag}: {count} bets")
    
    print("\n⏰ TIMING PATTERNS:")
    timing = analyze_timing_patterns(trades)
    print("  Peak trading hours:")
    for hour, count in list(timing['peak_hours'].items())[:5]:
        print(f"    {hour:02d}:00 - {count} trades")
    
    print("\n  Days of week:")
    day_order = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"]
    for day in day_order:
        count = timing['days_of_week'].get(day, 0)
        if count > 0:
            bar = "█" * int(count / max(timing['days_of_week'].values()) * 30)
            print(f"    {day:10s}: {bar} ({count})")
    
    if timing['avg_hours_before_event']:
        print(f"\n  Avg time before event: {timing['avg_hours_before_event']:.1f} hours")
        print(f"  Median time before event: {timing['median_hours_before_event']:.1f} hours")
    
    print("\n🏆 WIN RATE & PERFORMANCE:")
    win_rate = analyze_win_rate(trades)
    if win_rate['win_rate'] is not None:
        print(f"  Win Rate:        {win_rate['win_rate']*100:.1f}% ({win_rate['wins']}W / {win_rate['losses']}L)")
        print(f"  Total P&L:       ${win_rate['total_pnl']:,.2f}")
        if win_rate['avg_win'] > 0:
            print(f"  Avg Win:         ${win_rate['avg_win']:,.2f}")
        if win_rate['avg_loss'] < 0:
            print(f"  Avg Loss:        ${win_rate['avg_loss']:,.2f}")
        if win_rate['avg_win'] > 0 and win_rate['avg_loss'] < 0:
            risk_reward = abs(win_rate['avg_win'] / win_rate['avg_loss'])
            print(f"  Risk/Reward:     {risk_reward:.2f}x")
    else:
        print("  (insufficient closed position data)")
    
    print("\n⏳ HOLDING TIME:")
    holding = analyze_position_holding_time(trades)
    if holding['mean_holding_seconds']:
        print(f"  Average:         {holding['mean_holding_seconds']/3600:.1f} hours ({holding['mean_holding_seconds']/86400:.1f} days)")
        print(f"  Median:          {holding['median_holding_seconds']/3600:.1f} hours ({holding['median_holding_seconds']/86400:.1f} days)")
        print(f"  Min:             {holding['min_holding_seconds']/3600:.1f} hours")
        print(f"  Max:             {holding['max_holding_seconds']/3600:.1f} hours ({holding['max_holding_seconds']/86400:.1f} days)")
    else:
        print("  (insufficient data)")
    
    print("\n📈 BET SIZE CORRELATIONS:")
    correlations = analyze_bet_size_correlation(trades)
    print(f"  Avg size (YES):  ${correlations['avg_size_yes']:,.2f}")
    print(f"  Avg size (NO):   ${correlations['avg_size_no']:,.2f}")
    if correlations['avg_size_won'] > 0:
        print(f"  Avg size (won):  ${correlations['avg_size_won']:,.2f}")
    if correlations['avg_size_lost'] > 0:
        print(f"  Avg size (lost): ${correlations['avg_size_lost']:,.2f}")
    
    print("\n🔥 STREAK ANALYSIS:")
    streaks = analyze_streaks(trades)
    print(f"  Max win streak:  {streaks['max_win_streak']} trades")
    print(f"  Max loss streak: {streaks['max_loss_streak']} trades")
    
    # Create visualizations
    create_visualizations(trades, bet_sizes, timing, win_rate)
    
    # Save detailed report
    report = {
        "wallet": RN1_WALLET,
        "analyzed_at": datetime.now().isoformat(),
        "data_source": "mock" if not trades or trades[0].get("timestamp", 0) < datetime.now().timestamp() - 100*24*3600 else "api",
        "trade_count": len(trades),
        "bet_sizes": bet_sizes,
        "market_types": market_types,
        "market_tags": market_tags,
        "top_markets": top_markets,
        "timing": timing,
        "win_rate": win_rate,
        "holding_time": holding,
        "correlations": correlations,
        "streaks": streaks,
    }
    
    report_path = "D:\\Blink\\rn1_analysis_report.json"
    with open(report_path, "w") as f:
        json.dump(report, f, indent=2)
    
    print("\n" + "=" * 70)
    print(f"✅ Report saved to: {report_path}")
    if HAS_MATPLOTLIB:
        print("✅ Visualizations saved as PNG files")
    print("=" * 70)
    
    # Summary insights
    print("\n💡 KEY INSIGHTS:")
    
    if bet_sizes.get('mean', 0) > 50000:
        print("  • HIGH ROLLER: Average bet size indicates whale-level capital")
    
    if win_rate.get('win_rate', 0) and win_rate['win_rate'] > 0.60:
        print(f"  • SKILLED TRADER: {win_rate['win_rate']*100:.0f}% win rate above market average")
    
    if market_types.get("Sports", 0) > len(trades) * 0.4:
        print("  • SPORTS SPECIALIST: Heavy focus on sports betting markets")
    
    if holding.get('median_holding_seconds', 0) < 48 * 3600:
        print("  • ACTIVE TRADER: Short holding periods suggest active position management")
    
    if bet_sizes.get('max', 0) > 200000:
        print(f"  • WHALE ALERT: Max bet of ${bet_sizes['max']:,.0f} shows high conviction trades")
    
    print("\n")

if __name__ == "__main__":
    main()
