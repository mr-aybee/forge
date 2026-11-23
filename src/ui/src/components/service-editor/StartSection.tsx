import type { ServiceInput } from "../../api/types";
import { Input, Mono } from "../ui";
import { Check, FormField, Grid, NumberInput } from "./parts";

/** Systemd: start command, working directory, restart and listening port. */
export function StartSection({ value, set, home }: { value: ServiceInput; set: (p: Partial<ServiceInput>) => void; home: string }) {
  const portBad = value.listen_port != null && (value.listen_port < 1 || value.listen_port > 65535);
  return (
    <Grid>
      <FormField
        wide
        label="Command"
        hint={
          <>
            Run as the service's own user via <Mono>exec</Mono>, arguments included. Code lives in <Mono>{home}</Mono>.
          </>
        }
      >
        {(id) => (
          <Input
            id={id}
            required
            className="font-mono"
            value={value.command}
            onChange={(e) => set({ command: e.target.value })}
            placeholder={`${home}/venv/bin/gunicorn app:app -b 127.0.0.1:8000`}
            spellCheck={false}
          />
        )}
      </FormField>
      <FormField label="Working directory" hint="Default: the service directory.">
        {(id) => <Input id={id} className="font-mono" value={value.working_dir} onChange={(e) => set({ working_dir: e.target.value })} placeholder={home} spellCheck={false} />}
      </FormField>
      <FormField label="Listening port" error={portBad ? "1 to 65535." : undefined} hint="The TCP port it listens on, if any. The default health check connects to it.">
        {(id, invalid) => (
          <NumberInput
            id={id}
            className="w-36"
            min={1}
            max={65535}
            value={value.listen_port ?? null}
            empty={0}
            invalid={invalid}
            placeholder="8080"
            onChange={(n) => set({ listen_port: n === 0 || !Number.isInteger(n) ? null : n })}
          />
        )}
      </FormField>
      <div className="md:col-span-2">
        <Check checked={value.restart} onChange={(restart) => set({ restart })} label="Restart automatically when it exits" hint="systemd Restart=always with a short back-off." />
      </div>
    </Grid>
  );
}
