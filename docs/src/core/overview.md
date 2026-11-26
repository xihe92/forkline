# Overview

Forkline runs backend requests as plans: small modules wired into a graph in a
TOML file. You write each module once, in Rust, and declare which request
attributes it reads and writes. The plan decides the order, which steps run at
the same time, and how their results come back together. This page explains
why Forkline works this way, when it fits and when it doesn't, and how it uses
threads.

> **If you know Spring:** the WebFlux overview asks "Spring MVC or WebFlux?".
> Forkline answers a narrower question. A WebFlux controller composes calls in
> code with `Mono.zip` and `flatMap`; a Forkline plan composes them in
> configuration, and the engine runs the composition with the same deadline,
> error handling and metrics for every step.

## Why plans

A request handler in a typical service does four things: it parses the
request, calls a few backends (some of them in parallel), combines what came
back, and builds a response. In code, the order of those calls, what runs in
parallel, the timeout of each step and the handling of a failure are mixed into
the business logic. Changing the shape means changing and redeploying the code.

Forkline separates the two:

- **Modules** hold the logic. Each one is a Rust type that implements one
  trait, such as `Module` for an async step or `Join` for combining parallel
  results. A module never calls another module.
- **Plans** hold the shape. A plan names its nodes, the module each node runs,
  and the links between nodes. A node with two children forks; a node with two
  parents joins.
- **Contracts** connect the two. A module declares the keys it reads and
  writes with `interface!`. When a plan loads, Forkline checks that every key
  a node reads is written upstream on every path that reaches it, with the same
  type. A plan that could read a missing key never starts serving.

Because plans are data, they reload while the server runs. A request always
runs on one snapshot of the plans and services from start to finish, so a
reload never mixes two configurations inside one request.

## When to use Forkline

Forkline fits services whose work is a pipeline over backend calls:

- an API that fans out to several backends and merges their answers;
- a gateway or aggregation layer with per-request deadlines;
- a service whose flow changes more often than its logic, or differs per
  route, customer or experiment;
- a latency-sensitive service where predictable tails and low memory matter.

It fits less well when:

- the service is mostly CRUD over one database: a plan adds little over a
  handler, and Forkline has no database layer yet (use an async driver such as
  SQLx from a module);
- the work is long-running or streaming: a plan answers one request with one
  reply, within a time budget, and there is no streaming request or response;
- the team wants the whole flow in one place in code: plans move it to
  configuration, which some teams find harder to follow.

As with WebFlux, the advice is to start small: put one route on a plan, measure
it, and grow from there.

## Concurrency model

Forkline runs on Tokio's multi-threaded runtime, one worker thread per CPU core
by default. Each request is one Tokio task:

- nodes in a straight line run one after another on that task;
- at a fork, the paths run concurrently on the same task, interleaving at each
  `.await`, unless the fork node sets `spawn = true`, which gives each path its
  own task so CPU-heavy paths can use several cores;
- only `Module` steps are async; decode, branch, join, encode and terminal
  steps are plain functions over data already in the request map.

Like WebFlux's event loop, this model assumes that modules don't block. A
module that waits must `.await`, which frees the worker thread for other
requests. Synchronous work that takes more than about a millisecond belongs on
`cx.blocking` or a named thread pool. Forkline times every node and reports
one that holds a worker thread too long; see
[Blocking work and thread pools](blocking.md).

## Performance

The engine itself adds microseconds: on an Apple M5 laptop, a 7-node plan with
one fork and join costs 2.1 µs at p99, with no network and modules that do no
work. Real requests are dominated by the network and the backends, so Forkline
won't make a slow backend fast.

What it does change is cost. In a benchmark against Spring Boot 4.2 on JDK 25,
where both services parse an HTTP request and publish it to Kafka, Forkline
used about a tenth of Spring MVC's memory (7 MB idle against 223 MB) and about
two thirds of its CPU per request, with similar median latency. The method and
the full results are in [Performance](../testing/performance.md).

## See also

- [How a request runs](request-lifecycle.md): the steps of one request.
- [Plans](plans.md) and [Modules](modules.md): the two halves of a service.
- [Coming from Spring](../coming-from-spring.md): a concept map.
