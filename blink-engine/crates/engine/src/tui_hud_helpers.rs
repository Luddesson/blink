fn render_header(
    f:          &mut Frame,
    area:       Rect,
    snap:       &PortfolioSnapshot,
    rn1_wallet: &str,
    ws_live:    bool,
    paused:     bool,
    uptime_s:   u64,
    perf:       &PerfSnapshot,
) {
    let nav_delta = snap.nav - crate::paper_portfolio::STARTING_BALANCE_USDC;
    let nav_pct   = nav_delta / crate::paper_portfolio::STARTING_BALANCE_USDC * 100.0;
    let nav_color = pnl_color(nav_delta);

    let status_icon = if paused { "⏸" } else { "▶" };
    let status_color = if paused { MONO_GOLD } else { MONO_GREEN };

    let ws_icon = if ws_live { "🌐" } else { "💀" };
    let ws_color = if ws_live { MONO_GREEN } else { MONO_PINK };

    let time_str = Local::now().format("%H:%M:%S").to_string();
    let uptime_str = format!("{}:{:02}:{:02}", uptime_s / 3600, (uptime_s % 3600) / 60, uptime_s % 60);

    let rn1_short = if rn1_wallet.len() >= 10 {
        format!("{}...{}", &rn1_wallet[..6], &rn1_wallet[rn1_wallet.len()-4..])
    } else {
        rn1_wallet.to_string()
    };

    let main_line = Line::from(vec![
        Span::styled(format!(" {status_icon} BLINK "), Style::default().fg(status_color).add_modifier(Modifier::BOLD)),
        Span::styled(" │ ", Style::default().fg(MONO_GRAY)),
        Span::styled(format!("{ws_icon} WS "), Style::default().fg(ws_color)),
        Span::styled(" │ ", Style::default().fg(MONO_GRAY)),
        Span::styled("🐋 RN1: ", Style::default().fg(MONO_GRAY)),
        Span::styled(rn1_short, Style::default().fg(MONO_PURPLE)),
        Span::styled(" │ ", Style::default().fg(MONO_GRAY)),
        Span::styled("EQUITY: ", Style::default().fg(MONO_GRAY)),
        Span::styled(
            format!("${:.2} ({:>+.2}%)", snap.nav, nav_pct),
            Style::default().fg(nav_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" │ ", Style::default().fg(MONO_GRAY)),
        Span::styled(format!("{} msg/s ", perf.msgs_per_sec as u64), Style::default().fg(MONO_BLUE)),
        Span::styled(" │ ", Style::default().fg(MONO_GRAY)),
        Span::styled(time_str, Style::default().fg(MONO_GRAY)),
        Span::styled(format!(" (up {})", uptime_str), Style::default().fg(MONO_GRAY)),
    ]);

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(MONO_GRAY));

    f.render_widget(Paragraph::new(main_line).block(block).alignment(Alignment::Left), area);
}

fn render_portfolio(f: &mut Frame, area: Rect, snap: &PortfolioSnapshot) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8), // Stats
            Constraint::Min(4),    // High-Res Equity Graph
            Constraint::Length(3), // Risk Gauge
        ])
        .split(area);

    // 1. Stats Panel
    let label_style = Style::default().fg(MONO_GRAY);
    let val_style = Style::default().fg(Color::White).add_modifier(Modifier::BOLD);
    
    let stats_lines = vec![
        Line::from(vec![Span::styled("  CASH      ", label_style), Span::styled(format!("${:.2}", snap.cash_usdc), val_style)]),
        Line::from(vec![Span::styled("  INVESTED  ", label_style), Span::styled(format!("${:.2}", snap.total_invested), val_style)]),
        Line::from(vec![Span::styled("  UNREAL    ", label_style), Span::styled(format!("{:>+.2} USDC", snap.unrealized_pnl), pnl_style(snap.unrealized_pnl))]),
        Line::from(vec![Span::styled("  REALIZED  ", label_style), Span::styled(format!("{:>+.2} USDC", snap.realized_pnl), pnl_style(snap.realized_pnl))]),
        Line::from(""),
        Line::from(vec![Span::styled("  SIGNALS   ", label_style), Span::styled(format!("{}", snap.total_signals), Style::default().fg(MONO_BLUE))]),
        Line::from(vec![Span::styled("  SUCCESS   ", label_style), Span::styled(format!("{}", snap.filled_orders), Style::default().fg(MONO_GREEN))]),
    ];
    f.render_widget(Paragraph::new(stats_lines), chunks[0]);

    // 2. High-Res Braille Equity Graph
    if snap.equity_curve.len() > 2 {
        let min = snap.equity_curve.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = snap.equity_curve.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let range = (max - min).max(0.01);
        
        let canvas = Canvas::default()
            .block(Block::default().title(" PERFORMANCE DNA ").border_style(Style::default().fg(MONO_GRAY)))
            .x_bounds([0.0, snap.equity_curve.len() as f64])
            .y_bounds([min - range * 0.1, max + range * 0.1])
            .paint(|ctx| {
                for i in 0..snap.equity_curve.len().saturating_sub(1) {
                    ctx.draw(&CanvasLine {
                        x1: i as f64,
                        y1: snap.equity_curve[i],
                        x2: (i + 1) as f64,
                        y2: snap.equity_curve[i+1],
                        color: MONO_GREEN,
                    });
                }
            });
        f.render_widget(canvas, chunks[1]);
    }

    // 3. Risk Gauge (Gradient)
    let loss_pct = (snap.realized_pnl.min(0.0).abs() / 100.0).min(1.0); // Simple 100 USDC limit for gauge
    let gauge_width = area.width.saturating_sub(15) as usize;
    let filled = (loss_pct * gauge_width as f64) as usize;
    let gauge_color = if loss_pct > 0.8 { MONO_PINK } else if loss_pct > 0.5 { MONO_GOLD } else { MONO_GREEN };
    
    let gauge_bar = format!("{}{}", "█".repeat(filled), "░".repeat(gauge_width.saturating_sub(filled)));
    let gauge_line = Line::from(vec![
        Span::styled("  RISK HUD  ", label_style),
        Span::styled(gauge_bar, Style::default().fg(gauge_color)),
        Span::styled(format!(" {:.0}%", loss_pct * 100.0), Style::default().fg(gauge_color).add_modifier(Modifier::BOLD)),
    ]);
    f.render_widget(Paragraph::new(gauge_line), chunks[2]);
}
