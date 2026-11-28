import { Link, useSearch } from "@tanstack/react-router";
import {
  ChevronRight,
  Eye,
  EyeOff,
  Loader2,
  Lock,
  Play,
  RefreshCw,
  Save,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { useServers } from "../api/hooks";
import {
  type Preview,
  STACK_NAME,
  jobServer,
  previewStack,
  useCreateStack,
  useDeployStack,
} from "../api/stacks";
import { type Job, nodeName } from "../api/types";
import { errorText, useToast } from "../app/toast";
import { NodeJobTabs } from "../components/JobLog";
import { useEnvironmentChoices } from "../components/stacks/CreateStack";
import { FromTemplate } from "../components/stacks/FromTemplate";
import {
  Designer,
  MeshToggle,
  ServicePicker,
  UploadText,
  YamlEditor,
  startBlocker,
  useMfaRetry,
} from "../components/stacks/parts";
import {
  ActionBar,
  Button,
  Field,
  Input,
  PageCrumb,
  PageHeader,
  Select,
} from "../components/ui";
import { CodeField } from "../components/CodeField";

const SAMPLE = `services:
  web:
    image: nginx:1.27
    ports:
      - "8080:80"
    restart: unless-stopped
`;

const PREVIEW_KEY = "forge.newStack.preview";
const readPreviewPref = () => {
  try {
    return localStorage.getItem(PREVIEW_KEY) !== "0";
  } catch {
    return true;
  }
};

const Crumb = ({ current }: { current: string }) => (
  <PageCrumb back={<Link to="/stacks">Stacks</Link>} current={current} />
);

/**
 * New stack from Compose: paste or upload a compose file, preview where each
 * service runs and what the policy says, then save (and optionally start).
 */
export function NewStack() {
  const search = useSearch({ from: "/stacks/new" });
  const toast = useToast();
  const servers = (useServers().data ?? []).filter(
    (s) => s.status !== "revoked",
  );
  const [serverId, setServerId] = useState(search.node ?? "");
  const choices = useEnvironmentChoices(serverId);
  const [ns, setNs] = useState(search.environment ?? "");
  const [name, setName] = useState("");
  const [dir, setDir] = useState("");
  const [content, setContent] = useState(SAMPLE);
  const [docKey, setDocKey] = useState(0);
  const [env, setEnv] = useState("");
  const [envOpen, setEnvOpen] = useState(false);
  const [showPreview, setShowPreview] = useState(readPreviewPref);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [pvError, setPvError] = useState<string>();
  const [pvBusy, setPvBusy] = useState(false);
  const [ticked, setTicked] = useState<Set<string>>(new Set());
  const [done, setDone] = useState<{ id: string; name: string; jobs: Job[] }>();
  const create = useCreateStack();
  const deploy = useDeployStack();
  const mfa = useMfaRetry();
  const [starting, setStarting] = useState(false);
  const [mode, setMode] = useState<"compose" | "template">("compose");

  const togglePreview = () => {
    const next = !showPreview;
    setShowPreview(next);
    try {
      localStorage.setItem(PREVIEW_KEY, next ? "1" : "0");
    } catch {
      /* storage unavailable: the choice lasts for this visit only */
    }
  };

  // Default the environment to the node's own, else the first offered.
  useEffect(() => {
    if (ns && choices.list.some((n) => n.id === ns)) return;
    const own = choices.server?.environment_id;
    const pick = choices.list.find((n) => n.id === own) ?? choices.list[0];
    if (pick) setNs(pick.id);
  }, [choices.list, choices.server, ns]);

  // A .env with content (typed or uploaded) keeps its section open.
  useEffect(() => {
    if (env) setEnvOpen(true);
  }, [env]);

  const nameOk = STACK_NAME.test(name);
  const canPreview = !!ns && !!serverId && content.trim().length > 0;

  const runPreview = async () => {
    if (!canPreview) return;
    setPvBusy(true);
    try {
      const p = await previewStack({
        environment_id: ns,
        server_id: serverId,
        name: name || undefined,
        content,
        env: env || undefined,
        dir: dir.trim() || undefined,
      });
      setPreview(p);
      setPvError(undefined);
    } catch (e) {
      setPreview(null);
      setPvError(errorText(e));
    } finally {
      setPvBusy(false);
    }
  };

  // Re-preview shortly after the inputs settle. This also runs while the
  // column is hidden: Save & Start depends on the policy result.
  const sig = `${ns}|${serverId}|${content}|${env}|${dir}`;
  useEffect(() => {
    if (done || !canPreview) return;
    const t = setTimeout(() => void runPreview(), 900);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sig, done]);

  const blocker = startBlocker(preview);
  const services = preview?.services ?? [];
  const validTicked = useMemo(
    () =>
      new Set([...ticked].filter((t) => services.some((x) => x.name === t))),
    [ticked, services],
  );

  const submit = async (start: boolean) => {
    if (!nameOk) return toast("Enter a valid stack name first.", "error");
    try {
      const r = await create.mutateAsync({
        environment_id: ns,
        server_id: serverId,
        name,
        content,
        env: env || undefined,
        dir: dir.trim() || undefined,
      });
      if (!start) {
        toast(`Stack ${r.stack.name} saved`);
        setDone({ id: r.stack.id, name: r.stack.name, jobs: [] });
        return;
      }
      setStarting(true);
      try {
        const jobs = await mfa(() =>
          deploy.mutateAsync({
            id: r.stack.id,
            services: validTicked.size ? [...validTicked] : undefined,
          }),
        );
        toast(
          `Starting ${r.stack.name} on ${jobs.length} node${jobs.length === 1 ? "" : "s"}`,
        );
        setDone({ id: r.stack.id, name: r.stack.name, jobs });
      } catch (e) {
        toast(`Saved, but starting failed: ${errorText(e)}`, "error");
        setDone({ id: r.stack.id, name: r.stack.name, jobs: [] });
      } finally {
        setStarting(false);
      }
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const hostOf = (id: string) => {
    const s = servers.find((x) => x.id === id);
    return s ? nodeName(s) : id.slice(0, 8) || "node";
  };

  if (done)
    return (
      <>
        <Crumb current={done.name} />
        <PageHeader
          title={`Stack ${done.name}`}
          description={
            done.jobs.length
              ? "Following the deployment on each node."
              : "Saved. Deploy it from the stack page when ready."
          }
          actions={
            <Link to="/stacks/$stackId" params={{ stackId: done.id }}>
              <Button variant="primary">Open stack</Button>
            </Link>
          }
        />
        {done.jobs.length > 0 && (
          <NodeJobTabs
            jobs={done.jobs.map((j) => ({
              nodeId: jobServer(j) || j.id,
              nodeName: hostOf(jobServer(j)),
              jobId: j.id,
            }))}
          />
        )}
      </>
    );

  const modeToggle = (
    <div
      role="group"
      aria-label="Source"
      className="inline-flex rounded-md border border-line p-0.5 text-xs"
    >
      {(["compose", "template"] as const).map((m) => (
        <button
          key={m}
          type="button"
          aria-pressed={mode === m}
          onClick={() => setMode(m)}
          className={`rounded px-2 py-1 ${mode === m ? "bg-accent-soft text-accent" : "text-muted hover:text-fg"}`}
        >
          {m === "compose" ? "Compose file" : "From a template"}
        </button>
      ))}
    </div>
  );

  if (mode === "template")
    return (
      <div className="flex min-h-full flex-col">
        <Crumb current="New stack" />
        <PageHeader
          title="New stack"
          description="Start from an app template: choose one, fill in its variables and save."
          actions={modeToggle}
        />
        <FromTemplate
          node={serverId}
          environment={ns}
          onDone={(s) => setDone({ id: s.id, name: s.name, jobs: [] })}
        />
      </div>
    );

  const busy = create.isPending || starting;
  const saveOff = busy || !nameOk || !ns || !serverId || !content.trim();
  const startOff =
    busy || !nameOk || !ns || !serverId || !preview || !!blocker || !!pvError;
  const startWhy =
    blocker ??
    (pvError
      ? "The preview failed"
      : !preview
        ? "Preview the file first"
        : !nameOk
          ? "Enter a valid stack name"
          : undefined);
  const envLines = env.split("\n").filter((l) => l.trim()).length;

  return (
    <div className="flex min-h-full flex-col">
      <Crumb current="New stack" />
      <PageHeader
        title="New stack"
        description="Paste or upload a compose file. Forge places each service, binds ports to the internal network and checks the policy before anything runs."
        actions={
          <>
            {modeToggle}
            <Button
              type="button"
              aria-pressed={showPreview}
              onClick={togglePreview}
              title={showPreview ? "Hide the preview" : "Show the preview"}
              className={
                showPreview ? "border-accent/40 bg-accent-soft text-accent" : ""
              }
            >
              {showPreview ? (
                <Eye className="size-4" />
              ) : (
                <EyeOff className="size-4" />
              )}{" "}
              Preview
            </Button>
          </>
        }
      />

      <div
        className={`grid flex-1 gap-5 ${showPreview ? "xl:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]" : ""}`}
      >
        <div className="flex min-w-0 flex-col gap-4">
          <div className="grid items-start gap-x-4 gap-y-3 sm:grid-cols-2">
            <Field label="Node" hint="Services without placement run here.">
              <Select
                value={serverId}
                onChange={(e) => setServerId(e.target.value)}
                required
              >
                <option value="">Select a node</option>
                {servers.map((s) => (
                  <option key={s.id} value={s.id}>
                    {nodeName(s)}
                  </option>
                ))}
              </Select>
            </Field>
            <Field
              label="Environment"
              hint="Environments of the node's cluster."
            >
              <Select
                value={ns}
                onChange={(e) => setNs(e.target.value)}
                required
              >
                <option value="">Select an environment</option>
                {choices.list.map((n) => (
                  <option key={n.id} value={n.id}>
                    {n.name} · {choices.clusterName(n.cluster_id)}
                  </option>
                ))}
              </Select>
            </Field>
            <Field
              label="Stack name"
              hint={
                name && !nameOk ? (
                  <span className="text-bad">
                    Lower-case letters, digits, - and _; start with a letter or
                    digit.
                  </span>
                ) : (
                  "Unique in the environment."
                )
              }
            >
              <Input
                value={name}
                onChange={(e) => setName(e.target.value.toLowerCase())}
                placeholder="wordpress"
                maxLength={63}
                aria-invalid={!!name && !nameOk}
              />
            </Field>
            <Field
              label="Folder (optional)"
              hint="Default: /opt/forge/stacks/<environment>/<stack>"
            >
              <Input
                value={dir}
                onChange={(e) => setDir(e.target.value)}
                placeholder="/opt/forge/stacks/…"
                className="font-mono text-xs"
              />
            </Field>
          </div>

          <div className="flex min-h-0 flex-1 flex-col">
            <div className="mb-1 flex items-center justify-between">
              <span className="font-mono text-xs font-medium text-muted">
                compose.yaml
              </span>
              <div className="flex items-center gap-3">
              <MeshToggle
                content={content}
                onChange={(t) => {
                  setContent(t);
                  setDocKey((k) => k + 1);
                }}
              />
              <UploadText
                label="Upload file"
                accept=".yml,.yaml,text/yaml,text/plain"
                onText={(t, n) => {
                  if (!t) return toast(n, "error");
                  setContent(t);
                  setDocKey((k) => k + 1);
                }}
              />
              </div>
            </div>
            <YamlEditor
              value={content}
              docKey={`new-${docKey}`}
              onChange={setContent}
              height="h-[max(360px,calc(100dvh-470px))]"
            />
          </div>

          <section className="rounded-md border border-line">
            <button
              type="button"
              aria-expanded={envOpen}
              aria-controls="stack-env"
              onClick={() => setEnvOpen((o) => !o)}
              className="flex w-full min-w-0 items-center gap-1.5 rounded-md px-2.5 py-2 text-left text-xs font-medium text-muted hover:text-fg"
            >
              <ChevronRight
                className={`size-3.5 shrink-0 transition-transform ${envOpen ? "rotate-90" : ""}`}
              />
              <Lock className="size-3.5 shrink-0" />
              <span className="truncate">Environment (.env) — optional</span>
              {envLines > 0 && !envOpen && (
                <span className="ml-auto font-mono text-[11px] text-faint tabular-nums">
                  {envLines} lines
                </span>
              )}
            </button>
            {envOpen && (
              <div
                id="stack-env"
                className="space-y-1.5 border-t border-line px-2.5 py-2"
              >
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <p className="min-w-0 text-[11px] text-faint">
                    Stored sealed with the vault key; the console never shows
                    the values again.
                  </p>
                  <UploadText
                    label="Upload .env"
                    onText={(t, n) => (t ? setEnv(t) : toast(n, "error"))}
                  />
                </div>
                <CodeField lang="env" minLines={5} value={env} onChange={setEnv} placeholder={"DB_PASSWORD=…\nAPP_KEY=…"} aria-label=".env content" />
              </div>
            )}
          </section>
        </div>

        {showPreview && (
          <aside
            aria-label="Preview"
            className="min-w-0 space-y-3 xl:border-l xl:border-line xl:pl-5"
          >
            <div className="flex items-center justify-between">
              <span className="text-xs font-medium text-muted">Preview</span>
              <Button
                type="button"
                variant="ghost"
                onClick={() => void runPreview()}
                disabled={!canPreview || pvBusy}
                className="h-7 px-2 text-xs"
                title="Check the file again"
              >
                {pvBusy ? (
                  <Loader2 className="size-3.5 animate-spin" />
                ) : (
                  <RefreshCw className="size-3.5" />
                )}{" "}
                Refresh
              </Button>
            </div>
            {pvError && (
              <p
                role="alert"
                className="rounded-md bg-bad-soft px-3 py-2 font-mono text-[11.5px] break-words whitespace-pre-wrap text-bad"
              >
                {pvError}
              </p>
            )}
            {!preview && !pvError && (
              <p className="rounded-md border border-dashed border-line px-3 py-6 text-center text-xs text-muted">
                {canPreview
                  ? "Checking the file…"
                  : "Choose a node and environment to see where each service runs."}
              </p>
            )}
            {preview && <Designer preview={preview} />}
            {preview && (
              <ServicePicker
                services={services}
                ticked={validTicked}
                onChange={setTicked}
              />
            )}
            {blocker && (
              <p
                className={`rounded-md px-3 py-2 text-xs ${preview?.blocked ? "bg-bad-soft text-bad" : "bg-warn-soft text-warn"}`}
              >
                {blocker}
              </p>
            )}
          </aside>
        )}
      </div>

      <ActionBar note="Save keeps revision 1 without starting anything.">
        {!showPreview && pvError && (
          <span className="text-[11px] text-bad">
            The preview failed. Show the preview for details.
          </span>
        )}
        {!showPreview && !pvError && blocker && (
          <span
            className="min-w-0 truncate text-[11px] text-warn"
            title={blocker}
          >
            {blocker}
          </span>
        )}
        <Link to="/stacks">
          <Button variant="ghost">Cancel</Button>
        </Link>
        <Button onClick={() => void submit(false)} disabled={saveOff}>
          {create.isPending && !starting ? (
            <Loader2 className="size-4 animate-spin" />
          ) : (
            <Save className="size-4" />
          )}{" "}
          Save
        </Button>
        <span title={startOff ? startWhy : undefined} className="inline-flex">
          <Button
            variant="primary"
            onClick={() => void submit(true)}
            disabled={startOff}
          >
            {starting ? (
              <Loader2 className="size-4 animate-spin" />
            ) : (
              <Play className="size-4" />
            )}{" "}
            Save & Start
          </Button>
        </span>
      </ActionBar>
    </div>
  );
}
