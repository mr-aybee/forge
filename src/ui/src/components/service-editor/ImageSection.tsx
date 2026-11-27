import type { ServiceInput } from "../../api/types";
import { Input } from "../ui";
import { FormField, Grid } from "./parts";

/** Container: the image reference and overrides of its CMD and ENTRYPOINT. */
export function ImageSection({ value, set }: { value: ServiceInput; set: (p: Partial<ServiceInput>) => void }) {
  const bad = /\s/.test(value.image.trim());
  return (
    <Grid>
      <FormField wide label="Image" error={bad ? "No spaces in an image reference." : undefined} hint="registry/name:tag, for example nginx:1.27-alpine or ghcr.io/acme/api:1.4.2. Pin a tag; latest changes under you.">
        {(id, invalid) => (
          <Input id={id} required aria-invalid={invalid || undefined} className={`font-mono ${invalid ? "border-bad" : ""}`} value={value.image} onChange={(e) => set({ image: e.target.value.trim() })} placeholder="ghcr.io/acme/api:1.4.2" spellCheck={false} />
        )}
      </FormField>
      <FormField label="Command" hint="Overrides the image's CMD (run with /bin/sh -c).">
        {(id) => <Input id={id} className="font-mono" value={value.command} onChange={(e) => set({ command: e.target.value })} placeholder="Image default" spellCheck={false} />}
      </FormField>
      <FormField label="Entrypoint" hint="Overrides the image's ENTRYPOINT (words separated by spaces).">
        {(id) => <Input id={id} className="font-mono" value={value.entrypoint} onChange={(e) => set({ entrypoint: e.target.value })} placeholder="Image default" spellCheck={false} />}
      </FormField>
    </Grid>
  );
}
