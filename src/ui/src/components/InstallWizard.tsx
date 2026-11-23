import { Check, ChevronLeft, ChevronRight, Download, TriangleAlert } from "lucide-react";
import { NodeJobTabs } from "./JobLog";
import { useEffect, useMemo, useState } from "react";

import { useClusters, useInstallations, useInstallPackage, useMe, usePackageReleases, useServers } from "../api/hooks";
import { type CatalogPackage, nodeName, type PackageParam, type PackageRelease } from "../api/types";
import { errorText, useToast } from "../app/toast";
import { Badge, Button, Field, Input, Modal, Mono, Select, Status } from "./ui";

const STEPS = ["Configure", "Choose nodes", "Review"] as const;

function ParamInput({ p, value, onChange }: { p: PackageParam; value: string; onChange: (v: string) => void }) {
  switch (p.kind.type) {
    case "choice":
      return (
        <Select value={value} onChange={(e) => onChange(e.target.value)}>
          {p.kind.options.map((o) => (
            <option key={o}>{o}</option>
          ))}
        </Select>
      );
    case "bool":
      return (
        <label className="flex items-center gap-2 text-[13px]">
          <input type="checkbox" checked={value === "true"} onChange={(e) => onChange(String(e.target.checked))} />
          {p.must_accept ? "Accept" : "Enabled"}
        </label>
      );
    case "port":
      return <Input type="number" min={1} max={65535} value={value} onChange={(e) => onChange(e.target.value)} required />;
    case "list":
      return <Input value={value} onChange={(e) => onChange(e.target.value)} pattern="[a-z0-9_\-]+(,[a-z0-9_\-]+)*" />;
    default:
      return <Input value={value} onChange={(e) => onChange(e.target.value)} placeholder="auto" pattern="[A-Za-z0-9.:\-]*" />;
  }
}

/** Configure → choose one or more nodes → review & install (one job per node). */
/** " · LTS", " · latest", " · EOL in 40 d" for a catalog line, from vendor data. */
export function lineLabel(releases: PackageRelease[], product: string, line: string): string {
  const mine = releases.filter((r) => r.product === product);
  const r = mine.find((x) => x.line === line);
  if (!product || !r) return "";
  const tags: string[] = [];
  const newest = [...mine].sort((a, b) => b.line.localeCompare(a.line, undefined, { numeric: true }))[0];
  if (newest?.line === r.line) tags.push("latest");
  if (r.lts) tags.push("LTS");
  if (r.eol === "true") tags.push("EOL");
  else if (r.eol && !Number.isNaN(Date.parse(r.eol))) {
    const days = Math.ceil((Date.parse(r.eol) - Date.now()) / 86_400_000);
    if (days <= 0) tags.push("EOL");
    else if (days < 365) tags.push(`EOL in ${days} d`);
  }
  tags.unshift(r.latest);
  return ` · ${tags.join(" · ")}`;
}

export function InstallWizard({
  pkg,
  catalog,
  open,
  onOpenChange,
  preselect,
}: {
  pkg: CatalogPackage | null;
  catalog: CatalogPackage[];
  open: boolean;
  onOpenChange: (o: boolean) => void;
  preselect?: string;
}) {
  const nodes = (useServers().data ?? []).filter((n) => n.status !== "revoked");
  const clusters = useClusters().data ?? [];
  const installations = useInstallations().data ?? [];
  const me = useMe().data;
  const install = useInstallPackage();
  const toast = useToast();
  const [step, setStep] = useState(0);
  const [started, setStarted] = useState<{ nodeId: string; nodeName: string; jobId: string }[]>();
  const [version, setVersion] = useState("");
  const releases = usePackageReleases().data ?? [];
  const [params, setParams] = useState<Record<string, string>>({});
  const [selected, setSelected] = useState<Set<string>>(new Set());

  useEffect(() => {
    if (!open || !pkg) return;
    setStep(0);
    setStarted(undefined);
    setVersion(pkg.versions[0] ?? "");
    setParams(Object.fromEntries(pkg.params.map((p) => [p.key, p.default])));
    setSelected(new Set(preselect ? [preselect] : []));
  }, [open, pkg, preselect]);

  const nameOf = (id: string) => catalog.find((c) => c.id === id)?.name ?? id;
  const rows = useMemo(
    () =>
      nodes.map((n) => {
        const mine = installations.filter((i) => i.server_id === n.id);
        const missing = (pkg?.requires ?? []).filter((r) => !mine.some((i) => i.package === r && i.status === "installed"));
        const existing = mine.find((i) => i.package === pkg?.id && i.version === version);
        const osOk = pkg?.os.includes(n.os) ?? false;
        return { n, missing, existing, osOk, ok: osOk && missing.length === 0 && !existing?.status.endsWith("ing") };
      }),
    [nodes, installations, pkg, version],
  );

  if (!pkg) return null;
  const mustAccept = pkg.params.filter((p) => p.must_accept);
  const configOk = mustAccept.every((p) => params[p.key] === "true");
  const chosen = rows.filter((r) => selected.has(r.n.id));
  const toggle = (id: string) =>
    setSelected((s) => {
      const next = new Set(s);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  const submit = () =>
    install.mutate(
      { package: pkg.id, version, params, server_ids: chosen.map((r) => r.n.id) },
      {
        onSuccess: (res) => {
          toast(`Installing ${pkg.name} ${version} on ${res.length} node${res.length === 1 ? "" : "s"}`);
          // Stay open and follow the install, one tab per node.
          setStarted(
            res.map((r) => {
              const n = nodes.find((x) => x.id === r.server_id);
              return { nodeId: r.server_id, nodeName: n ? nodeName(n) : r.server_id.slice(0, 8), jobId: r.job.id };
            }),
          );
        },
        onError: (e) => toast(errorText(e), "error"),
      },
    );

  return (
    <Modal open={open} onOpenChange={onOpenChange} title={`Install ${pkg.name}`} description={pkg.summary} wide>
      <ol className="mb-5 flex items-center gap-2 text-xs">
        {STEPS.map((s, i) => (
          <li key={s} className="flex items-center gap-2">
            <span
              className={`grid size-6 place-items-center rounded-full border text-[11px] font-semibold ${
                i < step ? "border-ok bg-ok-soft text-ok" : i === step ? "border-accent bg-accent-soft text-accent" : "border-line text-faint"
              }`}
            >
              {i < step ? <Check className="size-3.5" /> : i + 1}
            </span>
            <span className={i === step ? "font-medium" : "text-muted"}>{s}</span>
            {i < STEPS.length - 1 && <span className="mx-1 h-px w-8 bg-line" />}
          </li>
        ))}
      </ol>

      {started ? (
        <div className="space-y-4">
          <p className="text-[13px] text-muted">
            Installing {pkg.name} {version}. Each node has its own tab with the job's steps and the recipe output as it runs. You can close this window; the installs continue, and the output stays in Jobs.
          </p>
          <NodeJobTabs jobs={started} />
          <div className="flex justify-end">
            <Button variant="primary" onClick={() => onOpenChange(false)}>
              Close
            </Button>
          </div>
        </div>
      ) : (
      <>
      {step === 0 && (
        <div className="space-y-4">
          <Field label="Version">
            <Select value={version} onChange={(e) => setVersion(e.target.value)}>
              {pkg.versions.map((v, i) => (
                <option key={v} value={v}>
                  {v}
                  {lineLabel(releases, pkg.upstream, v)}
                  {i === 0 ? " (recommended)" : ""}
                </option>
              ))}
            </Select>
          </Field>
          {pkg.params.map((p) => (
            <Field key={p.key} label={p.label} hint={p.help || undefined}>
              <ParamInput p={p} value={params[p.key] ?? ""} onChange={(v) => setParams((m) => ({ ...m, [p.key]: v }))} />
            </Field>
          ))}
          <div className="rounded-lg border border-line bg-panel-2/50 p-3 text-xs text-muted">
            <p>{pkg.notes}</p>
            <p className="mt-1.5">
              Files: <Mono>/opt/forge/pkgs/{pkg.id}/</Mono> · data: <Mono>/var/lib/forge/pkgs/{pkg.id}/</Mono> · variables:{" "}
              <Mono>/etc/forge/pkgs/{pkg.id}.env</Mono>
            </p>
            {pkg.requires.length > 0 && <p className="mt-1.5">Requires on the same node: {pkg.requires.map(nameOf).join(", ")}.</p>}
          </div>
        </div>
      )}

      {step === 1 && (
        <div className="space-y-2">
          <p className="text-[13px] text-muted">Pick every node that should run {pkg.name}. Each node gets its own durable job.</p>
          <ul className="max-h-[50vh] divide-y divide-line overflow-y-auto rounded-lg border border-line">
            {rows.map(({ n, missing, existing, osOk, ok }) => (
              <li key={n.id}>
                <label className={`flex items-center gap-3 px-3 py-2.5 ${ok ? "cursor-pointer hover:bg-panel-2" : "opacity-60"}`}>
                  <input type="checkbox" disabled={!ok} checked={selected.has(n.id)} onChange={() => toggle(n.id)} />
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="font-medium">{n.hostname}</span>
                      <Badge tone={n.role === "control" ? "accent" : "muted"}>{n.role}</Badge>
                      {n.cluster_id && <Badge tone="accent">{clusters.find((c) => c.id === n.cluster_id)?.name}</Badge>}
                      {existing && <Badge tone="ok">{existing.status === "installed" ? "installed — reinstall" : existing.status}</Badge>}
                    </div>
                    <div className="text-xs text-muted">
                      {n.os}/{n.arch}
                      {!osOk && " · OS not supported"}
                      {missing.length > 0 && (
                        <span className="text-warn">
                          {" "}
                          · needs {missing.map(nameOf).join(", ")} first
                        </span>
                      )}
                    </div>
                  </div>
                  <Status value={n.status} />
                </label>
              </li>
            ))}
          </ul>
          {rows.some((r) => r.n.status !== "online" && selected.has(r.n.id)) && (
            <p className="flex items-center gap-1.5 text-xs text-warn">
              <TriangleAlert className="size-3.5" /> Offline nodes run the install when their agent reconnects.
            </p>
          )}
        </div>
      )}

      {step === 2 && (
        <div className="space-y-3 text-[13px]">
          <dl className="grid grid-cols-[140px_1fr] gap-y-1.5">
            <dt className="text-muted">Package</dt>
            <dd>
              {pkg.name} <Mono>{version}</Mono>
            </dd>
            {pkg.params.map((p) => (
              <div key={p.key} className="contents">
                <dt className="text-muted">{p.label}</dt>
                <dd>
                  <Mono>{params[p.key] || "—"}</Mono>
                </dd>
              </div>
            ))}
            <dt className="text-muted">Nodes</dt>
            <dd>{chosen.map((r) => r.n.hostname).join(", ")}</dd>
          </dl>
          <p className="rounded-lg border border-line bg-panel-2/50 p-3 text-xs text-muted">
            Runs as root through the node's Forge helper, using a recipe compiled into the agent: Forge sends only the package,
            version and these values. Passwords are generated on the node and never leave it.
            {me && !me.mfa && <strong className="text-warn"> Requires an MFA-verified sign-in.</strong>}
          </p>
        </div>
      )}

      <div className="mt-5 flex justify-between gap-2">
        <Button variant="ghost" onClick={() => (step === 0 ? onOpenChange(false) : setStep(step - 1))}>
          {step === 0 ? "Cancel" : (
            <>
              <ChevronLeft className="size-4" /> Back
            </>
          )}
        </Button>
        {step < 2 ? (
          <Button
            variant="primary"
            disabled={(step === 0 && !configOk) || (step === 1 && chosen.length === 0)}
            title={step === 0 && !configOk ? "Fill in the required settings" : step === 1 && chosen.length === 0 ? "Choose at least one node" : undefined}
            onClick={() => setStep(step + 1)}
          >
            Next <ChevronRight className="size-4" />
          </Button>
        ) : (
          <Button variant="primary" disabled={install.isPending} onClick={submit}>
            <Download className="size-4" /> Install on {chosen.length} node{chosen.length === 1 ? "" : "s"}
          </Button>
        )}
      </div>
      </>
      )}
    </Modal>
  );
}
