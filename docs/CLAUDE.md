# CLAUDE.md: documentation (`docs/`)

An Astro Starlight site served at `/docs/`, so internal links use the `/docs/`
prefix. The root [`CLAUDE.md`](../CLAUDE.md) still applies.

```sh
npm ci
npm run build    # must succeed with no broken links before a PR
```

Docs are part of the definition of done. A change to behaviour, configuration,
API, CLI, UI, permissions, packaging or the upgrade path is not complete until
these pages describe it. Delete or rewrite pages that describe behaviour that
no longer exists.

## Structure (Diátaxis)

Each page has one purpose and sits in the matching folder of
`src/content/docs/`. The sidebar picks it up automatically.

| Folder | Kind | Answers | Style |
| --- | --- | --- | --- |
| `getting-started/` | Tutorial | "How do I start?" | Linear steps to a working result |
| `concepts/` | Explanation | "How does it work, and why?" | Mental model, diagrams, trade-offs; no step lists |
| `guides/` | How-to | "How do I do X?" | Prerequisites, steps, verification |
| `reference/` | Reference | "What exactly is Y?" | Every field, endpoint, flag and default |
| `development/` | Contributor | "How do I build, test or release Forge?" | For people changing Forge |

Don't mix kinds on one page; link to the other kind instead.

## Page format

- Frontmatter: `title`, a one-sentence `description`, and `sidebar.order`.
- Open with one or two sentences on what the page lets the reader do. Don't
  add an "Introduction" heading.
- Guides follow this order:
  1. **Before you begin**: roles, OS, prerequisites.
  2. Numbered **steps**, one action each.
  3. **Verify**.
  4. **Troubleshooting** (symptom → cause → fix).
  5. **Next steps**.
- Reference pages cover everything. Give the type, default, unit and an example
  for every key, field and flag. Change them whenever the code changes.
- Name the platforms. When Debian/Ubuntu, RHEL-likes and Windows differ, use
  tabs or a per-OS table. Say plainly when something is not supported.
- Use asides for consequences: `:::caution` for risk or downtime, `:::note` for
  context, `:::tip` for shortcuts. Name the Cedar action and role a task needs.

## Voice

- Use second person, present tense, active voice, short sentences and plain words.
- Put UI labels in **bold**, exactly as they appear. Put code, paths and values
  in `backticks`.
- Commands must be copy-pasteable and must work.
  - Use placeholders (`<node-name>`), never real secrets, hostnames or IPs.
  - Lab machines are `lab-a` … `lab-z`.
  - Example addresses are `192.0.2.x`.
- No marketing language, "simply" or "just".
- No dates or version history in the body. History goes in release notes and
  `DECISIONS.md`.

Use the `docs-writer` subagent for larger changes.
