# CLAUDE.md: project website (`website/`)

The public site at forge.orbyntech.com: React, TypeScript and Vite, served by
nginx. The same image also serves the docs at `/docs/`. The root
[`CLAUDE.md`](../CLAUDE.md) still applies.

```sh
npm ci
npm run build
docker build -f website/Dockerfile -t forge-website .   # from the repo root
```

- `src/styles/tokens.css` is the shared brand. The console and docs follow it.
- Links to releases and issues come from `src/content/site.ts`. They point at
  `github.com/mr-aybee/forge`. Don't hard-code them in pages.
- The license page renders the root `LICENSE`.
- Logos live in `branding/`. `forge-mark.svg` is the source.
- Don't use marketing superlatives. Claims must match what the docs and
  `BENCHMARKS.md` say.
