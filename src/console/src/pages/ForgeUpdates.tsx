import { Link } from "@tanstack/react-router";
import { ArrowRight, CheckCircle2, CircleArrowUp, ExternalLink, Loader2, RefreshCw, ShieldCheck, TriangleAlert } from "lucide-react";
import { useEffect, useMemo, useState, type ReactNode } from "react";

import { get } from "../api/client";
import {
  FORGE_BUSY_STATES,
  type ForgeRelease,
  type ForgeUpdateState,
  type ForgeUpdateStatus,
  useApplyForgeUpdate,
  useCheckForgeUpdate,
  useForgeReleases,
  useForgeUpdate,
  useMe,
} from "../api/hooks";
import { useStepUp } from "../app/stepUp";
import { errorText, useToast } from "../app/toast";
import { useAgentVersions } from "../api/agentUpdates";
import { AgentUpdates } from "../components/AgentUpdates";
import { Badge, Button, Field, Input, Modal, PageHeader, Panel, Select, Skeleton, Status, Switch, ago, time } from "../components/ui";

/** Release notes are Markdown; show them as plain text (no HTML from the feed). */
const plainNotes = (md: string) =>
  md
    .replace(/\r\n/g, "\n")
    .replace(/^#{1,6}\s*/gm, "")
    .replace(/\*\*(.+?)\*\*/g, "$1")
    .replace(/`([^`]+)`/g, "$1")
    .replace(/\[([^\]]+)\]\((?:[^)]+)\)/g, "$1")
    .trim();

const webUrl = (u: string) => (/^https?:\/\//i.test(u) ? u : null);
const published = (r: ForgeRelease) => {
  const t = r.published_at ? Date.parse(r.published_at) : NaN;
  return Number.isNaN(t) ? null : t;
};

interface Reason {
  tone: "warn" | "bad";
  text: ReactNode;
}

/** Plain reasons why this session can't update right now (empty: it can). */
function reasons(s: ForgeUpdateStatus | undefined, mfa: { verified: boolean; enrolled: boolean; bearer: boolean }, releasesError: string | null): Reason[] {
  if (!s) return [];
  const out: Reason[] = [];
  if (s.error_status === 403) out.push({ tone: "bad", text: "You don't have permission to update Forge (system.update). Ask a platform administrator." });
  else if (s.error_status === 404 || s.error_status === 405) out.push({ tone: "bad", text: "This server has no self-update API. Update it with the installer." });
  else if (s.error) out.push({ tone: "bad", text: `Update status is unavailable: ${s.error}` });
  if (s.error_status === null) {
    if (!s.enabled) out.push({ tone: "warn", text: "Self-update is turned off on this server, or no signed update feed is configured ([update] in forge.toml)." });
    else if (!s.can_update) out.push({ tone: "bad", text: "You don't have permission to update Forge (system.update), or this install can't replace its own binaries." });
    if (FORGE_BUSY_STATES.includes(s.state)) out.push({ tone: "warn", text: "Another update is running. Wait until it finishes." });
    if (s.state === "idle" && s.message) out.push({ tone: "warn", text: s.message });
  }
  if (releasesError) out.push({ tone: "warn", text: `The update feed is unreachable: ${releasesError}` });
  if (!mfa.verified && !mfa.bearer) {
    out.push({
      tone: "warn",
      text: mfa.enrolled ? (
        "Applying an update asks you to verify with MFA (authenticator code or passkey)."
      ) : (
        <>
          Updating needs MFA, and your account has none.{" "}
          <Link to="/profile/$section" params={{ section: "security" }} className="font-medium underline">
            Set up MFA in Password &amp; security
          </Link>
          .
        </>
      ),
    });
  }
  return out;
}

const STAGES: { state: ForgeUpdateState; label: string }[] = [
  { state: "downloading", label: "Download" },
  { state: "verifying", label: "Verify signature and checksums" },
  { state: "installing", label: "Back up and install" },
  { state: "restarting", label: "Restart" },
];

export function ForgeUpdates() {
  const q = useForgeUpdate();
  const s = q.data;
  const me = useMe().data;
  const allowedToTry = !!s && s.error_status === null;
  const rel = useForgeReleases(allowedToTry);
  const check = useCheckForgeUpdate();
  const toast = useToast();
  const agents = useAgentVersions();

  // Newer releases from the list; an older server without the list offers its "latest".
  const releases = useMemo<ForgeRelease[]>(() => {
    const list = rel.data?.releases ?? [];
    if (list.length) return list;
    if (rel.data && !rel.data.supported && s?.latest && s.available) return [s.latest];
    return [];
  }, [rel.data, s?.latest, s?.available]);
  const current = s?.current || rel.data?.current || "";
  const isInstalled = (r: ForgeRelease) => r.current === true || r.version === current;
  const newer = releases.filter((r) => !isInstalled(r));
  const [picked, setPicked] = useState("");
  const selected = newer.find((r) => r.version === picked) ?? newer[0];
  const [follow, setFollow] = useState(true);
  const [batchText, setBatchText] = useState("1");
  const batch = Math.min(100, Math.max(1, Math.round(Number(batchText)) || 1));
  const [confirming, setConfirming] = useState(false);

  const nodes = agents.data?.nodes ?? [];
  const linux = nodes.filter((n) => n.updatable);
  const linuxOnline = linux.filter((n) => n.status === "online");
  const windows = nodes.filter((n) => !n.updatable);

  const busy = !!s && FORGE_BUSY_STATES.includes(s.state);
  const why = reasons(s, { verified: !!me?.mfa, enrolled: !!me?.mfa_enrolled, bearer: me?.session_kind === "bearer" }, rel.data?.error ?? null);
  const blocking = !s || !s.can_update || s.error_status !== null || busy;

  const runCheck = () =>
    check.mutate(undefined, {
      onSuccess: (r) => toast(r.available && r.latest ? `Forge ${r.latest.version} is available` : "Forge is up to date"),
      onError: (e) => toast(errorText(e), "error"),
    });

  return (
    <>
      <PageHeader
        eyebrow="Administration"
        title="Forge updates"
        description="Upgrade the control plane to a newer signed release, then roll its agent out to your nodes. Older versions are never offered."
        actions={
          <Button onClick={runCheck} disabled={check.isPending || !allowedToTry} title={allowedToTry ? "Ask the update feed for new releases" : "Update status is unavailable"}>
            <RefreshCw className={`size-4 ${check.isPending ? "animate-spin" : ""}`} /> Check now
          </Button>
        }
      />

      <div className="mb-4 space-y-4">
        <Panel title="Status">
          {q.isLoading ? (
            <Skeleton rows={3} />
          ) : (
            <>
              <dl className="grid grid-cols-1 gap-x-6 px-4 py-2 text-[13px] sm:grid-cols-2">
                <Line k="Running">
                  <span className="font-mono text-[11px] tabular-nums">{current || "—"}</span>
                  {s?.target && <span className="ml-1.5 font-mono text-[11px] text-faint">{s.target}</span>}
                </Line>
                <Line k="Latest">
                  {s?.latest ? (
                    <span className="inline-flex items-center gap-1.5">
                      <span className="font-mono text-[11px] tabular-nums">{s.latest.version}</span>
                      {s.available ? <Badge tone="accent">newer</Badge> : <Badge tone="ok">installed</Badge>}
                    </span>
                  ) : (
                    "—"
                  )}
                </Line>
                <Line k="Feed">
                  {s?.feed_url ? (
                    <span className="block truncate font-mono text-[11px] select-all" title={s.feed_url}>
                      {s.feed_url}
                    </span>
                  ) : (
                    "—"
                  )}
                </Line>
                <Line k="Last checked">
                  <span title={s?.checked_at ? time(s.checked_at) : undefined}>{s?.checked_at ? ago(s.checked_at) : "—"}</span>
                </Line>
                <Line k="State">{s && s.error_status === null ? <Status value={s.state} /> : "—"}</Line>
              </dl>
              {s?.state === "failed" && (
                <Note tone="bad">
                  Last update failed{s.message ? `: ${s.message}` : ""}. The previous version kept running.
                </Note>
              )}
              {why.map((r, i) => (
                <Note key={i} tone={r.tone}>
                  {r.text}
                </Note>
              ))}
              {!why.length && s && !s.available && s.state !== "failed" && (
                <p className="flex items-center gap-1.5 border-t border-line px-4 py-2 text-xs text-ok">
                  <CheckCircle2 className="size-3.5" /> Forge is up to date.
                </p>
              )}
            </>
          )}
        </Panel>

        <Panel title="Upgrade" description="Pick a release. The newest is preselected; installed versions are shown for reference.">
          {rel.isLoading && allowedToTry ? (
            <Skeleton rows={3} />
          ) : !releases.length ? (
            <p className="px-4 py-3 text-[13px] text-muted">{allowedToTry ? "No newer release is available. Check now to ask the feed again." : "—"}</p>
          ) : (
            <div className="space-y-3 p-4">
              <div className="flex flex-wrap items-end gap-3">
                <div className="w-64 max-w-full">
                  <Field label="Version">
                    <Select value={selected?.version ?? ""} onChange={(e) => setPicked(e.target.value)} disabled={!newer.length}>
                      {releases.map((r, i) => (
                        <option key={r.version} value={r.version} disabled={isInstalled(r)}>
                          {r.version}
                          {isInstalled(r) ? " (installed)" : i === 0 ? " (latest)" : ""}
                        </option>
                      ))}
                    </Select>
                  </Field>
                </div>
                {selected && (
                  <div className="flex min-w-0 items-center gap-2 pb-1.5 text-xs text-muted">
                    <span className="font-mono text-[11px] tabular-nums">{current || "—"}</span>
                    <ArrowRight className="size-3.5 shrink-0" />
                    <span className="font-mono text-[11px] text-fg tabular-nums">{selected.version}</span>
                    {selected === newer[0] && <Badge tone="accent">latest</Badge>}
                    {published(selected) && <span title={time(published(selected))}>published {ago(published(selected))}</span>}
                    {webUrl(selected.url) && (
                      <a href={webUrl(selected.url)!} target="_blank" rel="noreferrer noopener" className="inline-flex items-center gap-1 text-accent hover:underline">
                        Release page <ExternalLink className="size-3" />
                      </a>
                    )}
                  </div>
                )}
              </div>

              {selected && (
                <>
                  {selected.notes ? (
                    <pre className="max-h-56 overflow-auto rounded-md border border-line bg-panel-2/40 p-3 font-sans text-[12.5px] whitespace-pre-wrap select-text">{plainNotes(selected.notes)}</pre>
                  ) : (
                    <p className="text-xs text-muted">No release notes.</p>
                  )}

                  <div className="rounded-md border border-line px-3 py-2 text-xs">
                    <div className="mb-1 font-medium text-fg">What will be updated</div>
                    <ol className="list-decimal space-y-0.5 pl-4 text-muted">
                      <li>
                        <span className="font-mono text-[11px]">forge-server</span> and <span className="font-mono text-[11px]">forge-plugin-runner</span> on this host
                        {s?.target ? ` (${s.target})` : ""}; a few seconds offline.
                      </li>
                      <li>Agent binaries served to nodes you add or update afterwards.</li>
                      {follow ? (
                        <li>
                          Then agents on <strong className="text-fg">{linuxOnline.length}</strong> online Linux node{linuxOnline.length === 1 ? "" : "s"}, in canary batches of {batch}
                          {linux.length > linuxOnline.length ? ` (${linux.length - linuxOnline.length} offline, skipped)` : ""}.
                        </li>
                      ) : (
                        <li>Agents stay on their version until you update them below.</li>
                      )}
                      {windows.length > 0 && (
                        <li className="text-warn">
                          {windows.length} Windows node{windows.length === 1 ? "" : "s"} need a reinstall of the agent; they don't self-update.
                        </li>
                      )}
                    </ol>
                  </div>

                  <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
                    <Switch checked={follow} onChange={setFollow} label="Update agents after the server" />
                    {follow && (
                      <label className="inline-flex items-center gap-2 text-xs text-muted">
                        Batch size
                        <Input type="number" min={1} max={100} value={batchText} onChange={(e) => setBatchText(e.target.value)} className="w-20" aria-label="Agents per batch" />
                      </label>
                    )}
                    <span className="ml-auto">
                      <Button
                        variant="primary"
                        disabled={blocking || !selected}
                        title={blocking ? (why[0] && typeof why[0].text === "string" ? why[0].text : "Updating is not possible right now") : undefined}
                        onClick={() => setConfirming(true)}
                      >
                        {busy ? <Loader2 className="size-4 animate-spin" /> : <CircleArrowUp className="size-4" />} Update to {selected.version}
                      </Button>
                    </span>
                  </div>
                </>
              )}
            </div>
          )}
        </Panel>
      </div>

      <AgentUpdates />

      {selected && (
        <ApplyDialog
          open={confirming}
          onOpenChange={setConfirming}
          status={s}
          release={selected}
          current={current}
          follow={follow}
          batch={batch}
          linuxCount={linuxOnline.length}
        />
      )}
    </>
  );
}

function Line({ k, children }: { k: string; children: ReactNode }) {
  return (
    <div className="flex min-w-0 items-center justify-between gap-4 border-b border-line py-1.5 last:border-b-0 sm:[&:nth-last-child(2)]:border-b-0">
      <dt className="shrink-0 text-muted">{k}</dt>
      <dd className="min-w-0 truncate text-right">{children}</dd>
    </div>
  );
}

function Note({ tone, children }: { tone: "warn" | "bad"; children: ReactNode }) {
  return (
    <p role={tone === "bad" ? "alert" : undefined} className={`flex items-start gap-1.5 border-t border-line px-4 py-2 text-xs ${tone === "bad" ? "text-bad" : "text-warn"}`}>
      <TriangleAlert className="mt-px size-3.5 shrink-0" />
      <span className="min-w-0 break-words">{children}</span>
    </p>
  );
}

function ApplyDialog({
  open,
  onOpenChange,
  status,
  release,
  current,
  follow,
  batch,
  linuxCount,
}: {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  status: ForgeUpdateStatus | undefined;
  release: ForgeRelease;
  current: string;
  follow: boolean;
  batch: number;
  linuxCount: number;
}) {
  const toast = useToast();
  const { ensureMfa } = useStepUp();
  const apply = useApplyForgeUpdate();
  const [typed, setTyped] = useState("");
  const [waiting, setWaiting] = useState<string>();
  const [reconnected, setReconnected] = useState(false);
  const [preparing, setPreparing] = useState(false);
  const state = status?.state ?? "idle";
  const running = !!waiting;

  // After the request, wait for the new version to answer, then reload onto its UI.
  useEffect(() => {
    if (!waiting) return;
    let stop = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const tick = async () => {
      try {
        const v = await get<{ version?: string }>("/api/version");
        if (v?.version === waiting) {
          setReconnected(true);
          toast(`Forge updated to ${waiting}${follow ? "; agents follow in batches" : ""}`);
          timer = setTimeout(() => window.location.reload(), 1500);
          return;
        }
      } catch {
        /* restarting: the server is briefly unavailable */
      }
      if (!stop) timer = setTimeout(() => void tick(), 2000);
    };
    void tick();
    return () => {
      stop = true;
      if (timer) clearTimeout(timer);
    };
  }, [waiting, follow, toast]);

  useEffect(() => {
    if (waiting && state === "failed") setWaiting(undefined);
  }, [waiting, state]);

  const start = async () => {
    setPreparing(true);
    try {
      const ok = await ensureMfa("Updating Forge needs an MFA-verified sign-in.");
      if (!ok) return;
      await apply.mutateAsync({ version: release.version, agents: follow ? "rollout" : "none", ...(follow ? { batch } : {}) });
      setWaiting(release.version);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setPreparing(false);
    }
  };

  const close = (o: boolean) => {
    if (!o && (preparing || (running && !reconnected && state !== "failed"))) return;
    onOpenChange(o);
    if (!o) {
      setTyped("");
      apply.reset();
    }
  };

  const stageIdx = STAGES.findIndex((x) => x.state === state);

  return (
    <Modal open={open} onOpenChange={close} title={`Update Forge to ${release.version}?`} description={`You run ${current || "—"}. Upgrades only; you can't go back from the console.`} wide>
      {running || state === "failed" ? (
        <div className="space-y-3 text-[13px]">
          <ol className="space-y-1">
            {STAGES.map((st, i) => {
              const done = reconnected || (stageIdx >= 0 && i < stageIdx);
              const now = !reconnected && i === stageIdx && state !== "failed";
              return (
                <li key={st.state} className={`flex items-center gap-2 ${done ? "text-ok" : now ? "text-accent" : "text-faint"}`}>
                  {done ? <CheckCircle2 className="size-3.5" /> : now ? <Loader2 className="size-3.5 animate-spin" /> : <span className="inline-block size-3.5 rounded-full border border-line" />}
                  {st.label}
                </li>
              );
            })}
            <li className={`flex items-center gap-2 ${reconnected ? "text-ok" : state === "restarting" ? "text-accent" : "text-faint"}`}>
              {reconnected ? <CheckCircle2 className="size-3.5" /> : state === "restarting" ? <Loader2 className="size-3.5 animate-spin" /> : <span className="inline-block size-3.5 rounded-full border border-line" />}
              Reconnect to {release.version}
            </li>
            {follow && (
              <li className="flex items-center gap-2 text-faint">
                <span className="inline-block size-3.5 rounded-full border border-line" />
                Roll agents out to {linuxCount} Linux node{linuxCount === 1 ? "" : "s"} (progress shows under Agents after the reload)
              </li>
            )}
          </ol>
          {state === "failed" ? (
            <p role="alert" className="rounded-md bg-bad-soft px-3 py-2 text-xs text-bad">
              Update failed{status?.message ? `: ${status.message}` : ""}. The previous version is still running.
            </p>
          ) : (
            <p className="text-xs text-muted">{reconnected ? "Reloading onto the new version…" : (status?.message ?? "Working… the page reloads when the new version answers.")}</p>
          )}
          <div className="flex justify-end">
            <Button onClick={() => close(false)} disabled={!reconnected && state !== "failed"}>
              Close
            </Button>
          </div>
        </div>
      ) : (
        <form
          className="space-y-3 text-[13px]"
          onSubmit={(e) => {
            e.preventDefault();
            if (typed.trim() === release.version) void start();
          }}
        >
          <div className="rounded-md border border-line px-3 py-2 text-xs text-muted">
            <div className="mb-1 flex items-center gap-1.5 font-medium text-fg">
              <ShieldCheck className="size-3.5 text-ok" /> What happens
            </div>
            <ol className="list-decimal space-y-0.5 pl-4">
              <li>Download {release.version} and check it against the release's signed checksums.</li>
              <li>Try the new version against your configuration and database migrations.</li>
              <li>Back up the database to the data directory's backups folder.</li>
              <li>Install, keep the current version as a fallback, and restart. The console is offline for a few seconds; nodes keep running.</li>
              <li>If the new version fails to start three times, the previous one is restored automatically.</li>
              {follow ? (
                <li>
                  Then update agents on {linuxCount} Linux node{linuxCount === 1 ? "" : "s"} in batches of {batch}, canary first. A node that doesn't reconnect rolls back and stops the
                  rollout.
                </li>
              ) : (
                <li>Agents are not touched.</li>
              )}
            </ol>
          </div>
          <Field label={`Type ${release.version} to confirm`}>
            <Input autoFocus value={typed} onChange={(e) => setTyped(e.target.value)} placeholder={release.version} spellCheck={false} autoComplete="off" className="font-mono" />
          </Field>
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" onClick={() => close(false)} disabled={preparing}>
              Cancel
            </Button>
            <Button variant="primary" disabled={typed.trim() !== release.version || preparing || FORGE_BUSY_STATES.includes(state)}>
              {preparing ? <Loader2 className="size-4 animate-spin" /> : <CircleArrowUp className="size-4" />} Update now
            </Button>
          </div>
        </form>
      )}
    </Modal>
  );
}
