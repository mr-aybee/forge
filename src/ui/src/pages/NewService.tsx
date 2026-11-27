import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import { Loader2, Plus, Rocket } from "lucide-react";
import { useState } from "react";

import { useAllEnvironments, useClusters, useDeployService, usePlaceService, useProjects, useSaveService, useServers } from "../api/hooks";
import type { ServiceInput } from "../api/types";
import { ApiError } from "../api/client";
import { errorText, useToast } from "../app/toast";
import { NewEnvironmentModal } from "../components/EnvironmentDialogs";
import { ProjectModal } from "../components/ProjectDialogs";
import { CreateBasics, type CreateBasicsValue } from "../components/service-editor/BasicsSection";
import { ServiceEditor } from "../components/service-editor/ServiceEditor";
import { allIssues, cleanInput, emptyInput, forSwarm, serviceFacts } from "../components/service-editor/sections";
import { ActionBar, Button, Empty, ErrorNote, PageCrumb, PageHeader, Skeleton } from "../components/ui";

/**
 * New service (`/services/new`): pick the kind, the project and environment,
 * define the service once and optionally its nodes. `?environment=`
 * preselects an environment (and so its project); `?cluster=` limits the
 * environments to one cluster and returns there.
 */
export function NewService() {
  const search = useSearch({ from: "/services/new" });
  const envQ = useAllEnvironments();
  const projectsQ = useProjects();
  const clusters = useClusters().data ?? [];
  const servers = useServers().data ?? [];
  const save = useSaveService();
  const place = usePlaceService();
  const deploy = useDeployService();
  const navigate = useNavigate();
  const toast = useToast();
  const [form, setForm] = useState<{ basics: CreateBasicsValue; input: ServiceInput }>(() => ({
    basics: { project: "", environment: search.environment ?? "", name: "", nodes: [] },
    input: emptyInput("container"),
  }));
  const [dialog, setDialog] = useState<"project" | "env" | null>(null);
  const [pending, setPending] = useState<"create" | "deploy" | null>(null);
  /** The server's validation message of the last create, shown above the editor. */
  const [invalid, setInvalid] = useState("");

  const cluster = clusters.find((c) => c.id === search.cluster);
  const projects = projectsQ.data ?? [];
  const environments = (envQ.data ?? []).filter((n) => !search.cluster || n.cluster_id === search.cluster);
  // A preselected environment decides the project until the user picks one.
  const preset = environments.find((n) => n.id === form.basics.environment);
  const basics: CreateBasicsValue = { ...form.basics, project: form.basics.project || preset?.project_id || "" };
  const env = environments.find((n) => n.id === basics.environment && n.project_id === basics.project);
  const facts = serviceFacts(env?.name ?? "", basics.name);
  const basicsState = { mode: "create" as const, environment: env?.id ?? "", name: basics.name };
  // A Swarm cluster runs containers only, with swarm settings; derived so a changed environment never leaves stale settings.
  const swarm = clusters.find((c) => c.id === env?.cluster_id)?.mode === "swarm";
  const input = forSwarm(form.input, swarm);
  const issues = allIssues(input, basicsState);
  const busy = pending !== null;

  const back = search.cluster ? (
    <Link to="/services" search={{ cluster: search.cluster }}>
      {cluster ? `${cluster.name} services` : "Cluster services"}
    </Link>
  ) : (
    <Link to="/services">Services</Link>
  );

  const submit = async (andDeploy: boolean) => {
    if (!env || issues.length) return;
    setPending(andDeploy ? "deploy" : "create");
    setInvalid("");
    let created = false;
    try {
      const svc = await save.mutateAsync({ ...cleanInput(input), environment_id: env.id, name: basics.name });
      created = true;
      if (basics.nodes.length && !swarm) await place.mutateAsync({ id: svc.id, placements: basics.nodes.map((server_id) => ({ server_id, role: "active" })) });
      if (andDeploy) {
        const jobs = await deploy.mutateAsync(svc.id);
        toast(swarm ? `Created ${svc.name}; deploying to the swarm` : `Created ${svc.name}; deploying to ${jobs.length} node${jobs.length === 1 ? "" : "s"}`);
      } else toast(`Created ${svc.name}`);
      void navigate({ to: "/services/$serviceId", params: { serviceId: svc.id } });
    } catch (e) {
      if (!created && e instanceof ApiError && e.status === 400) setInvalid(e.message);
      else toast(errorText(e), "error");
    } finally {
      setPending(null);
    }
  };

  const loading = envQ.isLoading || projectsQ.isLoading;
  const error = envQ.error ?? projectsQ.error;
  const noProjects = !loading && !error && projects.length === 0;
  const deployWhy = issues[0] ?? (!swarm && basics.nodes.length === 0 ? "Choose nodes under Basics → Placement first" : undefined);

  return (
    <form
      className="flex min-h-full flex-col"
      onSubmit={(e) => {
        e.preventDefault();
        void submit(false);
      }}
    >
      <PageCrumb back={back} current="New service" />
      <PageHeader title="New service" description="One part of a project in one environment: a container or a systemd service. Define it once, then deploy it to its nodes." />

      <div className="min-w-0 flex-1">
        {loading ? (
          <Skeleton rows={4} />
        ) : error ? (
          <ErrorNote error={error} />
        ) : noProjects ? (
          <div className="rounded-lg border border-line">
            <Empty
              title="No projects yet"
              action={
                <Button type="button" variant="primary" onClick={() => setDialog("project")}>
                  <Plus className="size-4" /> New project
                </Button>
              }
            >
              A service belongs to an environment of a project. Create the project (e.g. <b>shop</b>) first, then an environment (e.g. <b>test</b>){cluster ? ` in ${cluster.name}` : ""}.
            </Empty>
          </div>
        ) : (
          <>
          {invalid && (
            <p role="alert" className="mb-3 rounded-md border border-bad bg-bad-soft px-3 py-2 text-[13px] break-words text-bad">
              <b className="font-semibold">Not created:</b> {invalid}
            </p>
          )}
          <ServiceEditor
            mode="create"
            value={input}
            swarm={swarm}
            environmentId={env?.id}
            onChange={(next) => setForm((f) => ({ ...f, input: next }))}
            basicsState={basicsState}
            facts={facts}
            basics={
              <CreateBasics
                value={basics}
                onChange={(b) => setForm((f) => ({ ...f, basics: b }))}
                projects={projects}
                environments={environments}
                clusters={clusters}
                servers={servers}
                onNewProject={() => setDialog("project")}
                onNewEnvironment={() => setDialog("env")}
                swarm={swarm}
              />
            }
          />
          </>
        )}
      </div>

      <ActionBar note={issues.length ? <span className="text-warn">{issues[0]}</span> : swarm ? "The swarm places the tasks when you deploy." : basics.nodes.length ? `Runs on ${basics.nodes.length} node${basics.nodes.length === 1 ? "" : "s"} after it is deployed.` : "Nothing runs until you place and deploy it."}>
        <Link to="/services" search={search.cluster ? { cluster: search.cluster } : {}}>
          <Button type="button" variant="ghost">
            Cancel
          </Button>
        </Link>
        <span title={issues[0]} className="inline-flex">
          <Button type="submit" disabled={!!issues.length || busy}>
            {pending === "create" ? <Loader2 className="size-4 animate-spin" /> : <Plus className="size-4" />} Create service
          </Button>
        </span>
        <span title={deployWhy} className="inline-flex">
          <Button type="button" variant="primary" disabled={!!deployWhy || busy} onClick={() => void submit(true)}>
            {pending === "deploy" ? <Loader2 className="size-4 animate-spin" /> : <Rocket className="size-4" />} Create and deploy
          </Button>
        </span>
      </ActionBar>

      {dialog === "project" && <ProjectModal onClose={() => setDialog(null)} onCreated={(g) => setForm((f) => ({ ...f, basics: { ...f.basics, project: g.id, environment: "", nodes: [] } }))} />}
      {dialog === "env" && (
        <NewEnvironmentModal
          onClose={() => setDialog(null)}
          projectId={basics.project}
          fixedProject
          clusterId={search.cluster}
          fixedCluster={!!search.cluster}
          onCreated={(n) => setForm((f) => ({ ...f, basics: { ...f.basics, project: basics.project, environment: n.id, nodes: [] } }))}
        />
      )}
    </form>
  );
}
