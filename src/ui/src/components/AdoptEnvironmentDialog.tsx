import { Loader2 } from "lucide-react";
import { useState } from "react";

import { ApiError } from "../api/client";
import { useMoveEnvironment } from "../api/hooks";
import type { Environment } from "../api/types";
import { errorText, useToast } from "../app/toast";
import { Button, Field, Modal, Select } from "./ui";

/** Moves an unassigned environment (created before clusters were required), with its services, into a cluster. */
/** Render it only while open. */
export function AdoptEnvironmentDialog({ onClose, clusterId, candidates }: { onClose: () => void; clusterId: string; candidates: Environment[] }) {
  const move = useMoveEnvironment();
  const toast = useToast();
  const [form, setForm] = useState({ id: "", err: "" });
  const { id, err } = form;
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title="Add existing environment" description="Environments created before clusters were required are unassigned. Move one, with its services, into this cluster.">
      <form
        className="space-y-4"
        onSubmit={(ev) => {
          ev.preventDefault();
          move.mutate(
            { id, cluster_id: clusterId },
            {
              onSuccess: (n) => {
                toast(`${n?.name ?? "Environment"} added`);
                onClose();
              },
              onError: (e) => setForm((f) => ({ ...f, err: e instanceof ApiError && e.status === 409 ? "This cluster already has an environment with that name." : errorText(e) })),
            },
          );
        }}
      >
        <Field label="Environment">
          <Select
            required
            value={id}
            onChange={(e) => setForm({ id: e.target.value, err: "" })}
          >
            <option value="">Select an environment</option>
            {candidates.map((n) => (
              <option key={n.id} value={n.id}>
                {n.name}
              </option>
            ))}
          </Select>
        </Field>
        {err && (
          <p role="alert" className="rounded-md bg-bad-soft px-2.5 py-1.5 text-xs text-bad">
            {err}
          </p>
        )}
        <div className="flex justify-end">
          <Button variant="primary" disabled={move.isPending || !id}>
            {move.isPending && <Loader2 className="size-4 animate-spin" />}
            Add to cluster
          </Button>
        </div>
      </form>
    </Modal>
  );
}
