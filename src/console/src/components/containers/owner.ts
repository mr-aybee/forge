import { useAllEnvironments, useServices } from "../../api/hooks";
import { useStacks } from "../../api/stacks";
import type { Service } from "../../api/types";

/**
 * What a Docker container belongs to in Forge, when anything:
 * - `app`: a Forge service (`forge-app-<environment>-<service>`), or a compose
 *   service registered from a stack (matched through compose labels);
 * - `stack`: a compose stack (label `com.docker.compose.project`).
 */
export type ContainerOwner =
  | { kind: "app"; id: string; name: string; app: Service; stackId?: string; stackName?: string }
  | { kind: "stack"; id: string; name: string; service?: string }
  | undefined;

export const COMPOSE_PROJECT = "com.docker.compose.project";
export const COMPOSE_SERVICE = "com.docker.compose.service";

/** Pure matcher; every input may be partial. */
export function resolveOwner(
  name: string | undefined,
  labels: Record<string, string> | null | undefined,
  apps: Service[] | undefined,
  nsNameById: Record<string, string>,
  stacks: { id: string; name: string }[] | undefined,
): ContainerOwner {
  const list = Array.isArray(apps) ? apps : [];
  const n = (name ?? "").replace(/^\//, "");
  if (n.startsWith("forge-app-")) {
    const hit = list.find((a) => {
      const ns = a.spec?.environment || nsNameById[a.environment_id] || "";
      return ns && `forge-app-${ns}-${a.name}` === n;
    });
    if (hit) return { kind: "app", id: hit.id, name: hit.name, app: hit };
  }
  const project = labels?.[COMPOSE_PROJECT];
  const service = labels?.[COMPOSE_SERVICE];
  if (project) {
    const stack = (stacks ?? []).find((s) => s.name === project);
    if (service) {
      const svc = list.find((a) => a.runtime === "compose-service" && a.spec?.service === service && (a.spec?.stack === project || (stack && a.spec?.stack_id === stack.id)));
      if (svc) return { kind: "app", id: svc.id, name: svc.name, app: svc, stackId: svc.spec?.stack_id ?? stack?.id, stackName: project };
    }
    if (stack) return { kind: "stack", id: stack.id, name: stack.name, service };
  }
  return undefined;
}

/** Owner of one container, from the cached service, environment and stack lists. */
export function useContainerOwner(name: string | undefined, labels: Record<string, string> | null | undefined, enabled = true): ContainerOwner {
  const apps = useServices(undefined, undefined, enabled).data;
  const environments = useAllEnvironments().data ?? [];
  const stacks = useStacks({}, enabled).data;
  const nsNameById: Record<string, string> = {};
  for (const ns of Array.isArray(environments) ? environments : []) nsNameById[ns.id] = ns.name;
  return resolveOwner(name, labels, apps, nsNameById, (stacks ?? []).map((e) => ({ id: e.stack.id, name: e.stack.name })));
}

/** Resolver for many rows at once (one set of queries). */
export function useOwnerResolver(enabled = true) {
  const apps = useServices(undefined, undefined, enabled).data;
  const environments = useAllEnvironments().data ?? [];
  const stacks = useStacks({}, enabled).data;
  const nsNameById: Record<string, string> = {};
  for (const ns of Array.isArray(environments) ? environments : []) nsNameById[ns.id] = ns.name;
  const flat = (stacks ?? []).map((e) => ({ id: e.stack.id, name: e.stack.name }));
  return (name: string | undefined, labels: Record<string, string> | null | undefined) => resolveOwner(name, labels, apps, nsNameById, flat);
}
