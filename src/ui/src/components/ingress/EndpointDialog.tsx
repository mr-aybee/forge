import { Loader2 } from "lucide-react";
import { useState } from "react";

import {
  type Balance,
  type Member,
  NAME_RE,
  type Placement,
  RULE_KINDS,
  type RuleKind,
  type Site,
  type Target,
  slugFromDomain,
  useCreateSite,
} from "../../api/ingress";
import { errorText, useToast } from "../../app/toast";
import { Button, Field, Input, Modal, Select, TextArea } from "../ui";
import {
  type EndpointForm,
  blankSpec,
  defaultProxyFor,
  formProblems,
  lines,
  toBody,
  usePlacementOptions,
  useTargetContext,
} from "./endpointForm";
import { PlacementPicker } from "./EndpointParts";
import { TargetEditor } from "./TargetEditor";

/** Pre-filled values for a new endpoint (e.g. **Expose through ingress** on a service page). */
export interface EndpointInitial {
  name?: string;
  targets?: Target[];
  balance?: Balance;
  /** Hide the Balancing select (one upstream: the rule changes nothing). */
  hideBalance?: boolean;
  /** Focus the first target's port (no port could be guessed). */
  focusPort?: boolean;
}

interface WizardForm {
  domains: string;
  /** `undefined` while the name follows the first domain. */
  name?: string;
  kind: RuleKind;
  targets: Target[];
  redirect: string;
  root: string;
  placement: Placement;
}

/**
 * **New endpoint**: domains, the default route and where it is served. Saving
 * creates the endpoint (never applies it); the caller then opens the full
 * editor (`onCreated`) or tells the user where to apply it (`onSaved`).
 */
export function EndpointDialog({
  clusterId,
  initial,
  onClose,
  onSaved,
  onCreated,
}: {
  clusterId?: string;
  /** Kept for callers; node targets come from the chosen placement. */
  members?: Member[];
  initial?: EndpointInitial;
  onClose: () => void;
  /** Called instead of opening the editor (e.g. from a service page). */
  onSaved?: (s: Site) => void;
  onCreated?: (s: Site) => void;
}) {
  const toast = useToast();
  const opts = usePlacementOptions();
  const create = useCreateSite();
  const [f, setF] = useState<WizardForm>(() => ({
    domains: "",
    name: initial?.name,
    kind: initial?.balance && !initial.hideBalance ? "balancer" : "proxy",
    targets: initial?.targets ?? [],
    redirect: "",
    root: "",
    placement: clusterId
      ? { kind: "cluster", cluster_id: clusterId }
      : { kind: "nodes", server_ids: [] },
  }));
  const set = (p: Partial<WizardForm>) => setF((x) => ({ ...x, ...p }));
  const ctx = useTargetContext(f.placement);
  const domains = lines(f.domains);
  const name = f.name ?? slugFromDomain(domains[0] ?? "");
  const nameBad = !!name && !NAME_RE.test(name);

  const form: EndpointForm = {
    spec: {
      ...blankSpec(),
      name,
      domains,
      kind: f.kind,
      balance: initial?.balance ?? "round_robin",
      proxy: defaultProxyFor(f.kind),
      redirect:
        f.kind === "redirect"
          ? { to: f.redirect.trim(), code: 301, keep_path: true }
          : undefined,
      static_root:
        f.kind === "static"
          ? { root: f.root.trim(), index: "index.html", spa: false }
          : undefined,
    },
    targets: f.targets,
    standby_targets: [],
    rules: [],
    redirects: [],
    users: [],
    placement: f.placement,
    dns: null,
  };
  const problems = [
    ...(!name
      ? ["Enter a name."]
      : nameBad
        ? ["The name must start with a letter: a-z, 0-9 and dashes, up to 60."]
        : []),
    ...formProblems(form),
  ];

  const save = () =>
    create.mutate(toBody(form), {
      onSuccess: (s) => {
        if (!s)
          return toast(
            "Saved, but the server answered with something unexpected; reload the list",
            "error",
          );
        if (onSaved) {
          onSaved(s);
          onClose();
        } else {
          toast(
            `Endpoint ${s.name} saved. Nothing is live until you Apply it.`,
          );
          onCreated?.(s);
        }
      },
      onError: (e) => toast(errorText(e), "error"),
    });

  return (
    <Modal
      open
      onOpenChange={(o) => !o && onClose()}
      title="New endpoint"
      description="Domains, where they go and which Nginx serves them. You can fine-tune everything after saving."
      wide
    >
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          if (!problems.length) save();
        }}
      >
        <div className="grid gap-3 sm:grid-cols-[1fr_200px]">
          <Field label="Domains" hint="One per line; the first is the primary.">
            <TextArea
              rows={2}
              autoFocus
              className="font-mono text-xs"
              value={f.domains}
              onChange={(e) => set({ domains: e.target.value })}
              placeholder={"shop.example.com\nwww.shop.example.com"}
            />
          </Field>
          <Field
            label="Name"
            hint={
              f.name === undefined
                ? "From the first domain."
                : "Used for file names."
            }
          >
            <Input
              value={name}
              onChange={(e) => set({ name: e.target.value.toLowerCase() })}
              placeholder="shop-example-com"
              aria-invalid={nameBad}
              className="font-mono text-xs"
            />
          </Field>
        </div>

        <Field label="Default route (/)">
          <Select
            value={f.kind}
            onChange={(e) => set({ kind: e.target.value as RuleKind })}
          >
            {RULE_KINDS.map((k) => (
              <option key={k.id} value={k.id}>
                {k.label}
              </option>
            ))}
          </Select>
        </Field>
        {(f.kind === "proxy" || f.kind === "balancer") && (
          <TargetEditor
            label={f.kind === "balancer" ? "Targets" : "Target"}
            hint={
              f.kind === "balancer"
                ? "Requests are spread over these; add standbys in the editor."
                : "Where requests go. A service is reached on every replica."
            }
            value={f.targets}
            onChange={(targets) => set({ targets })}
            members={ctx.members}
            apps={ctx.apps}
            autoFocusPort={initial?.focusPort}
          />
        )}
        {f.kind === "redirect" && (
          <Field
            label="Redirect to"
            hint="301, keeping the path. Change the code in the editor."
          >
            <Input
              value={f.redirect}
              onChange={(e) => set({ redirect: e.target.value })}
              placeholder="https://www.example.com"
              className="font-mono text-xs"
            />
          </Field>
        )}
        {f.kind === "static" && (
          <Field
            label="Directory"
            hint="Served as-is on every edge. It must be inside an approved file root on each edge node."
          >
            <Input
              value={f.root}
              onChange={(e) => set({ root: e.target.value })}
              placeholder="/srv/www/shop"
              className="font-mono text-xs"
            />
          </Field>
        )}

        <div>
          <div className="mb-1 text-xs font-medium text-muted">Served by</div>
          <PlacementPicker
            value={f.placement}
            onChange={(placement) => set({ placement })}
            opts={opts}
          />
        </div>

        {problems.length > 0 && domains.length > 0 && (
          <p className="text-xs text-warn">{problems[0]}</p>
        )}
        <div className="flex justify-end gap-2 pt-1">
          <Button type="button" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            type="submit"
            variant="primary"
            disabled={create.isPending || problems.length > 0}
            title={problems[0]}
          >
            {create.isPending && <Loader2 className="size-4 animate-spin" />}{" "}
            Create endpoint
          </Button>
        </div>
      </form>
    </Modal>
  );
}
