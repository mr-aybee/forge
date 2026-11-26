import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { ChevronRight, Container, History, Database, Globe, HeartPulse, LayoutDashboard, Loader2, Rocket, Settings2, Trash2, Waypoints, Webhook } from "lucide-react";
import { useEffect, useState } from "react";

import { ApiError, get } from "../api/client";

import {
  useAllEnvironments,
  useClusters,
  useProjects,
  useServiceAction,
  type ServiceAction,
  useServiceHealth,
  useService,
  useDeleteService,
  useDeployService,
  usePlaceService,
  useServers,
  useSaveService,
} from "../api/hooks";
import { useStack } from "../api/stacks";
import type { ServiceInput, Service, Environment, Server } from "../api/types";
import type { AppTab } from "../app/router";
import { errorText, useToast } from "../app/toast";
import { useTerminals } from "../app/terminals";
import { EditBasics } from "../components/service-editor/BasicsSection";
import { ServiceEditor } from "../components/service-editor/ServiceEditor";
import { RUNTIMES, allIssues, cleanInput, forSwarm, sectionDomId, serviceFacts, toInput } from "../components/service-editor/sections";
import { HealthTab } from "../components/health/HealthTab";
import { HealthWarnings } from "../components/health/InstanceHealth";
import { IpTags, leaseOf, useClusterFabric } from "../components/IpTag";
import { JobDetail } from "../components/JobsDrawer";
import { InstanceStateChip, liveState } from "../components/InstanceState";
import { EndpointDialog, type EndpointInitial } from "../components/ingress/EndpointDialog";
import { ServiceRoutes } from "../components/ingress/ServiceRoutes";
import type { ProfileResponse } from "../components/ingress/types";
import { PlacementDialog } from "../components/PlacementDialog";
import { PinToggle } from "../components/Pins";
import { Button, Confirm, Drawer, Empty, ErrorNote, RowMenu, Modal, PageCrumb, PageHeader, Panel, Skeleton, Status, Table, Td, UnsavedBar, ago, short } from "../components/ui";
import { TagEditor } from "../components/TagEditor";
import { AppStorageTab } from "../components/storage/AppStorageTab";
import { ReleasesTab } from "../components/deploy/ReleasesTab";
import { WebhooksTab } from "../components/deploy/WebhooksTab";
import { LogIngestKeys, OpenLogStream } from "../components/logs/LogIngestKeys";
import { ServiceNodes } from "../components/service/ServiceNodes";
import { ServiceSwarm } from "../components/service/ServiceSwarm";
import { useServiceSwarmAction } from "../api/swarm";
import { ServiceStatusStrip } from "../components/service/ServiceStatusStrip";
import { ServiceSummary } from "../components/service/ServiceSummary";
import { ServicePorts } from "../components/tunnels/ServicePorts";
import { StackBadge, isStackService } from "../components/stacks/StackBadge";

/**
 * The port an ingress endpoint should forward to: `listen_port` for native
 * services, the host side of the first published port for containers, else 0
 * (the editor then asks for it).
 */
const ingressPort = (a: Service): number => {
  if (a.runtime !== "container") return a.spec.listen_port ?? 0;
  const first = Array.isArray(a.spec.ports) ? a.spec.ports[0] : undefined;
  const text = typeof first === "string" ? first : "";
  // "8080:80", "0.0.0.0:8080:80/tcp", "8080" → 8080
  const parts = text.split("/")[0]?.split(":") ?? [];
  const host = parts.length >= 2 ? parts[parts.length - 2] : parts[0];
  return Number(host) || 0;
};

/** Docker name of a Forge container service on a node. */
const containerName = (a: Pick<Service, "name" | "spec">) => serviceFacts(a.spec?.environment ?? "", a.name).container;

/** "container-name" link to the container page on `nodeId`. */
function ContainerLink({ nodeId, name, className = "" }: { nodeId: string; name: string; className?: string }) {
  return (
    <Link
      to="/nodes/$nodeId/containers/$containerId"
      params={{ nodeId, containerId: name }}
      className={`inline-flex min-w-0 items-center gap-1 font-mono text-[11px] text-muted hover:text-accent ${className}`}
      title={`Open container ${name}`}
    >
      <Container className="size-3 shrink-0" />
      <span className="truncate">{name}</span>
    </Link>
  );
}

/** `/services/$serviceId`; keyed on the id so a draft never carries over to another service. */
export function ServiceDetail() {
  const { serviceId } = useParams({ from: "/services/$serviceId" });
  return <ServicePage key={serviceId} appId={serviceId} />;
}

function ServicePage({ appId }: { appId: string }) {
  const { tab: tabParam } = useSearch({ from: "/services/$serviceId" });
  const app = useService(appId);
  const clusters = useClusters().data ?? [];
  const projects = useProjects().data ?? [];
  const nodes = useServers().data ?? [];
  const environments = useAllEnvironments().data ?? [];
  const save = useSaveService();
  const place = usePlaceService();
  const deploy = useDeployService();
  const act = useServiceAction();
  const del = useDeleteService();
  const toast = useToast();
  const navigate = useNavigate();
  // `undefined` until the first edit: the form shows the saved service, so there is nothing to sync.
  const [draft, setDraft] = useState<ServiceInput>();
  const [saving, setSaving] = useState<"save" | "deploy" | null>(null);
  const [logs, setLogs] = useState<{ host: string; text: string }>();
  const [job, setJob] = useState<string>();
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [removing, setRemoving] = useState<string>();
  // Node whose last deployment job is shown in the drawer.
  const [deployOut, setDeployOut] = useState<string>();
  const terminals = useTerminals();
  // "Run <service> on <node>": role choice for a second or later node.
  const [placing, setPlacing] = useState<Server>();
  // "Expose through ingress": the endpoint editor pre-filled with this service.
  const [expose, setExpose] = useState(false);
  // The tab lives in the URL (`?tab=`), so links and reloads land on the right one.
  const tab: AppTab = tabParam ?? "overview";
  const setTab = (t: AppTab) => void navigate({ to: "/services/$serviceId", params: { serviceId: appId }, search: { tab: t === "overview" ? undefined : t }, replace: true });
  // The Health Checks tab stays mounted once opened, so its draft survives tab switches.
  const [healthSeen, setHealthSeen] = useState(false);
  const [healthDirty, setHealthDirty] = useState(false);
  useEffect(() => {
    if (tab === "health") setHealthSeen(true);
  }, [tab]);
  const a = app.data;
  const ns0 = environments.find((n) => n.id === a?.environment_id);
  const fabric = useClusterFabric(ns0?.cluster_id).data;
  // Per-node health; hidden entirely when the server has no health endpoint (404) or denies it.
  const health = useServiceHealth(a && !isStackService(a) ? a.id : undefined, { live: true });
  const healthOn = !!health.data;
  // The environment's cluster ingress profile: its members feed the endpoint
  // editor, and without an ingress profile the server refuses new endpoints.
  const profile = useQuery({
    queryKey: ["cluster-profile", ns0?.cluster_id],
    queryFn: () => get<ProfileResponse>(`/api/v1/clusters/${ns0?.cluster_id}/profile`),
    enabled: !!ns0?.cluster_id,
    retry: false,
  });

  // Services in a Swarm cluster: the swarm places the tasks, so no node instances.
  const swarm = clusters.find((c) => c.id === ns0?.cluster_id)?.mode === "swarm";
  const swarmAct = useServiceSwarmAction(appId);
  const saved = a && !isStackService(a) ? forSwarm(toInput(a), swarm) : undefined;
  const form = draft ?? saved;
  const dirty = !!draft && !!saved && JSON.stringify(saved) !== JSON.stringify(draft);

  if (app.isLoading) return <Skeleton rows={6} />;
  if (!a || !form) {
    return (
      <>
        <PageCrumb back={<Link to="/services">Services</Link>} current={short(appId)} />
        {app.error && !(app.error instanceof ApiError && app.error.status === 404) ? (
          <ErrorNote error={app.error} />
        ) : (
          <Empty title="Service not found" action={<Link to="/services"><Button>All services</Button></Link>}>
            It was deleted, or you cannot view it.
          </Empty>
        )}
      </>
    );
  }
  if (isStackService(a)) return <StackServiceDetail a={a} ns={environments.find((n) => n.id === a.environment_id)} hostOf={(id) => nodes.find((n) => n.id === id)?.hostname ?? id.slice(0, 8)} />;
  const rt = RUNTIMES.find((r) => r.id === a.runtime);
  const ns = environments.find((n) => n.id === a.environment_id);
  const facts = serviceFacts(a.spec.environment ?? ns?.name ?? "", a.name);
  const hostOf = (id: string) => nodes.find((n) => n.id === id)?.hostname ?? id.slice(0, 8);
  const openLogs = (serverId: string) => terminals.openLogs({ source: "service", appId: a.id, serverId, name: a.name, node: hostOf(serverId) });

  const setPlacement = (server: string, role: string | null, onDone?: () => void) => {
    const list: { server_id: string; role: string }[] = a.instances
      .filter((i) => i.server_id !== server)
      .map((i) => ({ server_id: i.server_id, role: i.role }));
    if (role) list.push({ server_id: server, role });
    place.mutate({ id: a.id, placements: list }, { onSuccess: onDone, onError: (e) => toast(errorText(e), "error") });
  };
  const replicas = a.instances.filter((i) => i.role !== "standby").length;
  const deployedSomewhere = a.instances.some((i) => i.status !== "pending");
  const deployNow = () =>
    swarm
      ? swarmAct.mutate({ kind: "deploy" }, { onSuccess: () => toast("Deploying to the swarm"), onError: (e) => toast(errorText(e), "error") })
      : deploy.mutate(a.id, { onSuccess: (jobs) => toast(`Deploying to ${jobs.length} node${jobs.length === 1 ? "" : "s"}`), onError: (e) => toast(errorText(e), "error") });
  const deployBusy = deploy.isPending || swarmAct.isPending;
  const deployWhyHeader = dirty ? "Save changes first" : !swarm && a.instances.length === 0 ? "Choose nodes first" : undefined;
  const clusterId = ns?.cluster_id ?? undefined;
  const noProfile = !!clusterId && profile.isSuccess && !profile.data?.profile?.ingress;
  const exposeWhy = !clusterId
    ? "This environment has no cluster; the endpoint needs hand-picked edge nodes"
    : noProfile
      ? "The cluster has no ingress profile yet (Ingress → Profile)"
      : undefined;
  const exposeInitial = (): EndpointInitial => {
    const port = ingressPort(a);
    return { name: a.name, targets: [{ kind: "app", service_id: a.id, port }], balance: "round_robin", hideBalance: replicas < 2, focusPort: !port };
  };
  const openIngress = () => (clusterId ? void navigate({ to: "/ingress", search: { cluster: clusterId } }) : void navigate({ to: "/ingress" }));

  const run = (server: string, action: ServiceAction) =>
    act.mutate(
      { id: a.id, server, action, tail: 500 },
      {
        onSuccess: (r) => {
          setJob(r.job_id);
          if (r.status === "failed") toast(r.error ?? `${action} failed`, "error");
          else if (action === "logs" && r.result) setLogs({ host: hostOf(server), text: r.result.logs ?? "" });
          else toast(r.status === "running" ? `${action} started on ${hostOf(server)}` : `${action} done on ${hostOf(server)}`);
        },
        onError: (e) => toast(errorText(e), "error"),
      },
    );

  return (
    <>
      <ServiceCrumbs ns={ns} />
      <PageHeader
        eyebrow="Service"
        title={a.name}
        description={`${rt?.label ?? a.runtime} · updated ${ago(a.updated_at)}`}
        actions={
          <>
            <PinToggle kind="service" id={a.id} label={a.name} />
            <OpenLogStream appId={a.id} />
            <Button
              variant="primary"
              disabled={deployBusy || !!deployWhyHeader}
              title={deployWhyHeader ?? (swarm ? "Create or update the Docker service in the swarm" : "Deploy to every node")}
              onClick={deployNow}
            >
              {deployBusy ? <Loader2 className="size-4 animate-spin" /> : <Rocket className="size-4" />} Deploy
            </Button>
            <RowMenu
              label="More actions"
              actions={[
                { label: "Expose through ingress", icon: Globe, tone: "accent", disabled: noProfile, hint: exposeWhy, onSelect: () => setExpose(true) },
                { label: "Open Ingress", icon: Waypoints, tone: "accent", onSelect: openIngress },
                {
                  label: "Delete service",
                  icon: Trash2,
                  danger: true,
                  disabled: deployedSomewhere,
                  hint: deployedSomewhere ? "Remove it from its nodes first (node ⋯ → Remove from node)" : swarm ? "Remove it from the swarm first (Swarm → Remove)" : undefined,
                  onSelect: () => setConfirmDelete(true),
                },
              ]}
            />
          </>
        }
      />
      <div className="-mt-2 mb-4">
        <TagEditor kind="service" id={a.id} />
      </div>
      <IpTags
        tags={[
          { label: "Service IP", value: leaseOf(fabric, "service", a.id) },
          { label: "Name", value: fabric?.spec && ns ? `${a.name}.${ns.name}.${fabric.spec.domain}` : undefined, title: "Internal DNS name (copy)" },
          { label: "Short name", value: ns ? `${a.name}.${ns.name}` : undefined },
        ]}
      />
      <div className="mb-4 flex gap-1 overflow-x-auto overflow-y-hidden border-b border-line">
        {(
          [
            ["overview", "Overview", LayoutDashboard],
            ["configuration", "Configuration", Settings2],
            ["health", "Health Checks", HeartPulse],
            ["storage", "Storage", Database],
            ["releases", "Releases", History],
            ["webhooks", "Webhooks", Webhook],
          ] as const
        ).map(([id, label, Icon]) => (
          <button key={id} onClick={() => setTab(id)} className={`press -mb-px inline-flex shrink-0 items-center gap-1.5 border-b-2 px-3 py-2 text-[13px] whitespace-nowrap focus-visible:outline-2 focus-visible:outline-accent ${tab === id ? "border-accent text-accent" : "border-transparent text-muted hover:text-fg"}`}>
            <Icon className="size-4" /> {label}
            {((id === "configuration" && dirty) || (id === "health" && healthDirty)) && <span className="size-1.5 rounded-full bg-warn" title="Unsaved changes" />}
          </button>
        ))}
      </div>
      {tab !== "health" && <HealthWarnings warnings={health.data?.warnings} hostOf={hostOf} />}
      {healthSeen && (
        <div hidden={tab !== "health"} className={tab === "health" ? "view-enter" : undefined}>
          <HealthTab app={a} onDirtyChange={setHealthDirty} />
        </div>
      )}
      <div key={tab} className="view-enter" hidden={tab === "health"}>
      {tab === "health" ? null : tab === "releases" ? (
        <ReleasesTab app={a} swarm={swarm} />
      ) : tab === "webhooks" ? (
        <WebhooksTab target={{ kind: "service", id: a.id }} targetName={a.name} />
      ) : tab === "storage" ? (
        <AppStorageTab app={a} />
      ) : tab === "overview" ? (
        <div className="space-y-3">
          <ServiceStatusStrip a={a} nodes={nodes} health={health.data?.instances} onLogKeys={() => document.getElementById("log-ingest-keys")?.scrollIntoView({ behavior: "smooth", block: "center" })} />
          <div className="grid items-start gap-3 xl:grid-cols-[minmax(0,7fr)_minmax(0,5fr)]">
            {swarm ? (
              <ServiceSwarm
                a={a}
                onEditSettings={() => {
                  setTab("configuration");
                  // The editor mounts on the next render; then scroll to its Swarm section.
                  window.setTimeout(() => document.getElementById(sectionDomId("swarm"))?.scrollIntoView({ behavior: "smooth", block: "start" }), 60);
                }}
              />
            ) : (
            <Panel title={`Nodes · ${a.instances.length}`}>
              <ServiceNodes
                a={a}
                nodes={nodes}
                health={healthOn ? health.data?.instances : undefined}
                containerName={a.runtime === "container" ? containerName(a) : undefined}
                onLogs={openLogs}
                onAction={run}
                onRole={(server, role) => setPlacement(server, role, () => toast(`${hostOf(server)} is now a ${role === "standby" ? "standby" : "replica"}. Apply the endpoint again to update Nginx.`))}
                onRemove={(i) => (i.status === "pending" ? setPlacement(i.server_id, null) : setRemoving(i.server_id))}
                onDeployOutput={setDeployOut}
                onAdd={(n) => (a.instances.length ? setPlacing(n) : setPlacement(n.id, "active"))}
              />
            </Panel>
            )}
            <Panel title="Summary">
              <ServiceSummary a={a} environmentName={ns?.name} nodeName={hostOf} />
            </Panel>
          </div>
          {job && (
            <Panel title="Last action" actions={<button type="button" className="text-xs text-muted hover:text-fg" onClick={() => setJob(undefined)}>Dismiss</button>}>
              <div className="px-3 py-2">
                <JobDetail id={job} />
              </div>
            </Panel>
          )}
          <div className="grid items-start gap-3 xl:grid-cols-[minmax(0,7fr)_minmax(0,5fr)]">
            <div>
              <ServiceRoutes app={a} onExpose={noProfile ? undefined : () => setExpose(true)} className="bg-panel" />
              {noProfile && <p className="mt-1 px-1 text-[11px] text-faint">{exposeWhy}</p>}
            </div>
            <div id="log-ingest-keys" className="scroll-mt-4">
              <LogIngestKeys appId={a.id} appName={a.name} />
            </div>
          </div>
          <ServicePorts kind="app" service={a.name} />
        </div>
      ) : (
        <ConfigurationTab
          a={a}
          form={form}
          dirty={dirty}
          onChange={setDraft}
          onDiscard={() => setDraft(undefined)}
          saving={saving}
          swarm={swarm}
          basics={<EditBasics service={a} environment={ns} project={projects.find((g) => g.id === ns?.project_id)} cluster={clusters.find((c) => c.id === ns?.cluster_id)} nodeNameOf={hostOf} />}
          facts={facts}
          onSave={async (andDeploy) => {
            setSaving(andDeploy ? "deploy" : "save");
            try {
              await save.mutateAsync({ id: a.id, ...cleanInput(form) });
              setDraft(undefined);
              if (andDeploy && swarm) {
                await swarmAct.mutateAsync({ kind: "deploy" });
                toast("Saved; deploying to the swarm");
              } else if (andDeploy) {
                const jobs = await deploy.mutateAsync(a.id);
                toast(`Saved; deploying to ${jobs.length} node${jobs.length === 1 ? "" : "s"}`);
              } else toast("Saved. Deploy applies it to the nodes.");
            } catch (e) {
              toast(errorText(e), "error");
            } finally {
              setSaving(null);
            }
          }}
        />
      )}
      </div>
      <Modal open={!!logs} onOpenChange={(o) => !o && setLogs(undefined)} title={`Logs · ${a.name} on ${logs?.host}`} wide>
        <pre className="log-console h-[60vh] overflow-auto rounded-lg bg-term p-3 font-mono text-[11.5px] leading-relaxed text-term-fg">{logs?.text || "No output yet."}</pre>
      </Modal>
      {deployOut &&
        (() => {
          const inst = a.instances.find((x) => x.server_id === deployOut);
          return (
            <Drawer open onOpenChange={(o) => !o && setDeployOut(undefined)} title={`Deployment output · ${a.name} on ${hostOf(deployOut)}`} wide>
              {inst?.last_job_id ? (
                <div className="space-y-2">
                  <p className="text-xs text-muted">
                    The last job on this node ({inst.status}); output streams while it runs: installing runtimes, writing the service, starting it.
                  </p>
                  <JobDetail id={inst.last_job_id} />
                </div>
              ) : (
                <p className="text-sm text-muted">Nothing has run on this node yet. Deploy the service to see its output here.</p>
              )}
            </Drawer>
          );
        })()}
      {placing && (
        <PlacementDialog
          app={a}
          node={placing}
          busy={place.isPending}
          onClose={() => setPlacing(undefined)}
          onConfirm={(role) =>
            setPlacement(placing.id, role, () => {
              toast(`${a.name} runs on ${placing.hostname} as a ${role === "standby" ? "standby" : "replica"} after the next Deploy`);
              setPlacing(undefined);
            })
          }
        />
      )}
      {expose && (
        <EndpointDialog
          clusterId={clusterId}
          members={profile.data?.members ?? []}
          initial={exposeInitial()}
          onClose={() => setExpose(false)}
          onSaved={(s) => toast(`Endpoint ${s.name} saved. Apply it to the edges on the Ingress page.`, "ok", { label: "Open Ingress", onSelect: openIngress })}
        />
      )}
      <Confirm
        open={!!removing}
        onOpenChange={(o) => !o && setRemoving(undefined)}
        title={`Remove ${a.name} from ${removing ? hostOf(removing) : ""}?`}
        body="Stops it and deletes its unit/container and environment file on the node. Code and data under /srv/forge/apps are kept."
        confirmLabel="Remove"
        onConfirm={() => {
          if (removing) run(removing, "remove");
          setRemoving(undefined);
        }}
      />
      <Confirm
        open={confirmDelete}
        onOpenChange={setConfirmDelete}
        title={`Delete ${a.name}?`}
        body={a.instances.some((i) => i.status !== "pending") ? "Remove it from its nodes first." : "Deletes the definition and its sealed secrets."}
        confirmLabel="Delete"
        onConfirm={() =>
          del.mutate(a.id, {
            onSuccess: () => void navigate({ to: "/services" }),
            onError: (e) => toast(errorText(e), "error"),
          })
        }
      />
    </>
  );
}


/** A service registered from a compose stack: read-only; changes go through its stack. */
function StackServiceDetail({ a, ns, hostOf }: { a: Service; ns?: Environment; hostOf: (id: string) => string }) {
  const instances = Array.isArray(a.instances) ? a.instances : [];
  const nodes = useServers().data ?? [];
  const ports = Array.isArray(a.spec?.ports) ? a.spec.ports.map((p) => (typeof p === "string" ? p : JSON.stringify(p))) : [];
  // Container names come from the stack's last `compose ps` on each node.
  const stack = useStack(a.spec?.stack_id).data;
  const containersOn = (serverId: string): string[] => {
    const n = (stack?.nodes ?? []).find((x) => x.server_id === serverId);
    return (Array.isArray(n?.state) ? n.state : []).filter((x) => x.service === a.spec?.service && x.name).map((x) => x.name);
  };
  return (
    <>
      <ServiceCrumbs ns={ns} />
      <PageHeader
        eyebrow="Service"
        title={a.name}
        description={`Compose service${a.spec?.service ? ` ${a.spec.service}` : ""} · updated ${ago(a.updated_at)}`}
        actions={
          <>
            <PinToggle kind="service" id={a.id} label={a.name} />
            {a.spec?.stack_id && (
              <Link to="/stacks/$stackId" params={{ stackId: a.spec.stack_id }}>
                <Button variant="primary">Open stack</Button>
              </Link>
            )}
          </>
        }
      />
      <div className="mb-4 flex flex-wrap items-center gap-2 rounded-md border border-line px-3 py-2 text-[13px]">
        <StackBadge a={a} />
        <span className="text-muted">This service comes from a compose stack. Edit, deploy, start, stop and remove it on the stack; here it is read-only.</span>
      </div>
      <div className="grid gap-4 xl:grid-cols-12">
        <Panel title="Nodes" className="xl:col-span-7">
          {instances.length === 0 ? (
            <p className="px-4 py-3 text-xs text-muted">Not running on any node.</p>
          ) : (
            <Table head={["Node", "Container", "Deploy", "State"]}>
              {instances.map((i) => (
                <tr key={i.server_id}>
                  <Td>
                    <Link to="/nodes/$nodeId" params={{ nodeId: i.server_id }} className="font-medium hover:text-accent">
                      {hostOf(i.server_id)}
                    </Link>
                  </Td>
                  <Td className="max-w-[260px]">
                    {containersOn(i.server_id).length ? (
                      containersOn(i.server_id).map((name) => <ContainerLink key={name} nodeId={i.server_id} name={name} className="block" />)
                    ) : (
                      <span className="text-xs text-faint" title="No container reported by the stack on this node yet">—</span>
                    )}
                  </Td>
                  <Td>
                    <Status value={i.status === "deployed" ? "succeeded" : i.status === "failed" ? "failed" : i.status?.endsWith("ing") ? "running" : "pending"} />
                    <span className="ml-1.5 text-xs text-muted">{i.status ?? "—"}</span>
                  </Td>
                  <Td><InstanceStateChip s={liveState(i, nodes.find((n) => n.id === i.server_id))} /></Td>
                </tr>
              ))}
            </Table>
          )}
        </Panel>
        <Panel title="Summary" className="xl:col-span-5">
          <dl className="grid grid-cols-[110px_minmax(0,1fr)] gap-y-1.5 px-4 py-3 text-[13px]">
            <dt className="text-muted">Stack</dt>
            <dd className="truncate">{a.spec?.stack ?? "—"}</dd>
            <dt className="text-muted">Service</dt>
            <dd className="truncate font-mono text-xs">{a.spec?.service ?? "—"}</dd>
            <dt className="text-muted">Image</dt>
            <dd className="truncate font-mono text-xs" title={a.spec?.image}>
              {a.spec?.image || "—"}
            </dd>
            <dt className="text-muted">Ports</dt>
            <dd className="font-mono text-xs break-words">{ports.length ? ports.join(", ") : "—"}</dd>
          </dl>
        </Panel>
        <ServicePorts kind="stack" service={a.spec?.stack} only={a.spec?.service} className="xl:col-span-12" />
      </div>
    </>
  );
}

/** `Project / Environment / Services` trail above a service page; every known part is a link. */
function ServiceCrumbs({ ns }: { ns: Environment | undefined }) {
  const projects = useProjects().data ?? [];
  const project = ns?.project_id ? projects.find((g) => g.id === ns.project_id) : undefined;
  const sep = <ChevronRight className="size-3 shrink-0 text-faint" aria-hidden />;
  return (
    <nav aria-label="Breadcrumb" className="mb-2 flex min-w-0 flex-wrap items-center gap-1 text-xs text-muted [&_a:hover]:text-fg">
      {project ? (
        <Link to="/projects" search={{ sel: `project:${project.id}` }} className="truncate" title={`Project ${project.name}`}>
          {project.name}
        </Link>
      ) : (
        <Link to="/projects" className="truncate" title="No project assigned to this environment yet">
          Projects
        </Link>
      )}
      {sep}
      {ns ? (
        <Link to="/projects" search={{ sel: `env:${ns.id}` }} className="truncate" title={`Environment ${ns.name}`}>
          {ns.name}
        </Link>
      ) : (
        <span className="truncate" title="Environment">
          —
        </span>
      )}
      {sep}
      <Link to="/services" className="truncate" title="All services">
        Services
      </Link>
    </nav>
  );
}

/** The Configuration tab: the shared service editor with Save and Save and deploy in the unsaved bar. */
function ConfigurationTab({
  a,
  form,
  dirty,
  onChange,
  onDiscard,
  onSave,
  saving,
  basics,
  facts,
  swarm,
}: {
  a: Service;
  form: ServiceInput;
  dirty: boolean;
  onChange: (v: ServiceInput) => void;
  onDiscard: () => void;
  onSave: (andDeploy: boolean) => void;
  saving: "save" | "deploy" | null;
  basics: React.ReactNode;
  facts: ReturnType<typeof serviceFacts>;
  swarm: boolean;
}) {
  const basicsState = { mode: "edit" as const, environment: a.environment_id, name: a.name };
  const issues = allIssues(form, basicsState);
  const denied = a.can_deploy === false;
  const deployWhy = issues[0] ?? (denied ? "Needs the deploy permission on this environment" : !swarm && a.instances.length === 0 ? "Choose nodes on the Overview tab first" : undefined);
  return (
    <div>
      <ServiceEditor mode="edit" swarm={swarm} environmentId={a.environment_id} value={form} onChange={onChange} basics={basics} basicsState={basicsState} facts={facts} serviceId={a.id} />
      <UnsavedBar
        dirty={dirty}
        summary={issues[0]}
        saveDisabled={issues.length > 0}
        onDiscard={onDiscard}
        saving={saving !== null}
        onSave={() => onSave(false)}
        extra={
          <span title={deployWhy} className="inline-flex">
            <Button type="button" disabled={!!deployWhy || saving !== null} onClick={() => onSave(true)}>
              {saving === "deploy" ? <Loader2 className="size-4 animate-spin" /> : <Rocket className="size-4" />} Save and deploy
            </Button>
          </span>
        }
      />
    </div>
  );
}
