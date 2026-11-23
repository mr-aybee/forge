import { useState } from "react";

import { useMe, useNameSuffix, useSetNameSuffix } from "../api/hooks";
import { errorText, useToast } from "../app/toast";
import { Badge, Button, Confirm, Field, Input, Mono } from "./ui";

// The suffix is fleet-wide: `system.update` on the platform (platform admins).
const MANAGERS = ["platform_admin"];
// One-label suffixes the server accepts (`forge_catalog::names::check_suffix`).
const SINGLE = ["local", "internal", "lan", "home", "test", "localdomain", "intranet", "corp"];
const LABEL = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/;

/** The same rules as the server and the agent; `null` when valid. */
export function suffixProblem(s: string): string | null {
  if (!s) return "Enter a suffix.";
  if (s.length > 63) return "At most 63 characters in all.";
  const labels = s.split(".");
  if (!labels.every((l) => LABEL.test(l))) return "Lowercase DNS labels (a-z, 0-9, -) separated by dots; no label starts or ends with -.";
  if (labels.some((l) => l === "localhost" || l === "localhost6")) return "The suffix cannot contain localhost.";
  if (labels.length === 1 && !SINGLE.includes(s)) return `${s} could be a public top-level domain. Use one of ${SINGLE.join(", ")}, or a domain you own (home.arpa, corp.example.com).`;
  return null;
}

const SOURCE: Record<string, string> = { setting: "set here", config: "config file", default: "default" };

/** "Local name suffix": shown to everyone, editable by platform admins. */
export function NameSuffixEditor({ example }: { example?: string }) {
  const q = useNameSuffix();
  const save = useSetNameSuffix();
  const toast = useToast();
  const bindings = useMe().data?.bindings;
  const canEdit = (Array.isArray(bindings) ? bindings : []).some((b) => MANAGERS.includes(b?.role));
  const [draft, setDraft] = useState<string>();
  const [confirm, setConfirm] = useState<string | null>(null);

  if (!q.data) return null; // Loading, or an older server without the setting.
  const current = q.data.suffix;
  const value = draft ?? current;
  const next = value.trim();
  const problem = suffixProblem(next);
  const dirty = next !== current;
  const node = example || "lab-b";

  const run = (suffix: string) =>
    save.mutate(suffix, {
      onSuccess: (r) => {
        setDraft(undefined);
        setConfirm(null);
        toast(`Local names now end in .${r.suffix}`);
      },
      onError: (e) => {
        setConfirm(null);
        toast(errorText(e), "error");
      },
    });

  return (
    <div className="space-y-2 border-b border-line px-4 py-3">
      <div className="flex flex-wrap items-end gap-3">
        <div className="w-72">
          <Field label="Local name suffix" hint={canEdit ? undefined : "Only platform admins change this."}>
            <Input
              value={value}
              disabled={!canEdit || save.isPending}
              spellCheck={false}
              aria-invalid={dirty && !!problem}
              onChange={(e) => setDraft(e.target.value.toLowerCase())}
            />
          </Field>
        </div>
        <Badge>{SOURCE[q.data.source] ?? q.data.source}</Badge>
        {canEdit && (
          <>
            <Button variant="primary" disabled={!dirty || !!problem || save.isPending} onClick={() => setConfirm(next)}>
              Save
            </Button>
            {q.data.source === "setting" && (
              <Button disabled={save.isPending} onClick={() => setConfirm("")} title={`Go back to .${q.data.config_suffix} from the config file`}>
                Reset
              </Button>
            )}
          </>
        )}
      </div>
      {dirty && problem ? (
        <div className="text-xs text-bad">{problem}</div>
      ) : (
        <div className="text-xs text-muted">
          Names look like <Mono>{`${node}.lab.${next || current}`}</Mono> in a cluster named lab, <Mono>{`${node}.${next || current}`}</Mono> outside one.
        </div>
      )}
      <div className="text-xs text-faint">
        .local is also used by mDNS (Avahi, Bonjour); if your network uses it, pick home.arpa or a domain you own, such as corp.example.com.
      </div>
      <Confirm
        open={confirm !== null}
        onOpenChange={(o) => !o && setConfirm(null)}
        title="Change the local name suffix?"
        confirmLabel="Change suffix"
        busy={save.isPending}
        onConfirm={() => confirm !== null && run(confirm)}
        body={
          <div className="space-y-2">
            <p>
              Every node's hosts file is rewritten on the next pass: names change from <Mono>{`${node}.${current}`}</Mono> to{" "}
              <Mono>{`${node}.${confirm || q.data.config_suffix}`}</Mono>. The old names stop resolving on all nodes.
            </p>
            <p>Forge containers follow on their next sync. Update anything that uses the old names.</p>
          </div>
        }
      />
    </div>
  );
}
