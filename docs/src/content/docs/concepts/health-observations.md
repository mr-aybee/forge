---
title: Health observations
description: How Forge records health evidence about nodes and services, keeps it fresh, and turns changes into an ordered feed.
sidebar:
  order: 14
---

An observation is a timestamped piece of evidence: "this check on this
resource was in this state at this time". Health checks produce them. Alerts,
automation and observability read the changes they cause, instead of running
the checks again.

## What an observation holds

Each observation belongs to a **key**: a resource, the node the observed
instance lives on, and a subject.

| Part | Meaning | Example |
| --- | --- | --- |
| Resource | The node, service or cluster observed | `server:<id>`, `service:<id>` |
| Server | The node of the instance, for resources that span nodes | a service on `web-01` |
| Subject | What was observed, as a lowercase dotted name | `health.readiness.web` |

The value is a state:

| Kind | States |
| --- | --- |
| Readiness and liveness checks | `pending`, `passing`, `failing`, `unknown`, `disabled` |
| Startup checks | `waiting`, `succeeded`, `exhausted`, `unknown` |
| Node measurements | a number in thousandths of a unit (`percent`, `bytes`, `seconds`, `count`), or unknown |

Forge records three measurements for every node from its heartbeats:
`node.cpu.percent`, `node.mem.used_percent` and `node.disk.used_percent`
(the root filesystem). A measurement is written when it moves by more than
1 percentage point (1 % for other units), or to keep it fresh.
A failed check also carries a **reason**, so you can tell a service
failure from a broken check: `app_failure`, `timeout`, `connect_failed`,
`tls`, `auth`, `invalid_config`, `unsupported_executor`,
`location_unavailable`, `permission_denied` or `stale`. Diagnostic text is
cleaned of control characters and cut to 512 characters.

The **generation** identifies one run of a workload, such as a container id.
A new generation never inherits the state of the old one: a failure of a
replaced container can't affect its replacement.

## Freshness and unknown

Every observation says how long it counts as current. Forge measures that on
the **server's clock**, from the moment the observation arrived. The clock of
the node that ran the check is not trusted for freshness:

- A value whose freshness ran out reads as `unknown`. Forge keeps the last
  reported state next to it, so you still see what it was.
- `unknown` is not proof of failure, and not proof of recovery. A node that
  lost its connection makes its checks `unknown`; it doesn't make them fail.
- If a node's clock is more than 30 seconds off, Forge clamps the
  observation time to the server's clock and marks the row as `skewed`.

A background sweep on the server records the moment each value goes stale.

## The transition feed

When a key's effective state changes, Forge appends a **transition** to an
ordered feed: the key, the old and new state, the reason and the time. Each
transition has an increasing sequence number.

```diagram
alt: Health observation flow
flow:
  rows:
    - steps:
        - { label: agent checks, edge: observations }
        - { label: recorder (server), tone: accent, edge: current value }
        - { label: latest values }
    - steps:
        - { label: recorder (server), tone: accent, edge: on change }
        - { label: transition feed, edge: "" }
        - { label: "alerts, automation, observability" }
    - steps:
        - { label: stale sweep (server), edge: went unknown }
        - { label: transition feed }
```

A measurement changing value is not a transition, or the feed would fill
with every CPU sample. Only a measurement going unknown or coming back is.
Rules on numbers read the current values instead.

Consumers keep their own position in the feed. One check result produces one
transition, however many consumers read it. A replayed report (for example
after a crash) adds nothing, so a transition is never delivered twice for the
same evidence.

## What is stored, and for how long

Forge doesn't store every check result. That keeps the database small at
fleet scale:

| Table | Written | Kept |
| --- | --- | --- |
| Latest values | When the state changes, or when freshness drops below half | One row per key |
| History | On every change, plus one keep-alive row every 5 minutes | 7 days |
| Transitions | On every change | The newest 100,000 |

A node may only report about itself, and about a service where an
instance of it is placed on that node. Reports about other nodes, clusters,
unplaced services or anything else are dropped and logged. When a node
or a service is deleted, its current values and history are removed on
the next sweep; its transitions stay as the record of what happened.

## Who can see observations

Reading observations needs the `health.view` action on the resource. Every
role that has `view` also has `health.view`. See
[Authorization](/docs/concepts/authorization/).

## Related

- [`forge nodes health`](/docs/reference/cli/nodes/#forge-nodes-health)
- [REST API: Observations](/docs/reference/rest-api/#observations)
