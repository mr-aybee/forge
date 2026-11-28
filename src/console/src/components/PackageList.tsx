import { Link } from "@tanstack/react-router";
import { BellPlus, ChevronDown, ChevronRight, Play, RefreshCw, RotateCw, Square, Trash2 } from "lucide-react";
import { Fragment, useState } from "react";

import { useCatalog, usePackageAction, useServers } from "../api/hooks";
import type { Installation } from "../api/types";
import { errorText, useToast } from "../app/toast";
import { categoryIcon } from "../pages/AppStore";
import { JobDetail } from "./JobsDrawer";
import { RuleDialog } from "./alerts/RuleDialog";
import { Confirm, CopyButton, RowMenu, type RowAction, Mono, Status, Table, Td, ago, useRowMenu } from "./ui";

/** Slimmer rows than the default table cell. */
const dense = "py-1.5!";

type Act = "start" | "stop" | "restart" | "status" | "uninstall" | "install";

/** Installed packages with lifecycle actions; expand a row for its variables, error and last job. */
export function PackageList({ items, showNode = true }: { items: Installation[]; showNode?: boolean }) {
  const catalog = useCatalog().data?.packages ?? [];
  const nodes = useServers().data ?? [];
  const act = usePackageAction();
  const toast = useToast();
  const [open, setOpen] = useState<string>();
  const [removing, setRemoving] = useState<Installation>();
  /** The installation an "Add alert" dialog is open for (D-118). */
  const [alerting, setAlerting] = useState<Installation>();
  const rm = useRowMenu();

  const run = (i: Installation, action: Act) =>
    act.mutate(
      { id: i.id, action },
      { onSuccess: () => toast(`${action} ${i.package} queued`), onError: (e) => toast(errorText(e), "error") },
    );

  const head = ["Package", ...(showNode ? ["Node"] : []), "Status", "Service", "Updated", ""];
  return (
    <>
      <Table head={head}>
        {items.map((i) => {
          const pkg = catalog.find((c) => c.id === i.package);
          const Icon = pkg ? categoryIcon(pkg.category) : ChevronRight;
          const busy = i.status.endsWith("ing");
          const node = nodes.find((n) => n.id === i.server_id);
          const expanded = open === i.id;
          const wait = busy ? `Wait: ${i.status}` : undefined;
          const actions: RowAction[] = [
            { label: "Refresh status", icon: RefreshCw, disabled: act.isPending, hint: act.isPending ? "Another action is being queued" : undefined, onSelect: () => run(i, "status") },
            i.status === "failed"
              ? { label: "Retry install", icon: RotateCw, disabled: busy, hint: wait, onSelect: () => run(i, "install") }
              : i.state === "running"
                ? { label: "Stop", icon: Square, tone: "bad", disabled: busy, hint: wait, onSelect: () => run(i, "stop") }
                : { label: "Start", icon: Play, tone: "ok", disabled: busy, hint: wait, onSelect: () => run(i, "start") },
            { label: "Restart", icon: RotateCw, tone: "warn", disabled: busy || i.status !== "installed", hint: wait ?? (i.status !== "installed" ? "Only installed packages restart" : undefined), onSelect: () => run(i, "restart") },
            { label: "Add alert", icon: BellPlus, onSelect: () => setAlerting(i) },
            { label: "Uninstall", icon: Trash2, danger: true, disabled: busy, hint: wait, onSelect: () => setRemoving(i) },
          ];
          return (
            <Fragment key={i.id}>
              <tr className="hover:bg-panel-2/60" {...rm.rowProps(actions, `Actions for ${pkg?.name ?? i.package}`)}>
                <Td className={dense}>
                  <button className="flex items-center gap-2 text-left" onClick={() => setOpen(expanded ? undefined : i.id)}>
                    {expanded ? <ChevronDown className="size-3.5 text-faint" /> : <ChevronRight className="size-3.5 text-faint" />}
                    <Icon className="size-4 text-accent" />
                    <span className="font-medium">{pkg?.name ?? i.package}</span>
                    <Mono>{i.version}</Mono>
                  </button>
                </Td>
                {showNode && (
                  <Td className={dense}>
                    <Link to="/nodes/$nodeId" params={{ nodeId: i.server_id }} className="hover:text-accent">
                      {node?.hostname ?? i.server_id.slice(0, 8)}
                    </Link>
                  </Td>
                )}
                <Td className={dense}>
                  <Status value={i.status === "installed" ? "succeeded" : i.status === "failed" ? "failed" : busy ? "running" : i.status} />
                  <span className="ml-1.5 text-xs text-muted">{i.status}</span>
                </Td>
                <Td className={`${dense} text-muted`}>{i.state ?? "—"}</Td>
                <Td className={`${dense} text-muted`}>{ago(i.updated_at)}</Td>
                <Td className={`${dense} text-right`}>
                  <RowMenu actions={actions} label={`Actions for ${pkg?.name ?? i.package}`} />
                </Td>
              </tr>
              {expanded && (
                <tr>
                  <td colSpan={head.length} className="bg-panel-2/40 px-4 py-3">
                    <div className="grid gap-4 lg:grid-cols-2">
                      <div>
                        <p className="mb-1.5 text-xs font-medium text-muted">
                          Variables in <Mono>/etc/forge/pkgs/{i.package}.env</Mono>
                        </p>
                        {Object.keys(i.env).length ? (
                          <dl className="space-y-1 font-mono text-xs">
                            {Object.entries(i.env).map(([k, v]) => (
                              <div key={k} className="flex items-center gap-2">
                                <dt className="text-accent">{k}</dt>
                                <dd className="min-w-0 flex-1 truncate text-muted">= {v}</dd>
                                <CopyButton text={k} label="Copy name" />
                              </div>
                            ))}
                          </dl>
                        ) : (
                          <p className="text-xs text-faint">Reported after a successful install.</p>
                        )}
                        {Object.keys(i.params).length > 0 && (
                          <p className="mt-2 text-xs text-muted">
                            Installed with{" "}
                            {Object.entries(i.params)
                              .map(([k, v]) => `${k}=${v}`)
                              .join(", ")}
                          </p>
                        )}
                        {i.last_error && <p className="mt-2 rounded border border-bad/40 bg-bad-soft p-2 text-xs text-bad">{i.last_error}</p>}
                      </div>
                      <div>{i.last_job_id ? <JobDetail id={i.last_job_id} /> : <p className="text-xs text-faint">No job yet.</p>}</div>
                    </div>
                  </td>
                </tr>
              )}
            </Fragment>
          );
        })}
      </Table>
      {rm.menu}
      <Confirm
        open={!!removing}
        onOpenChange={(o) => !o && setRemoving(undefined)}
        title={`Uninstall ${removing?.package} ${removing?.version}?`}
        body="Removes the package from the node. Data under /var/lib/forge/pkgs is kept unless the recipe purges it."
        confirmLabel="Uninstall"
        onConfirm={() => {
          if (removing) run(removing, "uninstall");
          setRemoving(undefined);
        }}
      />
      {alerting && (
        <RuleDialog
          rule={null}
          prefill={{ place: "database", templateId: "database-stopped", targets: { installations: [alerting.id], servers: [alerting.server_id] } }}
          onClose={() => setAlerting(undefined)}
        />
      )}
    </>
  );
}
