import { useEffect, useRef } from 'react';

interface Point {
  nav: number;
  drawdown: number;
  time: string;
}

interface Props {
  data: Point[];
  height?: number;
}

/**
 * Lightweight canvas equity chart — replaces Recharts to eliminate SVG GC pressure.
 * Renders NAV as a green area + drawdown as a red dashed line.
 */
export function EquityChart({ data, height = 200 }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || data.length < 2) return;

    const dpr = window.devicePixelRatio || 1;
    const w = canvas.clientWidth;
    const h = canvas.clientHeight;
    canvas.width = w * dpr;
    canvas.height = h * dpr;

    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    ctx.scale(dpr, dpr);
    ctx.clearRect(0, 0, w, h);

    const PAD_L = 54, PAD_R = 46, PAD_T = 8, PAD_B = 24;
    const chartW = w - PAD_L - PAD_R;
    const chartH = h - PAD_T - PAD_B;

    const navVals = data.map(d => d.nav);
    const minNav = Math.min(...navVals);
    const maxNav = Math.max(...navVals);
    const navRange = maxNav - minNav || 1;

    const ddVals = data.map(d => d.drawdown);
    const minDd = Math.min(...ddVals, 0);
    const ddRange = Math.abs(minDd) || 1;

    const xOf = (i: number) => PAD_L + (i / (data.length - 1)) * chartW;
    const yNav = (v: number) => PAD_T + chartH - ((v - minNav) / navRange) * chartH;
    const yDd = (v: number) => PAD_T + chartH - ((v - minDd) / ddRange) * chartH;

    // --- NAV gradient fill ---
    const grad = ctx.createLinearGradient(0, PAD_T, 0, PAD_T + chartH);
    grad.addColorStop(0, 'rgba(16,185,129,0.28)');
    grad.addColorStop(1, 'rgba(16,185,129,0)');
    ctx.beginPath();
    ctx.moveTo(xOf(0), yNav(data[0].nav));
    for (let i = 1; i < data.length; i++) ctx.lineTo(xOf(i), yNav(data[i].nav));
    ctx.lineTo(xOf(data.length - 1), PAD_T + chartH);
    ctx.lineTo(xOf(0), PAD_T + chartH);
    ctx.closePath();
    ctx.fillStyle = grad;
    ctx.fill();

    // --- NAV line ---
    ctx.beginPath();
    ctx.strokeStyle = '#10b981';
    ctx.lineWidth = 2;
    ctx.moveTo(xOf(0), yNav(data[0].nav));
    for (let i = 1; i < data.length; i++) ctx.lineTo(xOf(i), yNav(data[i].nav));
    ctx.stroke();

    // --- Drawdown dashed line ---
    ctx.beginPath();
    ctx.strokeStyle = '#ef4444';
    ctx.lineWidth = 1;
    ctx.setLineDash([4, 4]);
    ctx.moveTo(xOf(0), yDd(data[0].drawdown));
    for (let i = 1; i < data.length; i++) ctx.lineTo(xOf(i), yDd(data[i].drawdown));
    ctx.stroke();
    ctx.setLineDash([]);

    // --- Y-axis labels (NAV, left) ---
    ctx.fillStyle = '#6b7280';
    ctx.font = '10px monospace';
    ctx.textAlign = 'right';
    for (let t = 0; t <= 4; t++) {
      const v = minNav + (navRange * t) / 4;
      const y = yNav(v);
      ctx.fillText(`$${v.toFixed(0)}`, PAD_L - 4, y + 4);
      ctx.beginPath();
      ctx.strokeStyle = '#1f2937';
      ctx.lineWidth = 0.5;
      ctx.moveTo(PAD_L, y);
      ctx.lineTo(PAD_L + chartW, y);
      ctx.stroke();
    }

    // --- Y-axis labels (drawdown, right) ---
    ctx.textAlign = 'left';
    ctx.fillStyle = '#ef4444';
    ctx.font = '9px monospace';
    ctx.fillText(`${minDd.toFixed(1)}%`, PAD_L + chartW + 4, PAD_T + chartH);
    ctx.fillText('0%', PAD_L + chartW + 4, PAD_T + 10);

    // --- X-axis time labels ---
    ctx.fillStyle = '#6b7280';
    ctx.font = '10px monospace';
    ctx.textAlign = 'center';
    const step = Math.max(1, Math.floor(data.length / 5));
    for (let i = 0; i < data.length; i += step) {
      if (data[i].time) {
        ctx.fillText(data[i].time, xOf(i), PAD_T + chartH + 14);
      }
    }
  }, [data]);

  return (
    <canvas
      ref={canvasRef}
      style={{ width: '100%', height, display: 'block' }}
    />
  );
}
