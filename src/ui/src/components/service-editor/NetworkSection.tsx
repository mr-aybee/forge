import type { ServiceInput } from "../../api/types";
import { Mono } from "../ui";
import { ConsoleFields } from "./ConsoleFields";
import { Check, Grid, ListEditor, SubBlock } from "./parts";
import { HOST_RE, NETWORK_RE, PORT_RE, ipOk } from "./sections";

/** Container: published ports, cluster network, extra networks, hosts and DNS. */
export function NetworkSection({ value, set }: { value: ServiceInput; set: (p: Partial<ServiceInput>) => void }) {
  return (
    <Grid>
      <SubBlock title="Published ports" hint="host:container or ip:host:container[/udp]. Bind to 127.0.0.1 when only the node's ingress should reach it. The first port is the default target of an ingress endpoint.">
        <ListEditor label="Port" values={value.ports} onChange={(ports) => set({ ports })} placeholder="127.0.0.1:8080:80" validate={(v) => PORT_RE.test(v)} addLabel="Add port" />
      </SubBlock>
      {!value.swarm && (
        <div className="md:col-span-2">
          <Check
            checked={value.mesh !== false}
            onChange={(on) => set({ mesh: on ? undefined : false })}
            label="Cluster network"
            hint="Join the node's cluster container network, so other containers in the cluster reach it by name without published ports. Off: Docker's default bridge only."
          />
        </div>
      )}
      <SubBlock title="Extra networks" hint="Existing Docker networks on the node to join as well. Not host. At most 10.">
        <ListEditor label="Network" values={value.networks} onChange={(networks) => set({ networks })} placeholder="backend" validate={(v) => NETWORK_RE.test(v) && v !== "host"} addLabel="Add network" />
      </SubBlock>
      <SubBlock
        title="Extra hosts"
        hint={
          <>
            <Mono>name:ipv4</Mono> entries added to <Mono>/etc/hosts</Mono>.
          </>
        }
      >
        <ListEditor label="Extra host" values={value.extra_hosts} onChange={(extra_hosts) => set({ extra_hosts })} placeholder="db.internal:192.0.2.5" validate={(v) => HOST_RE.test(v)} addLabel="Add host" />
      </SubBlock>
      <SubBlock title="DNS servers" hint="IP addresses, at most 8. Empty: the node's resolvers (or cluster DNS on the cluster network).">
        <ListEditor label="DNS server" values={value.dns} onChange={(dns) => set({ dns })} placeholder="192.0.2.53" validate={ipOk} addLabel="Add DNS server" />
      </SubBlock>
      <ConsoleFields value={value} set={set} />
    </Grid>
  );
}
