import { Link } from "@tanstack/react-router";
import { Container, FolderOpen } from "lucide-react";

import { uniqueRoots, useFileRoots } from "../../api/files";
import { useCatalog } from "../../api/hooks";
import type { Service } from "../../api/types";
import { Meta, MetaGrid } from "../feed/Feed";
import { RUNTIMES, serviceFacts } from "../service-editor/sections";

/** The app's file root on one node, as a link that opens it in the node's Files tab. */
function AppFileRoot({ serverId, appId, home, node }: { serverId: string; appId: string; home: string; node: string }) {
  const roots = useFileRoots(serverId);
  const list = uniqueRoots(roots.data?.roots);
  const r = list.find((x) => x.kind === "service" && x.service_id === appId) ?? list.find((x) => x.kind === "service" && x.base === home);
  if (roots.isLoading) return <span className="text-faint">{node}…</span>;
  if (!r) {
    return (
      <span className="text-muted" title="No approved file root for this app on this node (Files → Roots), or you can't see it.">
        {node}: none
      </span>
    );
  }
  return (
    <Link to="/nodes/$nodeId" params={{ nodeId: serverId }} search={{ tab: "files", root: r.id }} className="inline-flex items-center gap-1 text-accent hover:underline" title={`${r.label} — ${r.base}`}>
      <FolderOpen className="size-3.5" /> {node}
    </Link>
  );
}

const Divider = ({ children }: { children: string }) => (
  <div className="contents">
    <dt className="col-span-2 mt-1.5 border-t border-line/60 pt-1.5 text-[10px] font-semibold tracking-wide text-faint uppercase">{children}</dt>
  </div>
);

/**
 * What the service is (type, command or image, runtimes, limits, variables)
 * and where it lives on each node (directory, files, unit, user, file roots
 * or container). Long values wrap in full, with a copy button.
 */
export function ServiceSummary({ a, environmentName, nodeName }: { a: Service; environmentName?: string; nodeName: (serverId: string) => string }) {
  const catalog = useCatalog().data?.packages ?? [];
  const spec = a.spec ?? ({} as Partial<Service["spec"]>);
  const rt = RUNTIMES.find((r) => r.id === a.runtime);
  const facts = serviceFacts(spec.environment ?? environmentName ?? "", a.name);
  const container = a.runtime === "container";
  const deps = Array.isArray(spec.dependencies) ? spec.dependencies : [];
  const ports = Array.isArray(spec.ports) ? spec.ports.map(String) : [];
  const env = Array.isArray(a.env) ? a.env : [];
  const deployed = (Array.isArray(a.instances) ? a.instances : []).filter((i) => i.status !== "pending");
  const nodeIds = [...new Set((Array.isArray(a.instances) ? a.instances : []).map((i) => i.server_id))];
  const limits = [spec.cpu_millis ? `${spec.cpu_millis / 1000} CPU` : "", spec.memory_mb ? `${spec.memory_mb} MiB` : ""].filter(Boolean).join(" · ");

  return (
    <MetaGrid className="max-w-none px-3 py-2">
      <Meta label="Type" value={rt?.label ?? a.runtime} />
      {container ? <Meta label="Image" value={spec.image || "—"} mono copy={spec.image} wrap /> : <Meta label="Command" value={spec.command || "—"} mono copy={spec.command} wrap />}
      {!container && (
        <Meta label="Runtimes">
          {deps.length ? (
            <span className="flex flex-wrap gap-1">
              {deps.map((d) => (
                <span key={d.package} className="rounded bg-accent-soft px-1 text-[11px] leading-4 text-accent">
                  {catalog.find((c) => c.id === d.package)?.name ?? d.package} {d.version}
                  {d.params?.flavor ? ` · ${d.params.flavor}` : ""}
                </span>
              ))}
            </span>
          ) : (
            <span className="text-muted">none (self-contained)</span>
          )}
        </Meta>
      )}
      {container && ports.length > 0 && <Meta label="Ports" value={ports.join(", ")} mono wrap />}
      {!container && spec.listen_port ? <Meta label="Listen port" value={`:${spec.listen_port}`} mono /> : null}
      <Meta label="Limits" value={limits || "none"} />
      {container && <Meta label="Health check" value={spec.healthcheck?.command || "image default"} mono={!!spec.healthcheck} wrap />}
      <Meta label="Variables" value={`${env.length} (${env.filter((e) => e.secret).length} secret)`} />

      <Divider>On the node</Divider>
      <Meta label="Service directory" value={facts.home} mono copy={facts.home} wrap />
      <Meta label="Environment file" value={facts.envFile} mono copy={facts.envFile} wrap />
      {container ? (
        <Meta label="Container">
          <span className="flex min-w-0 flex-col">
            <span className="font-mono text-[11px] break-all">{facts.container}</span>
            {deployed.length > 0 && (
              <span className="flex flex-wrap gap-x-3">
                {deployed.map((i) => (
                  <Link
                    key={i.server_id}
                    to="/nodes/$nodeId/containers/$containerId"
                    params={{ nodeId: i.server_id, containerId: facts.container }}
                    className="inline-flex items-center gap-1 text-accent hover:underline"
                    title={`Open the container on ${nodeName(i.server_id)}`}
                  >
                    <Container className="size-3" /> on {nodeName(i.server_id)}
                  </Link>
                ))}
              </span>
            )}
          </span>
        </Meta>
      ) : (
        <>
          <Meta label="systemd unit" value={facts.unit} mono copy={facts.unit} wrap />
          <Meta label="Runs as" wrap>
            <span className="font-mono text-[11px]">{facts.user}</span>
            <span className="text-faint"> · its own system user, never root</span>
          </Meta>
          <Meta label="File root">
            <span className="flex flex-wrap gap-x-3 gap-y-0.5">
              {nodeIds.length === 0 ? (
                <span className="text-muted">deploy to a node first</span>
              ) : (
                nodeIds.map((sid) => <AppFileRoot key={sid} serverId={sid} appId={a.id} home={facts.home} node={nodeName(sid)} />)
              )}
            </span>
          </Meta>
        </>
      )}
    </MetaGrid>
  );
}
