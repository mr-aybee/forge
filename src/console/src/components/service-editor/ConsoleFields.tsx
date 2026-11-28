import type { ConsoleLink, ServiceInput } from "../../api/types";
import { Input, Select } from "../ui";
import { FormField } from "./parts";
import { consolePathOk } from "./sections";

/** The service's web console link: port, path and scheme. Port empty: no link. */
export function ConsoleFields({ value, set }: { value: ServiceInput; set: (p: Partial<ServiceInput>) => void }) {
  const c = value.console;
  const portBad = !!c && (!Number.isInteger(c.port) || c.port < 1 || c.port > 65535);
  const pathBad = !!c && !consolePathOk(c.path ?? "/");
  const patch = (p: Partial<ConsoleLink>) => c && set({ console: { ...c, ...p } });
  return (
    <>
      <FormField label="Web console port" error={portBad ? "1 to 65535." : undefined} hint="The port its admin page listens on (the published host port for a container). Adds Open console to the service page. Empty: no link.">
        {(id, invalid) => (
          <Input
            id={id}
            className="w-36"
            inputMode="numeric"
            aria-invalid={invalid}
            value={c ? String(c.port) : ""}
            placeholder="9001"
            onChange={(e) => {
              const t = e.target.value.trim();
              if (!t) return set({ console: null });
              const n = Number(t);
              set({ console: { path: "/", scheme: "http", ...c, port: Number.isFinite(n) ? n : 0 } });
            }}
          />
        )}
      </FormField>
      {c && (
        <>
          <FormField label="Console path" error={pathBad ? "Starts with /, no spaces." : undefined} hint="Path of the admin page.">
            {(id, invalid) => <Input id={id} className="font-mono" aria-invalid={invalid} value={c.path ?? "/"} onChange={(e) => patch({ path: e.target.value })} placeholder="/" spellCheck={false} />}
          </FormField>
          <FormField label="Console scheme" hint="Forge opens the link only; it does not proxy the page or sign you in.">
            {(id) => (
              <Select id={id} value={c.scheme ?? "http"} onChange={(e) => patch({ scheme: e.target.value === "https" ? "https" : "http" })}>
                <option value="http">http</option>
                <option value="https">https</option>
              </Select>
            )}
          </FormField>
        </>
      )}
    </>
  );
}
