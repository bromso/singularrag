export type Pt = { x: number; y: number };

const cross = (o: Pt, a: Pt, b: Pt) => (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);

/** Andrew's monotone chain; counter-clockwise; collinear points dropped. */
export function convexHull(points: Pt[]): Pt[] {
  const pts = [...points].sort((a, b) => a.x - b.x || a.y - b.y);
  if (pts.length < 3) return pts;
  const lower: Pt[] = [];
  for (const p of pts) { while (lower.length >= 2 && cross(lower[lower.length - 2], lower[lower.length - 1], p) <= 0) lower.pop(); lower.push(p); }
  const upper: Pt[] = [];
  for (const p of [...pts].reverse()) { while (upper.length >= 2 && cross(upper[upper.length - 2], upper[upper.length - 1], p) <= 0) upper.pop(); upper.push(p); }
  upper.pop(); lower.pop();
  return [...lower, ...upper];
}

export function padHull(points: Pt[], padding: number): Pt[] {
  if (points.length === 0) return [];
  if (points.length === 1) {
    const c = points[0];
    return Array.from({ length: 12 }, (_, i) => { const t = (i / 12) * Math.PI * 2; return { x: c.x + Math.cos(t) * padding, y: c.y + Math.sin(t) * padding }; });
  }
  if (points.length === 2) {
    const [a, b] = points;
    const out: Pt[] = [];
    const ang = Math.atan2(b.y - a.y, b.x - a.x);
    for (let i = 0; i <= 7; i++) { const t = ang + Math.PI / 2 + (i / 7) * Math.PI; out.push({ x: a.x + Math.cos(t) * padding, y: a.y + Math.sin(t) * padding }); }
    for (let i = 0; i <= 7; i++) { const t = ang - Math.PI / 2 + (i / 7) * Math.PI; out.push({ x: b.x + Math.cos(t) * padding, y: b.y + Math.sin(t) * padding }); }
    return out;
  }
  const cx = points.reduce((s, p) => s + p.x, 0) / points.length;
  const cy = points.reduce((s, p) => s + p.y, 0) / points.length;
  return points.map((p) => {
    const dx = p.x - cx, dy = p.y - cy, d = Math.hypot(dx, dy) || 1;
    return { x: p.x + (dx / d) * padding, y: p.y + (dy / d) * padding };
  });
}
