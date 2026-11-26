# How a request runs

A request enters through a listener, runs its plan's graph from the decode node
to the encode node, and leaves as one reply. This page follows one request
through each step, using the `quote` plan from [Plans](plans.md).

> **If you know Spring:** this is WebFlux's `DispatcherHandler` processing
> order. The listener plays the part of the `HttpHandler` adapter, the plan
> replaces handler mapping and the handler, and the encode node replaces
> result handling.

## The steps

1. **Admission.** The listener counts the request in. If admission control is
   configured and the server already has `red` requests in flight, the request
   is refused at once (HTTP 503, gRPC `RESOURCE_EXHAUSTED`, TCP status 4). See
   [Admission control](../server/admission.md).
2. **Snapshot.** The engine takes the current snapshot of plans and services.
   The request uses it to the end, even if a reload swaps in a new one.
3. **Deadline.** The deadline is the time the listener received the request
   plus the plan's `time_budget_ms`, or the caller's own deadline (gRPC's
   `grpc-timeout`) if that is earlier. A request that is already late, or that
   waited longer than `max_queue_ms` before it could start, is shed without
   running anything.
4. **Decode.** The root node turns the request body into values in the
   [request map](request-map.md).
5. **Run the graph.** Each node runs when its parent finishes. A node with two
   or more children forks; a branch picks which child runs; a join waits for
   its paths and merges them. See [Branching and forking](branching.md).
6. **Encode.** The encode node turns map values into the reply, and the reply
   goes out immediately.
7. **Detached paths.** Paths that can't reach the encode node keep running
   after the reply, until they finish or the deadline passes. Their failures
   are logged, not sent to the caller.

If any step fails, the engine cancels the request's other paths and runs the
plan's terminal node, which builds the error reply instead. See
[Errors and deadlines](errors.md).

## Where each step runs

Every node of a request runs on the request's one Tokio task, unless a fork
sets `spawn = true`. The paths of a fork interleave on that task: while one
path awaits a backend call, the others make progress. This keeps a request's
work together, avoids locking the request map, and makes a request cheap: the
engine adds about 1 to 4 µs to a request, depending on the plan's shape.

Detached paths move to a separate task when the reply goes out, so the reply
isn't held up by them.

## Joins and merging

A join's module is called once for each arriving path, one at a time, so it
needs no locks. The join continues once no running path can still reach it,
or as soon as its module returns `JoinStep::Done`, which cancels the paths
still on their way. Then the engine merges the paths' changes:

1. keys the join module wrote keep the join's value;
2. keys no path changed carry through from before the fork;
3. keys changed on exactly one path take that path's value;
4. keys changed on several paths are conflicts: the plan's `on_conflict`
   decides whether they are dropped or fail the request (the default).

## See also

- [Plans](plans.md): the file that describes the graph.
- [The request map](request-map.md): what the nodes read and write.
- [Shutdown](../server/shutdown.md): what happens to running and detached
  work when the server stops.
