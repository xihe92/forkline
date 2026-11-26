# Errors and deadlines

When anything in a plan fails, Forkline stops the request's other paths and
runs the plan's terminal node, which builds the error reply. Every failure has
a kind, and the listener turns the kind into the protocol's status, so one
plan answers HTTP, gRPC and TCP callers consistently.

> **If you know Spring:** the terminal node does the job of a WebFlux
> `@ExceptionHandler` or `WebExceptionHandler` for the whole plan, and the
> kind-to-status mapping replaces `ResponseStatusException`. There is no
> exception hierarchy: a failure is a value with a kind.

## Failure kinds

| Kind | Raised when | HTTP | gRPC | TCP |
| --- | --- | --- | --- | --- |
| `BadRequest` | A module returned `NodeError::bad_request` | 400 | `INVALID_ARGUMENT` | 1 |
| `Module` | A module returned any other error | 500 | `INTERNAL` | 2 |
| `Deadline` | The request's deadline passed | 504 | `DEADLINE_EXCEEDED` | 3 |
| `Cancelled` | The request was cancelled before the work could run | 503 | `CANCELLED` | 2 |
| `Panic` | A module panicked | 500 | `INTERNAL` | 2 |
| `MergeConflict` | Paths changed the same key and `on_conflict = "error"` | 500 | `INTERNAL` | 2 |
| `Internal` | The engine hit a state validation should have ruled out | 500 | `INTERNAL` | 2 |
| `Overloaded` | A resource was full, such as a thread pool's queue | 503 | `RESOURCE_EXHAUSTED` | 4 |

Requests refused by admission control, or shed because they waited too long to
start, never reach the plan: they get HTTP 503 with `Retry-After: 1`, gRPC
`RESOURCE_EXHAUSTED` or TCP status 4.

## Failing a module

```rust
return Err(NodeError::bad_request("`ticker` must be letters and digits"));
return Err(NodeError::msg("inventory answered nonsense"));
return Err(NodeError::new(ErrorKind::Overloaded, "no capacity"));
```

Any standard error converts into a `Module` error with `?`, keeping it as the
source. A failed service call is such an error.

## The terminal node

The terminal gets a `PlanError`: the kind, the name of the node that failed,
and the message. It returns the reply to send:

```rust
{{#include ../../snippets/src/lib.rs:terminal}}
```

`Reply::error(kind, body)` carries the kind, and the listener picks the status
from it. The terminal decides what the caller sees: here, the message of a bad
request, and a generic message for everything else, so internal details stay
in the logs. Every failure is logged with its plan, node, kind and message,
and counted in `forkline_node_errors_total`.

If the terminal itself fails, the caller gets a plain HTTP 500 ("internal
error"), gRPC `INTERNAL` or TCP status 2.

## Deadlines

Every request has a deadline: the time the listener received it plus the
plan's `time_budget_ms`, or the caller's deadline if that is earlier (gRPC
callers send one as `grpc-timeout`). The engine enforces it everywhere:

- a node isn't started after the deadline;
- a running node is cut off at the deadline, and its future is dropped;
- service calls stop at the deadline, and gRPC calls pass the time left to the
  backend as `grpc-timeout`;
- a request that is already late when it arrives is shed without running.

A failure after the deadline has passed is reported as `Deadline`, even when
the module saw it as its own error, for example a service call cut off by the
same deadline. That way a slow backend shows up as 504s, not 500s.

Modules can read the deadline with `cx.deadline()` or `cx.remaining()`, for
example to skip optional work when little time is left.

## Cancellation and panics

When a request fails, the engine cancels its other paths: their futures are
dropped at their next `.await`. A module that loops over long work can check
`cx.is_cancelled()` or await `cx.cancellation().cancelled()`.

A panic in a module is caught. It fails the request with kind `Panic`, the
terminal answers, and the server keeps running. A panic in a thread-pool job
fails that job, not the pool's thread.

## See also

- [Error kinds](../appendix/error-kinds.md): the full mapping table.
- [Admission control](../server/admission.md): refusing requests early.
- [Blocking work and thread pools](blocking.md): work that can't be cut off.
