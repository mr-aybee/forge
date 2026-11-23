import type { ServiceInput } from "../../api/types";
import { Check, ListEditor } from "./parts";
import { VOL_RE } from "./sections";

/** Container: bind mounts and the read-only root filesystem. */
export function VolumesSection({ value, set, home }: { value: ServiceInput; set: (p: Partial<ServiceInput>) => void; home: string }) {
  return (
    <div className="space-y-3">
      <ListEditor label="Volume" values={value.volumes} onChange={(volumes) => set({ volumes })} placeholder={`${home}/data:/data`} validate={(v) => VOL_RE.test(v)} addLabel="Add volume" />
      <Check checked={value.read_only} onChange={(read_only) => set({ read_only })} label="Read-only root filesystem" hint="The container can write only to its volumes." />
    </div>
  );
}
