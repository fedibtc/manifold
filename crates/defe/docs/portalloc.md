# Port allocation

`defe-portalloc` provides cross-process allocation of local port ranges.


## Source

Adapt [Fedimint's upstream `utils/portalloc` implementation](https://github.com/fedimint/fedimint/tree/master/utils/portalloc).

Keep the same high-level design unless there is a clear reason to simplify.


## Requirements

- Separate workspace crate.
- Allocate contiguous port ranges.
- Coordinate across separate `defe exec` processes.
- Use an advisory file lock.
- Store reservations in JSON.
- Check that candidate ports can bind before reserving them.
- Default to a cache directory.
- Allow override through `DEV_DEFE_PORTALLOC_DATA_DIR`.


## Port range

The allocator uses:

```text
10000..32000
```

This avoids low well-known ports and avoids much of the OS ephemeral range.


## Expiration

Reservations expire after 120 seconds. The spawned process should bind the port
shortly after allocation. Expiration protects against abandoned reservations
from crashed test processes.


## API sketch

```rust
pub fn port_alloc(range_size: u16) -> anyhow::Result<u16>;
```

Return value is the first port in the allocated range.


## Tests

- Reject range size zero.
- Allocate non-overlapping ranges.
- Reclaim expired ranges.
- Skip ports that are already bound.
- Coordinate through lock state across multiple allocator instances.
