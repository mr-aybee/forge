// Settings of a managed RustFS store as a page (`/storage/$storeId/settings`): release, ports,
// region and the Docker options. Saving recreates the service, so it confirms with the impact.
import { Link, useNavigate, useParams } from "@tanstack/react-router";
import { Container, Loader2, Network, Save, Tag } from "lucide-react";
import { useState } from "react";

import { ApiError } from "../api/client";
import { useObjectStore, useSaveStoreSettings, useStoreSettings } from "../api/storage";
import type { Job, ObjectStore, StoreSettings } from "../api/types";
import { errorText, useToast } from "../app/toast";
import { FieldError, FormPage, FormSection, type FormSectionMeta, SummaryPanel, SummaryRow } from "../components/FormPage";
import { DockerFields, type DockerDraft, REGION, dockerBody, dockerDraft, dockerProblems } from "../components/storage/DockerOptions";
import { InstallJobs } from "../components/storage/StoreDialogs";
import { Button, Confirm, Empty, ErrorNote, Field, Input, Skeleton } from "../components/ui";

interface Draft {
  version: string;
  s3_port: string;
  console: boolean;
  console_port: string;
  region: string;
  docker: DockerDraft | null;
}

const toDraft = (s: StoreSettings): Draft => ({
  version: s.version ?? "",
  s3_port: s.s3_port ? String(s.s3_port) : "9000",
  console: s.console_port !== null && s.console_port !== undefined,
  console_port: s.console_port ? String(s.console_port) : "",
  region: s.region ?? "",
  docker: s.docker ? dockerDraft(s.docker) : null,
});

const port = (v: string) => Number(v) >= 1024 && Number(v) <= 65535 && Number.isInteger(Number(v));

/** What a save changes, as short lines, so the confirmation names the impact. */
function changes(a: Draft, b: Draft): string[] {
  const out: string[] = [];
  if (a.version !== b.version) out.push(`Release ${a.version} → ${b.version}`);
  if (a.s3_port !== b.s3_port) out.push(`S3 port ${a.s3_port} → ${b.s3_port}`);
  const con = (d: Draft) => (d.console ? d.console_port || "?" : "off");
  if (con(a) !== con(b)) out.push(`Console ${con(a)} → ${con(b)}`);
  if (a.region !== b.region) out.push(`Region ${a.region} → ${b.region}`);
  if (a.docker && b.docker) {
    if (a.docker.image !== b.docker.image) out.push(`Image ${a.docker.image} → ${b.docker.image}`);
    if (a.docker.restart !== b.docker.restart) out.push(`Restart ${a.docker.restart} → ${b.docker.restart}`);
    if (a.docker.memory_mb !== b.docker.memory_mb) out.push(`Memory ${a.docker.memory_mb || "no limit"} → ${b.docker.memory_mb || "no limit"} MB`);
    if (a.docker.cpus !== b.docker.cpus) out.push(`CPUs ${a.docker.cpus || "no limit"} → ${b.docker.cpus || "no limit"}`);
    const env = (d: DockerDraft) => JSON.stringify(d.env.filter((e) => e.key).map((e) => [e.key, e.value]).sort());
    if (env(a.docker) !== env(b.docker)) out.push("Environment variables changed");
  }
  return out;
}

export function StorageSettings() {
  const { storeId } = useParams({ from: "/storage/$storeId/settings" });
  const store = useObjectStore(storeId);
  const settings = useStoreSettings(storeId, store.data?.mode === "managed");
  const s = store.data;

  const crumb = (
    <Link to="/storage/$storeId" params={{ storeId }}>
      {s?.name ?? "Store"}
    </Link>
  );

  const shell = (body: React.ReactNode) => (
    <FormPage back={crumb} current="Settings" title={`Settings of ${s?.name ?? "store"}`} description="Release, ports, region and runtime options.">
      {body}
    </FormPage>
  );

  if (store.isLoading || (s?.mode === "managed" && settings.isLoading)) return shell(<Skeleton rows={6} />);
  if (store.error || !s) return shell(<ErrorNote error={store.error ?? "Store unavailable"} />);
  if (s.mode !== "managed")
    return shell(
      <Empty title="Nothing to configure here" action={<Link to="/storage/$storeId" params={{ storeId }}><Button>Back to store</Button></Link>}>
        Connected stores are configured by their provider.
      </Empty>,
    );
  if (settings.error instanceof ApiError && settings.error.status === 404) return shell(<p className="text-xs text-muted">This server cannot change store settings yet. Update Forge to use them.</p>);
  if (settings.error || !settings.data) return shell(<ErrorNote error={settings.error ?? "Settings unavailable"} />);

  // Keyed on the store so the draft starts from the server values and never resets on refetch.
  return <SettingsForm key={s.id} s={s} initial={settings.data} crumb={crumb} />;
}

function SettingsForm({ s, initial, crumb }: { s: ObjectStore; initial: StoreSettings; crumb: React.ReactNode }) {
  const save = useSaveStoreSettings();
  const toast = useToast();
  const navigate = useNavigate();
  const [orig] = useState(() => toDraft(initial));
  const [d, setD] = useState<Draft>(() => toDraft(initial));
  const [confirm, setConfirm] = useState(false);
  const [jobs, setJobs] = useState<Job[] | null>(null);
  const editable = initial.editable === true;
  const docker = initial.runtime === "docker";
  const set = (p: Partial<Draft>) => setD((o) => ({ ...o, ...p }));
  const toStore = () => void navigate({ to: "/storage/$storeId", params: { storeId: s.id } });

  const err = {
    version: /^(latest|[0-9A-Za-z.-]+)$/.test(d.version.trim()) ? undefined : "latest or an exact version, e.g. 1.0.6.",
    s3_port: port(d.s3_port) ? undefined : "A port from 1024 to 65535.",
    console_port: d.console && (!port(d.console_port) || d.console_port === d.s3_port) ? "1024–65535, different from the S3 port." : undefined,
    region: REGION.test(d.region.trim()) ? undefined : "Lowercase letters, digits and hyphens.",
    docker: d.docker ? dockerProblems(d.docker).join(" ") || undefined : undefined,
  };
  const problems = Object.values(err).filter(Boolean).length;
  const diff = changes(orig, d);
  const nodes = (s.members ?? []).filter((m) => m.status !== "removed").map((m) => m.hostname || m.server_id.slice(0, 8));

  const submit = () => {
    const dk = d.docker ? dockerBody(d.docker) : null;
    save.mutate(
      {
        id: s.id,
        version: d.version.trim(),
        s3_port: Number(d.s3_port),
        console_port: d.console ? Number(d.console_port) : null,
        region: d.region.trim(),
        docker: dk ? { image: dk.image, restart: dk.restart, memory_mb: dk.memory_mb ?? null, cpu_millis: dk.cpu_millis ?? null, env: dk.env ?? {} } : undefined,
      },
      {
        onSuccess: (r) => {
          setJobs(Array.isArray(r?.jobs) ? r.jobs.filter((j) => j?.id) : []);
          setConfirm(false);
          toast(`Applying settings to ${s.name}`);
        },
        onError: (e) => toast(errorText(e), "error"),
      },
    );
  };

  if (jobs) {
    return (
      <FormPage
        back={crumb} current="Settings"
        title={`Applying settings to ${s.name}`}
        description="The change runs as a job on each node."
        footer={
          <Button variant="primary" onClick={toStore}>
            Back to store
          </Button>
        }
      >
        <InstallJobs result={{ jobs }} nodeLabel={(id) => (s.members ?? []).find((m) => m.server_id === id)?.hostname || id.slice(0, 8)} submitted="Settings submitted." />
      </FormPage>
    );
  }

  const sections: FormSectionMeta[] = [
    { id: "release", title: "Release", icon: Tag, tone: "violet", state: err.version ? "error" : "ok" },
    { id: "network", title: "Network", icon: Network, tone: "teal", state: err.s3_port || err.console_port || err.region ? "error" : "ok" },
    ...(d.docker ? [{ id: "docker", title: "Docker", icon: Container, tone: "orange" as const, state: err.docker ? ("error" as const) : ("ok" as const) }] : []),
  ];
  const impact = `${docker ? "Recreates the container" : "Restarts the service"} on ${nodes.join(", ") || "its node"}; S3 clients see a short outage${s.topology === "distributed" ? " on each node in turn" : ""}.`;
  const whatHappens = (
    <ol className="list-decimal space-y-0.5 pl-4 text-xs text-muted">
      <li>{docker ? "The new image is pulled before anything stops." : "The new release is downloaded and checksum-verified before anything stops."}</li>
      <li>{docker ? "The container is recreated with the new settings, on the same data paths and volumes." : "The service is reconfigured and restarted on the same data paths."} Data is kept.</li>
      <li>Forge waits for the health check. If it fails, Forge rolls back and the job fails with the reason.</li>
      {d.s3_port !== orig.s3_port && <li>S3 clients and attached services must use the new port.</li>}
    </ol>
  );
  const why = !editable ? "Not editable for this store" : problems ? "Fix the problems above" : !diff.length ? "No changes yet" : undefined;

  return (
    <FormPage
      back={crumb} current="Settings"
      title={`Settings of ${s.name}`}
      description="Release, ports, region and runtime options. Saving restarts the store."
      sections={sections}
      onSubmit={() => !why && setConfirm(true)}
      note={editable ? why : undefined}
      summary={
        <SummaryPanel title="Changes">
          {diff.length ? (
            <ul className="space-y-0.5 font-mono text-[11px] break-all tabular-nums">
              {diff.map((c) => (
                <li key={c}>{c}</li>
              ))}
            </ul>
          ) : (
            <p className="text-xs text-faint">No changes yet.</p>
          )}
          <SummaryRow label="Runtime">{docker ? "Docker" : "Native"}</SummaryRow>
          <SummaryRow label="Nodes">{nodes.join(", ") || "—"}</SummaryRow>
          {diff.length > 0 && (
            <div className="border-t border-line pt-2">
              <p className="mb-1 text-xs text-fg">{impact}</p>
              {whatHappens}
            </div>
          )}
        </SummaryPanel>
      }
      footer={
        <>
          <Button type="button" variant="ghost" onClick={toStore}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" disabled={!!why} title={why}>
            <Save className="size-4" /> Save and apply
          </Button>
        </>
      }
    >
      {!editable && <p className="rounded-md border border-warn/40 bg-warn-soft px-3 py-2 text-xs text-warn">These settings cannot be changed here: an adopted or connected store keeps the container it was created with.</p>}
      <fieldset disabled={!editable} className="min-w-0 space-y-3">
        <FormSection id="release" title="Release" icon={Tag} tone="violet" description={docker ? "The image tag. A newer one upgrades RustFS." : "latest or an exact version. A newer one upgrades RustFS."}>
          <div className="max-w-xs">
            <Field label="Release" hint={err.version ? <FieldError>{err.version}</FieldError> : undefined}>
              <Input className={`font-mono ${err.version ? "!border-bad" : ""}`} value={d.version} onChange={(e) => set({ version: e.target.value })} />
            </Field>
          </div>
        </FormSection>
        <FormSection id="network" title="Network" icon={Network} tone="teal" description="The S3 API port, the region clients see, and the optional web console.">
          <div className="grid gap-3 sm:grid-cols-3">
            <Field label="S3 port" hint={err.s3_port ? <FieldError>{err.s3_port}</FieldError> : undefined}>
              <Input type="number" min={1024} max={65535} className={`font-mono ${err.s3_port ? "!border-bad" : ""}`} value={d.s3_port} onChange={(e) => set({ s3_port: e.target.value })} />
            </Field>
            <Field label="Region" hint={err.region ? <FieldError>{err.region}</FieldError> : undefined}>
              <Input className={`font-mono ${err.region ? "!border-bad" : ""}`} value={d.region} onChange={(e) => set({ region: e.target.value })} />
            </Field>
            <Field label="Web console" hint={err.console_port ? <FieldError>{err.console_port}</FieldError> : d.console ? "RustFS web UI on its own port." : "Off: only the S3 API."}>
              <div className="flex items-center gap-2">
                <label className="flex shrink-0 items-center gap-1.5 text-[13px]">
                  <input type="checkbox" checked={d.console} onChange={(e) => set({ console: e.target.checked, console_port: e.target.checked && !d.console_port ? "9001" : d.console_port })} />
                  On
                </label>
                <Input aria-label="Console port" type="number" min={1024} max={65535} className="font-mono" disabled={!d.console} value={d.console ? d.console_port : ""} placeholder="—" onChange={(e) => set({ console_port: e.target.value })} />
              </div>
            </Field>
          </div>
        </FormSection>
        {d.docker && (
          <FormSection id="docker" title="Docker" icon={Container} tone="orange" description="Image, restart policy, limits and environment. Container name and network are fixed after install.">
            <DockerFields value={d.docker} onChange={(dk) => set({ docker: dk })} fixed />
            {(initial.docker?.volumes ?? []).some((v) => v.kind === "named") && (
              <p className="font-mono text-[11px] text-faint">
                volumes {(initial.docker?.volumes ?? []).map((v) => (v.kind === "named" ? v.name : "bind")).join(", ")} (fixed)
              </p>
            )}
            <FieldError>{err.docker}</FieldError>
          </FormSection>
        )}
      </fieldset>
      <Confirm
        open={confirm}
        onOpenChange={setConfirm}
        title={`Apply settings to ${s.name}?`}
        busy={save.isPending}
        confirmLabel={save.isPending ? "Applying…" : "Save and apply"}
        onConfirm={submit}
        body={
          <div className="space-y-2 text-xs">
            <ul className="font-mono text-[11px] text-fg">
              {diff.map((c) => (
                <li key={c}>{c}</li>
              ))}
            </ul>
            <p className="text-fg">{impact}</p>
            {whatHappens}
            {save.isPending && <Loader2 className="size-4 animate-spin text-accent" />}
            <ErrorNote error={save.error} />
          </div>
        }
      />
    </FormPage>
  );
}
