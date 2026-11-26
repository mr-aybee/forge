import type { ServiceInput } from "../../api/types";
import { Input, Mono } from "../ui";
import { Check, Grid, ListEditor, PrivilegedHint, RowsEditor, SubBlock } from "./parts";
import { CAP_RE, PERMS_RE, SYSCTL_RE, pathOk } from "./sections";

const upper = (v: string[]) => v.map((c) => c.toUpperCase());

/** Container: read-only root, capabilities, devices and sysctls. */
export function SecuritySection({ value, set }: { value: ServiceInput; set: (p: Partial<ServiceInput>) => void }) {
  return (
    <Grid>
      <div className="md:col-span-2">
        <Check checked={value.read_only} onChange={(read_only) => set({ read_only })} label="Read-only root filesystem" hint="The container can write only to its volumes and tmpfs mounts." />
      </div>
      <SubBlock title="Drop capabilities" hint="Linux capabilities to remove, for example ALL then add back only what is needed.">
        <ListEditor label="Dropped capability" values={value.cap_drop} onChange={(v) => set({ cap_drop: upper(v) })} placeholder="ALL" validate={(v) => CAP_RE.test(v)} addLabel="Drop capability" />
      </SubBlock>
      <SubBlock title="Add capabilities" badge={value.cap_add.some(Boolean) ? <PrivilegedHint /> : undefined} hint="For example NET_ADMIN or NET_BIND_SERVICE (a CAP_ prefix is accepted).">
        <ListEditor label="Added capability" values={value.cap_add} onChange={(v) => set({ cap_add: upper(v) })} placeholder="NET_BIND_SERVICE" validate={(v) => CAP_RE.test(v)} addLabel="Add capability" />
      </SubBlock>
      <SubBlock
        title="Devices"
        badge={value.devices.some((d) => d.host) ? <PrivilegedHint /> : undefined}
        hint={
          <>
            Host devices under <Mono>/dev/</Mono>. Container path defaults to the host path; permissions from <Mono>rwm</Mono> (default rwm).
          </>
        }
      >
        <RowsEditor
          label="Device"
          rows={value.devices}
          onChange={(devices) => set({ devices })}
          blank={() => ({ host: "", container: "", permissions: "" })}
          addLabel="Add device"
          max={20}
          render={(d, setRow, i) => {
            const hostBad = !!d.host && (!d.host.startsWith("/dev/") || !pathOk(d.host));
            const ctrBad = !!d.container && !pathOk(d.container);
            const permBad = !PERMS_RE.test(d.permissions ?? "");
            return (
              <>
                <Input aria-label={`Device ${i + 1} host path`} aria-invalid={hostBad || undefined} className={`h-7 min-w-0 flex-1 py-0 font-mono text-xs ${hostBad ? "border-bad" : ""}`} value={d.host} placeholder="/dev/fuse" onChange={(e) => setRow({ ...d, host: e.target.value.trim() })} spellCheck={false} />
                <span className="text-xs text-faint" aria-hidden>→</span>
                <Input aria-label={`Device ${i + 1} container path`} aria-invalid={ctrBad || undefined} className={`h-7 min-w-0 flex-1 py-0 font-mono text-xs ${ctrBad ? "border-bad" : ""}`} value={d.container ?? ""} placeholder="same path" onChange={(e) => setRow({ ...d, container: e.target.value.trim() })} spellCheck={false} />
                <Input aria-label={`Device ${i + 1} permissions`} aria-invalid={permBad || undefined} maxLength={3} className={`h-7 w-16 shrink-0 py-0 font-mono text-xs ${permBad ? "border-bad" : ""}`} value={d.permissions ?? ""} placeholder="rwm" onChange={(e) => setRow({ ...d, permissions: e.target.value.trim().toLowerCase() })} spellCheck={false} />
              </>
            );
          }}
        />
      </SubBlock>
      <SubBlock title="Sysctls" badge={value.sysctls.some(([k]) => k) ? <PrivilegedHint /> : undefined} hint="Namespaced kernel parameters, for example net.core.somaxconn = 1024. At most 50.">
        <RowsEditor
          label="Sysctl"
          rows={value.sysctls}
          onChange={(sysctls) => set({ sysctls })}
          blank={() => ["", ""] as [string, string]}
          addLabel="Add sysctl"
          max={50}
          render={([k, v], setRow, i) => {
            const keyBad = !!k && !SYSCTL_RE.test(k);
            const valBad = !!k && (!v || v.length > 256);
            return (
              <>
                <Input aria-label={`Sysctl ${i + 1} key`} aria-invalid={keyBad || undefined} className={`h-7 min-w-0 flex-1 py-0 font-mono text-xs ${keyBad ? "border-bad" : ""}`} value={k} placeholder="net.core.somaxconn" onChange={(e) => setRow([e.target.value.trim(), v])} spellCheck={false} />
                <span className="text-xs text-faint" aria-hidden>=</span>
                <Input aria-label={`Sysctl ${i + 1} value`} aria-invalid={valBad || undefined} className={`h-7 min-w-0 flex-1 py-0 font-mono text-xs ${valBad ? "border-bad" : ""}`} value={v} placeholder="1024" onChange={(e) => setRow([k, e.target.value])} spellCheck={false} />
              </>
            );
          }}
        />
      </SubBlock>
    </Grid>
  );
}
