import { Mono } from "../ui";

/** Systemd: where the service keeps code and data. */
export function StorageSection({ home, envFile }: { home: string; envFile: string }) {
  return (
    <dl className="grid grid-cols-[150px_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-[13px]">
      <dt className="text-muted">Service directory</dt>
      <dd className="min-w-0 truncate select-all" title={home}>
        <Mono>{home}</Mono>
      </dd>
      <dt className="text-muted">Environment file</dt>
      <dd className="min-w-0 truncate select-all" title={envFile}>
        <Mono>{envFile}</Mono>
      </dd>
    </dl>
  );
}
