---
title: Diagrams in the docs
description: Write flow, state, tree and sequence diagrams as small YAML blocks that render as styled HTML.
sidebar:
  order: 8
---

This page shows you how to draw a diagram in a docs page. Write a fenced
code block with the language `diagram` and a short YAML description. At
build time `docs/src/plugins/diagrams.mjs` turns it into HTML styled by
`docs/src/styles/diagrams.css`. Diagrams follow the light and dark theme,
need no JavaScript and work on narrow screens.

Don't draw diagrams with box characters in a plain code block. Keep plain
code blocks for commands and their output.

## Common fields

Every block starts with one kind: `flow`, `states`, `tree` or `sequence`.

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `alt` | string | the kind name | Accessible name read by screen readers. Always set it |
| `caption` | string | none | Text shown under the diagram |

Labels, notes and edge text accept `` `code` `` and `**bold**`. Write
`\n` inside a double-quoted string for a line break. Quote any value that
contains `:`, `{`, `}`, `#` or starts with `` ` ``.

### Tones

Set `tone` on a node, state or tree item to change its border:

| Tone | Use for |
| --- | --- |
| `accent` | The component the diagram is about |
| `ok` | A successful end state |
| `warn` | A state that needs attention |
| `danger` | A failed or destructive state |
| `muted` | An external or inactive element (dashed border) |

## Flow

A chain of boxes joined by arrows. Use it for request paths and pipelines.

````md
```diagram
alt: SFTP gateway path
flow:
  steps:
    - { label: SFTP client, edge: SSH }
    - { label: gateway (Forge), note: "password = token", tone: accent }
    - { label: node }
```
````

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `direction` | `row` or `column` | `row` | `column` draws top to bottom. Use it for long chains or long labels |
| `steps` | list | required | The boxes, in order. A plain string is a label |
| `rows` | list of `{title, steps}` | none | Several chains, one under the other, instead of `steps`. Use it for branches |
| step `label` | string | required | Text in the box |
| step `note` | string | none | Smaller text under the label |
| step `edge` | string | empty | Text on the arrow to the next step |
| step `dir` | `fwd`, `back` or `both` | `fwd` | Arrow direction to the next step. `back` shows a connection dialled by the right side |

Row flows switch to top to bottom on screens narrower than 50rem.

## States

A main path of states, plus a list of the other transitions. Use it for
lifecycles.

````md
```diagram
alt: Job states
states:
  path:
    - { state: queued, edge: claim }
    - { state: running, edge: all steps ok, tone: accent }
    - { state: succeeded, tone: ok }
  transitions:
    - { from: running, to: failed, when: retries exhausted, tone: danger }
```
````

| Field | Type | Description |
| --- | --- | --- |
| `path` | list | The usual order of states. Each has `state`, and optionally `edge`, `dir` and `tone` |
| `transitions` | list | Other moves: `from`, `to`, `when`, `tone` (of `to`) and `fromTone` |

## Tree

Nested items. Use it for hierarchies and containment.

````md
```diagram
alt: Project, environments and services
tree:
  - label: Project
    tone: accent
    children:
      - label: "Environment: test"
        children:
          - { label: "Service: api", note: system service }
```
````

Each item has `label`, and optionally `note`, `tone` and `children`. A plain
string is a label.

## Sequence

Actors with lifelines and the messages between them, top to bottom. Use it
for protocols and sign-in flows.

````md
```diagram
alt: MCP approval sequence
sequence:
  actors: [Agent, Forge, Person]
  messages:
    - { from: Agent, to: Forge, label: "tools/call" }
    - { at: Forge, note: store approval }
    - { from: Person, to: Forge, label: Approve and run }
```
````

| Field | Type | Description |
| --- | --- | --- |
| `actors` | list of strings | Column headings, left to right |
| message `from`, `to`, `label` | strings | An arrow between two actors. Arrows can point either way |
| note `at`, `note` | strings | A note on one actor's lifeline |
| note `span` | string | Stretch the note from `at` to this actor |

Sequences with many actors scroll sideways on narrow screens.

## Verify

Run `(cd docs && npm run build)`. An unknown actor, a missing label or
invalid YAML fails the build with a `diagram:` error that names the problem.
Then open the page with `npm run dev` and check it in both themes.

## Next steps

- [Website and docs site](/docs/development/website/): how the docs share
  the website's design.
