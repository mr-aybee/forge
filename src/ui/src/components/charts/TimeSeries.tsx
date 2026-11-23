import { useEffect, useId, useMemo, useRef, useState } from "react";

export interface Series {
  label: string;
  /** CSS colour, e.g. `var(--color-accent)`. */
  color: string;
  values: (number | null)[];
}

const H = 160;
const PAD = { l: 44, r: 8, t: 8, b: 20 };

function niceMax(v: number) {
  if (v <= 0) return 1;
  const p = 10 ** Math.floor(Math.log10(v));
  for (const m of [1, 2, 2.5, 5, 10]) if (m * p >= v) return m * p;
  return 10 * p;
}

const clock = (t: number) => new Date(t).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });

/**
 * Small dependency-free SVG line/area chart for live metrics. Scales to its
 * container's width; hovering shows the values at that moment.
 */
export function TimeSeries({
  times,
  series,
  max,
  format,
  from,
  to,
  height = H,
}: {
  times: number[];
  series: Series[];
  /** Fixed y maximum (e.g. 100 for percentages); auto otherwise. */
  max?: number;
  format: (v: number) => string;
  /** Visible window (epoch ms). */
  from: number;
  to: number;
  height?: number;
}) {
  const gid = useId();
  const ref = useRef<SVGSVGElement>(null);
  const box = useRef<HTMLDivElement>(null);
  const [W, setW] = useState(600);
  const [hover, setHover] = useState<number | null>(null);
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setW(Math.max(200, Math.round(el.clientWidth))));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const yMax = useMemo(() => max ?? niceMax(Math.max(0, ...series.flatMap((s) => s.values.filter((v): v is number => v !== null))) * 1.1), [max, series]);
  const span = Math.max(1, to - from);
  const x = (t: number) => PAD.l + ((t - from) / span) * (W - PAD.l - PAD.r);
  const y = (v: number) => PAD.t + (1 - Math.min(v, yMax) / yMax) * (height - PAD.t - PAD.b);

  const paths = series.map((s) => {
    let d = "";
    let area = "";
    let start: number | null = null;
    let last: number | null = null;
    s.values.forEach((v, i) => {
      const t = times[i];
      if (v === null || t === undefined || t < from) {
        if (start !== null && last !== null) area += `L${last},${y(0)}L${start},${y(0)}Z`;
        start = null;
        return;
      }
      const px = x(t).toFixed(1);
      const py = y(v).toFixed(1);
      if (start === null) {
        d += `M${px},${py}`;
        area += `M${px},${y(0)}L${px},${py}`;
        start = Number(px);
      } else {
        d += `L${px},${py}`;
        area += `L${px},${py}`;
      }
      last = Number(px);
    });
    if (start !== null && last !== null) area += `L${last},${y(0)}L${start},${y(0)}Z`;
    return { d, area };
  });

  const ticks = [0, 0.5, 1].map((f) => f * yMax);
  const xTicks = [0, 0.25, 0.5, 0.75, 1].map((f) => from + f * span);

  const onMove = (e: React.PointerEvent) => {
    const r = ref.current?.getBoundingClientRect();
    if (!r || times.length === 0) return;
    const t = from + ((((e.clientX - r.left) / r.width) * W - PAD.l) / (W - PAD.l - PAD.r)) * span;
    let best = 0;
    for (let i = 1; i < times.length; i++) if (Math.abs((times[i] ?? 0) - t) < Math.abs((times[best] ?? 0) - t)) best = i;
    setHover((times[best] ?? 0) >= from ? best : null);
  };

  const ht = hover !== null ? times[hover] : undefined;

  return (
    <div ref={box} className="relative w-full">
      <svg
        ref={ref}
        width={W}
        height={height}
        viewBox={`0 0 ${W} ${height}`}
        className="block touch-none select-none"
        onPointerMove={onMove}
        onPointerLeave={() => setHover(null)}
        role="img"
      >
        <defs>
          {series.map((s, i) => (
            <linearGradient key={s.label} id={`${gid}-${i}`} x1="0" x2="0" y1="0" y2="1">
              <stop offset="0%" stopColor={s.color} stopOpacity="0.28" />
              <stop offset="100%" stopColor={s.color} stopOpacity="0.02" />
            </linearGradient>
          ))}
        </defs>
        {ticks.map((v) => (
          <g key={v}>
            <line x1={PAD.l} x2={W - PAD.r} y1={y(v)} y2={y(v)} stroke="var(--color-line)" strokeDasharray={v === 0 ? undefined : "3 4"} vectorEffect="non-scaling-stroke" />
            <text x={PAD.l - 6} y={y(v) + 3} textAnchor="end" fontSize="10" fill="var(--color-faint)">
              {format(v)}
            </text>
          </g>
        ))}
        {xTicks.slice(1, -1).map((t) => (
          <text key={t} x={x(t)} y={height - 5} textAnchor="middle" fontSize="10" fill="var(--color-faint)">
            {clock(t).slice(0, 5)}
          </text>
        ))}
        {paths.map((p, i) => (
          <g key={series[i]?.label}>
            <path d={p.area} fill={`url(#${gid}-${i})`} />
            <path d={p.d} fill="none" stroke={series[i]?.color} strokeWidth="1.75" strokeLinejoin="round" vectorEffect="non-scaling-stroke" />
          </g>
        ))}
        {ht !== undefined && <line x1={x(ht)} x2={x(ht)} y1={PAD.t} y2={height - PAD.b} stroke="var(--color-faint)" vectorEffect="non-scaling-stroke" />}
      </svg>
      {hover !== null && ht !== undefined && (
        <div
          className="pointer-events-none absolute top-1 z-10 rounded-md border border-line bg-panel px-2 py-1 text-[11px] shadow-md"
          style={{ left: `${Math.min(80, Math.max(2, ((x(ht) / W) * 100) - 10))}%` }}
        >
          <div className="text-faint">{clock(ht)}</div>
          {series.map((s) => {
            const v = s.values[hover];
            return (
              <div key={s.label} className="flex items-center gap-1.5">
                <span className="size-2 rounded-full" style={{ background: s.color }} />
                {s.label}: <span className="font-medium">{v === null || v === undefined ? "—" : format(v)}</span>
              </div>
            );
          })}
        </div>
      )}
      {times.length === 0 && <div className="absolute inset-0 grid place-items-center text-xs text-faint">Waiting for samples…</div>}
    </div>
  );
}

/** Inline sparkline for table cells. */
export function Sparkline({ values, color = "var(--color-accent)", width = 96, height = 24 }: { values: (number | null)[]; color?: string; width?: number; height?: number }) {
  const nums = values.filter((v): v is number => v !== null);
  if (nums.length < 2) return <span className="text-xs text-faint">—</span>;
  const max = Math.max(...nums) || 1;
  const step = width / Math.max(1, values.length - 1);
  let d = "";
  values.forEach((v, i) => {
    if (v === null) return;
    d += `${d ? "L" : "M"}${(i * step).toFixed(1)},${(height - 2 - (v / max) * (height - 4)).toFixed(1)}`;
  });
  return (
    <svg width={width} height={height} className="inline-block align-middle" aria-hidden>
      <path d={d} fill="none" stroke={color} strokeWidth="1.5" />
    </svg>
  );
}

export const fmtBytes = (b: number) => {
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  let v = b;
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v >= 100 || i === 0 ? v.toFixed(0) : v.toFixed(1)} ${u[i]}`;
};
export const fmtRate = (b: number) => `${fmtBytes(b)}/s`;
export const fmtPct = (v: number) => `${v.toFixed(v < 10 && v > 0 ? 1 : 0)}%`;
