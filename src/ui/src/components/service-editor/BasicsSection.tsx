import { Link } from "@tanstack/react-router";
import { Plus, ServerCog } from "lucide-react";

import type { Cluster, Environment, Project, Server, Service } from "../../api/types";
import { nodeName } from "../../api/types";
import { Badge, Button, Input, Select } from "../ui";
import { FormField, Grid } from "./parts";
import { NAME_RE } from "./sections";

/** The create form's Basics: one object, owned by the page. */
export interface CreateBasicsValue {
  project: string;
  environment: string;
  name: string;
  /** Nodes to run on after creating; empty means "choose later". */
  nodes: string[];
}

/**
 * Basics on create: project → environment (each with a **New** button), the
 * name, and the nodes to run on. Environments follow the chosen project.
 */
export function CreateBasics({
  value,
  onChange,
  projects,
  environments,
  clusters,
  servers,
  onNewProject,
  onNewEnvironment,
  swarm = false,
}: {
  value: CreateBasicsValue;
  onChange: (v: CreateBasicsValue) => void;
  projects: Project[];
  environments: Environment[];
  clusters: Cluster[];
  servers: Server[];
  onNewProject: () => void;
  onNewEnvironment: () => void;
  /** The environment's cluster runs Docker Swarm: the swarm places tasks, so no node picker. */
  swarm?: boolean;
}) {
  const set = (p: Partial<CreateBasicsValue>) => onChange({ ...value, ...p });
  const inProject = environments.filter((n) => (value.project ? n.project_id === value.project : false));
  const env = environments.find((n) => n.id === value.environment);
  const cluster = clusters.find((c) => c.id === env?.cluster_id);
  const nameBad = !!value.name && !NAME_RE.test(value.name);
  // Nodes of the environment's cluster first; without a cluster, every node.
  const candidates = servers.filter((s) => s.status !== "revoked" && (!env?.cluster_id || s.cluster_id === env.cluster_id));
  const toggleNode = (id: string) => set({ nodes: value.nodes.includes(id) ? value.nodes.filter((x) => x !== id) : [...value.nodes, id] });
  return (
    <Grid>
      <FormField label="Project" hint="The product this service is part of.">
        {(id) => (
          <div className="flex items-center gap-1.5">
            <div className="min-w-0 flex-1">
              <Select id={id} required value={value.project} onChange={(e) => set({ project: e.target.value, environment: environments.find((n) => n.project_id === e.target.value)?.id ?? "", nodes: [] })}>
                <option value="">{projects.length ? "Select a project" : "No projects yet"}</option>
                {projects.map((g) => (
                  <option key={g.id} value={g.id}>
                    {g.name}
                  </option>
                ))}
              </Select>
            </div>
            <Button type="button" onClick={onNewProject} title="New project" aria-label="New project">
              <Plus className="size-4" />
            </Button>
          </div>
        )}
      </FormField>
      <FormField
        label="Environment"
        hint={
          !value.project ? (
            "Pick a project first."
          ) : inProject.length === 0 ? (
            <span className="text-warn">This project has no environment yet. Create one with +.</span>
          ) : cluster ? (
            <>
              → cluster{" "}
              <Link to="/clusters/$clusterId" params={{ clusterId: cluster.id }} className="text-accent hover:underline">
                {cluster.name}
              </Link>
            </>
          ) : (
            "Cluster: —"
          )
        }
      >
        {(id) => (
          <div className="flex items-center gap-1.5">
            <div className="min-w-0 flex-1">
              <Select id={id} required disabled={!value.project} value={value.environment} onChange={(e) => set({ environment: e.target.value, nodes: [] })}>
                <option value="">{inProject.length ? "Select an environment" : "No environments"}</option>
                {inProject.map((n) => {
                  const cl = clusters.find((c) => c.id === n.cluster_id);
                  return (
                    <option key={n.id} value={n.id}>
                      {n.name}
                      {cl ? ` · ${cl.name}` : ""}
                    </option>
                  );
                })}
              </Select>
            </div>
            <Button type="button" disabled={!value.project} onClick={onNewEnvironment} title={value.project ? "New environment" : "Pick a project first"} aria-label="New environment">
              <Plus className="size-4" />
            </Button>
          </div>
        )}
      </FormField>
      <FormField label="Name" error={nameBad ? "Lower-case letters, digits and dashes; start with a letter; up to 40 characters." : undefined} hint="Unique in the environment, for example orders-api.">
        {(id, invalid) => (
          <Input
            id={id}
            required
            maxLength={40}
            aria-invalid={invalid || undefined}
            className={`font-mono ${invalid ? "border-bad" : ""}`}
            value={value.name}
            onChange={(e) => set({ name: e.target.value.toLowerCase() })}
            placeholder="orders-api"
            spellCheck={false}
          />
        )}
      </FormField>
      <div className="min-w-0 space-y-1 md:col-span-2">
        <p className="text-xs font-medium text-muted">Placement</p>
        {swarm ? (
          <p className="text-xs text-faint">The cluster runs Docker Swarm: the swarm places the tasks. Set replicas and constraints under Swarm.</p>
        ) : !env ? (
          <p className="text-xs text-faint">Choose an environment to see its nodes.</p>
        ) : candidates.length === 0 ? (
          <p className="text-xs text-faint">{cluster ? `${cluster.name} has no nodes yet.` : "No nodes yet."} You can place the service after creating it.</p>
        ) : (
          <div className="flex flex-wrap gap-1.5" role="group" aria-label="Nodes to run on">
            {candidates.map((s) => {
              const on = value.nodes.includes(s.id);
              const i = value.nodes.indexOf(s.id);
              return (
                <button
                  key={s.id}
                  type="button"
                  aria-pressed={on}
                  onClick={() => toggleNode(s.id)}
                  title={on ? `Runs on ${nodeName(s)} as ${i === 0 ? "a replica" : "a replica (change roles on the service page)"}` : `Run on ${nodeName(s)}`}
                  className={`inline-flex max-w-[220px] items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-xs ${on ? "border-accent bg-accent-soft text-accent" : "border-line text-muted hover:text-fg"}`}
                >
                  <span className={`size-1.5 shrink-0 rounded-full ${s.status === "online" ? "bg-ok" : "bg-faint"}`} aria-hidden />
                  <span className="truncate">{nodeName(s)}</span>
                </button>
              );
            })}
          </div>
        )}
        {!swarm && <p className="text-xs text-faint">Optional. Every chosen node runs a replica; add standbys and change roles on the service page.</p>}
      </div>
    </Grid>
  );
}

/** Basics on edit: read-only identity and where it runs, every name a link. */
export function EditBasics({ service, environment, project, cluster, nodeNameOf }: { service: Service; environment?: Environment; project?: Project; cluster?: Cluster; nodeNameOf: (id: string) => string }) {
  const placed = service.instances;
  return (
    <dl className="grid grid-cols-[120px_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-[13px]">
      <dt className="text-muted">Name</dt>
      <dd className="min-w-0 truncate font-mono text-xs" title="Names are fixed: they are part of the unit, container and file names on the nodes.">
        {service.name}
      </dd>
      <dt className="text-muted">Lives in</dt>
      <dd className="flex min-w-0 flex-wrap items-center gap-1 text-xs">
        {project ? (
          <Link to="/projects" search={{ sel: `project:${project.id}` }} className="truncate hover:text-accent">
            {project.name}
          </Link>
        ) : (
          <span className="text-faint">no project</span>
        )}
        <span className="text-faint">/</span>
        {environment ? (
          <Link to="/projects" search={{ sel: `env:${environment.id}` }} className="truncate hover:text-accent">
            {environment.name}
          </Link>
        ) : (
          <span className="text-faint">—</span>
        )}
        <span className="text-faint">→</span>
        {cluster ? (
          <Link to="/clusters/$clusterId" params={{ clusterId: cluster.id }} className="inline-flex min-w-0 items-center gap-1 truncate hover:text-accent">
            <ServerCog className="size-3.5 shrink-0" aria-hidden /> {cluster.name}
          </Link>
        ) : (
          <span className="text-faint">no cluster</span>
        )}
      </dd>
      <dt className="text-muted">Placement</dt>
      <dd className="flex min-w-0 flex-wrap items-center gap-1 text-xs">
        {placed.length === 0 ? (
          <span className="text-faint">Not placed on any node.</span>
        ) : (
          placed.map((i) => (
            <Link key={i.server_id} to="/nodes/$nodeId" params={{ nodeId: i.server_id }} className="inline-flex items-center gap-1 rounded border border-line px-1.5 hover:border-accent hover:text-accent">
              {nodeNameOf(i.server_id)}
              {i.role === "standby" && <Badge>standby</Badge>}
            </Link>
          ))
        )}
        <Link to="/services/$serviceId" params={{ serviceId: service.id }} className="text-accent hover:underline">
          Change on Overview
        </Link>
      </dd>
    </dl>
  );
}
