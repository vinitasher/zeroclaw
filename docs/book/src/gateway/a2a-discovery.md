# A2A agent discovery

ZeroClaw can present its agents to the outside world through the
[Agent2Agent protocol](https://a2a-protocol.org) (A2A, Linux Foundation). A2A
is how agents built on different frameworks, by different vendors, on separate
hosts discover and collaborate as peers without routing through a chat channel
as a makeshift bus.

This page covers the **discovery surface**: the well-known catalog card and the
per-alias agent cards a remote A2A client reads first. Inbound task execution,
outbound client tooling, and credential-to-peer binding are separate slices and
are not part of this surface.

> Tracked under RFC #7218.

## The one-agent-per-origin problem

A2A discovery centers on a single well-known URI:
`GET /.well-known/agent-card.json` returns one Agent Card describing one
agent's identity, capabilities, skills, endpoint, and auth. The mechanism
assumes **one agent per origin**.

A ZeroClaw install hosts many agents under `[agents.<alias>]`, each with its
own workspace, skill bundles, guardrails, and memory. There is no single
install-level agent to occupy the origin root. ZeroClaw resolves this with a
**catalog card**.

## What the two endpoints return

| Endpoint | Returns |
|---|---|
| `GET /.well-known/agent-card.json` | A ZeroClaw discovery **catalog card**: skills empty, enumerating the published aliases and each one's per-alias endpoint and card URL. It is **not** a runnable A2A agent; it advertises a `catalog` interface binding. |
| `GET /a2a/<alias>/.well-known/agent-card.json` | A spec-conforming A2A **agent card** for one published alias: a `JSONRPC` interface at the alias's endpoint, with the skills that alias exposes. |

The catalog card is the first-contact surface. A client reads it, sees the list
of published agents, then fetches each per-alias card to learn that agent's
skills and endpoint. The catalog deliberately does **not** merge every alias's
skills into one card: that would collapse the per-alias boundary and leak the
cross-agent skill surface.

Cards are built on demand from the canonical `[agents.<alias>]` config. There is
no stored second agent list, and the skill entries on a card resolve through the
same skill-bundle reader the dashboard uses, so the bundles stay the single
source of truth for what an agent can do.

## Enabling discovery

Exposure is **opt-in and default-closed** at every level. Nothing is reachable
until you turn it on.

### 1. Enable the server

```toml
[a2a.server]
enabled = true        # default false — master switch for the whole surface
bind = "127.0.0.1"    # default loopback; set non-loopback only deliberately
port = 18800          # default
```

With the server disabled (the default), every A2A route answers `404`.

### 2. Publish an agent

Even with the server on, an alias is absent from discovery until it opts in:

```toml
[agents.researcher.a2a]
published = true              # default false
exposed_skills = ["research"] # default empty — see below
```

An unpublished alias does not appear in the catalog and serves no per-alias
card.

### 3. Select exposed skills

`exposed_skills` is a **filter** over the alias's resolved skill set, not a
parallel registry. It names which skill ids from the agent's bundles appear on
its card:

- Empty (the default) advertises **no skills** on the card.
- A non-empty list selects matching skill ids; entries that do not resolve to a
  real skill in the alias's bundles are dropped.

The skill bundles remain canonical. This filter only chooses which of the
already-defined skills are visible to external agents.

## Public exposure

When ZeroClaw sits behind a reverse proxy, the endpoint URLs advertised on the
cards must match the public origin, not the internal bind address. Set
`public_base_url`:

```toml
[a2a.server]
enabled = true
public_base_url = "https://agents.example.com"
```

Card endpoint and card-URL fields are then built against that origin. When
`public_base_url` is empty, endpoints derive from `bind` and `port`.

The well-known discovery card is anonymous-readable by design (A2A discovery is
first contact, before any credential exchange). It therefore exposes only the
opted-in surface: published aliases and their selected skills, nothing more.
Binding to a non-loopback address is an explicit operator choice; do it only
behind TLS and a trusted proxy.

## Wire format

Cards serialize to the A2A v1.0 protobuf-JSON wire shape (camelCase field
names, `supportedInterfaces`, the security-scheme oneof wrappers). ZeroClaw
implements the card types directly rather than depending on a protobuf SDK,
keeping the single-static-binary footprint flat. The canonical A2A message
definitions are vendored alongside the gateway crate as the conformance
reference.

## Build feature

The discovery surface is compiled behind the gateway crate's `a2a` feature,
which is in the default feature set. The runtime surface stays default-closed
regardless: a standard build ships the code, but `[a2a.server] enabled = false`
means it does nothing until an operator opts in.
