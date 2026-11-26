# Branching and forking

A plan can split a request into paths in three ways. A **fork** runs all of a
node's children at the same time. A **branch** runs one child, chosen per
request. A **multibranch** runs any set of its children, chosen per request.
Paths come back together at a **join**.

> **If you know Spring:** a fork is `Mono.zip` over several calls; a branch is
> the `if` in a handler that decides which service to call; a join is the
> function you pass to `zip` to combine the results. In Forkline the shape is
> in the plan and the decisions are small modules.

## Forks

Any node that isn't a branch and has two or more children is a fork. Its
children start together, each with its own copy of the request map, and run
concurrently on the request's task: while one path waits for a backend, the
others make progress.

In this plan, `decode` forks: pricing and notification run at the same time.

```toml
{{#include ../../snippets/plans.toml:notify}}
```

For paths that do heavy CPU work, set `spawn = true` on the fork node. Each
path then runs on its own Tokio task, so several paths can use several cores.
For one heavy step, a [thread pool](blocking.md) is usually the better tool.

## Branches

A branch module looks at the map and returns the label of the one child to
run. The plan gives each outgoing link a label:

```toml
links = [
  "decode -> route",
  "route -> standard : standard",
  "route -> express : express",
  "standard -> merge", "express -> merge",
  "merge -> encode",
]
```

The condition lives in the module:

```rust
{{#include ../../snippets/src/lib.rs:branch}}
```

Returning an error, as `ByFlag` does for an unknown flag, fails the request
like any module error. Returning a label the plan doesn't have fails the
request too, with a message listing the labels the plan does have.

Each child of a branch sees only its own path, so a key written on the
`standard` path is not available on the `express` path. The load-time data
check follows this: a node after a branch may only rely on keys written on
every path that reaches it.

The paths of a branch end at a join, because a node with two incoming links
must be a join. When only one path ran, a join that touches no keys lets it
through, and the merge rules carry that path's changes forward:

```rust
{{#include ../../snippets/src/lib.rs:pass_join}}
```

## Multibranches

A multibranch returns any number of labels, including none, and the chosen
children run concurrently, like a fork's:

```rust
{{#include ../../snippets/src/lib.rs:multibranch}}
```

Each notification path runs the same `Notify` module; the plan renames its
`receipt` key per node, so the paths write different keys and don't conflict.

## Joins

A join's `on_path` is called once for each path that reaches it, one at a
time, in the order the paths finish. It gets two views of the map: `path`, the
arriving path's values, and `merged`, the result so far. It returns
`JoinStep::Wait` to wait for more paths, or `JoinStep::Done` to continue at
once and cancel the paths still on their way (useful for "first answer wins").

```rust
{{#include ../../snippets/src/lib.rs:join}}
```

`Receipts` sorts the list because paths arrive in whatever order they finish.
The join continues when no running path can still reach it; with a
multibranch that chose nothing, that is immediately.

After the last call, the engine merges the paths with the rules in
[The request map](request-map.md#values-at-a-fork). Here `receipts` is written
by the join, so the join's value wins; `price_cents` was written on one path,
so it carries through.

## Detached paths

A path that can't reach the encode node, such as a node with no children that
records an audit entry, is detached. It runs with the rest of the request, but
the reply doesn't wait for it: when the encode node runs, the reply goes out and
detached paths continue on a separate task until they finish or the request's
deadline passes. Their failures are logged, not returned. At shutdown, the
server gives them what is left of the drain timeout.

## See also

- [The request map](request-map.md): copies at a fork and merge rules.
- [Plans](plans.md): link syntax and load-time checks.
- [Testing modules](../testing/modules.md): test a join with the merge rules.
