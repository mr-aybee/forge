import type { ServiceInput } from "../../api/types";
import { Input } from "../ui";
import { FormField, Grid } from "./parts";

/** Container: the image reference. */
export function ImageSection({ value, set }: { value: ServiceInput; set: (p: Partial<ServiceInput>) => void }) {
  const bad = /\s/.test(value.image.trim());
  return (
    <Grid>
      <FormField wide label="Image" error={bad ? "No spaces in an image reference." : undefined} hint="registry/name:tag, for example nginx:1.27-alpine or ghcr.io/acme/api:1.4.2. Pin a tag; latest changes under you.">
        {(id, invalid) => (
          <Input id={id} required aria-invalid={invalid || undefined} className={`font-mono ${invalid ? "border-bad" : ""}`} value={value.image} onChange={(e) => set({ image: e.target.value.trim() })} placeholder="ghcr.io/acme/api:1.4.2" spellCheck={false} />
        )}
      </FormField>
    </Grid>
  );
}
