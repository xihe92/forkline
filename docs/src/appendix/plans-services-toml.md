# plans.toml and services.toml reference

Both files are reloaded together while the server runs (see
[Hot reload](../server/reload.md)). Unknown keys are rejected, except inside a
node's `config` table, which belongs to the module.

## plans.toml

```toml
{{#include ../../snippets/plans.toml:quote}}
```

### `[plans.<name>]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `root` | string | required | Node every request starts at; runs a decode module |
| `terminal` | string | required | Node that answers when the plan fails; runs a terminal module |
| `time_budget_ms` | integer | `1000` | Time a request may take, from when it was received |
| `on_conflict` | `"error"` or `"drop"` | `"error"` | What a join does with a key several paths changed that it didn't write |
| `links` | list of strings | `[]` | `"from -> to"`, or `"from -> to : label"` out of a branch |
| `nodes` | table | required | Nodes by name |

### `[plans.<name>.nodes]`, one entry per node

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `module` | string | required | Name the module was registered under |
| `config` | table | none | The module's settings, parsed into its `Config` type |
| `rename` | table | `{}` | `{ local_key = "plan_key" }` for this node |
| `spawn` | boolean | `false` | On a fork: run each path on its own task |

The checks a plan must pass are listed in
[Plans](../core/plans.md#what-is-checked-when-a-plan-loads).

## services.toml

```toml
{{#include ../../snippets/reference/services.toml}}
```

### Every service

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `kind` | string | required | `http`, `grpc`, or a kind registered in code |
| `timeout_ms` | integer | none | Timeout for each attempt |
| `retries` | integer | `0` | Retries after a retryable failure |
| `retry_budget_percent` | number | `20` | Retries may add at most this share of extra calls |
| `retry_backoff_ms` | integer | `0` | Pause before each retry |
| `concurrency_limit` | integer | none | Calls in flight at once |
| `load_shed` | boolean | `false` | At the limit, fail at once instead of waiting |
| `breaker` | `{ failures, open_ms }` | none | Open after `failures` consecutive failures, for `open_ms` |

### `kind = "http"`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `url` | string | required | Base URL; request paths are joined onto it |
| `connect_timeout_ms` | integer | none | TCP connect timeout |
| `pool_max_idle_per_host` | integer | reqwest's default | Idle connections kept per host |

### `kind = "grpc"`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `url` | string | required | Endpoint, such as `http://backend:50051` |
| `connect_timeout_ms` | integer | none | TCP connect timeout |
