import type { ServiceInput } from "../../api/types";
import { Input, Mono } from "../ui";
import { Grid, ListEditor, NumberInput, PrivilegedHint, RowsEditor, SubBlock } from "./parts";
import { VOL_RE, pathOk } from "./sections";

/** Container: bind mounts and tmpfs mounts. */
export function VolumesSection({ value, set, home }: { value: ServiceInput; set: (p: Partial<ServiceInput>) => void; home: string }) {
  const outside = value.volumes.some((v) => v && VOL_RE.test(v) && !(v.split(":")[0] ?? "").startsWith(`${home}/`) && v.split(":")[0] !== home);
  return (
    <Grid>
      <SubBlock
        title="Volumes"
        badge={outside ? <PrivilegedHint /> : undefined}
        hint={
          <>
            <Mono>/host/path:/container/path[:ro]</Mono>. Paths under <Mono>{home}</Mono> need nothing extra; other host paths need stack.privileged unless an administrator approved the root.
          </>
        }
      >
        <ListEditor label="Volume" values={value.volumes} onChange={(volumes) => set({ volumes })} placeholder={`${home}/data:/data`} validate={(v) => VOL_RE.test(v)} addLabel="Add volume" />
      </SubBlock>
      <SubBlock title="tmpfs mounts" hint="In-memory mounts, emptied on every restart. Size 0 or empty: Docker's default (half the node's memory). At most 20.">
        <RowsEditor
          label="tmpfs mount"
          rows={value.tmpfs}
          onChange={(tmpfs) => set({ tmpfs })}
          blank={() => ({ target: "", size_mb: 0 })}
          addLabel="Add tmpfs"
          max={20}
          render={(t, setRow, i) => {
            const bad = !!t.target && !pathOk(t.target);
            return (
              <>
                <Input aria-label={`tmpfs ${i + 1} path`} aria-invalid={bad || undefined} className={`h-7 min-w-0 flex-1 py-0 font-mono text-xs ${bad ? "border-bad" : ""}`} value={t.target} placeholder="/tmp" onChange={(e) => setRow({ ...t, target: e.target.value.trim() })} spellCheck={false} />
                <div className="flex shrink-0 items-center gap-1">
                  <NumberInput id={`tmpfs-size-${i}`} label={`tmpfs ${i + 1} size in MiB`} className="h-7 w-24 py-0 text-xs" min={0} value={t.size_mb} placeholder="default" onChange={(n) => setRow({ ...t, size_mb: Math.max(0, Math.round(n) || 0) })} />
                  <span className="text-xs text-faint">MiB</span>
                </div>
              </>
            );
          }}
        />
      </SubBlock>
    </Grid>
  );
}
