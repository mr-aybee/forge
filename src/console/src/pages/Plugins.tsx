import { Pin, PinOff, PlugZap, Power, PowerOff } from "lucide-react";

import { ApiError } from "../api/client";
import { usePluginAction, usePlugins } from "../api/hooks";
import { errorText, useToast } from "../app/toast";
import { Badge, Button, Empty, PageHeader, Panel, Status, Table, Td, ago } from "../components/ui";

export function Plugins() {
  const plugins = usePlugins();
  const act = usePluginAction();
  const toast = useToast();
  const forbidden = plugins.error instanceof ApiError && plugins.error.status === 403;
  const run = (id: string, action: "enable" | "disable" | "pin" | "unpin") =>
    act.mutate({ id, action }, { onSuccess: () => toast(`${id}: ${action}d`), onError: (e) => toast(errorText(e), "error") });

  return (
    <>
      <PageHeader
        eyebrow="Administration"
        title="Plugins"
        description="Signed WebAssembly extensions in a sandbox with fuel, memory and time budgets. A plugin can only do what both it and the invoking user are allowed to do."
      />
      <Panel>
        {forbidden ? (
          <Empty icon={<PlugZap className="size-5" />} title="You can't view plugins" />
        ) : (plugins.data ?? []).length === 0 ? (
          <Empty icon={<PlugZap className="size-5" />} title="No plugins installed">
            Install signed packages with <code className="font-mono text-xs">forge plugin install</code>. Managing plugins requires a platform admin with MFA.
          </Empty>
        ) : (
          <Table head={["Plugin", "Version", "State", "Permissions", "Faults", "Updated", ""]}>
            {plugins.data?.map((p) => (
              <tr key={p.plugin_id}>
                <Td>
                  <div className="font-medium">{p.plugin_id}</div>
                  <div className="text-xs text-faint">by {p.publisher}</div>
                  {p.quarantine_reason && <div className="text-xs text-bad">{p.quarantine_reason}</div>}
                </Td>
                <Td className="font-mono text-xs">
                  {p.version} {p.pinned && <Badge tone="accent">pinned</Badge>}
                </Td>
                <Td>
                  <Status value={p.state} />
                </Td>
                <Td>
                  <div className="flex flex-wrap gap-1">
                    {p.approved_permissions.map((x) => (
                      <Badge key={x}>{x}</Badge>
                    ))}
                  </div>
                </Td>
                <Td>{p.fault_count}</Td>
                <Td className="text-muted">{ago(p.updated_at)}</Td>
                <Td>
                  <div className="flex justify-end gap-1">
                    {p.state === "enabled" ? (
                      <Button onClick={() => run(p.plugin_id, "disable")}>
                        <PowerOff className="size-4" /> Disable
                      </Button>
                    ) : (
                      <Button onClick={() => run(p.plugin_id, "enable")}>
                        <Power className="size-4" /> {p.state === "quarantined" ? "Release" : "Enable"}
                      </Button>
                    )}
                    <Button variant="ghost" onClick={() => run(p.plugin_id, p.pinned ? "unpin" : "pin")}>
                      {p.pinned ? <PinOff className="size-4" /> : <Pin className="size-4" />}
                    </Button>
                  </div>
                </Td>
              </tr>
            ))}
          </Table>
        )}
      </Panel>
    </>
  );
}
