---
title: Website and docs site
description: How forge.orbyntech.com is built, how the website and the documentation share one design, and how to run and deploy it.
sidebar:
  order: 7
---

`forge.orbyntech.com` serves two static sites from one container: the
website at `/` and this documentation at `/docs/`. Both are built from this
repository and share one set of brand tokens.

| Path | What | Stack |
| --- | --- | --- |
| `website/` | Home, features, use cases, security, download and license pages | React 19, TypeScript, Vite, Tailwind CSS v4, pre-rendered to static HTML |
| `docs/` | Guides, concepts and reference | Astro Starlight, base path `/docs` |
| `website/src/styles/tokens.css` | Colours, fonts and radii for both sites | CSS custom properties |
| `website/Dockerfile`, `website/nginx.conf` | The image that serves both | Node build stages, unprivileged nginx |

## One design

`tokens.css` is the only place colours are defined. The website maps the
tokens onto Tailwind colour names in `website/src/styles/site.css`. The docs
import the same file from `docs/src/styles/forge.css` and map it onto
Starlight's `--sl-color-*` variables. The docs header shows links back to
the website through `docs/src/components/SiteNav.astro`.

Both sites store the light/dark choice under the `starlight-theme` key in
`localStorage`. They share an origin, so the choice carries across.

## Content

Copy, links and figures for the website live in
`website/src/content/site.ts`. Keep it in sync with `README.md` and
`BENCHMARKS.md`. Every feature and use case links to a docs page; check that
the page exists when you add one.

The license page renders the repository's root `LICENSE` (Apache License
2.0), the same text that ships in every release as `LICENSE.txt`.

## Links in the docs

The docs are built with `base: "/docs"`. Write internal links with the
prefix:

```md
[Installation](/docs/getting-started/installation/)
```

`starlight-links-validator` fails the build on a link without the prefix or
to a page that does not exist.

## Run locally

```sh
(cd docs && npm ci && npm run dev)       # http://localhost:4321/docs/
(cd website && npm ci && npm run dev)    # http://localhost:5173, proxies /docs/ to 4321
```

`npm run build` in `website/` runs the TypeScript check, the client build
and the server-side render build. It then writes one HTML file per page,
plus `sitemap.xml` and `robots.txt`, with `scripts/prerender.mjs`. Pages are
pre-rendered so that search engines and link previews see the full content.
React then hydrates them for the theme toggle, menu, copy buttons and
download tabs.

## Build and run the image

From the repository root:

```sh
docker build -f website/Dockerfile -t forge-website .
docker run --rm -p 8080:8080 forge-website
```

Open `http://localhost:8080` and `http://localhost:8080/docs/`.
`website/Dockerfile.dockerignore` limits the build context to `website/`,
`docs/`, `branding/` and the license file.

The image listens on port 8080 as a non-root user and answers
`GET /healthz`. It sends a Content Security Policy, HSTS and other security
headers. Terminate TLS in front of it.

## Publish

Pull requests and pushes only check that the website and docs build
(`ci.yml`). The image is built and pushed to
`ghcr.io/mr-aybee/forge-website` by the release workflow, after each
release, and by hand with **Actions → Website image → Run workflow**
(`.github/workflows/website.yml`) for a fix between releases. A release dry
run builds the image without pushing it.

| Tag | When |
| --- | --- |
| `<version>` | Each release, for example `1.4.0` or `1.4.0-rc-01` |
| `<major>.<minor>`, `latest` | Each stable release, for example `1.4` for `v1.4.0` |
| `sha-<short>` | Each release and each manual run |
| `<branch>` | A manual run on that branch, for example `develop` |

To deploy, pull `:latest`, or a version or `sha-` tag to pin, on the web host and put it
behind the TLS edge for `forge.orbyntech.com`.
