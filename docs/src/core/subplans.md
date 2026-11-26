# Subplans

A module can run another plan of the same snapshot inline, with
`cx.execute_plan`, and use its reply. This lets several plans share a sequence
of steps without repeating it in each plan.

> **If you know Spring:** this is like one handler calling another through the
> router instead of through HTTP: the same request context, no network hop.

## Calling a plan

```rust
{{#include ../../snippets/src/lib.rs:subplan}}
```

```toml
{{#include ../../snippets/plans.toml:price}}
```

`execute_plan(plan, body)` runs `plan` from its decode node with `body` as the
request body and returns its `Reply`.

## What a subplan shares

- **The request.** The subplan runs with the caller's request id, request
  environment (headers, peer, route), services and snapshot, so a reload
  in between doesn't change it.
- **The deadline.** The subplan's deadline is the earlier of its own
  `time_budget_ms`, counted from the call, and the caller's deadline.
- **Cancellation.** Cancelling the caller cancels the subplan.
- **Tracing.** The subplan's span is a child of the caller's.

## Results and errors

A failure inside the subplan runs the subplan's own terminal node, and that
error reply comes back as `Ok(reply)` with an error status, so the caller can
inspect it. `reply.into_result()` turns it into an `Err`, as in the example
above.

`execute_plan` itself returns `Err` when the plan doesn't exist or when calls
nest too deeply: a subplan may call another subplan, up to 8 levels.

## See also

- [Modules](modules.md): the request context.
- [Errors and deadlines](errors.md).
