import type { ServiceInput } from "../../api/types";
import { ListEditor } from "./parts";
import { PORT_RE } from "./sections";

/** Container: published ports. */
export function PortsSection({ value, set }: { value: ServiceInput; set: (p: Partial<ServiceInput>) => void }) {
  return (
    <div className="space-y-2">
      <ListEditor label="Port" values={value.ports} onChange={(ports) => set({ ports })} placeholder="127.0.0.1:8080:80" validate={(v) => PORT_RE.test(v)} addLabel="Add port" />
      <p className="text-xs text-faint">Bind to 127.0.0.1 when only the node's ingress should reach it. The first port is the default target of an ingress endpoint.</p>
    </div>
  );
}
