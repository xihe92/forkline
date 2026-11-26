# Error kinds

Every failure has a kind. The terminal node receives it in `PlanError::kind`,
logs and metrics label it with its short name, and each listener turns it into
its protocol's status.

| Kind | Short name | Raised when | HTTP | gRPC | TCP |
| --- | --- | --- | --- | --- | --- |
| `BadRequest` | `bad_request` | A module returned `NodeError::bad_request` | 400 | `INVALID_ARGUMENT` | 1 |
| `Module` | `module` | A module returned any other error, or a branch returned an unknown label | 500 | `INTERNAL` | 2 |
| `Deadline` | `deadline` | The deadline passed, or a failure came after it | 504 | `DEADLINE_EXCEEDED` | 3 |
| `Cancelled` | `cancelled` | The request was cancelled before the work could run | 503 | `CANCELLED` | 2 |
| `Panic` | `panic` | A module panicked | 500 | `INTERNAL` | 2 |
| `MergeConflict` | `merge_conflict` | Paths changed one key and `on_conflict = "error"` | 500 | `INTERNAL` | 2 |
| `Internal` | `internal` | The engine hit a state validation should rule out | 500 | `INTERNAL` | 2 |
| `Overloaded` | `overloaded` | A resource was full, such as a thread pool's queue | 503 | `RESOURCE_EXHAUSTED` | 4 |

Answers that don't come from a plan:

| Situation | HTTP | gRPC | TCP |
| --- | --- | --- | --- |
| Refused by admission control, or shed before starting | 503, `Retry-After: 1` | `RESOURCE_EXHAUSTED` | 4 |
| The terminal node itself failed | 500 | `INTERNAL` | 2 |
| Request body over `max_body_bytes` | 413 | n/a | connection closed |

A failure after the request's deadline has passed is reported as `Deadline`,
whatever the module called it.
