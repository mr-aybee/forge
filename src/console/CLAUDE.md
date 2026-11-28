# CLAUDE.md: web console (`src/ui`)

React 19, TypeScript, Vite, Tailwind v4 tokens, TanStack Query, Radix and
lucide icons. The root [`CLAUDE.md`](../../CLAUDE.md) still applies.

```sh
npm ci
npm run dev      # dev server, proxies the API to a local forge-server
npm run build    # type-check and production build; must pass before a PR
```

## Rules

- **Server access lives in `src/api/`.**
  - Queries and mutations are defined there, with query keys from one key factory.
  - Components call hooks. They never build URLs or key arrays.
  - Mutations invalidate through `useAction`.
- **Server data is never held in `useState` + `useEffect`.** Use a query, so
  switching the subject cannot show stale data.
- **No derived state in effects.** Compute it during render, or key the
  component on its identity.
- **Dialogs mount when open**, with `{open && <Dialog/>}` or a `key`, so the
  initial state is the reset. Form state is one object, not a `useState` per field.
- **A failed request shows an error**, never an empty state.
- **Types are declared once** in `src/api/`. Never redeclare a server type in
  a page or component.
- **Use the shared pieces.**
  - Use `Confirm`, never native `confirm()`.
  - Use the formatters, `errorText`, `usePrefs` and the components in `ui.tsx`.
- **Stable keys** on editable lists, and **keyboard-reachable** row actions.
- Clean up timers, sockets and subscriptions on unmount.
- **No `any`, `as never` or `@ts-ignore`** without a comment that says why.
- The style is slim, dense and operator-focused. Use the design tokens, never
  raw colours.
- Use the `ui-engineer` subagent for larger UI work.
