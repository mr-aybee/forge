// The service's Health Checks tab: about defining checks. A one-line
// status strip on top (full status is on Overview), then the checks as a
// lifecycle flow, Startup → Readiness → Liveness, each stage saying what it
// does and what a failure causes. Selecting a check opens its editor under
// the flow; YAML mode edits the same policy. Drafts are validated on the
// server as you type (debounced), reviewed as a diff and saved with
// optimistic concurrency.
import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { AlertTriangle, ArrowRight, BellPlus, ChevronRight, CircleDashed, Clock, Code2, Copy, Equal, FileInput, HeartPulse, Hourglass, Info, ListChecks, Loader2, type LucideIcon, Pencil, Plus, Power, Route, Save, ShieldCheck, TerminalSquare, Trash2, X } from "lucide-react";
import { Fragment, Suspense, lazy, useEffect, useMemo, useState } from "react";

import { ApiError } from "../../api/client";
import { useServiceHealth, useSaveHealth, useServers, useValidateHealth } from "../../api/hooks";
import type { Service, HealthFieldError, HealthProbeType, HealthPurpose } from "../../api/types";
import { errorText, useToast } from "../../app/toast";
import { Badge, Button, Empty, ErrorNote, Modal, Mono, type RowAction, RowMenu, Skeleton, Switch, UnsavedBar, ago, time } from "../ui";
import { AddAlertButton } from "../alerts/AddAlert";
import { RuleDialog } from "../alerts/RuleDialog";
import { CheckCard, type ErrLookup } from "./CheckCard";
import { CheckResultPill, HealthSummary, HealthWarnings } from "./InstanceHealth";
import { type Draft, type DraftCheck, MAX_CHECKS, PRESETS, PURPOSES, diff, legacyCheck, newKey, parseYaml, presetCheck, runsCode, toDraft, toPolicy, toYaml, uniqueName } from "./model";

const YamlEditor = lazy(() => import("../stacks/parts").then((m) => ({ default: m.YamlEditor })));

/** The template "Alert when this fails" starts from, per purpose (D-118). */
const ALERT_TEMPLATE: Record<HealthPurpose, string> = {
  startup: "service-startup-exhausted",
  readiness: "service-readiness-failing",
  liveness: "service-liveness-failing",
};

interface PurposeMeta {
  title: string;
  tone: "accent" | "ok" | "warn";
  /** Tinted square behind the stage icon. */
  soft: string;
  icon: LucideIcon;
  short: string;
  /** What the stage does, in plain words. */
  what: string;
  failIcon: LucideIcon;
  failTone: string;
  /** What a failure does; `n` is the failure threshold when all checks agree. */
  fail: (n?: number) => string;
  none: (defaults: boolean) => string;
}

const after = (n?: number) => (n ? `after ${n} failed attempt${n === 1 ? "" : "s"}` : "after its failure threshold");

// Effects follow docs/concepts/health-checks.md: Forge reports, it never restarts on its own.
const PURPOSE: Record<HealthPurpose, PurposeMeta> = {
  startup: {
    title: "Startup",
    tone: "accent",
    soft: "bg-accent-soft text-accent",
    icon: Hourglass,
    short: "Has the instance finished starting?",
    what: "Runs first, until its first success. Readiness and liveness wait for it.",
    failIcon: Clock,
    failTone: "text-bad",
    fail: (n) => `${after(n)} it's exhausted: the node reports Unhealthy until the instance restarts.`,
    none: () => "No startup checks: readiness and liveness start right away.",
  },
  readiness: {
    title: "Readiness",
    tone: "ok",
    soft: "bg-ok-soft text-ok",
    icon: Route,
    short: "Can the instance take traffic?",
    what: "Pass: eligible for traffic. Runs for as long as the instance runs.",
    failIcon: Route,
    failTone: "text-warn",
    fail: (n) => `${after(n)} reported not ready (Unhealthy if required, else Degraded). Not restarted.`,
    none: () => "No readiness checks: the instance counts as ready once it is live.",
  },
  liveness: {
    title: "Liveness",
    tone: "warn",
    soft: "bg-warn-soft text-warn",
    icon: HeartPulse,
    short: "Is the instance working rather than stuck?",
    what: "Keep it cheap and local, so a broken dependency doesn't look like a dead instance.",
    failIcon: AlertTriangle,
    failTone: "text-bad",
    fail: (n) => `${after(n)} reported Unhealthy. Forge doesn't restart it on its own.`,
    none: (d) => (d ? "No liveness checks." : "No liveness checks: Forge can't tell whether the instance is working."),
  },
};

/** "every 10 s · 3 failures" from the stage's enabled checks (and the built-in one: 10 s, 3). */
function cadence(list: DraftCheck[], withDefault: boolean): { text?: string; threshold?: number } {
  const on = list.filter((c) => c.enabled);
  const periods = on.map((c) => c.period_secs).concat(withDefault ? [10] : []).filter((n) => Number.isFinite(n) && n > 0);
  const limits = on.map((c) => c.failure_threshold).concat(withDefault ? [3] : []).filter((n) => Number.isFinite(n) && n > 0);
  if (periods.length === 0) return {};
  const span = (xs: number[], unit: string) => {
    const lo = Math.min(...xs);
    const hi = Math.max(...xs);
    return lo === hi ? `${lo}${unit}` : `${lo}–${hi}${unit}`;
  };
  const same = limits.length > 0 && limits.every((n) => n === limits[0]) ? limits[0] : undefined;
  return { text: `every ${span(periods, " s")}${limits.length ? ` · ${span(limits, "")} failure${Math.max(...limits) === 1 ? "" : "s"}` : ""}`, threshold: same };
}

/** The edge between two stages: an arrow (down when stacked), or a "parallel" mark. */
function Connector({ label, parallel }: { label: string; parallel?: boolean }) {
  const Icon = parallel ? Equal : ArrowRight;
  return (
    <li aria-hidden className="flex items-center justify-center gap-1.5 py-0.5 text-[10.5px] text-faint lg:flex-col lg:py-0">
      <Icon className={`size-3.5 ${parallel ? "" : "rotate-90 lg:rotate-0"}`} />
      <span className="lg:[writing-mode:vertical-rl]">{label}</span>
    </li>
  );
}

/** Errors the form can show next to an input; everything else goes to the top. */
const MAPPED =
  /^(startup|readiness|liveness)(\[\d+\](\.(name|required|period_secs|timeout_secs|initial_delay_secs|failure_threshold|success_threshold|probe\.(scheme|tls_verify|show_output|host|allow_remote_host|port|path|expected_statuses|server_name|argv|script|expected_exit_codes|headers(\[\d+\](\.(name|value|secret))?)?)))?)?$/;

const PRESET_ICON: Record<HealthProbeType, typeof Plus> = { http: HeartPulse, tcp: ListChecks, exec: TerminalSquare, shell: Code2 };
const TYPE_SHORT: Record<HealthProbeType, string> = { http: "HTTP", tcp: "TCP", exec: "Command", shell: "Shell" };

const menuContent = "animate-in z-50 min-w-[168px] rounded-lg border border-line bg-panel p-1 shadow-[var(--shadow-pop)]";
const menuItem = "flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-[13px] outline-none data-[highlighted]:bg-panel-2 data-[disabled]:cursor-not-allowed data-[disabled]:opacity-50";

/** "Add check": purpose, then probe type. `counts` disables a full purpose and explains why. */
function AddCheckMenu({ counts, disabled, title, onAdd }: { counts: Record<HealthPurpose, number>; disabled?: boolean; title?: string; onAdd: (p: HealthPurpose, t: HealthProbeType) => void }) {
  return (
    <DropdownMenu.Root modal={false}>
      <DropdownMenu.Trigger asChild disabled={disabled}>
        <Button variant="primary" className="h-7 px-2 text-xs" disabled={disabled} title={title}>
          <Plus className="size-3.5" /> Add check
        </Button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content align="end" sideOffset={4} className={menuContent}>
          {PURPOSES.map((p) => {
            const n = counts[p] ?? 0;
            const full = n >= MAX_CHECKS;
            return (
              <DropdownMenu.Sub key={p}>
                <DropdownMenu.SubTrigger disabled={full} className={`${menuItem} data-[state=open]:bg-panel-2`}>
                  <Badge tone={PURPOSE[p].tone}>{PURPOSE[p].title}</Badge>
                  <span className="min-w-0 flex-1 truncate text-[11px] text-muted">{PURPOSE[p].short}</span>
                  {(full || n >= MAX_CHECKS - 2) && (
                    <span className="font-mono text-[11px] text-faint tabular-nums" title={full ? `At most ${MAX_CHECKS} ${p} checks` : undefined}>
                      {n}/{MAX_CHECKS}
                    </span>
                  )}
                  <ChevronRight className="size-3.5 text-faint" />
                </DropdownMenu.SubTrigger>
                <DropdownMenu.Portal>
                  <DropdownMenu.SubContent sideOffset={6} className={menuContent}>
                    {PRESETS.map((pr) => {
                      const Icon = PRESET_ICON[pr.type];
                      return (
                        <DropdownMenu.Item key={pr.type} onSelect={() => onAdd(p, pr.type)} className={menuItem}>
                          <Icon className="size-3.5 text-muted" /> {pr.label}
                        </DropdownMenu.Item>
                      );
                    })}
                  </DropdownMenu.SubContent>
                </DropdownMenu.Portal>
              </DropdownMenu.Sub>
            );
          })}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

export function HealthTab({ app, onDirtyChange }: { app: Service; onDirtyChange?: (dirty: boolean) => void }) {
  const q = useServiceHealth(app.id, { live: true });
  const servers = useServers().data;
  const hostOf = (id: string) => (Array.isArray(servers) ? servers : []).find((n) => n.id === id)?.hostname ?? id.slice(0, 8);
  const save = useSaveHealth();
  const toast = useToast();
  const [base, setBase] = useState<{ json: string; updated_at: number }>();
  const [draft, setDraft] = useState<Draft>();
  const [mode, setMode] = useState<"form" | "yaml">("form");
  const [yamlText, setYamlText] = useState("");
  const [yamlKey, setYamlKey] = useState(0);
  const [yamlErrors, setYamlErrors] = useState<string[]>([]);
  const [review, setReview] = useState(false);
  const [saveError, setSaveError] = useState<unknown>();
  /** Keys of the checks whose editor is open. */
  const [editing, setEditing] = useState<Set<string>>(() => new Set());
  /** The check an "Alert when this fails" dialog is open for. */
  const [alertFor, setAlertFor] = useState<{ purpose: HealthPurpose; name: string } | null>(null);

  const policy = useMemo(() => (draft ? toPolicy(draft) : undefined), [draft]);
  const json = policy ? JSON.stringify(policy) : "";
  const dirty = !!base && !!draft && json !== base.json;

  // Load the stored policy; follow server changes only while there is no draft.
  useEffect(() => {
    const d = q.data;
    if (!d) return;
    if (!base || (!dirty && d.updated_at !== base.updated_at)) {
      const next = toDraft(d.policy);
      setDraft(next);
      setBase({ json: JSON.stringify(toPolicy(next)), updated_at: d.updated_at });
      if (mode === "yaml") {
        setYamlText(toYaml(next));
        setYamlKey((k) => k + 1);
        setYamlErrors([]);
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [q.data]);

  useEffect(() => onDirtyChange?.(dirty || yamlErrors.length > 0), [dirty, yamlErrors.length, onDirtyChange]);

  const v = useValidateHealth(app.id, dirty ? policy : undefined);
  const live = dirty ? v.data : undefined;
  const errors: HealthFieldError[] = live?.errors ?? [];
  const summaries = live?.summaries ?? (dirty ? [] : (q.data?.summaries ?? []));
  const invalid = yamlErrors.length > 0 || (dirty && v.current && live?.valid === false);
  const checking = dirty && v.pending;

  // Open the editor of every check that has an error, so nothing is hidden.
  useEffect(() => {
    if (!draft || errors.length === 0) return;
    const keys: string[] = [];
    for (const p of PURPOSES) draft[p].forEach((c, i) => errors.some((e) => e.field === `${p}[${i}]` || e.field.startsWith(`${p}[${i}].`)) && keys.push(c.key));
    if (keys.some((k) => !editing.has(k))) setEditing((s) => new Set([...s, ...keys]));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [errors]);

  if (q.isLoading || (!draft && !q.error)) return <Skeleton rows={6} />;
  if (q.error || !draft || !policy) {
    const e = q.error;
    const missing = e instanceof ApiError && e.status === 404;
    const denied = e instanceof ApiError && e.status === 403;
    return (
      <div className="rounded-lg border border-line">
        <Empty icon={<HeartPulse className="size-5" />} title={missing ? "Health checks aren't available" : denied ? "You can't view health checks" : "Couldn't load health checks"}>
          {missing ? "This server doesn't support health checks yet. Update Forge to configure them." : denied ? "Viewing them needs the health.view permission on this service." : <ErrorNote error={e} />}
        </Empty>
      </div>
    );
  }

  // ---------------------------------------------------------------- edits

  const container = app.runtime === "container";
  const readOnly = !!q.data && !q.data.can_configure;
  const readOnlyWhy = readOnly ? "You can view these checks but not change them (needs health.configure)" : undefined;
  const total = PURPOSES.reduce((n, k) => n + draft[k].length, 0);
  const counts = { startup: draft.startup.length, readiness: draft.readiness.length, liveness: draft.liveness.length };
  const update = (purpose: HealthPurpose, fn: (list: DraftCheck[]) => DraftCheck[]) => setDraft((d) => (d ? { ...d, [purpose]: fn(d[purpose]) } : d));
  const toggleEdit = (key: string, on?: boolean) =>
    setEditing((s) => {
      const n = new Set(s);
      if (on ?? !n.has(key)) n.add(key);
      else n.delete(key);
      return n;
    });
  const add = (purpose: HealthPurpose, t: HealthProbeType) => {
    if (draft[purpose].length >= MAX_CHECKS) return;
    const c = presetCheck(draft[purpose], t);
    update(purpose, (l) => (l.length >= MAX_CHECKS ? l : [...l, c]));
    toggleEdit(c.key, true);
  };
  const legacy = q.data?.legacy_healthcheck ?? null;
  const legacyImported = !!legacy && draft.liveness.some((c) => c.probe.type === "shell" && c.probe.script === legacy.command);

  const errAt = (purpose: HealthPurpose, i: number): ErrLookup => {
    const at = `${purpose}[${i}]`;
    return (suffix) => errors.find((e) => e.field === `${at}.${suffix}`)?.message;
  };
  const hasErrAt = (purpose: HealthPurpose, i: number) => (suffixes: string[]) => {
    const at = `${purpose}[${i}]`;
    return errors.some((e) => suffixes.some((s) => (s === "" ? e.field === at || e.field.startsWith(`${at}.`) : e.field === `${at}.${s}` || e.field.startsWith(`${at}.${s}[`) || e.field.startsWith(`${at}.${s}.`))));
  };
  // Errors on a whole header row (`…headers[1]`) and anything unknown go to the top.
  const unmapped = mode === "yaml" ? errors : errors.filter((e) => !MAPPED.test(e.field) || PURPOSES.includes(e.field as HealthPurpose));
  // The server lists configured checks per purpose in order; its default check is tagged.
  const summaryFor = (purpose: HealthPurpose, i: number) => summaries.filter((s) => s.purpose === purpose && !s.default)[i]?.summary;
  const defaultSummary = (purpose: HealthPurpose) => summaries.find((s) => s.default && s.purpose === purpose)?.summary;

  const actionsFor = (purpose: HealthPurpose, i: number): RowAction[] => {
    const list = draft[purpose];
    const c = list[i];
    if (!c) return [];
    const full = list.length >= MAX_CHECKS;
    const open = editing.has(c.key);
    return [
      { label: open ? "Close editor" : "Edit", icon: Pencil, onSelect: () => toggleEdit(c.key) },
      { label: "Duplicate", icon: Copy, disabled: full, hint: full ? `${PURPOSE[purpose].title} is full (${MAX_CHECKS} checks)` : undefined, onSelect: () => update(purpose, (l) => (l.length >= MAX_CHECKS ? l : [...l.slice(0, i + 1), { ...structuredClone(c), key: newKey(), name: uniqueName(l, `${c.name || "check"}-copy`.slice(0, 40)) }, ...l.slice(i + 1)])) },
      { label: c.enabled ? "Disable" : "Enable", icon: Power, onSelect: () => update(purpose, (l) => l.map((x, j) => (j === i ? { ...x, enabled: !x.enabled } : x))) },
      { label: "Delete", icon: Trash2, danger: true, onSelect: () => update(purpose, (l) => l.filter((_, j) => j !== i)) },
      // Last, so the indexes above stay as the editor header uses them.
      { label: "Alert when this fails", icon: BellPlus, disabled: !c.name, hint: c.name ? undefined : "Name the check first", onSelect: () => setAlertFor({ purpose, name: c.name }) },
    ];
  };
  /** Open editors, in lifecycle order, shown full width under the flow. */
  const openChecks = PURPOSES.flatMap((purpose) => draft[purpose].map((c, i) => ({ purpose, c, i })).filter((x) => editing.has(x.c.key)));

  // ---------------------------------------------------------------- modes

  const toYamlMode = () => {
    setYamlText(toYaml(draft));
    setYamlKey((k) => k + 1);
    setYamlErrors([]);
    setMode("yaml");
  };
  const onYaml = (text: string) => {
    setYamlText(text);
    const r = parseYaml(text);
    setYamlErrors(r.errors);
    if (r.draft) {
      // Keep the UI keys of checks that stayed in place, so rows don't remount.
      const next = r.draft;
      for (const k of PURPOSES) next[k] = next[k].map((c, i) => ({ ...c, key: draft[k][i]?.key ?? c.key }));
      setDraft(next);
    }
  };
  const toFormMode = () => {
    if (yamlErrors.length) return;
    setMode("form");
  };

  const discard = () => {
    const next = toDraft(q.data?.policy);
    setDraft(next);
    setYamlErrors([]);
    setYamlText(toYaml(next));
    setYamlKey((k) => k + 1);
    setSaveError(undefined);
    setEditing(new Set());
  };

  const importLegacy = () => {
    if (!legacy) return;
    const c = legacyCheck(draft.liveness, legacy);
    const next = { ...draft, liveness: [...draft.liveness, c] };
    setDraft(next);
    toggleEdit(c.key, true);
    if (mode === "yaml") {
      setYamlText(toYaml(next));
      setYamlKey((k) => k + 1);
    }
    toast("Added a liveness check to the draft. Review and save to apply it.");
  };

  const reload = async () => {
    setReview(false);
    setSaveError(undefined);
    const r = await q.refetch();
    if (r.data) {
      const next = toDraft(r.data.policy);
      setDraft(next);
      setBase({ json: JSON.stringify(toPolicy(next)), updated_at: r.data.updated_at });
      setYamlText(toYaml(next));
      setYamlKey((k) => k + 1);
      setYamlErrors([]);
      setEditing(new Set());
    }
  };

  const doSave = () => {
    setSaveError(undefined);
    save.mutate(
      { id: app.id, policy, expected_updated_at: base?.updated_at ?? 0 },
      {
        onSuccess: (view) => {
          const next = toDraft(view.policy);
          setDraft(next);
          setBase({ json: JSON.stringify(toPolicy(next)), updated_at: view.updated_at });
          setReview(false);
          setEditing(new Set());
          toast("Health checks saved");
        },
        onError: (e) => setSaveError(e),
      },
    );
  };

  const changes = base ? diff(JSON.parse(base.json), policy) : [];
  const saveBlocked = readOnly
    ? "You can view these checks but not change them"
    : policy && runsCode(policy) && q.data && !q.data.can_command
      ? "Command and shell checks need the health.command permission"
      : !dirty ? "No unsaved changes" : yamlErrors.length ? "Fix the YAML first" : invalid ? "Fix the problems first" : checking ? "Checking the draft…" : undefined;
  const unsaved = dirty || yamlErrors.length > 0;

  // What the default checks do, for the header tooltip and the default rows on older servers.
  const defaultWhat = container
    ? "Container running, plus its image HEALTHCHECK if it has one, else TCP on the published port"
    : app.spec.listen_port ? `systemd unit active, and TCP connect to port ${app.spec.listen_port}` : "systemd unit active (set a listening port on the service to add a TCP check)";
  const defaultsTip = `Forge's built-in checks, run by every node without any setup: ${defaultWhat}; every 10 s; unhealthy after 3 failures. Your readiness checks replace the default readiness check.`;

  // ---------------------------------------------------------------- render

  return (
    <div className="space-y-3 pb-2">
      <HealthSummary instances={q.data?.instances} hostOf={hostOf} />
      <HealthWarnings warnings={q.data?.warnings} hostOf={hostOf} />

      {legacy && (
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5 rounded-md border border-line px-3 py-1.5 text-xs">
          <FileInput className="size-3.5 shrink-0 text-muted" />
          <span className="min-w-0 flex-1 truncate" title={legacy.command || undefined}>
            Docker also runs this container's HEALTHCHECK: <Mono>{legacy.command || "—"}</Mono>
            <span className="text-faint">
              {" "}
              · every {legacy.interval_secs || "—"} s, timeout {legacy.timeout_secs || "—"} s, {legacy.retries || "—"} retries
            </span>
          </span>
          <Button
            className="h-7 px-2 text-xs"
            disabled={readOnly || legacyImported || draft.liveness.length >= MAX_CHECKS}
            title={readOnlyWhy ?? (legacyImported ? "Already in the draft" : draft.liveness.length >= MAX_CHECKS ? `Liveness is full (${MAX_CHECKS} checks)` : "Adds a shell liveness check to the draft; Docker keeps running its own")}
            onClick={importLegacy}
          >
            Import as liveness check
          </Button>
        </div>
      )}

      <section aria-labelledby="hs-checks" className="overflow-hidden rounded-lg border border-line bg-panel">
        <header className="flex flex-wrap items-center gap-x-3 gap-y-1.5 border-b border-line bg-panel-2 px-3 py-1.5">
          <h3 id="hs-checks" className="text-[13px] font-semibold">
            Checks
          </h3>
          {readOnly && <Badge tone="muted">View only</Badge>}
          <div className="ml-auto flex flex-wrap items-center gap-2">
            <div role="radiogroup" aria-label="Editor mode" className="inline-flex rounded-md border border-line bg-panel p-0.5 text-xs">
              {(["form", "yaml"] as const).map((m) => (
                <button
                  key={m}
                  type="button"
                  role="radio"
                  aria-checked={mode === m}
                  disabled={m === "form" && mode === "yaml" && yamlErrors.length > 0}
                  title={m === "form" && yamlErrors.length > 0 ? "Fix the YAML first" : undefined}
                  onClick={() => (m === "yaml" ? mode !== "yaml" && toYamlMode() : toFormMode())}
                  className={`rounded px-2 py-0.5 disabled:cursor-not-allowed disabled:opacity-50 ${mode === m ? "bg-accent-soft text-accent" : "text-muted hover:text-fg"}`}
                >
                  {m === "form" ? "Form" : "YAML"}
                </button>
              ))}
            </div>
            <AddAlertButton prefill={{ place: "service", targets: { services: [app.id] } }} title={`Create an alert rule for ${app.name}`} />
            {alertFor && (
              <RuleDialog
                rule={null}
                prefill={{ place: "service", templateId: ALERT_TEMPLATE[alertFor.purpose], check: alertFor.name, targets: { services: [app.id] } }}
                onClose={() => setAlertFor(null)}
              />
            )}
            <AddCheckMenu counts={counts} disabled={readOnly || mode === "yaml"} title={readOnlyWhy ?? (mode === "yaml" ? "Add checks in the YAML, or switch to Form" : undefined)} onAdd={add} />
          </div>
        </header>
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1.5 border-b border-line px-3 py-2 text-xs">
          <p className="min-w-0 flex-1 basis-[16rem] text-muted">
            Checks run on every node of {app.name}, against {container ? "the container" : "the app on 127.0.0.1"} unless a check sets a host.
          </p>
          <span className="inline-flex shrink-0 items-center gap-1">
            <Switch
              checked={draft.defaultChecks}
              disabled={readOnly || mode === "yaml"}
              title={readOnlyWhy ?? (mode === "yaml" ? "Set default_checks in the YAML" : undefined)}
              onChange={(on) => setDraft((d) => (d ? { ...d, defaultChecks: on } : d))}
              label={
                <span className="text-xs">
                  Default checks: <span className={`font-medium ${draft.defaultChecks ? "text-fg" : "text-muted"}`}>{draft.defaultChecks ? "on" : "off"}</span>
                </span>
              }
            />
            <span className="inline-grid size-5 place-items-center text-faint" title={defaultsTip} aria-label={defaultsTip} role="img">
              <Info className="size-3.5" />
            </span>
          </span>
        </div>

        {unmapped.length > 0 && (
          <div role="alert" className="border-b border-line bg-bad-soft px-3 py-2 text-xs text-bad">
            <div className="mb-1 flex items-center gap-1.5 font-medium">
              <AlertTriangle className="size-3.5" /> {unmapped.length === 1 ? "1 problem" : `${unmapped.length} problems`}. The saved checks stay active until you fix and save.
            </div>
            <ul className="list-disc space-y-0.5 pl-5">
              {unmapped.map((e, i) => (
                <li key={i} className="break-words">
                  {e.field && <Mono>{e.field}</Mono>} {e.message}
                </li>
              ))}
            </ul>
          </div>
        )}
        {dirty && v.error && <p className="border-b border-line px-3 py-1.5 text-[11px] text-warn">Couldn't check the draft: {errorText(v.error)}. The server still checks it when you save.</p>}
        {!draft.defaultChecks && total === 0 && (
          <p role="status" className="flex items-center gap-1.5 border-b border-line bg-warn-soft px-3 py-1.5 text-[11px] text-warn">
            <AlertTriangle className="size-3.5 shrink-0" /> No checks at all: Forge won't know whether this service is healthy.
          </p>
        )}

        {mode === "yaml" ? (
          <div className="space-y-1.5 p-2">
            {yamlErrors.length > 0 ? (
              <div role="alert" className="rounded-md border border-bad/40 bg-bad-soft px-3 py-2 text-xs text-bad">
                <div className="mb-0.5 font-medium">The YAML can't be used yet. You stay in YAML until it's fixed.</div>
                <ul className="list-disc space-y-0.5 pl-5">
                  {yamlErrors.slice(0, 12).map((e, i) => (
                    <li key={i} className="break-words font-mono text-[11px]">
                      {e}
                    </li>
                  ))}
                  {yamlErrors.length > 12 && <li>…and {yamlErrors.length - 12} more</li>}
                </ul>
              </div>
            ) : (
              <p className="px-1 text-[11px] text-faint">Same policy as the form. Unknown fields are refused. Switching back to the form normalizes formatting and drops comments.</p>
            )}
            <Suspense fallback={<Skeleton rows={8} />}>
              <YamlEditor value={yamlText} docKey={`health-${yamlKey}`} onChange={onYaml} height="h-[min(28rem,55vh)]" />
            </Suspense>
          </div>
        ) : (
          <div className="p-3">
            <ol aria-label="Check lifecycle" className="grid gap-1.5 lg:grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)_auto_minmax(0,1fr)] lg:gap-2">
              {PURPOSES.map((purpose, pi) => {
                const list = draft[purpose];
                const sectionErr = errors.find((e) => e.field === purpose)?.message;
                const hasDefault = draft.defaultChecks && purpose !== "startup";
                const replaced = purpose === "readiness" && list.length > 0;
                const addDisabled = readOnly ? readOnlyWhy : list.length >= MAX_CHECKS ? `At most ${MAX_CHECKS} ${purpose} checks` : undefined;
                const meta = PURPOSE[purpose];
                const sched = cadence(list, hasDefault && !replaced);
                const StageIcon = meta.icon;
                const FailIcon = meta.failIcon;
                return (
                  <Fragment key={purpose}>
                    {pi > 0 && <Connector label={pi === 1 ? "once started" : "alongside"} parallel={pi === 2} />}
                    <li className={`flex min-w-0 flex-col rounded-lg border bg-panel ${sectionErr ? "border-bad/50" : "border-line"}`} aria-labelledby={`hs-stage-${purpose}`}>
                      <div className="flex min-w-0 items-center gap-2 px-3 pt-2.5">
                        <span aria-hidden className={`inline-grid size-6 shrink-0 place-items-center rounded-md ${meta.soft}`}>
                          <StageIcon className="size-3.5" />
                        </span>
                        <div className="min-w-0 flex-1">
                          <h4 id={`hs-stage-${purpose}`} className="flex items-baseline gap-1.5 text-[13px] leading-tight font-semibold">
                            <span className="font-mono text-[10.5px] font-normal text-faint tabular-nums">{pi + 1}</span>
                            {meta.title}
                          </h4>
                          <p className="truncate text-[11px] text-muted" title={meta.short}>
                            {meta.short}
                          </p>
                        </div>
                        {list.length >= MAX_CHECKS - 2 && (
                          <span className="font-mono text-[11px] text-faint tabular-nums" title={list.length >= MAX_CHECKS ? `At most ${MAX_CHECKS} ${purpose} checks` : `${MAX_CHECKS - list.length} more allowed`}>
                            {list.length}/{MAX_CHECKS}
                          </span>
                        )}
                        <AddLink purpose={purpose} disabled={addDisabled} onAdd={(t) => add(purpose, t)} />
                      </div>
                      <p className="px-3 pt-1.5 text-xs">{meta.what}</p>
                      {sectionErr && <p className="px-3 pt-1 text-[11px] text-bad">{sectionErr}</p>}

                      <ul className="flex-1 space-y-1 px-2 pt-2 pb-2" aria-label={`${meta.title} checks`}>
                        {hasDefault && (
                          <DefaultRow purpose={purpose} replaced={replaced} text={defaultSummary(purpose) ?? (replaced ? "Replaced by your readiness checks." : `${defaultWhat}; every 10 s; unhealthy after 3 failures.`)} />
                        )}
                        {list.map((c, i) => {
                          const open = editing.has(c.key);
                          const bad = hasErrAt(purpose, i)([""]);
                          const actions = actionsFor(purpose, i);
                          const TypeIcon = PRESET_ICON[c.probe.type];
                          return (
                            <li key={c.key} className={`flex min-w-0 items-center gap-1 rounded-md border pr-0.5 ${open ? "border-accent/50 bg-accent-soft/40" : bad ? "border-bad/50 bg-bad-soft/40" : "border-line hover:bg-panel-2/60"} ${c.enabled ? "" : "opacity-60"}`}>
                              <button
                                type="button"
                                aria-expanded={open}
                                aria-controls={open ? `hs-edit-${c.key}` : undefined}
                                aria-label={`${open ? "Close" : "Edit"} ${purpose} check ${c.name || i + 1}`}
                                onClick={() => toggleEdit(c.key)}
                                className="flex min-w-0 flex-1 items-center gap-1.5 rounded-md py-1 pl-2 text-left text-xs focus-visible:outline-2 focus-visible:outline-accent"
                              >
                                <TypeIcon className="size-3.5 shrink-0 text-muted" aria-hidden />
                                <span className="min-w-0 flex-1 truncate font-medium" title={`${c.name || "unnamed"} · ${TYPE_SHORT[c.probe.type]}`}>
                                  {c.name || <span className="text-faint">unnamed</span>}
                                </span>
                                {bad && <AlertTriangle className="size-3.5 shrink-0 text-bad" aria-label="has problems" />}
                                {!c.enabled && <span className="text-[11px] text-faint">off</span>}
                                <span className="shrink-0 font-mono text-[10.5px] text-faint tabular-nums" title={`Every ${c.period_secs} s; fails after ${c.failure_threshold} failed attempt${c.failure_threshold === 1 ? "" : "s"}`}>
                                  {c.period_secs}s·{c.failure_threshold}×
                                </span>
                              </button>
                              <CheckResultPill dot instances={q.data?.instances} purpose={purpose} name={c.name} hostOf={hostOf} />
                              {!readOnly && <RowMenu actions={actions} label={`Actions for ${c.name || "check"}`} />}
                            </li>
                          );
                        })}
                        {list.length === 0 && !hasDefault && (
                          <li className="flex min-w-0 items-start gap-2 rounded-md border border-dashed border-line px-2.5 py-2 text-[11px] text-muted">
                            <CircleDashed className="mt-px size-3.5 shrink-0 text-faint" aria-hidden />
                            <span className="min-w-0">{meta.none(draft.defaultChecks)}</span>
                          </li>
                        )}
                      </ul>

                      <div className={`mt-auto flex min-w-0 items-start gap-1.5 border-t border-dashed border-line px-3 py-1.5 text-[11px] ${meta.failTone}`}>
                        <FailIcon className="mt-px size-3.5 shrink-0" aria-hidden />
                        <span className="min-w-0">
                          <span className="font-medium">On failure:</span> <span className="text-muted">{meta.fail(sched.threshold)}</span>
                          {sched.text && <span className="block font-mono text-[10.5px] text-faint tabular-nums">{sched.text}</span>}
                        </span>
                      </div>
                    </li>
                  </Fragment>
                );
              })}
            </ol>

            {openChecks.length > 0 && (
              <div className="mt-3 space-y-2">
                {openChecks.map(({ purpose, c, i }) => {
                  const full = draft[purpose].length >= MAX_CHECKS;
                  const actions = actionsFor(purpose, i);
                  return (
                    <section key={c.key} id={`hs-edit-${c.key}`} aria-label={`Edit ${purpose} check ${c.name || i + 1}`} className="min-w-0 rounded-lg border border-line">
                      <header className="flex min-w-0 flex-wrap items-center gap-2 border-b border-line bg-panel-2 px-3 py-1.5 text-xs">
                        <Badge tone={PURPOSE[purpose].tone}>{PURPOSE[purpose].title}</Badge>
                        <span className="min-w-0 flex-1 truncate text-[13px] font-medium" title={c.name}>
                          {c.name || <span className="text-faint">unnamed</span>}
                        </span>
                        <CheckResultPill instances={q.data?.instances} purpose={purpose} name={c.name} hostOf={hostOf} />
                        <Switch
                          checked={c.enabled}
                          disabled={readOnly}
                          title={readOnlyWhy ?? (c.enabled ? "Enabled: turn off to keep it without running it" : "Disabled: kept but not run")}
                          onChange={(enabled) => update(purpose, (l) => l.map((x, j) => (j === i ? { ...x, enabled } : x)))}
                          label={<span className="text-xs text-muted">Enabled</span>}
                        />
                        {!readOnly && <RowMenu actions={actions.slice(1)} label={`Actions for ${c.name || "check"}`} />}
                        <Button variant="ghost" className="h-7 px-1.5 text-xs" onClick={() => toggleEdit(c.key, false)} aria-label={`Close editor of ${c.name || "check"}`}>
                          <X className="size-3.5" /> Close
                        </Button>
                      </header>
                      {(summaryFor(purpose, i) || checking) && (
                        <p className="truncate border-b border-line px-3 py-1 text-[11px] text-muted" title={summaryFor(purpose, i)}>
                          {summaryFor(purpose, i) ?? "Checking…"}
                        </p>
                      )}
                      <div className="p-2">
                        <CheckCard
                          embedded
                          purpose={purpose}
                          index={i}
                          check={c}
                          container={container}
                          summary={summaryFor(purpose, i)}
                          err={errAt(purpose, i)}
                          hasErr={hasErrAt(purpose, i)}
                          canDuplicate={!full}
                          onChange={(n) => update(purpose, (l) => l.map((x, j) => (j === i ? n : x)))}
                          onDuplicate={() => actions[1]?.onSelect()}
                          onRemove={() => actions[3]?.onSelect()}
                        />
                      </div>
                    </section>
                  );
                })}
              </div>
            )}
          </div>
        )}
        {!unsaved && (
          <footer className="flex flex-wrap items-center gap-x-1.5 gap-y-0.5 border-t border-line bg-panel-2 px-3 py-1.5 text-[11px] text-muted">
            <span className="inline-flex items-center gap-1 font-medium text-ok">
              <Save className="size-3" /> Saved
            </span>
            {q.data?.updated_at ? (
              <span>
                · updated <span title={time(q.data.updated_at)}>{ago(q.data.updated_at)}</span>
              </span>
            ) : null}
            <span>· {total ? `${total} custom check${total === 1 ? "" : "s"}` : "no custom checks"}</span>
            <span>· default checks {draft.defaultChecks ? "on" : "off"}</span>
          </footer>
        )}
      </section>

      <UnsavedBar
        dirty={unsaved}
        summary={
          <span className="inline-flex min-w-0 items-center gap-1.5">
            {checking && <Loader2 className="size-3.5 animate-spin text-muted" />}
            {invalid ? "Has problems; the saved checks stay active." : changes.length > 0 ? <span className="font-mono text-[11px] text-faint tabular-nums">{changes.length} change{changes.length === 1 ? "" : "s"}</span> : null}
          </span>
        }
        onDiscard={discard}
        saveLabel="Review and save"
        saveDisabled={!!saveBlocked}
        onSave={() => (setSaveError(undefined), setReview(true))}
      />

      <Modal open={review} onOpenChange={(o) => !save.isPending && setReview(o)} title={`Save health checks of ${app.name}?`} description="Replaces the stored health policy of this service.">
        <ReviewBody changes={changes} runs={runsCode(policy)} error={saveError} />
        <div className="mt-5 flex flex-wrap justify-end gap-2">
          {saveError instanceof ApiError && saveError.status === 409 ? (
            <>
              <Button onClick={() => setReview(false)}>Keep my draft</Button>
              <Button variant="primary" onClick={() => void reload()}>
                Reload latest (drops the draft)
              </Button>
            </>
          ) : (
            <>
              <Button onClick={() => setReview(false)} disabled={save.isPending}>
                Cancel
              </Button>
              <Button variant="primary" onClick={doSave} disabled={save.isPending || changes.length === 0}>
                {save.isPending && <Loader2 className="size-4 animate-spin" />}
                Save
              </Button>
            </>
          )}
        </div>
      </Modal>
    </div>
  );
}

/** Per-phase "Add" button: picks the probe type. Disabled with the reason as tooltip. */
function AddLink({ purpose, disabled, onAdd }: { purpose: HealthPurpose; disabled?: string; onAdd: (t: HealthProbeType) => void }) {
  return (
    <DropdownMenu.Root modal={false}>
      <DropdownMenu.Trigger asChild disabled={!!disabled}>
        <Button variant="ghost" className="h-6 shrink-0 px-1.5 text-xs" disabled={!!disabled} title={disabled ?? `Add a ${purpose} check`} aria-label={`Add ${purpose} check`}>
          <Plus className="size-3.5" /> Add
        </Button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content align="end" sideOffset={4} className={menuContent}>
          {PRESETS.map((pr) => {
            const Icon = PRESET_ICON[pr.type];
            return (
              <DropdownMenu.Item key={pr.type} onSelect={() => onAdd(pr.type)} className={menuItem}>
                <Icon className="size-3.5 text-muted" /> {pr.label}
              </DropdownMenu.Item>
            );
          })}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

/** Forge's default check as a row tagged Built-in; greyed and tagged Replaced when readiness checks replace it. */
function DefaultRow({ purpose, replaced, text }: { purpose: HealthPurpose; replaced: boolean; text: string }) {
  return (
    <li
      className={`flex min-w-0 items-center gap-1.5 rounded-md border border-dashed border-line-strong px-2 py-1 text-xs ${replaced ? "opacity-60" : ""}`}
      aria-label={`Default ${purpose} check${replaced ? ", replaced" : ""}`}
      title={`${text} Turn default checks off above to drop it.`}
    >
      <ShieldCheck className="size-3.5 shrink-0 text-muted" aria-hidden />
      <span className={`min-w-0 flex-1 truncate ${replaced ? "line-through decoration-faint" : ""}`}>Default check</span>
      <Badge tone={replaced ? "muted" : "accent"}>{replaced ? "Replaced" : "Built-in"}</Badge>
      {!replaced && <span className="shrink-0 font-mono text-[10.5px] text-faint tabular-nums">10s·3×</span>}
    </li>
  );
}

function ReviewBody({ changes, runs, error }: { changes: ReturnType<typeof diff>; runs: boolean; error: unknown }) {
  const tone = { added: "ok", removed: "bad", changed: "warn" } as const;
  return (
    <div className="space-y-3 text-[13px]">
      {changes.some((c) => c.purpose === "defaults") && (
        <div className="flex items-center gap-2">
          <Badge tone="warn">changed</Badge>
          <span>Default checks: turned {changes.find((c) => c.purpose === "defaults")?.name}</span>
        </div>
      )}
      {changes.length === 0 ? (
        <p className="text-muted">No changes.</p>
      ) : (
        PURPOSES.filter((p) => changes.some((c) => c.purpose === p)).map((p) => (
          <div key={p}>
            <div className="mb-1 text-[11px] font-semibold tracking-wide text-faint uppercase">{PURPOSE[p].title}</div>
            <ul className="space-y-0.5">
              {changes
                .filter((c) => c.purpose === p)
                .map((c) => (
                  <li key={`${c.change}-${c.name}`} className="flex min-w-0 items-center gap-2">
                    <Badge tone={tone[c.change]}>{c.change}</Badge>
                    <span className="truncate font-mono text-xs" title={c.name}>
                      {c.name || "—"}
                    </span>
                  </li>
                ))}
            </ul>
          </div>
        ))
      )}
      {runs && (
        <p className="rounded border border-warn/40 bg-warn-soft px-2 py-1.5 text-xs text-warn">
          A check runs a command or script on the nodes. Saving needs an MFA-verified sign-in; you may be asked for a code.
        </p>
      )}
      {error !== undefined && <SaveError error={error} />}
    </div>
  );
}

function SaveError({ error }: { error: unknown }) {
  const e = error instanceof ApiError ? error : undefined;
  const text =
    e?.status === 409
      ? "Someone changed this service since you opened it. Reload to get the latest version, then make your change again."
      : e?.code === "step_up_required"
        ? `Not verified: ${e.message}. Saving a command or script needs an MFA-verified sign-in.`
        : e?.status === 422
          ? `The server refused the policy: ${e.message}`
          : e?.status === 403
            ? `Not allowed: ${e.message}`
            : errorText(error);
  return (
    <p role="alert" className="rounded-md bg-bad-soft px-3 py-2 text-xs break-words text-bad">
      {text}
    </p>
  );
}
