import { Link } from "@tanstack/react-router";

import { Mono } from "../ui";

/** Systemd: where the service keeps code and data, and where buckets are attached. */
export function StorageSection({ home, envFile, serviceId }: { home: string; envFile: string; serviceId?: string }) {
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
      <dt className="text-muted">Buckets</dt>
      <dd className="text-xs text-muted">
        {serviceId ? (
          <>
            Attach object-store buckets on the{" "}
            <Link to="/services/$serviceId" params={{ serviceId }} search={{ tab: "storage" }} className="text-accent hover:underline">
              Storage
            </Link>{" "}
            tab; their variables reach the nodes with the next deploy.
          </>
        ) : (
          "Attach object-store buckets on the service's Storage tab after you create it."
        )}
      </dd>
    </dl>
  );
}
