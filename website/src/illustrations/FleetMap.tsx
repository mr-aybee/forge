import { animate, createTimeline, stagger, svg, utils } from "animejs";
import { useAnime } from "../anim";

// Nodes around the control plane. Positions are in the 600×520 viewBox.
const NODES = [
  { id: "web-01", os: "linux", x: 70, y: 70, tag: "Ubuntu" },
  { id: "sql-01", os: "windows", x: 430, y: 60, tag: "Windows Server" },
  { id: "edge-fra", os: "linux", x: 20, y: 250, tag: "Debian · edge" },
  { id: "docker-03", os: "docker", x: 470, y: 240, tag: "Docker host" },
  { id: "build-02", os: "linux", x: 80, y: 410, tag: "Rocky Linux" },
  { id: "win-app", os: "windows", x: 420, y: 410, tag: "Windows 11" },
];
const CX = 300;
const CY = 250;
const W = 128;
const H = 54;

function pathFor(n: (typeof NODES)[number]) {
  // From the node card's inner edge to the hub, with a gentle curve.
  const sx = n.x + (n.x < CX ? W : 0);
  const sy = n.y + H / 2;
  const mx = (sx + CX) / 2;
  const my = (sy + CY) / 2 + (sy < CY ? -24 : 24);
  return `M${sx},${sy} Q${mx},${my} ${CX},${CY}`;
}

function OsGlyph({ os, x, y }: { os: string; x: number; y: number }) {
  if (os === "windows")
    return (
      <g transform={`translate(${x} ${y})`} fill="currentColor" className="text-accent">
        <rect width="7" height="7" rx="1" />
        <rect x="8.5" width="7" height="7" rx="1" />
        <rect y="8.5" width="7" height="7" rx="1" />
        <rect x="8.5" y="8.5" width="7" height="7" rx="1" />
      </g>
    );
  if (os === "docker")
    return (
      <g transform={`translate(${x} ${y + 2})`} className="text-accent" fill="currentColor">
        {[0, 5, 10].map((dx) => (
          <rect key={dx} x={dx} y="5" width="4" height="4" rx="0.8" />
        ))}
        <rect x="5" y="0" width="4" height="4" rx="0.8" />
        <path d="M-1 10h17c-1 3-4 5-9 5s-7-2-8-5Z" />
      </g>
    );
  return (
    <g transform={`translate(${x} ${y})`} className="text-accent" fill="none" stroke="currentColor" strokeWidth="1.6">
      <path d="M1 3l5 5-5 5" strokeLinecap="round" strokeLinejoin="round" />
      <path d="M8 13h7" strokeLinecap="round" />
    </g>
  );
}

function setup(root: SVGSVGElement) {
  // Nodes and links draw in, then agents keep dialling out to the hub.
  const lines = svg.createDrawable(".fm-link");
  const tl = createTimeline({ defaults: { ease: "outExpo" } });
  tl.add(".fm-hub", { scale: [0.6, 1], opacity: [0, 1], duration: 900 })
    .add(".fm-node", { opacity: [0, 1], y: [12, 0], delay: stagger(90), duration: 700 }, "-=500")
    .add(lines, { draw: ["0 0", "0 1"], delay: stagger(80), duration: 900, ease: "inOutQuad" }, "-=600")
    .add(".fm-badge", { opacity: [0, 1], scale: [0.8, 1], delay: stagger(140), duration: 600 }, "-=300")
    .add(".fm-block", { opacity: [0, 1], x: [-10, 0], delay: stagger(120), duration: 500 }, "-=400");

  // Packets travel node → hub along each link: agents dial out.
  root.querySelectorAll<SVGCircleElement>(".fm-packet").forEach((p, i) => {
    const { translateX, translateY } = svg.createMotionPath(`#fm-path-${i}`);
    animate(p, {
      translateX,
      translateY,
      opacity: [{ to: 1, duration: 150 }, { to: 1, duration: 1100 }, { to: 0, duration: 250 }],
      duration: 1500,
      delay: 1800 + i * 420,
      loopDelay: 1400 + utils.random(0, 1400),
      loop: true,
      ease: "inOutSine",
    });
  });

  // The hub breathes; the newest audit block keeps re-sealing.
  animate(".fm-pulse", { scale: [1, 1.9], opacity: [0.45, 0], duration: 2400, loop: true, delay: stagger(800), ease: "outSine" });
  animate(".fm-block-new", { opacity: [0.35, 1], duration: 900, alternate: true, loop: true, ease: "inOutSine" });
  animate(".fm-badge", { y: [0, -5], duration: 2600, alternate: true, loop: true, delay: stagger(500), ease: "inOutSine" });
}

/** The hero artwork: a fleet whose agents dial out to one control plane. */
export function FleetMap() {
  const ref = useAnime<SVGSVGElement>(setup);
  return (
    <svg
      ref={ref}
      viewBox="0 0 600 560"
      className="h-auto w-full overflow-visible"
      role="img"
      aria-label="Linux, Windows and Docker nodes connect out to the Forge control plane over mutual TLS; every action is sealed into a hash-chained audit log."
    >
      <defs>
        <radialGradient id="fm-hub-glow">
          <stop offset="0" stopColor="var(--forge-accent)" stopOpacity="0.35" />
          <stop offset="1" stopColor="var(--forge-accent)" stopOpacity="0" />
        </radialGradient>
        <linearGradient id="fm-hub-fill" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#174a6c" />
          <stop offset="1" stopColor="#0d2a40" />
        </linearGradient>
        <filter id="fm-soft" x="-20%" y="-20%" width="140%" height="160%">
          <feDropShadow dx="0" dy="6" stdDeviation="8" floodColor="#10314b" floodOpacity="0.12" />
        </filter>
      </defs>

      {/* orbit rings */}
      <g fill="none" stroke="var(--forge-border)" strokeDasharray="3 7">
        <circle cx={CX} cy={CY} r="120" />
        <circle cx={CX} cy={CY} r="200" opacity="0.7" />
      </g>

      {/* links and dial-out packets */}
      {NODES.map((n, i) => (
        <g key={n.id}>
          <path id={`fm-path-${i}`} d={pathFor(n)} fill="none" stroke="none" />
          <path className="fm-link" d={pathFor(n)} fill="none" stroke="var(--forge-accent)" strokeOpacity="0.45" strokeWidth="1.6" strokeDasharray="5 5" />
          <circle className="fm-packet" r="4.5" fill="var(--forge-ember)" opacity="0" />
        </g>
      ))}

      {/* hub: the control plane */}
      <g className="fm-hub" style={{ transformOrigin: `${CX}px ${CY}px` }}>
        <circle cx={CX} cy={CY} r="92" fill="url(#fm-hub-glow)" />
        <circle className="fm-pulse" cx={CX} cy={CY} r="48" fill="none" stroke="var(--forge-accent)" strokeWidth="1.5" style={{ transformOrigin: `${CX}px ${CY}px` }} />
        <circle className="fm-pulse" cx={CX} cy={CY} r="48" fill="none" stroke="var(--forge-accent)" strokeWidth="1.5" style={{ transformOrigin: `${CX}px ${CY}px` }} />
        <rect x={CX - 46} y={CY - 46} width="92" height="92" rx="22" fill="url(#fm-hub-fill)" filter="url(#fm-soft)" />
        <path
          transform={`translate(${CX - 26} ${CY - 28}) scale(0.82)`}
          d="M58 49V26c0-1-.3-2-.8-2.9s-1.2-1.6-2.1-2.1L34.6 9.6C33.8 9.2 32.9 9 32 9s-1.8.2-2.6.6L9 21c-.9.5-1.6 1.2-2.1 2.1S6 25 6 26v23c0 1 .3 2 .8 2.9s1.2 1.6 2.1 2.1l20.4 11.4c.8.4 1.7.6 2.6.6s1.8-.2 2.6-.6L55 54c.9-.5 1.6-1.2 2.1-2.1S58 50 58 49Z"
          fill="none"
          stroke="#8fdcf0"
          strokeWidth="3.4"
          strokeLinejoin="round"
        />
        <text x={CX} y={CY + 72} textAnchor="middle" className="fill-fg text-[13px] font-semibold">
          control plane
        </text>
      </g>

      {/* nodes */}
      {NODES.map((n) => (
        <g key={n.id} className="fm-node" filter="url(#fm-soft)">
          <rect x={n.x} y={n.y} width={W} height={H} rx="12" fill="var(--forge-bg-raised)" stroke="var(--forge-border)" />
          <circle cx={n.x + W - 12} cy={n.y + 12} r="3.5" fill="var(--forge-success)" />
          <OsGlyph os={n.os} x={n.x + 12} y={n.y + 12} />
          <text x={n.x + 34} y={n.y + 23} className="fill-fg font-mono text-[12px] font-semibold">
            {n.id}
          </text>
          <text x={n.x + 12} y={n.y + 42} className="fill-muted text-[10.5px]">
            {n.tag}
          </text>
        </g>
      ))}

      {/* floating capability badges */}
      {[
        { x: 196, y: 120, t: "mTLS · dial-out" },
        { x: 330, y: 352, t: "Cedar ✓" },
        { x: 176, y: 352, t: "MFA" },
      ].map((b) => (
        <g key={b.t} className="fm-badge">
          <rect x={b.x} y={b.y} width={b.t.length * 6.6 + 18} height="24" rx="12" fill="var(--forge-ember-soft)" stroke="var(--forge-ember)" strokeOpacity="0.4" />
          <text x={b.x + 9} y={b.y + 16} className="text-[11px] font-semibold" fill="var(--forge-ember)">
            {b.t}
          </text>
        </g>
      ))}

      {/* hash-chained audit log */}
      <g transform="translate(78 500)">
        <text x="0" y="-12" className="fill-muted text-[10.5px] font-semibold tracking-[0.12em] uppercase">
          audit chain
        </text>
        {["deploy shop-api", "open terminal", "grant bucket", "rollout agents", "sealed…"].map((t, i) => (
          <g key={t} className={`fm-block ${i === 4 ? "fm-block-new" : ""}`} transform={`translate(${i * 92} 0)`}>
            {i > 0 && <path d="M-14 14h14" stroke="var(--forge-accent)" strokeWidth="1.6" strokeDasharray="2 3" />}
            <rect width="78" height="28" rx="7" fill={i === 4 ? "var(--forge-accent-soft)" : "var(--forge-bg-raised)"} stroke={i === 4 ? "var(--forge-accent)" : "var(--forge-border)"} />
            <text x="8" y="18" className="fill-muted font-mono text-[8.5px]">
              {t}
            </text>
          </g>
        ))}
      </g>
    </svg>
  );
}
