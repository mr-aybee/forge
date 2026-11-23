// Small animated scenes for the feature cards. Each one renders a complete,
// meaningful static picture; anime.js adds motion when it scrolls into view.
import { animate, createTimeline, stagger, svg } from "animejs";
import { useAnime, whenVisible } from "../anim";

/* ---------------------------------------------------------------- terminal */

const TERM = [
  { p: "$", t: "forge ssh web-01", c: "text-[#8fdcf0]" },
  { p: "", t: "✓ certificate issued · expires in 5 min", c: "text-[#4cd6a3]" },
  { p: "deploy@web-01:~$", t: "systemctl status shop-api", c: "text-white/85" },
  { p: "", t: "● shop-api.service  active (running)", c: "text-[#4cd6a3]" },
];

export function TerminalScene() {
  const ref = useAnime<HTMLDivElement>((root) => {
    createTimeline({ autoplay: whenVisible(root) })
      .add(".ts-line", { opacity: [0, 1], x: [-6, 0], delay: stagger(550), duration: 400, ease: "outQuad" })
      .add(".ts-caret", { opacity: [1, 0], duration: 500, loop: true, alternate: true, ease: "steps(1)" });
  });
  return (
    <div ref={ref} className="h-full overflow-hidden rounded-xl bg-[#071521] p-4 font-mono text-[11.5px] leading-relaxed shadow-inner" aria-hidden="true">
      <div className="mb-3 flex gap-1.5">
        <span className="size-2 rounded-full bg-[#ff5f57]" />
        <span className="size-2 rounded-full bg-[#febc2e]" />
        <span className="size-2 rounded-full bg-[#28c840]" />
      </div>
      {TERM.map((l) => (
        <div key={l.t} className={`ts-line whitespace-nowrap ${l.c}`}>
          {l.p && <span className="mr-2 text-white/40">{l.p}</span>}
          {l.t}
        </div>
      ))}
      <span className="ts-caret mt-1 inline-block h-3.5 w-2 bg-[#8fdcf0]" />
    </div>
  );
}

/* ----------------------------------------------------------------- rollout */

const WAVES = [
  { label: "canary", pct: 10 },
  { label: "batch 2", pct: 50 },
  { label: "batch 3", pct: 100 },
];

export function RolloutScene() {
  const ref = useAnime<HTMLDivElement>((root) => {
    createTimeline({ autoplay: whenVisible(root), loop: true, loopDelay: 1800 })
      .set(".rs-bar", { scaleX: 0 })
      .set(".rs-gate", { opacity: 0.25 })
      .add(".rs-bar", { scaleX: [0, 1], delay: stagger(900), duration: 800, ease: "inOutCubic" })
      .add(".rs-gate", { opacity: [0.25, 1], scale: [0.6, 1], delay: stagger(900), duration: 350, ease: "outBack" }, 450);
  });
  return (
    <div ref={ref} className="space-y-3" aria-hidden="true">
      <div className="flex items-center justify-between text-[11px] text-muted">
        <span className="font-mono">shop-api 2.4.0 → 2.4.1</span>
        <span className="rounded-full bg-accent-soft px-2 py-0.5 font-semibold text-accent">canary</span>
      </div>
      {WAVES.map((w) => (
        <div key={w.label} className="flex items-center gap-3">
          <span className="w-14 shrink-0 text-[11px] text-muted">{w.label}</span>
          <div className="h-2.5 flex-1 overflow-hidden rounded-full bg-sunken">
            <div className="rs-bar h-full origin-left rounded-full bg-gradient-to-r from-accent to-[#4cc3dd]" style={{ width: `${w.pct}%` }} />
          </div>
          <span className="rs-gate inline-flex size-5 items-center justify-center rounded-full bg-ok text-[10px] font-bold text-white">✓</span>
        </div>
      ))}
      <div className="flex gap-2 pt-1 text-[10.5px]">
        <span className="rounded-md border border-line px-2 py-0.5 text-muted">error rate &lt; 1%</span>
        <span className="rounded-md border border-line px-2 py-0.5 text-muted">p95 &lt; 300 ms</span>
        <span className="rounded-md border border-line px-2 py-0.5 text-muted">auto-rollback</span>
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------- audit */

export function AuditScene() {
  const ref = useAnime<SVGSVGElement>((root) => {
    const links = svg.createDrawable(".as-link");
    createTimeline({ autoplay: whenVisible(root) })
      .add(".as-block", { opacity: [0, 1], y: [10, 0], delay: stagger(260), duration: 450, ease: "outBack" })
      .add(links, { draw: ["0 0", "0 1"], delay: stagger(260), duration: 400 }, 200)
      .add(".as-seal", { scale: [0, 1], opacity: [0, 1], duration: 500, ease: "outElastic(1, .6)" });
  });
  const blocks = ["a1f3…", "7c09…", "e44b…", "19d2…"];
  return (
    <svg ref={ref} viewBox="0 0 300 96" className="w-full" aria-hidden="true">
      {blocks.map((h, i) => (
        <g key={h} transform={`translate(${i * 74 + 4} 22)`}>
          {i > 0 && <path className="as-link" d="M-18 22h18" stroke="var(--forge-accent)" strokeWidth="2" fill="none" />}
          <g className="as-block">
            <rect width="56" height="44" rx="9" fill="var(--forge-bg-raised)" stroke="var(--forge-border)" />
            <rect x="8" y="9" width="28" height="4" rx="2" fill="var(--forge-border)" />
            <rect x="8" y="17" width="38" height="4" rx="2" fill="var(--forge-border)" />
            <text x="8" y="35" className="font-mono text-[8.5px]" fill="var(--forge-accent)">
              {h}
            </text>
          </g>
        </g>
      ))}
      <g className="as-seal" style={{ transformOrigin: "284px 22px" }}>
        <circle cx="284" cy="22" r="11" fill="var(--forge-ember)" />
        <path d="M279 22l3.5 3.5 6.5-7" stroke="#fff" strokeWidth="2" fill="none" strokeLinecap="round" strokeLinejoin="round" />
      </g>
    </svg>
  );
}

/* --------------------------------------------------------------- app store */

const APPS = ["PostgreSQL", "Nginx", "Redis", "Node.js", ".NET", "MariaDB", "Python", "RustFS", "Docker"];

export function AppStoreScene() {
  const ref = useAnime<HTMLDivElement>((root) => {
    createTimeline({ autoplay: whenVisible(root) })
      .add(".ap-tile", { opacity: [0, 1], scale: [0.85, 1], delay: stagger(70, { grid: [3, 3], from: "center" }), duration: 500, ease: "outBack" })
      .add(".ap-done", { opacity: [0, 1], scale: [0, 1], delay: stagger(160, { grid: [3, 3], from: "first" }), duration: 300, ease: "outBack" });
  });
  return (
    <div ref={ref} className="grid grid-cols-3 gap-2" aria-hidden="true">
      {APPS.map((a, i) => (
        <div key={a} className="ap-tile relative rounded-lg border border-line bg-raised px-2 py-2.5 text-center text-[11px] font-medium shadow-card">
          {a}
          {i % 3 !== 2 && <span className="ap-done absolute -top-1.5 -right-1.5 inline-flex size-4 items-center justify-center rounded-full bg-ok text-[9px] text-white">✓</span>}
        </div>
      ))}
    </div>
  );
}

/* --------------------------------------------------------------------- MCP */

export function McpScene() {
  const ref = useAnime<HTMLDivElement>((root) => {
    createTimeline({ autoplay: whenVisible(root), loop: true, loopDelay: 2500 })
      .set(".mc-step", { opacity: 0, y: 8 })
      .set(".mc-knob", { x: 0 })
      .set(".mc-track", { backgroundColor: "var(--forge-border)" })
      .add(".mc-step", { opacity: 1, y: 0, delay: stagger(700), duration: 450, ease: "outQuad" })
      .add(".mc-knob", { x: 16, duration: 300, ease: "outBack" }, "-=600")
      .add(".mc-track", { backgroundColor: "var(--forge-success)", duration: 300 }, "<<");
  });
  return (
    <div ref={ref} className="space-y-2 text-[11.5px]" aria-hidden="true">
      <div className="mc-step ml-auto w-fit max-w-[85%] rounded-2xl rounded-br-sm bg-accent px-3 py-2 text-on-accent">Roll out shop-api 2.4.1 to production</div>
      <div className="mc-step w-fit max-w-[90%] rounded-2xl rounded-bl-sm border border-line bg-raised px-3 py-2">
        <span className="font-mono text-accent">forge_deploy</span> needs approval from a person.
      </div>
      <div className="mc-step flex items-center justify-between rounded-xl border border-ember/40 bg-ember-soft px-3 py-2">
        <span className="font-medium">Approve &amp; run as you</span>
        <span className="mc-track relative inline-block h-5 w-9 rounded-full bg-line">
          <span className="mc-knob absolute top-0.5 left-0.5 size-4 rounded-full bg-white shadow" />
        </span>
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------- files */

export function FilesScene() {
  const ref = useAnime<HTMLDivElement>((root) => {
    createTimeline({ autoplay: whenVisible(root) })
      .add(".fs-row", { opacity: [0, 1], x: [-8, 0], delay: stagger(110), duration: 400, ease: "outQuad" })
      .add(".fs-hl", { backgroundColor: ["rgba(0,0,0,0)", "var(--forge-accent-soft)"], duration: 400 });
  });
  const rows: [number, string, string][] = [
    [0, "📁", "/srv/shop-api"],
    [1, "📁", "releases"],
    [1, "📄", "appsettings.json"],
    [1, "📄", ".env  · sealed"],
    [0, "🔗", "sftp access · expires 2 h"],
  ];
  return (
    <div ref={ref} className="space-y-1 font-mono text-[11.5px]" aria-hidden="true">
      {rows.map(([d, ic, t], i) => (
        <div key={t} className={`fs-row flex items-center gap-2 rounded-md px-2 py-1 ${i === 2 ? "fs-hl" : ""}`} style={{ paddingLeft: `${8 + d * 16}px` }}>
          <span>{ic}</span>
          <span className={i === 4 ? "text-ember" : ""}>{t}</span>
        </div>
      ))}
    </div>
  );
}

/* --------------------------------------------------------- count-up number */

export function CountUp({ value, className }: { value: string; className?: string }) {
  const ref = useAnime<HTMLSpanElement>((root) => {
    const m = value.match(/^([\d,]+)(.*)$/);
    if (!m) return;
    const target = Number(m[1].replace(/,/g, ""));
    const o = { n: 0 };
    animate(o, {
      n: target,
      duration: 1600,
      ease: "outExpo",
      autoplay: whenVisible(root),
      onUpdate: () => {
        root.textContent = `${Math.round(o.n).toLocaleString("en-US")}${m[2]}`;
      },
    });
  });
  return (
    <span ref={ref} className={className}>
      {value}
    </span>
  );
}
