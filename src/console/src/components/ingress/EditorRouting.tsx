import { ArrowDown, ArrowUp, Plus, X } from "lucide-react";

import {
  BALANCE_LABELS,
  type Balance,
  type MatchKind,
  type ProxyOptions,
  type RedirectTo,
  RULE_KINDS,
  type RuleKind,
  type StaticRoot,
  type Target,
  defaultProxy,
} from "../../api/ingress";
import type { Service } from "../../api/types";
import { Button, Field, IconButton, Input, Select } from "../ui";
import {
  type EndpointForm,
  type FormRule,
  defaultProxyFor,
  isProxyKind,
  newKey,
  useTargetContext,
} from "./endpointForm";
import { Check, SectionHead } from "./EndpointParts";
import { TargetEditor } from "./TargetEditor";
import type { Member } from "../../api/ingress";

/** One route's settings, whether the default `/` or a path rule. */
interface RouteValue {
  kind: RuleKind;
  targets: Target[];
  standby: Target[];
  balance: Balance;
  websockets: boolean;
  proxy?: ProxyOptions;
  redirect?: RedirectTo;
  static_root?: StaticRoot;
}

function RouteFields({
  v,
  on,
  members,
  apps,
  prefix,
}: {
  v: RouteValue;
  on: (p: Partial<RouteValue>) => void;
  members: Member[];
  apps: Service[];
  prefix?: string;
}) {
  const p = v.proxy ?? defaultProxy();
  const setP = (x: Partial<ProxyOptions>) => on({ proxy: { ...p, ...x } });
  const r = v.redirect ?? { to: "", code: 301, keep_path: true };
  const s = v.static_root ?? { root: "", index: "index.html", spa: false };
  const canStrip = !!prefix && prefix.endsWith("/");
  return (
    <div className="space-y-2.5">
      {isProxyKind(v.kind) && (
        <>
          <TargetEditor
            label={v.kind === "balancer" ? "Active targets" : "Target"}
            hint={
              v.kind === "balancer"
                ? "Requests are spread over these."
                : "Where requests go."
            }
            value={v.targets}
            onChange={(targets) => on({ targets })}
            members={members}
            apps={apps}
          />
          {v.kind === "balancer" && (
            <>
              <TargetEditor
                label="Standby targets"
                hint="Used only when every active target is down."
                value={v.standby}
                onChange={(standby) => on({ standby })}
                members={members}
                apps={apps}
              />
              <Field
                label="Balancing"
                hint="IP hash keeps a client on one target."
              >
                <Select
                  value={v.balance}
                  onChange={(e) => on({ balance: e.target.value as Balance })}
                >
                  {(Object.keys(BALANCE_LABELS) as Balance[]).map((b) => (
                    <option key={b} value={b}>
                      {BALANCE_LABELS[b]}
                    </option>
                  ))}
                </Select>
              </Field>
            </>
          )}
          <div className="grid gap-x-4 gap-y-1.5 sm:grid-cols-2">
            <Check
              checked={p.preserve_host}
              onChange={(preserve_host) => setP({ preserve_host })}
              hint="Send the client's Host header upstream (off: the target's address)."
            >
              Preserve Host
            </Check>
            <Check
              checked={p.strip_prefix}
              disabled={!canStrip}
              onChange={(strip_prefix) => setP({ strip_prefix })}
              hint={
                canStrip
                  ? `Upstream sees /x instead of ${prefix}x.`
                  : "Only for prefix rules whose path ends in /."
              }
            >
              Strip prefix
            </Check>
            <Check
              checked={v.websockets}
              onChange={(websockets) => on({ websockets })}
            >
              WebSockets
            </Check>
            <Check
              checked={p.buffering}
              onChange={(buffering) => setP({ buffering })}
              hint="Off for streaming responses (server-sent events)."
            >
              Buffering
            </Check>
          </div>
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label="Read timeout (s)">
              <Input
                type="number"
                min={1}
                max={86400}
                value={p.read_timeout_s}
                onChange={(e) =>
                  setP({ read_timeout_s: Number(e.target.value) })
                }
              />
            </Field>
            <Field label="Connect timeout (s)">
              <Input
                type="number"
                min={1}
                max={300}
                value={p.connect_timeout_s}
                onChange={(e) =>
                  setP({ connect_timeout_s: Number(e.target.value) })
                }
              />
            </Field>
          </div>
        </>
      )}
      {v.kind === "redirect" && (
        <div className="grid gap-3 sm:grid-cols-[1fr_120px]">
          <Field label="Redirect to">
            <Input
              className="font-mono text-xs"
              value={r.to}
              placeholder="https://www.example.com"
              onChange={(e) => on({ redirect: { ...r, to: e.target.value } })}
            />
          </Field>
          <Field label="Code">
            <Select
              value={r.code}
              onChange={(e) =>
                on({ redirect: { ...r, code: Number(e.target.value) } })
              }
            >
              {[301, 302, 307, 308].map((c) => (
                <option key={c} value={c}>
                  {c}
                </option>
              ))}
            </Select>
          </Field>
          <Check
            checked={r.keep_path}
            onChange={(keep_path) => on({ redirect: { ...r, keep_path } })}
          >
            Keep the path and query
          </Check>
        </div>
      )}
      {v.kind === "static" && (
        <>
          <div className="grid gap-3 sm:grid-cols-[1fr_160px]">
            <Field
              label="Directory"
              hint="Must be inside an approved file root on every edge node."
            >
              <Input
                className="font-mono text-xs"
                value={s.root}
                placeholder="/srv/www/site"
                onChange={(e) =>
                  on({ static_root: { ...s, root: e.target.value } })
                }
              />
            </Field>
            <Field label="Index file">
              <Input
                className="font-mono text-xs"
                value={s.index}
                onChange={(e) =>
                  on({ static_root: { ...s, index: e.target.value } })
                }
              />
            </Field>
          </div>
          <Check
            checked={s.spa}
            onChange={(spa) => on({ static_root: { ...s, spa } })}
            hint="Unknown paths serve the index file (single-page apps)."
          >
            SPA fallback
          </Check>
        </>
      )}
    </div>
  );
}

const KindSelect = ({
  value,
  onChange,
  label,
}: {
  value: RuleKind;
  onChange: (k: RuleKind) => void;
  label: string;
}) => (
  <Select
    aria-label={label}
    value={value}
    onChange={(e) => onChange(e.target.value as RuleKind)}
  >
    {RULE_KINDS.map((k) => (
      <option key={k.id} value={k.id}>
        {k.label}
      </option>
    ))}
  </Select>
);

/** Routing: the default `/` route and the path rules (checked before it, in order). */
export function RoutingSection({
  f,
  set,
}: {
  f: EndpointForm;
  set: (p: Partial<EndpointForm>) => void;
}) {
  const ctx = useTargetContext(f.placement);
  const s = f.spec;
  const kind = s.kind ?? "proxy";
  const def: RouteValue = {
    kind,
    targets: f.targets,
    standby: f.standby_targets,
    balance: s.balance,
    websockets: s.websockets,
    proxy: s.proxy,
    redirect: s.redirect,
    static_root: s.static_root,
  };
  const onDef = (p: Partial<RouteValue>) =>
    set({
      spec: {
        ...s,
        ...(p.kind !== undefined
          ? { kind: p.kind, proxy: s.proxy ?? defaultProxyFor(p.kind) }
          : {}),
        ...(p.balance !== undefined ? { balance: p.balance } : {}),
        ...(p.websockets !== undefined ? { websockets: p.websockets } : {}),
        ...(p.proxy ? { proxy: p.proxy } : {}),
        ...(p.redirect ? { redirect: p.redirect } : {}),
        ...(p.static_root ? { static_root: p.static_root } : {}),
      },
      ...(p.targets ? { targets: p.targets } : {}),
      ...(p.standby ? { standby_targets: p.standby } : {}),
    });
  const setRule = (k: string, p: Partial<FormRule>) =>
    set({ rules: f.rules.map((r) => (r._k === k ? { ...r, ...p } : r)) });
  const move = (i: number, d: -1 | 1) => {
    const next = [...f.rules];
    const [x] = next.splice(i, 1);
    if (x) next.splice(i + d, 0, x);
    set({ rules: next });
  };
  const add = () =>
    set({
      rules: [
        ...f.rules,
        {
          _k: newKey(),
          path: "/api/",
          match_kind: "prefix",
          kind: "proxy",
          targets: [],
          standby_targets: [],
          upstreams: [],
          standby: [],
          balance: "round_robin",
          websockets: true,
          proxy: defaultProxy(),
        },
      ],
    });

  return (
    <div className="space-y-5">
      <section>
        <SectionHead
          title="Default route ( / )"
          hint="Every request no path rule matches."
          actions={
            <div className="w-40">
              <KindSelect
                label="Default route type"
                value={kind}
                onChange={(k) => onDef({ kind: k })}
              />
            </div>
          }
        />
        <RouteFields v={def} on={onDef} members={ctx.members} apps={ctx.apps} />
      </section>
      <section>
        <SectionHead
          title={`Path rules (${f.rules.length})`}
          hint="Matched with Nginx location rules (exact, regex in order, longest prefix); the default route takes the rest."
          actions={
            <Button type="button" onClick={add}>
              <Plus className="size-4" /> Add rule
            </Button>
          }
        />
        {f.rules.length === 0 && (
          <p className="text-xs text-faint">
            No path rules: everything goes to the default route.
          </p>
        )}
        <div className="space-y-3">
          {f.rules.map((r, i) => {
            const v: RouteValue = {
              kind: r.kind ?? "proxy",
              targets: r.targets ?? [],
              standby: r.standby_targets ?? [],
              balance: r.balance,
              websockets: r.websockets,
              proxy: r.proxy,
              redirect: r.redirect,
              static_root: r.static_root,
            };
            return (
              <div key={r._k} className="rounded-md border border-line p-2.5">
                <div className="mb-2 flex flex-wrap items-center gap-2">
                  <div className="w-28">
                    <Select
                      aria-label="Match"
                      value={r.match_kind}
                      onChange={(e) =>
                        setRule(r._k, {
                          match_kind: e.target.value as MatchKind,
                        })
                      }
                    >
                      <option value="prefix">Prefix</option>
                      <option value="exact">Exact</option>
                      <option value="regex">Regex</option>
                    </Select>
                  </div>
                  <Input
                    aria-label="Path"
                    className="min-w-0 flex-1 font-mono text-xs"
                    value={r.path}
                    placeholder="/api/"
                    onChange={(e) => setRule(r._k, { path: e.target.value })}
                  />
                  <div className="w-36">
                    <KindSelect
                      label="Rule type"
                      value={v.kind}
                      onChange={(kind) =>
                        setRule(r._k, {
                          kind,
                          proxy: r.proxy ?? defaultProxyFor(kind),
                        })
                      }
                    />
                  </div>
                  <IconButton
                    label="Move up"
                    disabled={i === 0}
                    onClick={() => move(i, -1)}
                  >
                    <ArrowUp className="size-4" />
                  </IconButton>
                  <IconButton
                    label="Move down"
                    disabled={i === f.rules.length - 1}
                    onClick={() => move(i, 1)}
                  >
                    <ArrowDown className="size-4" />
                  </IconButton>
                  <IconButton
                    label="Remove rule"
                    onClick={() =>
                      set({ rules: f.rules.filter((x) => x._k !== r._k) })
                    }
                  >
                    <X className="size-4" />
                  </IconButton>
                </div>
                <RouteFields
                  v={v}
                  prefix={r.match_kind === "prefix" ? r.path : undefined}
                  members={ctx.members}
                  apps={ctx.apps}
                  on={(p) =>
                    setRule(r._k, {
                      ...(p.kind !== undefined ? { kind: p.kind } : {}),
                      ...(p.targets ? { targets: p.targets } : {}),
                      ...(p.standby ? { standby_targets: p.standby } : {}),
                      ...(p.balance !== undefined
                        ? { balance: p.balance }
                        : {}),
                      ...(p.websockets !== undefined
                        ? { websockets: p.websockets }
                        : {}),
                      ...(p.proxy ? { proxy: p.proxy } : {}),
                      ...(p.redirect ? { redirect: p.redirect } : {}),
                      ...(p.static_root ? { static_root: p.static_root } : {}),
                    })
                  }
                />
              </div>
            );
          })}
        </div>
      </section>
    </div>
  );
}
