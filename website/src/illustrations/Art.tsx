// Larger artworks: the security checkpoints, the three-step journey, and the
// forge sparks behind the final call to action.
import { animate, createTimeline, stagger, svg, utils } from "animejs";
import { useAnime, whenVisible } from "../anim";

/* ------------------------------------------------------ cedar checkpoints */

const GATES = ["API", "MCP", "Job runner", "Agent hub", "Agent", "Plugin host"];

/** A request travels through every layer; each one checks it with Cedar. */
export function Checkpoints() {
  const ref = useAnime<SVGSVGElement>((root) => {
    const track = svg.createDrawable(".cp-track");
    const { translateX, translateY } = svg.createMotionPath(".cp-route");
    createTimeline({ autoplay: whenVisible(root), loop: true, loopDelay: 1600 })
      .set(".cp-ok", { opacity: 0, scale: 0.4 })
      .set(".cp-gate", { stroke: "rgba(143,220,240,0.35)" })
      .add(track, { draw: ["0 0", "0 1"], duration: 900, ease: "inOutQuad" })
      .add(".cp-req", { translateX, translateY, duration: 3600, ease: "inOutSine" }, 0)
      .add(".cp-gate", { stroke: "#8fdcf0", delay: stagger(560, { start: 300 }), duration: 200 }, 0)
      .add(".cp-ok", { opacity: 1, scale: 1, delay: stagger(560, { start: 420 }), duration: 350, ease: "outBack" }, 0);
  });
  const step = 96;
  return (
    <svg ref={ref} viewBox={`0 0 ${GATES.length * step + 40} 150`} className="w-full" role="img" aria-label="A request is authorized by Cedar at the API, MCP endpoint, job runner, agent hub, agent and plugin host.">
      <path className="cp-route" d={`M20 70 H${GATES.length * step + 20}`} fill="none" stroke="none" />
      <path className="cp-track" d={`M20 70 H${GATES.length * step + 20}`} fill="none" stroke="rgba(143,220,240,0.4)" strokeWidth="2" strokeDasharray="4 6" />
      {GATES.map((g, i) => {
        const x = 20 + step / 2 + i * step;
        return (
          <g key={g}>
            <rect className="cp-gate" x={x - 30} y="40" width="60" height="60" rx="16" fill="rgba(255,255,255,0.04)" stroke="#8fdcf0" strokeWidth="1.5" />
            <text x={x} y="74" textAnchor="middle" className="fill-white font-mono text-[10px]">
              cedar
            </text>
            <g className="cp-ok" style={{ transformOrigin: `${x + 26}px 42px` }}>
              <circle cx={x + 26} cy="42" r="9" fill="#4cd6a3" />
              <path d={`M${x + 22} 42l3 3 5-6`} stroke="#07141f" strokeWidth="2" fill="none" strokeLinecap="round" />
            </g>
            <text x={x} y="126" textAnchor="middle" className="fill-white/70 text-[11px]">
              {g}
            </text>
          </g>
        );
      })}
      <g className="cp-req">
        <circle cx="0" cy="0" r="8" fill="#ff8a55" transform="translate(20 70)" />
        <circle cx="0" cy="0" r="14" fill="#ff8a55" opacity="0.25" transform="translate(20 70)" />
      </g>
    </svg>
  );
}

/* ------------------------------------------------------------ three steps */

export function Journey() {
  const ref = useAnime<HTMLDivElement>((root) => {
    const line = svg.createDrawable(".jr-line");
    createTimeline({ autoplay: whenVisible(root) })
      .add(line, { draw: ["0 0", "0 1"], duration: 1400, ease: "inOutQuad" })
      .add(".jr-dot", { scale: [0, 1], delay: stagger(420), duration: 500, ease: "outBack" }, 0)
      .add(".jr-card", { opacity: [0, 1], y: [16, 0], delay: stagger(420), duration: 600, ease: "outQuad" }, 100);
  });
  const steps = [
    ["Install the control plane", "One package with the console built in. SQLite by default, PostgreSQL if you want.", "sudo ./install.sh"],
    ["Add your nodes", "Give Forge an SSH login and it installs the agent, or paste a one-line installer.", "Nodes → Add node"],
    ["Operate from one place", "Deploy, open terminals and files, watch alerts and review the audit log.", "forge deploy apply"],
  ];
  return (
    <div ref={ref} className="relative">
      <svg className="pointer-events-none absolute top-6 left-0 hidden h-4 w-full md:block" viewBox="0 0 1000 16" preserveAspectRatio="none" aria-hidden="true">
        <path className="jr-line" d="M160 8 H840" stroke="var(--forge-accent)" strokeWidth="2" strokeDasharray="6 6" fill="none" vectorEffect="non-scaling-stroke" />
      </svg>
      <ol className="relative grid gap-6 md:grid-cols-3">
        {steps.map(([t, b, c], i) => (
          <li key={t} className="flex flex-col items-center text-center">
            <span className="jr-dot relative z-10 flex size-12 items-center justify-center rounded-2xl bg-accent text-lg font-bold text-on-accent shadow-[0_10px_30px_-10px_var(--forge-accent)]">
              {i + 1}
            </span>
            <div className="jr-card card mt-5 w-full p-6">
              <h3 className="font-semibold">{t}</h3>
              <p className="mt-2 text-sm text-muted">{b}</p>
              <code className="mt-4 inline-block rounded-md bg-sunken px-2 py-1 font-mono text-xs text-accent">{c}</code>
            </div>
          </li>
        ))}
      </ol>
    </div>
  );
}

/* ------------------------------------------------------------ forge sparks */

/** Embers rising from an anvil: the background of the final call to action. */
export function Sparks() {
  const ref = useAnime<HTMLDivElement>((root) => {
    root.querySelectorAll<HTMLElement>(".sp").forEach((el) => {
      const run = () =>
        animate(el, {
          x: [utils.random(-20, 20), utils.random(-160, 160)],
          y: [0, utils.random(-220, -120)],
          opacity: [{ to: 1, duration: 120 }, { to: 0, duration: 1300 }],
          scale: [1, 0.2],
          duration: utils.random(1200, 2000),
          delay: utils.random(0, 2400),
          ease: "outQuad",
          onComplete: run,
        });
      run();
    });
  });
  return (
    <div ref={ref} className="pointer-events-none absolute inset-x-0 bottom-0 flex h-full justify-center" aria-hidden="true">
      <div className="absolute bottom-0 h-40 w-[60%] rounded-full bg-[radial-gradient(closest-side,color-mix(in_srgb,var(--forge-ember)_28%,transparent),transparent)] blur-2xl" />
      <div className="absolute bottom-6 left-1/2 h-0 w-0">
        {Array.from({ length: 28 }, (_, i) => (
          <span key={i} className="sp absolute size-1.5 rounded-full bg-ember opacity-0 shadow-[0_0_8px_var(--forge-ember)]" />
        ))}
      </div>
    </div>
  );
}
