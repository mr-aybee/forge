// Settings of a managed RustFS store: release, ports, region and the Docker options. Saving recreates the service.
import { Loader2 } from "lucide-react";
import { useEffect, useState } from "react";

import { ApiError } from "../../api/client";
import { useSaveStoreSettings, useStoreSettings } from "../../api/storage";
import type { Job, ObjectStore, StoreSettings } from "../../api/types";
import { errorText, useToast } from "../../app/toast";
import { Button, ErrorNote, Field, Input, Modal, Skeleton } from "../ui";
import { DockerFields, type DockerDraft, REGION, dockerBody, dockerDraft, dockerProblems } from "./DockerOptions";
import { InstallJobs } from "./StoreDialogs";

interface Draft {
  version: string;
  s3_port: string;
  console: boolean;
  console_port: string;
  region: string;
  docker: DockerDraft | null;
}

const toDraft = (s: StoreSettings): Draft => ({
  version: s.version,
  s3_port: s.s3_port ? String(s.s3_port) : "9000",
  console: s.console_port !== null,
  console_port: s.console_port ? String(s.console_port) : "",
  region: s.region,
  docker: s.docker ? dockerDraft(s.docker) : null,
});

const port = (v: string) => Number(v) >= 1024 && Number(v) <= 65535 && Number.isInteger(Number(v));

/** What a save changes, as short lines, so the review names the impact. */
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

export function StoreSettingsDialog({ s, open, onOpenChange }: { s: ObjectStore; open: boolean; onOpenChange: (o: boolean) => void }) {
  const settings = useStoreSettings(s.id, open);
  const save = useSaveStoreSettings();
  const toast = useToast();
  const [d, setD] = useState<Draft | null>(null);
  const [step, setStep] = useState<"form" | "review" | "job">("form");
  const [jobs, setJobs] = useState<Job[]>([]);

  useEffect(() => {
    if (open) {
      setD(null);
      setStep("form");
      setJobs([]);
      save.reset();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  // Fill the form once the settings arrive (not on every refetch, which would drop edits).
  useEffect(() => {
    if (open && !d && settings.data) setD(toDraft(settings.data));
  }, [open, d, settings.data]);

  const missing = settings.error instanceof ApiError && settings.error.status === 404;
  const orig = settings.data ? toDraft(settings.data) : null;
  const editable = settings.data?.editable === true;
  const docker = settings.data?.runtime === "docker";
  const set = (p: Partial<Draft>) => setD((o) => (o ? { ...o, ...p } : o));

  const problems: string[] = [];
  if (d) {
    if (!/^(latest|[0-9A-Za-z.-]+)$/.test(d.version.trim())) problems.push("Release: latest or an exact version, e.g. 1.0.6.");
    if (!port(d.s3_port)) problems.push("S3 port: 1024–65535.");
    if (d.console && (!port(d.console_port) || d.console_port === d.s3_port)) problems.push("Console port: 1024–65535, different from the S3 port.");
    if (!REGION.test(d.region.trim())) problems.push("Region: lowercase letters, digits and hyphens.");
    if (d.docker) problems.push(...dockerProblems(d.docker));
  }
  const diff = d && orig ? changes(orig, d) : [];

  const submit = () => {
    if (!d) return;
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
          setStep("job");
          toast(`Applying settings to ${s.name}`);
        },
        onError: (e) => toast(errorText(e), "error"),
      },
    );
  };

  const nodes = s.members.filter((m) => m.status !== "removed").map((m) => m.hostname || m.server_id.slice(0, 8));

  return (
    <Modal open={open} onOpenChange={onOpenChange} title={`Settings of ${s.name}`} description="Release, ports, region and runtime options." wide>
      {settings.isLoading || (!d && !settings.error) ? (
        <Skeleton rows={4} />
      ) : missing ? (
        <p className="text-xs text-muted">This server cannot change store settings yet. Update Forge to use them.</p>
      ) : settings.error || !d ? (
        <ErrorNote error={settings.error} />
      ) : step === "job" ? (
        <div className="space-y-3">
          <InstallJobs result={{ jobs }} nodeLabel={(id) => s.members.find((m) => m.server_id === id)?.hostname || id.slice(0, 8)} submitted="Settings submitted." />
          <div className="flex justify-end">
            <Button variant="primary" onClick={() => onOpenChange(false)}>
              Close
            </Button>
          </div>
        </div>
      ) : step === "review" ? (
        <div className="space-y-3">
          <ul className="space-y-0.5 font-mono text-[11px] tabular-nums">
            {diff.map((c) => (
              <li key={c}>{c}</li>
            ))}
          </ul>
          <p className="text-[13px] text-fg">
            {docker ? "Recreates the container" : "Restarts the service"} on {nodes.join(", ") || "its node"}; S3 clients see a short outage{s.topology === "distributed" ? " on each node in turn" : ""}.
          </p>
          <div className="text-xs text-muted">
            <div className="mb-1 font-medium text-fg">What happens</div>
            <ol className="list-decimal space-y-0.5 pl-4">
              <li>{docker ? "The new image is pulled before anything stops." : "The new release is downloaded and checksum-verified before anything stops."}</li>
              <li>{docker ? "The container is recreated with the new settings, on the same data paths and volumes." : "The service is reconfigured and restarted on the same data paths."} Data is kept.</li>
              <li>Forge waits for the health check. If it fails, Forge rolls back to the previous {docker ? "container" : "configuration"} and the job fails with the reason.</li>
              {d.s3_port !== orig?.s3_port && <li>S3 clients and attached services must use the new port.</li>}
            </ol>
          </div>
          <ErrorNote error={save.error} />
          <div className="flex justify-end gap-2">
            <Button variant="ghost" disabled={save.isPending} onClick={() => setStep("form")}>
              Back
            </Button>
            <Button variant="primary" disabled={save.isPending} onClick={submit}>
              {save.isPending && <Loader2 className="size-4 animate-spin" />}
              Save and apply
            </Button>
          </div>
        </div>
      ) : (
        <form
          className="space-y-4"
          onSubmit={(e) => {
            e.preventDefault();
            if (editable && !problems.length && diff.length) setStep("review");
          }}
        >
          {!editable && <p className="rounded-md border border-line px-3 py-2 text-xs text-muted">These settings cannot be changed here: an adopted or connected store keeps the container it was created with.</p>}
          <fieldset disabled={!editable} className="space-y-4">
            <div className="grid gap-3 sm:grid-cols-3">
              <Field label="Release" hint={docker ? "The image tag. A newer one upgrades RustFS." : "latest or exact. A newer one upgrades RustFS."}>
                <Input required className="font-mono" value={d.version} onChange={(e) => set({ version: e.target.value })} />
              </Field>
              <Field label="S3 port">
                <Input required type="number" min={1024} max={65535} className="font-mono" value={d.s3_port} onChange={(e) => set({ s3_port: e.target.value })} />
              </Field>
              <Field label="Region">
                <Input required className="font-mono" value={d.region} onChange={(e) => set({ region: e.target.value })} />
              </Field>
              <Field label="Web console" hint={d.console ? "The RustFS web console on its own port." : "Off: only the S3 API is reachable."}>
                <div className="flex items-center gap-2">
                  <label className="flex shrink-0 items-center gap-1.5 text-[13px]">
                    <input type="checkbox" checked={d.console} onChange={(e) => set({ console: e.target.checked, console_port: e.target.checked && !d.console_port ? "9001" : d.console_port })} />
                    On
                  </label>
                  <Input aria-label="Console port" type="number" min={1024} max={65535} className="font-mono" disabled={!d.console} value={d.console ? d.console_port : ""} placeholder="—" onChange={(e) => set({ console_port: e.target.value })} />
                </div>
              </Field>
            </div>
            {d.docker && (
              <section aria-label="Docker" className="space-y-2 border-t border-line pt-3">
                <div className="text-xs font-medium text-muted">Docker</div>
                <DockerFields value={d.docker} onChange={(docker) => set({ docker })} fixed />
                {(settings.data?.docker?.volumes ?? []).some((v) => v.kind === "named") && (
                  <p className="font-mono text-[11px] text-faint">
                    volumes {(settings.data?.docker?.volumes ?? []).map((v) => (v.kind === "named" ? v.name : "bind")).join(", ")} (fixed)
                  </p>
                )}
              </section>
            )}
          </fieldset>
          {problems.length > 0 && (
            <ul className="space-y-0.5 text-xs text-bad">
              {problems.map((p) => (
                <li key={p}>{p}</li>
              ))}
            </ul>
          )}
          <div className="flex items-center justify-end gap-2">
            {editable && !diff.length && <span className="text-xs text-faint">No changes</span>}
            <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button variant="primary" disabled={!editable || problems.length > 0 || !diff.length} title={!editable ? "Not editable for this store" : undefined}>
              Review
            </Button>
          </div>
        </form>
      )}
    </Modal>
  );
}
