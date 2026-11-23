import { Link } from "@tanstack/react-router";
import {
  ArrowLeft,
  ChevronRight,
  LayoutTemplate,
  Loader2,
  Save,
} from "lucide-react";
import { useState } from "react";

import { useServers } from "../../api/hooks";
import {
  STACK_NAME,
  type Stack,
  type StackTemplate,
  useCreateFromTemplate,
  useStackTemplates,
} from "../../api/stacks";
import { nodeName } from "../../api/types";
import { errorText, useToast } from "../../app/toast";
import {
  ActionBar,
  Badge,
  Button,
  CopyButton,
  Empty,
  ErrorNote,
  Field,
  Input,
  Select,
  Skeleton,
} from "../ui";
import { useEnvironmentChoices } from "./CreateStack";

/** Template picker: a card grid grouped by category. */
function TemplateGrid({ onPick }: { onPick: (t: StackTemplate) => void }) {
  const q = useStackTemplates();
  if (q.isLoading) return <Skeleton rows={4} />;
  if (q.isError) return <ErrorNote error={q.error} />;
  const list = q.data ?? [];
  if (!list.length)
    return (
      <Empty icon={<LayoutTemplate className="size-5" />} title="No templates">
        This server offers no app templates. Paste a compose file instead.
      </Empty>
    );
  const groups = new Map<string, StackTemplate[]>();
  for (const t of list)
    groups.set(t.category, [...(groups.get(t.category) ?? []), t]);
  return (
    <div className="space-y-4">
      {[...groups.entries()]
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([cat, items]) => (
          <section key={cat} aria-label={cat}>
            <h2 className="mb-1.5 text-xs font-medium text-muted">{cat}</h2>
            <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
              {items.map((t) => (
                <button
                  key={t.id}
                  type="button"
                  onClick={() => onPick(t)}
                  className="min-w-0 rounded-md border border-line px-3 py-2 text-left hover:border-line-strong hover:bg-panel-2 focus-visible:outline-2 focus-visible:outline-accent"
                >
                  <div className="flex min-w-0 items-center gap-2">
                    <span
                      className="truncate text-[13px] font-medium"
                      title={t.title}
                    >
                      {t.title}
                    </span>
                    <span className="ml-auto shrink-0 font-mono text-[11px] text-faint tabular-nums">
                      {t.variables.length ? `${t.variables.length} vars` : "—"}
                    </span>
                  </div>
                  <p
                    className="mt-0.5 line-clamp-2 text-xs text-muted"
                    title={t.description}
                  >
                    {t.description || "—"}
                  </p>
                </button>
              ))}
            </div>
          </section>
        ))}
    </div>
  );
}

interface Form {
  serverId: string;
  ns: string;
  name: string;
  values: Record<string, string>;
}

/** The form for one template: node, environment, name and its variables. */
function TemplateForm({
  template,
  node,
  environment,
  onBack,
  onDone,
}: {
  template: StackTemplate;
  node: string;
  environment: string;
  onBack: () => void;
  onDone: (s: Stack) => void;
}) {
  const toast = useToast();
  const servers = (useServers().data ?? []).filter(
    (s) => s.status !== "revoked",
  );
  const [form, setForm] = useState<Form>(() => ({
    serverId: node,
    ns: environment,
    name: STACK_NAME.test(template.id) ? template.id : "",
    values: Object.fromEntries(
      template.variables.map((v) => [v.name, v.secret ? "" : v.default]),
    ),
  }));
  const [composeOpen, setComposeOpen] = useState(false);
  const choices = useEnvironmentChoices(form.serverId);
  const create = useCreateFromTemplate();

  // The environment falls back to the node's own, else the first offered.
  const ns = choices.list.some((n) => n.id === form.ns)
    ? form.ns
    : ((
        choices.list.find((n) => n.id === choices.server?.environment_id) ??
        choices.list[0]
      )?.id ?? "");
  const nameOk = STACK_NAME.test(form.name);
  const off = create.isPending || !nameOk || !ns || !form.serverId;
  const why = !form.serverId
    ? "Select a node"
    : !ns
      ? "Select an environment"
      : !nameOk
        ? "Enter a valid stack name"
        : undefined;

  const submit = async () => {
    try {
      // Empty secrets are left out so the server generates them.
      const values = Object.fromEntries(
        Object.entries(form.values).filter(
          ([k, v]) =>
            v !== "" || !template.variables.find((x) => x.name === k)?.secret,
        ),
      );
      const r = await create.mutateAsync({
        template: template.id,
        environment_id: ns,
        server_id: form.serverId,
        name: form.name,
        values,
      });
      toast(`Stack ${r.stack.name} saved`);
      onDone(r.stack);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <div className="flex min-h-full flex-col gap-4">
      <div className="flex min-w-0 items-center gap-2">
        <Button
          type="button"
          variant="ghost"
          onClick={onBack}
          className="h-7 px-2 text-xs"
        >
          <ArrowLeft className="size-3.5" /> Templates
        </Button>
        <span
          className="truncate text-[13px] font-medium"
          title={template.title}
        >
          {template.title}
        </span>
        <Badge>{template.category}</Badge>
      </div>
      {template.description && (
        <p className="-mt-2 text-xs text-muted">{template.description}</p>
      )}

      <div className="grid items-start gap-x-4 gap-y-3 sm:grid-cols-2">
        <Field label="Node" hint="Services without placement run here.">
          <Select
            value={form.serverId}
            onChange={(e) =>
              setForm((f) => ({ ...f, serverId: e.target.value }))
            }
            required
          >
            <option value="">Select a node</option>
            {servers.map((s) => (
              <option key={s.id} value={s.id}>
                {nodeName(s)}
              </option>
            ))}
          </Select>
        </Field>
        <Field label="Environment" hint="Environments of the node's cluster.">
          <Select
            value={ns}
            onChange={(e) => setForm((f) => ({ ...f, ns: e.target.value }))}
            required
          >
            <option value="">Select an environment</option>
            {choices.list.map((n) => (
              <option key={n.id} value={n.id}>
                {n.name} · {choices.clusterName(n.cluster_id)}
              </option>
            ))}
          </Select>
        </Field>
        <Field
          label="Stack name"
          hint={
            form.name && !nameOk ? (
              <span className="text-bad">
                Lower-case letters, digits, - and _; start with a letter or
                digit.
              </span>
            ) : (
              "Unique in the environment."
            )
          }
        >
          <Input
            value={form.name}
            onChange={(e) =>
              setForm((f) => ({ ...f, name: e.target.value.toLowerCase() }))
            }
            maxLength={63}
            aria-invalid={!!form.name && !nameOk}
          />
        </Field>
      </div>

      {template.variables.length > 0 && (
        <section aria-label="Variables" className="space-y-2">
          <h2 className="text-xs font-medium text-muted">Variables</h2>
          <div className="grid items-start gap-x-4 gap-y-3 sm:grid-cols-2">
            {template.variables.map((v) => (
              <Field
                key={v.name}
                label={v.label}
                hint={
                  v.secret ? (
                    "Leave empty to generate. Stored sealed; not shown again."
                  ) : (
                    <span className="font-mono">{v.name}</span>
                  )
                }
              >
                <Input
                  type={v.secret ? "password" : "text"}
                  autoComplete={v.secret ? "new-password" : "off"}
                  value={form.values[v.name] ?? ""}
                  placeholder={v.secret ? "Generated" : v.default || "—"}
                  onChange={(e) =>
                    setForm((f) => ({
                      ...f,
                      values: { ...f.values, [v.name]: e.target.value },
                    }))
                  }
                />
              </Field>
            ))}
          </div>
        </section>
      )}

      <section className="rounded-md border border-line">
        <button
          type="button"
          aria-expanded={composeOpen}
          aria-controls="template-compose"
          onClick={() => setComposeOpen((o) => !o)}
          className="flex w-full min-w-0 items-center gap-1.5 rounded-md px-2.5 py-2 text-left text-xs font-medium text-muted hover:text-fg"
        >
          <ChevronRight
            className={`size-3.5 shrink-0 transition-transform ${composeOpen ? "rotate-90" : ""}`}
          />
          <span className="truncate font-mono">compose.yaml</span>
          <span className="ml-auto font-mono text-[11px] text-faint tabular-nums">
            {template.compose
              ? `${template.compose.split("\n").length} lines`
              : "—"}
          </span>
        </button>
        {composeOpen && (
          <div id="template-compose" className="relative border-t border-line">
            {template.compose && (
              <div className="absolute top-1.5 right-1.5">
                <CopyButton text={template.compose} />
              </div>
            )}
            <pre className="max-h-96 overflow-auto px-2.5 py-2 font-mono text-[11px] whitespace-pre select-text">
              {template.compose || "—"}
            </pre>
          </div>
        )}
      </section>

      <ActionBar note="Save keeps revision 1 without starting anything.">
        <Link to="/stacks">
          <Button variant="ghost">Cancel</Button>
        </Link>
        <span title={off ? why : undefined} className="inline-flex">
          <Button
            variant="primary"
            onClick={() => void submit()}
            disabled={off}
          >
            {create.isPending ? (
              <Loader2 className="size-4 animate-spin" />
            ) : (
              <Save className="size-4" />
            )}{" "}
            Save
          </Button>
        </span>
      </ActionBar>
    </div>
  );
}

/** New stack from an app template: pick one, fill its variables, save. */
export function FromTemplate({
  node,
  environment,
  onDone,
}: {
  node: string;
  environment: string;
  onDone: (s: Stack) => void;
}) {
  const [picked, setPicked] = useState<StackTemplate>();
  if (!picked) return <TemplateGrid onPick={setPicked} />;
  return (
    <TemplateForm
      key={picked.id}
      template={picked}
      node={node}
      environment={environment}
      onBack={() => setPicked(undefined)}
      onDone={onDone}
    />
  );
}
