// Turns ```diagram code blocks (a small YAML spec) into styled HTML at build
// time. No client JavaScript; styles are in src/styles/diagrams.css.
// Authoring reference: docs/src/content/docs/development/diagrams.md.
import { parse } from "yaml";

const esc = (s) =>
  String(s ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");

/** Escapes, then turns `code` into <code> and **bold** into <strong>. */
const md = (s) =>
  esc(s)
    .replace(/`([^`]+)`/g, "<code>$1</code>")
    .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
    .replace(/\n/g, "<br>");

const TONES = new Set(["accent", "muted", "ok", "warn", "danger"]);
const tone = (t) => (t && TONES.has(t) ? ` fd-tone-${t}` : "");

const fail = (msg) => {
  throw new Error(`diagram: ${msg}`);
};

function node(n) {
  const o = typeof n === "string" ? { label: n } : n;
  if (!o || o.label == null) fail(`node needs a label: ${JSON.stringify(n)}`);
  return `<div class="fd-node${tone(o.tone)}"><span class="fd-label">${md(o.label)}</span>${
    o.note ? `<span class="fd-note">${md(o.note)}</span>` : ""
  }</div>`;
}

function edge(label, dir = "fwd") {
  const cls = dir === "both" ? " fd-both" : dir === "back" ? " fd-back" : "";
  return `<div class="fd-edge${cls}" aria-hidden="${label ? "false" : "true"}"><span class="fd-edge-label">${md(label ?? "")}</span><span class="fd-arrow"></span></div>`;
}

function chain(steps) {
  if (!Array.isArray(steps) || steps.length === 0) fail("a flow row needs steps");
  let out = "";
  steps.forEach((s, i) => {
    out += node(s);
    if (i < steps.length - 1) out += edge(typeof s === "object" ? s.edge : "", typeof s === "object" ? s.dir : undefined);
  });
  return out;
}

function flow(spec) {
  const dir = spec.direction === "column" ? "column" : "row";
  const rows = spec.rows ?? [{ steps: spec.steps }];
  return `<div class="fd fd-flow fd-${dir}">${rows
    .map(
      (r) =>
        `<div class="fd-row">${r.title ? `<div class="fd-row-title">${md(r.title)}</div>` : ""}<div class="fd-chain">${chain(r.steps)}</div></div>`,
    )
    .join("")}</div>`;
}

function states(spec) {
  const path = spec.path ?? fail("states needs a path");
  const pill = (s) => {
    const o = typeof s === "string" ? { state: s } : s;
    return `<div class="fd-state${tone(o.tone)}">${md(o.state)}</div>`;
  };
  let main = "";
  path.forEach((s, i) => {
    main += pill(s);
    if (i < path.length - 1) main += edge(typeof s === "object" ? s.edge : "", typeof s === "object" ? s.dir : undefined);
  });
  const tr = (spec.transitions ?? [])
    .map(
      (t) =>
        `<li><span class="fd-state fd-sm${tone(t.fromTone)}">${md(t.from)}</span><span class="fd-tr-arrow"><span class="fd-edge-label">${md(t.when ?? "")}</span></span><span class="fd-state fd-sm${tone(t.tone)}">${md(t.to)}</span></li>`,
    )
    .join("");
  return `<div class="fd fd-states"><div class="fd-chain fd-main">${main}</div>${
    tr ? `<ul class="fd-transitions" aria-label="Other transitions">${tr}</ul>` : ""
  }</div>`;
}

function treeItems(items) {
  return `<ul>${items
    .map((it) => {
      const o = typeof it === "string" ? { label: it } : it;
      return `<li><div class="fd-tnode${tone(o.tone)}"><span class="fd-label">${md(o.label)}</span>${
        o.note ? `<span class="fd-note">${md(o.note)}</span>` : ""
      }</div>${o.children?.length ? treeItems(o.children) : ""}</li>`;
    })
    .join("")}</ul>`;
}

function tree(spec) {
  const items = Array.isArray(spec) ? spec : spec.items;
  if (!items?.length) fail("tree needs items");
  return `<div class="fd fd-tree">${treeItems(items)}</div>`;
}

function sequence(spec) {
  const actors = spec.actors ?? fail("sequence needs actors");
  const n = actors.length;
  const col = (name) => {
    const i = actors.indexOf(name);
    if (i < 0) fail(`unknown actor "${name}"`);
    return i;
  };
  const pct = (x) => `${((x / n) * 100).toFixed(4)}%`;
  const rows = (spec.messages ?? [])
    .map((m) => {
      if (m.at !== undefined) {
        const i = col(m.at);
        const span = m.span ? col(m.span) : i;
        const a = Math.min(i, span);
        const b = Math.max(i, span);
        return `<div class="fd-seq-row fd-seq-noterow"><div class="fd-seq-note" style="left:calc(${pct(a)} + 6px);width:calc(${pct(b - a + 1)} - 12px)">${md(m.note)}</div></div>`;
      }
      const f = col(m.from);
      const t = col(m.to);
      const a = Math.min(f, t);
      const b = Math.max(f, t);
      const back = t < f;
      return `<div class="fd-seq-row"><div class="fd-seq-msg${back ? " fd-back" : ""}" style="left:${pct(a + 0.5)};width:${pct(b - a)}"><span class="fd-edge-label">${md(m.label)}</span><span class="fd-arrow"></span></div></div>`;
    })
    .join("");
  const heads = actors.map((a) => `<div class="fd-seq-actor">${md(a)}</div>`).join("");
  const lines = actors.map((_, i) => `<span class="fd-seq-life" style="left:${pct(i + 0.5)}"></span>`).join("");
  return `<div class="fd fd-seq" style="--fd-cols:${n}"><div class="fd-seq-inner"><div class="fd-seq-heads">${heads}</div><div class="fd-seq-body">${lines}${rows}</div></div></div>`;
}

const KINDS = { flow, states, tree, sequence };

function render(value, file) {
  let spec;
  try {
    spec = parse(value);
  } catch (e) {
    fail(`invalid YAML in ${file}: ${e.message}`);
  }
  const kind = Object.keys(spec ?? {}).find((k) => k in KINDS);
  if (!kind) fail(`${file}: start the block with one of ${Object.keys(KINDS).join(", ")}`);
  const body = KINDS[kind](spec[kind]);
  const caption = spec.caption ? `<figcaption>${md(spec.caption)}</figcaption>` : "";
  const label = esc(spec.alt ?? spec.caption ?? `${kind} diagram`);
  return `<figure class="forge-diagram not-content" role="figure" aria-label="${label}">${body}${caption}</figure>`;
}

/** Sätteri (Astro's Markdown processor) plugin. */
export const diagrams = {
  name: "forge-diagrams",
  code(node) {
    if (node.lang !== "diagram") return undefined;
    return { type: "html", value: render(node.value, "docs") };
  },
};
