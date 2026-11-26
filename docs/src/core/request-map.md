# The request map

Every request carries a map of named, typed values that its nodes read and
write. The decoder fills it from the request body; modules add their results;
the encoder builds the reply from it. A module can only touch the keys it
declares in its contract, and the plan checks the contracts against each other
before it serves a request.

> **If you know Spring:** the closest thing is the exchange's attributes
> (`ServerWebExchange.getAttributes()`), but typed and checked. Where a WebFlux
> handler receives a bound `@RequestBody` object, a Forkline decoder writes
> the object into the map and later nodes read it by key.

## Declaring a contract

`interface!` declares the keys a module reads and writes, with their types:

```rust
forkline::interface! {
    pub struct ReceiptsIo {
        opt   email_receipt: String,
        opt   sms_receipt: String,
        inout receipts: Vec<String>,
    }
}
```

It generates the struct `ReceiptsIo<'a>`, a view of the map with one accessor
per key. The access word decides what the module may do with each key:

| Access | Generated methods | Load-time check |
| --- | --- | --- |
| `in` | `key()` returns `Option<&T>` | Written upstream on every path that reaches the node |
| `opt` | `key()` returns `Option<&T>` | None: the key may be absent |
| `out` | `set_key(value)` | None |
| `inout` | `key()`, `set_key(value)`, `key_mut()` returns `Option<&mut T>` | None |
| `unset` | `unset_key()` | None; downstream nodes can't rely on the key |

Getters return `Option` even for `in` keys: the load-time check means a plan
can't be built where an `in` key could be missing, but a module stays correct
if it handles `None`.

Any type that is `Clone + Send + Sync + 'static` can be a value: a `String`, a
number, `bytes::Bytes`, a `Vec`, or your own struct.

## Whole objects or separate fields

A decoder can split the request into separate keys, one per field, or store
the whole parsed object under one key. Both work; the order examples store the
whole object:

```rust
{{#include ../../snippets/src/lib.rs:order}}
```

Later nodes declare `in order: Order` and read `io.order()`, a reference to the
stored value, not a copy.

Separate fields work better for values that drive routing or that different
paths change: a branch can read one field, and two paths can change two
different keys without a conflict. A whole object is less code, and is best
treated as read-only input: each module writes what it produces to its own new
key (`price_cents`, `receipts`) instead of changing the object.

## Keys in a plan

Within a plan, a key is identified by its name, and has one type everywhere.
If one module writes `price: u64` and another reads `price: String`, the plan
is rejected. A node can rename its module's keys with `rename` (see
[Plans](plans.md#renaming-keys)), which is how one module can write different
keys at different nodes.

When a plan loads, Forkline turns each key name into a slot number, so reading
or writing a key at request time is an array index, not a hash lookup.

## Values at a fork

When a plan forks, each path gets its own copy of the map. Values are kept
behind shared pointers, so the copy costs one pointer per key, however large
the values are. A value is copied for real only if one path changes it in
place (with `key_mut()`), and then only for that path.

At the join, the engine merges what the paths changed, using these rules:

1. keys the join module wrote keep the join's value;
2. keys no path changed carry through from before the fork;
3. keys changed on exactly one path take that path's value;
4. keys changed on several paths are conflicts.

A conflict either drops the key (`on_conflict = "drop"`) or fails the request
with a `merge_conflict` error (`on_conflict = "error"`, the default). Both are
logged and counted in `forkline_merge_conflicts_total`. To combine values that
several paths write, give each path its own key with `rename`, or have the join
module write the combined value, as `Receipts` does in
[Branching and forking](branching.md#joins).

## See also

- [Modules](modules.md): where contracts are used.
- [Branching and forking](branching.md): joins in practice.
- [Testing modules](../testing/modules.md): give a module inputs by key name.
