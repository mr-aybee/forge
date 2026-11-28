import { Link } from "@tanstack/react-router";
import { Loader2, Plug, RefreshCw, Unplug } from "lucide-react";
import { useState, type FormEvent, type ReactNode } from "react";

import {
  type DockerEndpoint,
  useCheckDockerEndpoint,
  useDisableDockerEndpoint,
  useDockerEndpoint,
  useDockerEndpointJob,
  useEnableDockerEndpoint,
} from "../../api/docker";
import { nodeName, type Server } from "../../api/types";
import { errorText, useToast } from "../../app/toast";
import { JobLog } from "../JobLog";
import { Button, Confirm, CopyValue, ErrorNote, Field, Input, Modal, Panel, Skeleton, Status, ago, short, time } from "../ui";

const DEFAULT_PORT = 2376;

/** The node's Docker Engine API endpoint (mutual TLS on dockerd): state, enable, check, disable. */
export function DockerApiPanel({ node }: { node: Server }) {
  const toast = useToast();
  const online = node.status === "online";
  const ep = useDockerEndpoint(node.id);
  const check = useCheckDockerEndpoint(node.id);
  const [dialog, setDialog] = useState<"enable" | "disable" | null>(null);
  const [jobId, setJobId] = useState<string>();
  const job = useDockerEndpointJob(node.id, jobId, (j) =>
    j.status === "succeeded" ? toast("Docker API job finished") : toast(j.last_error ?? `Docker API job ${j.status}`, "error"),
  );
  const running = !!jobId && (!job.data || job.data.status === "queued" || job.data.status === "running");
  const e = ep.data;
  const active = !!e && e.state !== "disabled" && e.state !== "unknown";
  const blocked = !online ? "The agent must be online" : running ? "A Docker API job is running" : undefined;

  const actions = (
    <div className="flex flex-wrap items-center gap-1.5">
      {active && (
        <Button
          onClick={() =>
            check.mutate(undefined, {
              onSuccess: (r) => (r.state === "enabled" ? toast("Docker API answers") : toast(r.last_error ?? `Docker API is ${r.state}`, "error")),
              onError: (err) => toast(errorText(err), "error"),
            })
          }
          disabled={check.isPending}
        >
          {check.isPending ? <Loader2 className="size-4 animate-spin" /> : <RefreshCw className="size-4" />} Check now
        </Button>
      )}
      {active ? (
        <span title={running ? blocked : undefined}>
          <Button onClick={() => setDialog("disable")} disabled={running}>
            <Unplug className="size-4" /> Disable
          </Button>
        </span>
      ) : (
        <span title={blocked}>
          <Button variant="primary" onClick={() => setDialog("enable")} disabled={!!blocked || !e}>
            <Plug className="size-4" /> Enable
          </Button>
        </span>
      )}
    </div>
  );

  return (
    <Panel title="Docker API" description="dockerd's Engine API over mutual TLS, for the control plane." actions={ep.error ? undefined : actions}>
      {ep.isLoading ? (
        <Skeleton rows={2} />
      ) : ep.error ? (
        <div className="px-4 py-3">
          <ErrorNote error={ep.error} />
        </div>
      ) : e ? (
        <EndpointFacts e={e} />
      ) : null}
      {jobId && (
        <div className="space-y-2 border-t border-line px-4 py-3">
          <div className="flex items-center gap-2 text-xs">
            <span className="text-muted">Job</span>
            <Link to="/jobs" className="font-mono text-[11px] text-accent hover:underline" title={jobId}>
              {short(jobId)}
            </Link>
            <Status value={job.data?.status ?? "queued"} />
          </div>
          <JobLog jobId={jobId} height="h-40" />
        </div>
      )}
      {dialog === "enable" && <EnableDialog node={node} onClose={() => setDialog(null)} onStarted={setJobId} />}
      {dialog === "disable" && <DisableDialog node={node} host={e?.host} onClose={() => setDialog(null)} onStarted={setJobId} />}
    </Panel>
  );
}

function EndpointFacts({ e }: { e: DockerEndpoint }) {
  const addr = e.host ? `${e.host}:${e.port ?? DEFAULT_PORT}` : "";
  const fp = e.ca_fingerprint ?? "";
  const expires = e.daemon_cert_not_after;
  const expired = !!expires && expires < Date.now();
  return (
    <div className="space-y-2 px-4 py-3">
      <dl className="grid grid-cols-[repeat(auto-fill,minmax(160px,1fr))] gap-x-4 gap-y-2 text-xs">
        <Fact k="State">
          <Status value={e.state} />
        </Fact>
        <Fact k="Address">
          <CopyValue text={addr} label="Address" />
        </Fact>
        <Fact k="Engine / API">
          <span className="font-mono text-[11px] tabular-nums">
            {e.engine_version ?? "—"} / {e.api_version ?? "—"}
          </span>
        </Fact>
        <Fact k="CA fingerprint (SHA-256)">
          <CopyValue text={fp} label="CA fingerprint" className="max-w-[12rem]" />
        </Fact>
        <Fact k="Cert expires">
          {expires ? (
            <span title={time(expires)} className={expired ? "text-bad" : ""}>
              {expired ? `expired ${ago(expires)}` : time(expires)}
            </span>
          ) : (
            <span className="text-faint">—</span>
          )}
        </Fact>
        <Fact k="Updated">
          <span title={time(e.updated_at)}>{e.updated_at ? ago(e.updated_at) : "—"}</span>
        </Fact>
      </dl>
      {e.last_error && (
        <p className="truncate font-mono text-[11px] text-bad select-text" title={e.last_error}>
          {e.last_error}
        </p>
      )}
      {e.state === "disabled" && <p className="text-xs text-faint">Off. Enable it to let Forge call this node's Docker Engine API directly.</p>}
    </div>
  );
}

function Fact({ k, children }: { k: string; children: ReactNode }) {
  return (
    <div className="min-w-0">
      <dt className="text-faint">{k}</dt>
      <dd className="mt-0.5 min-w-0 truncate">{children}</dd>
    </div>
  );
}

type Started = (jobId: string) => void;

function EnableDialog({ node, onClose, onStarted }: { node: Server; onClose: () => void; onStarted: Started }) {
  const toast = useToast();
  const enable = useEnableDockerEndpoint(node.id);
  const [form, setForm] = useState({ host: "", port: String(DEFAULT_PORT), listen: "", ack: false });
  const port = Number(form.port);
  const portOk = Number.isInteger(port) && port >= 1024 && port <= 65535;
  const submit = (ev: FormEvent) => {
    ev.preventDefault();
    enable.mutate(
      { host: form.host.trim() || undefined, port, listen: form.listen.trim() || undefined, acknowledge_restart: form.ack },
      {
        onSuccess: (r) => {
          toast("Enabling the Docker API");
          onStarted(r.job_id);
          onClose();
        },
        onError: (err) => toast(errorText(err), "error"),
      },
    );
  };
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title="Enable the Docker API" description={`Expose dockerd on ${nodeName(node)} over mutual TLS.`}>
      <form onSubmit={submit} className="space-y-3">
        <div className="rounded-md border border-line px-3 py-2 text-xs text-muted">
          <p className="mb-1 font-medium text-fg">What happens</p>
          <ol className="list-decimal space-y-0.5 pl-4">
            <li>Forge issues a CA and certificates for dockerd and the control plane.</li>
            <li>The agent writes dockerd's TLS settings and restarts Docker.</li>
            <li>Without live-restore, running containers stop briefly; those with a restart policy start again.</li>
            <li>Forge connects and records the engine and API versions.</li>
          </ol>
        </div>
        <div className="grid grid-cols-[minmax(0,1fr)_7rem] gap-3">
          <Field label="Host" hint="Blank = the address the agent connects from.">
            <Input value={form.host} onChange={(ev) => setForm({ ...form, host: ev.target.value })} placeholder="address the agent connects from" />
          </Field>
          <Field label="Port" hint={portOk ? undefined : "1024–65535"}>
            <Input type="number" min={1024} max={65535} value={form.port} onChange={(ev) => setForm({ ...form, port: ev.target.value })} required />
          </Field>
        </div>
        <Field label="Listen address (optional)" hint="Address dockerd binds; by default the host, when it is one of the node's addresses.">
          <Input value={form.listen} onChange={(ev) => setForm({ ...form, listen: ev.target.value })} placeholder="0.0.0.0" />
        </Field>
        <label className="flex items-start gap-2 text-[13px]">
          <input type="checkbox" className="mt-0.5" checked={form.ack} onChange={(ev) => setForm({ ...form, ack: ev.target.checked })} />
          <span>Restart Docker now; running containers stop briefly.</span>
        </label>
        <div className="flex justify-end gap-2 pt-1">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <span title={form.ack ? undefined : "Confirm the Docker restart first"}>
            <Button type="submit" variant="primary" disabled={!form.ack || !portOk || enable.isPending}>
              {enable.isPending && <Loader2 className="size-4 animate-spin" />} Enable
            </Button>
          </span>
        </div>
      </form>
    </Modal>
  );
}

function DisableDialog({ node, host, onClose, onStarted }: { node: Server; host?: string; onClose: () => void; onStarted: Started }) {
  const toast = useToast();
  const disable = useDisableDockerEndpoint(node.id);
  const [force, setForce] = useState(false);
  return (
    <Confirm
      open
      onOpenChange={(o) => !o && onClose()}
      title="Disable the Docker API"
      confirmLabel="Disable"
      busy={disable.isPending}
      body={
        <div className="space-y-3 text-[13px]">
          <p>
            Removes dockerd's TCP listener{host ? ` on ${host}` : ""} and restarts Docker on {nodeName(node)}. Running containers stop briefly unless
            live-restore is on. Forge can no longer call this node's Engine API directly.
          </p>
          <label className="flex items-start gap-2">
            <input type="checkbox" className="mt-0.5" checked={force} onChange={(ev) => setForce(ev.target.checked)} />
            <span>
              Forget without asking the node
              <span className="block text-xs text-faint">Only clears Forge's record; dockerd keeps listening until you change it on the node.</span>
            </span>
          </label>
        </div>
      }
      onConfirm={() =>
        disable.mutate(force, {
          onSuccess: (r) => {
            toast("Disabling the Docker API");
            onStarted(r.job_id);
            onClose();
          },
          onError: (err) => toast(errorText(err), "error"),
        })
      }
    />
  );
}
