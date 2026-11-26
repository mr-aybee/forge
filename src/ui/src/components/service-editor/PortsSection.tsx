import type { ServiceInput } from "../../api/types";
import { Check, ListEditor } from "./parts";
import { PORT_RE } from "./sections";

/** Container: published ports and the cluster container network. */
export function PortsSection({ value, set }: { value: ServiceInput; set: (p: Partial<ServiceInput>) => void }) {
  return (
    <div className="space-y-2">
      <ListEditor label="Port" values={value.ports} onChange={(ports) => set({ ports })} placeholder="127.0.0.1:8080:80" validate={(v) => PORT_RE.test(v)} addLabel="Add port" />
      <p className="text-xs text-faint">Bind to 127.0.0.1 when only the node's ingress should reach it. The first port is the default target of an ingress endpoint.</p>
      {!value.swarm && (
        <Check
          checked={value.mesh !== false}
          onChange={(on) => set({ mesh: on ? undefined : false })}
          label="Cluster network"
          hint="Join the node's cluster container network, so other containers in the cluster reach it by name without published ports. Off: Docker's default bridge only."
        />
      )}
    </div>
  );
}
