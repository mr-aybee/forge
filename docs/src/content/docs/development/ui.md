---
title: Web UI
description: Structure of the React app, data flow and conventions.
sidebar:
  order: 4
---

`src/ui` — React 19, TypeScript, Vite, Tailwind CSS, TanStack Query, Radix
primitives.

```sh
cd src/ui
npm ci
npm run dev        # http://localhost:55132, proxies /api and /oauth to :55130 (FORGE_API and FORGE_UI_PORT override)
npm run build      # dist/ — serve with ui_dir in forge.toml
```

## Structure

| Path | Purpose |
| --- | --- |
| `src/api/client.ts` | `fetch` wrapper: same-origin cookies, CSRF header, problem-details errors, MFA step-up retry |
| `src/api/hooks.ts` | Query and mutation hooks, and the live-events WebSocket |
| `src/api/types.ts` | API types, including `ConsoleLine` and `OnboardingNode` |
| `src/app/` | Router (typed `validateSearch` per route), layout, toasts, per-user preferences, step-up, terminals |
| `src/components/ui.tsx` | Primitives: buttons, `CopyIcon`, `CopyButton`, `CopyValue` (click-to-copy value with a trailing copy icon), `Status`, `Badge`, tables, forms, `Modal`, `Drawer`, `Confirm`, `RowMenu`, `PointerMenu` and `useRowMenu` (right-click), `PageCrumb` and `ActionBar` (full-page forms); `ago`, `time`, `duration`, `bytes` |
| `src/components/Pager.tsx` | Client-side paging for long lists: `usePaged(items, { reset })` returns `{ rows, page, pages, pageSize, total, setPage, setPageSize }`; `<Pager paged={…} />` shows **Rows** 10 / 20 / 50, the range (`1–10 of 1,234`) and previous / next, and hides itself at 10 rows or fewer. Pass the filter as `reset` so a new filter returns to page 1. Every table that lists nodes uses it |
| `src/lib/clipboard.ts` | `copyText()`: the only way the console writes to the clipboard. Uses the Clipboard API on secure origins and falls back to `execCommand("copy")` on plain HTTP; returns `false` when blocked so callers can show `COPY_BLOCKED`. Never call `navigator.clipboard.writeText` directly |
| `src/components/SearchSelect.tsx` | Searchable dropdown with the `<select>` props contract; `Select` in `ui.tsx` is this component |
| `src/components/TreeMultiSelect.tsx` | Searchable two-level multi-select (group → leaf, e.g. service → node) with tri-state checkboxes, ARIA tree keyboard support and a `{ groups, leaves }` selection; used by the Logs **Service** filter |
| `src/pages/Projects.tsx`, `src/pages/explorer/ProjectExplorer.tsx`, `src/lib/explorerTree.ts`, `src/components/ProjectDialogs.tsx` | The project explorer at `/projects` (search `sel`, `q`, `cluster`, `runtime`, `health`): the project → environment → service tree, the project and environment panes, and the project create/edit/delete dialogs |
| `src/components/ServiceTable.tsx` | Shared service rows used by **Services** (list and **By environment** layouts) |
| `src/components/EnvironmentDialogs.tsx` | Environment dialogs: new (requires a project), rename, move or attach to a project, move to cluster, delete |
| `src/components/service-editor/` | The service editor used by **New service** (`/services/new`) and the **Configuration** tab: kind cards (**Container**, **Systemd**), one section component per card, the sticky section rail with its status marks (`StatusMark`, `sections.ts`) and the compose YAML mapping (`yaml.ts`) |
| `src/components/service/` | The service page's **Overview** tab: `ServiceStatusStrip` (replicas, last deploy, endpoint, log ingest keys), `ServiceNodes` (one compact line per node with chips, **Logs** and a ⋯ / right-click menu, **Add node**) and `ServiceSummary` (definition and **On the node**, long values wrapped in full with copy buttons) |
| `src/components/PlacementDialog.tsx` | Replica or standby choice when a service is placed on a further node; checks Docker Engine for container services |
| `src/components/ingress/` | `types.ts` (Site, SiteSpec, Balance labels), `sites.ts` (`useSites`, never throws; `sitesForApp`, `fanOut`), `EndpointDialog.tsx` (the endpoint editor, shared by the Ingress page and **Expose through ingress** on a service, takes `initial`), `ServiceRoutes.tsx` (the service page's Routes block), `Routing.tsx` (replica / standby chips) |
| `src/components/feed/Feed.tsx`, `src/components/filters/`, `src/components/charts/StackedHistogram.tsx` | Shared feed pieces for **Logs**, **Audit log** and key/value detail grids: `Meta`/`MetaGrid` (`wrap` shows long values in full), `FeedTail`, `QueryInput`, `TimeRange`, `CheckDropdown`, `StackedHistogram` |
| `src/components/containers/` | `ContainerLogs` (tail, filter, follow live; shared by the container page and the node's Containers tab), `owner.ts` (maps a container to its Forge service or stack from its name or compose labels) and `OwnerLink` |
| `src/app/terminals.tsx`, `src/components/terminal/TerminalDock.tsx` | The bottom panel's tabs. A session has a `kind`: `node` (SSH), `container` (exec) or `logs`. A `logs` tab carries a `LogSpec` union (`source: "service"` today); a new log source adds a variant there and a case in `LogPane`. `openLogs(spec)` activates an open tab for the same source instead of adding one. Log tabs have no pop-out, login or terminal settings |
| `src/components/LogPane.tsx`, `src/api/logFollow.ts` | Log tab body and `useLogFollow`, the segment-by-segment follow loop (starts on mount, stops on unmount or pause, resumes from its cursor) |
| `src/components/Console.tsx` | Terminal panel for install and job output, and its feeds: `JobConsole`, `TokenConsole` |
| `src/components/JobsDrawer.tsx` | Jobs drawer and `JobDetail` (status band, step timeline, error with a hint, result card, console, raw payload and result), reused wherever a job is shown. The drawer groups jobs by time, folds runs of identical background checks into one row (`×12`), and hides healthy background checks unless **Show background checks** is on (`localStorage` `forge.jobs.background`); failed checks always show, in amber |
| `src/components/JobVisuals.tsx` | `KindIcon` (category icon in a tinted circle), `StatusIcon`, `JobStatusPill`, `jobTone` and tone classes, `ResultCard` (flat JSON results as key/value badges), `JsonView` (highlighted JSON) |
| `src/components/jobMeta.ts` | Human job titles and targets ("Install redis · lab-c", "Ingress status check · lab-c"), `jobAction` (title without target), `jobCategory`, `isBackground` (read-only checks, classified by operation), timings |
| `src/components/CancelJob.tsx` | Cancel button with confirmation for queued or running jobs |
| `src/components/JobLog.tsx` | `NodeJobTabs`: one tab per node for multi-node installs |
| `src/app/nav.ts` | Fleet navigation (`NAV_SECTIONS`): headed sections (Workloads, Infrastructure, Operations, Admin) of items and collapsible groups, at most two group levels. Items take an optional `cap` (a field of the console capabilities), `badge` (a `NavBadgeKey` from `api/navBadges.ts`) and `tag` (`new`, `beta`). `visibleSections(caps)` / `visibleNav(caps)` drop items whose flag is not `true`, drop empty groups and sections, and render a group with one visible child as that child. `NAV` is the flat list of top-level entries. `gateOf(path)` finds the gate of a route. `flatItems` feeds the header title and the command palette. See [Navigation gating](#navigation-gating) |
| `src/api/access.ts` | `useMyCapabilities()` (`GET /api/v1/me/capabilities`, key `accessKeys.capabilities` under `["me"]`, `staleTime` 60 s) and the `ConsoleCapabilities` type |
| `src/app/RouteGate.tsx` | Wraps every page in the shell: a gated route the user may not open shows `NoAccess` (from `ui.tsx`) instead of the page |
| `src/components/FleetNav.tsx` | Renders the sections in the sidebar: headings, counters from `useNavBadges` (`src/api/navBadges.ts`, built on the existing queries and fetched only with the matching capability), tags, tree keyboard navigation, expanded state in `localStorage` (`forge.nav.open`), the active page's groups open by themselves, flyout menus and icon dots when the sidebar is collapsed |
| `src/components/Pins.tsx` | Pinned items: the **Pinned** sidebar section and collapsed flyout, `PinToggle` for detail headers and `usePinAction` for row menus. Hooks and types in `src/api/pins.ts` (`GET/PUT/DELETE /api/v1/me/pins`, optimistic through `useAction`); a missing endpoint hides everything |
| `src/components/CommandPalette.tsx` | <kbd>Ctrl</kbd>+<kbd>K</kbd>: pages, actions, and nodes, services and clusters from the cached list queries (services and local names load only while it is open) |
| `src/components/OnboardingDrawer.tsx` | Console and error of a node that is being onboarded |
| `src/components/SettingsLayout.tsx` | Settings shell: sticky left rail of sections (sidebar `Row` look), a horizontal scroller on narrow windows. Used by **Profile** (`pages/Settings.tsx`): sections are `/profile/$section`; `/profile`, `/settings/$section` and the old `#connected-apps` / `#remote-access` anchors redirect (the latter to **Tunnels**). `/settings` itself is the server-level page `pages/ServerSettings.tsx` (hooks in `src/api/server.ts`) |
| `src/components/tunnels/` | Port tunnels (D-100): `parts.tsx` (`TunnelTable`, target links, `tunnelCommand`), `CreateGrant.tsx`, `ServicePorts.tsx` (the read-only **Ports** block on service, stack and package pages), `MyTunnels.tsx` (Settings → Tunnels). `pages/Tunnels.tsx` is **Administration → Tunnels**. Hooks `useTunnels`, `useTunnelHistory`, `useTunnelGrants`, `useServicePorts` never throw; an older server without the endpoints yields `supported: false` |
| `src/components/UpdateBadge.tsx` | Header pill when a newer Forge release is out; links to `/admin/updates` (`pages/ForgeUpdates.tsx`). Both use `useForgeUpdate`, `useForgeReleases` and `useApplyForgeUpdate` from `api/hooks.ts` |
| `src/pages/` | One file per route, lazy-loaded |

`src/app/Layout.tsx` renders the terminal pop-out windows (`/popout/…`) and
the MCP consent page (`/connect`) without the console shell.

## Navigation gating

The console hides what the server would refuse. The server stays the
authority: every endpoint authorizes its own requests, and the console only
avoids showing entries and pages that would fail.

1. `GET /api/v1/me/capabilities` returns one boolean per gated area, computed
   with the Cedar authorizer (see the
   [REST reference](/docs/reference/rest-api/#console-capabilities) for each flag's
   action).
2. An item in `NAV` names the flag it needs with `cap`. `FleetNav` and the
   command palette show it only when the flag is `true`; a group with no
   items left is hidden. While the capabilities load, only items without a
   `cap` show, so forbidden entries never flash.
3. `RouteGate` in the layout checks the route of the current page with
   `gateOf(path)`: the gated item whose `to` is the path or a parent of it.
   Detail routes inherit their list's gate (`/nodes/<id>` uses **Nodes**).
   Without the flag it renders `NoAccess`.
4. The query key sits under `["me"]`: signing in, stepping up and granting or
   revoking a role binding invalidate `["me"]` and refresh the flags.

To gate a new page, add a field to `ConsoleCapabilities` in
`src/apps/forge-server/src/console_access.rs` and `src/api/access.ts`, map it
to the action the page's main endpoint authorizes, set `cap` on its `NAV`
item, extend the test in `tests/it/console_access.rs` and document the field.

## Conventions

- The session never touches JavaScript: no tokens in `localStorage`.
- Server state lives in TanStack Query; WebSocket events invalidate queries.
- Hooks return complete, normalized objects. An older server without the
  endpoint, or a failed request, gives empty defaults rather than a crash
  (see `useTraffic`, `useOnboardingNodes`, `useOnboardingConsole`,
  `useForgeUpdate`).
- Unsupported actions explain why (`title`) instead of disappearing.
- Light and dark themes come from the tokens in `styles.css`; never use
  hard-coded colours. Terminal and console panels use the `term-*` tokens
  (`bg-term`, `text-term-fg`, `text-term-prompt`, `text-term-bad`), which
  stay dark in both themes.
- Access colours: the `grant-*` tokens (`grant`, `grant-add`,
  `grant-remove`, `grant-narrow`, each with a `-soft` background, and
  `grant-line`) colour access tags, and the `level-*` tokens give each
  resource level (workspace, project, environment, service, cluster,
  node) its own chip colour. Use `Tag` (states: held, add, remove,
  inherited, narrow), `TagLegend` and `LevelChip` from
  `components/access/` rather than styling tags by hand.
- Command and job output goes through `Console`: `$ ` lines are prompts,
  `error:`, `forge-install: error:` and `installer failed (exit N)` lines
  are errors. It follows the end until the user scrolls up and has a copy-all
  button.
- Name jobs with `jobTitle` and link their target with `TargetLink`, rather
  than showing raw kinds and ids.
- Keyboard-first, ARIA labels on controls, Radix for dialogs and menus.
- Dropdowns are `Select` (`SearchSelect`), never a plain `<select>`.
- A table row with a `RowMenu` also opens the same actions on right-click:
  build the `RowAction[]` once, pass it to `RowMenu` and spread
  `useRowMenu().rowProps(actions, label)` on the `<tr>`, then render its
  `menu` once. The browser menu stays on links, form fields and selected text.
- Disabled actions say why: `title` on buttons, `hint` on `RowAction`.
- Large create forms are pages, not modals (`/stacks/new`,
  `/stacks/new-container`, `/services/new`): a `PageCrumb` back link,
  `PageHeader`, the form, and an `ActionBar` with Cancel and the submit
  button. Preselect through search parameters (`?node=`, `?environment=`).
  Keep `Modal` for short forms and confirmations.
- The sidebar is always the fleet navigation. A per-cluster view is a filter
  (`/nodes?cluster=<id>`, `/services?cluster=<id>`, `/ingress?cluster=<id>`)
  or a tab of the cluster page (`/clusters/<id>?tab=overview|network|containers`),
  never a separate route.
- List pages put their filters in a `FilterBar` (`components/filters/`): an
  optional status `Segmented` control with counts, a fixed-width search,
  pill dropdowns (`FilterDef`, single or multi select), **Clear filters** and
  the `ViewToggle` on the right. Every filter is typed URL search declared in
  `validateSearch` (helpers in `lib/search.ts`), so a filtered view is a link.
- Event feeds (**Logs**, **Audit log**) share one layout. Row one is the
  full-width `QueryInput` with its joined **Run** | **Live** control. Row two
  holds joined chips on the left (levels or outcomes, plus `TimeRange`:
  presets, **All** and a **Custom** popover) and `CheckDropdown` pills
  (searchable, grouped checkboxes, multi or `single`) on the right. Below sit
  a `StackedHistogram` (`components/charts/`) behind a **Chart** toggle
  remembered in `localStorage`, then a `<ul>` of one-line rows. Each row is a
  `<button aria-expanded>` with a compact `stamp()` time (full `time()` in the
  tooltip), and it expands to a `MetaGrid` of `Meta` key/value lines
  (`components/feed/Feed.tsx`). `FeedTail` loads older pages when it scrolls
  into view.
- Layout modes: declare the page's options (`VIEW_LIST`, `VIEW_CARDS`,
  `groupedView("cluster", "By cluster", Icon)`), read the mode with
  `usePageView(key, options, search.view, setUrl)` (`?view=` wins, then the
  remembered choice) and render the grouped mode with `GroupedSections`,
  which reuses the page's table rows inside collapsible sections.
- `PageHeader` is one compact row: eyebrow, title, a one-line description
  (full text in the tooltip) and the actions. A tab panel that needs its own
  note and actions uses `SectionBar`, not a second header.
- Edit forms show `UnsavedBar` only while the draft differs from the server
  value: Discard, Save (with spinner) and an optional extra action (the
  service editor's **Save and deploy**). It sticks to the bottom of the
  content area and asks with `Confirm` before leaving the page while dirty.
  Success is a toast; there is no always-visible save strip.
- Motion uses the `--motion-fast`, `--motion-base` and `--ease-out` tokens:
  pages fade and rise 4 px on route change, `FadeSwitch` (and Radix tab
  panels) fade on tab and view switches, controls with the `press` class
  scale to 0.97 when pressed, and dialogs and drawers fade by `data-state`.
  Everything is off under `prefers-reduced-motion` (spinners keep turning).
- Lists show `Skeleton` while loading, `ErrorNote` on failure and `Empty`
  with a next step when there is nothing; mutations toast success and
  `errorText(e)` on failure.
- On solid `accent`, `ok` or `bad` fills use `text-bg`, not `text-white`: those
  colours are light in the dark theme.
- Budget: under 200 KB of gzipped initial JavaScript (about 185 KB today);
  `npm run build` prints the chunk sizes.
