# Plans

A plan is a named graph of nodes in `plans.toml`. Each node runs one
registered module; links say which node runs after which. One file holds any
number of plans, and a listener routes each request to one of them.

> **If you know Spring:** a plan does the job of a functional endpoint's
> `RouterFunction` and the handler behind it, but as data. Where WebFlux wires
> a route to code with `route(GET("/quote"), handler::quote)`, Forkline maps
> the route to a plan in `server.toml` and the plan wires the steps.

## A first plan

This plan prices an order with standard or express delivery, depending on a
flag in the request:

```toml
{{#include ../../snippets/plans.toml:quote}}
```

Read it top to bottom:

- `root` is the node every request starts at; it must run a decode module.
- `terminal` is the node that builds the reply when anything fails; it must run
  a terminal module.
- `time_budget_ms` is how long a request may take, counted from when the
  listener received it.
- `links` are the edges. `"route -> standard : standard"` is a labelled link
  out of a branch: when the `ByFlag` module behind `route` returns the label
  `standard`, the `standard` node runs.
- `[plans.quote.nodes]` names each node and the module it runs. `standard`
  and `express` run the same module with different `config` tables.

The modules themselves are in [Modules](modules.md), and the branch in
[Branching and forking](branching.md).

## Plan settings

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `root` | string | required | Node every request starts at (a decode module) |
| `terminal` | string | required | Node that answers when the plan fails (a terminal module) |
| `time_budget_ms` | integer | `1000` | Time a request may take, from when it was received |
| `on_conflict` | `"error"` or `"drop"` | `"error"` | What a join does with a key several paths changed and the join didn't resolve |
| `links` | list of strings | `[]` | Edges: `"from -> to"`, or `"from -> to : label"` out of a branch |
| `nodes` | table | required | Nodes by name |

Each node:

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `module` | string | required | Name the module was registered under |
| `config` | table | none | The module's own settings, parsed into its `Config` type |
| `rename` | table | `{}` | Contract keys renamed for this node: `{ local = "plan_key" }` |
| `spawn` | boolean | `false` | On a fork: run each path on its own task (for CPU-heavy paths) |

Names of plans, nodes and labels are letters, digits, `_` and `-`.

## Renaming keys

A module's contract names its keys, but a plan may need the same module to
write different keys at different nodes. `rename` maps the module's name for a
key to the name the plan uses. In this plan, two nodes run `Notify` and each
writes its `receipt` under its own name, so the join can tell them apart:

```toml
email = { module = "Notify", config = { channel = "email" }, rename = { receipt = "email_receipt" } }
sms   = { module = "Notify", config = { channel = "sms" }, rename = { receipt = "sms_receipt" } }
```

## What is checked when a plan loads

Forkline checks every plan when the server starts and on every reload, and
reports every problem at once. A plan with any problem is rejected whole; on a
reload, the running plans stay in place.

Graph rules:

- `root` and `terminal` exist and are different nodes; the root has no incoming
  links and the terminal has no links at all;
- links name existing nodes, aren't listed twice and don't link a node to
  itself; the links form no cycle;
- every node is reachable from the root;
- out of a branch, every link carries a label and no label repeats; out of any
  other node, no link carries one;
- `time_budget_ms` is greater than 0.

Module rules:

- every `module` is registered, and its `config` parses into the module's
  config type (unknown keys are rejected when the type says so) and passes the
  module's own `build` checks;
- the root runs a decode module, and a decode module runs only at the root;
  the terminal runs a terminal module, and nowhere else;
- a node with labelled links runs a branch or multibranch module; a node with
  two or more incoming links runs a join module, and a join has at least two;
- there is exactly one encode node, and it has no outgoing links;
- `spawn = true` is set only on a fork;
- services and thread pools a module resolves while it is built exist.

Data rules:

- a key has the same type everywhere in the plan;
- `rename` names keys the module has, and doesn't map two keys to one;
- every `in` key a node reads is written upstream on every path that reaches
  it (the terminal is exempt: it can run after a failure anywhere).

## Checking plans before you deploy

Run the same checks without starting a server:

```sh
my-service check --config server.toml --plans plans.toml --services services.toml
forkline-plan check plans.toml   # graph rules only, no modules needed
```

`check` with `--config` also checks that every route points at a plan that
exists and every pool a module asks for is configured. To make `cargo test`
fail on a broken plan, see [Testing plans](../testing/plans.md).

## See also

- [Branching and forking](branching.md): forks, branches, joins.
- [plans.toml reference](../appendix/plans-services-toml.md).
- [Hot reload](../server/reload.md): changing plans on a running server.
