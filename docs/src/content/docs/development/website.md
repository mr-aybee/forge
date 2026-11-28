---
title: Website and docs site
description: Where the website lives, how forge.orbyntech.com combines it with this documentation, and how to build and run the docs.
sidebar:
  order: 7
---

`forge.orbyntech.com` serves two static sites from one container: the
website at `/` and this documentation at `/docs/`. The website has its own
repository; the documentation stays here, next to the code it describes.

| Part | Repository | Path |
| --- | --- | --- |
| Website (home, features, download, license pages) | `github.com/orbyntech/forge-website` | the repository root |
| Documentation (Astro Starlight) | `github.com/orbyntech/forge` | `docs/` |
| Brand tokens (colours, fonts, radii) | both | `src/styles/tokens.css` there, `docs/src/styles/tokens.css` here |

The docs keep their own copy of the brand tokens. When you change the
palette or fonts, change both copies, and the console's theme in
`src/console/src/styles.css`, together.

## Build the docs

```sh
cd docs
npm ci
npm run build      # must pass with no broken links
npm run dev        # http://localhost:4321/docs/
```

The docs are served under `/docs/`, so internal links start with `/docs/`.

## Build the site image

The image is built in the website repository. Its Dockerfile clones this
repository at a branch or tag (`FORGE_REF`, default `master`), builds
`docs/`, and serves it at `/docs/` beside the website:

```sh
git clone https://github.com/orbyntech/forge-website.git
cd forge-website
docker build --build-arg FORGE_REF=master -t forge-website .
docker run --rm -p 8080:8080 forge-website     # http://localhost:8080 and /docs/
```

The website repository's **Website image** workflow pushes
`ghcr.io/orbyntech/forge-website` on every push to its `master`, or by hand
with a chosen Forge ref. A docs change here reaches the live site the next
time that image is built.
