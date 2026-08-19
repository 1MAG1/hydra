# hya-core

I/O-free multi-source download scheduler: interval algebra, divergence-triggered
repair, and liveness — as a pure state machine.

`hya-core` is the scheduler kernel of [HYDRA](https://github.com/ja7ad/hydra).
It decides *which connection should fetch which byte range next* when an object
is being pulled from one or more mirrors over range requests. It contains **no
sockets, no clock, no async runtime, and no `unsafe`** (`#![forbid(unsafe_code)]`),
and it allocates nothing in the steady state. The caller drives it: feed
observations, call `tick(now)`, act on the returned `Action`s. That inversion is
what lets the same scheduler run under a discrete-event simulator in tests and
under real HTTP/FTP in [`hya-net`](https://crates.io/crates/hya-net) with no
changes.

## What it does

- **Dynamic range partitioning** — byte ranges are tracked client-side as a
  sorted, coalesced interval set, so a slow or stalled connection's remaining
  work can be repartitioned and handed to faster connections at any time.
- **Divergence-triggered steal-to-equalize** — instead of waiting for timeouts,
  the scheduler compares each connection's projected finish time and steals from
  laggards when divergence exceeds a threshold.
- **Fast collapse detection** — a two-sided CUSUM plus dual-window estimator
  (`detect`) identifies a rate collapse in far less time than an EWMA smoother
  can, because a smoother is structurally the wrong tool for detecting a step
  change.
- **Online concurrency admission** — `admission` probes connection counts one at
  a time against measured marginal goodput and settles where extra connections
  stop paying.
- **Capability-aware scheduling** — sources that honour ranges with a strong
  validator get full scheduling; range-but-no-validator sources are pinned;
  range-ignoring sources are raced (`Capability`).
- **Format sniffing** — `format` classifies files from magic bytes, extension,
  and media type, with magic bytes taking precedence.

## Verified invariants

Two properties are exposed as separate predicates and exercised by
property-based tests (`proptest`) across randomized schedules, rates, and
failure patterns:

- `Scheduler::coverage_holds()` — **safety**: assigned and completed ranges
  never overlap and never leave a gap; every byte is owned exactly once.
- `Scheduler::liveness_holds()` — **liveness**: every reachable state has an
  enabled transition that decreases remaining work within a bounded window.

## Example

```rust
use hya_core::{Action, Capability, Scheduler, Source};

// Two mirrors, one connection each, fetching a 100 MB object.
let sources = vec![
    Source { caps: Capability::Full, gamma_est: 10.0e6, ..Source::default() },
    Source { caps: Capability::Full, gamma_est: 2.0e6,  ..Source::default() },
];
let mut sched = Scheduler::new(100_000_000, sources, &[1, 1]);

let mut now = 0.0;
while !sched.is_complete() {
    // 1. Let the scheduler decide.
    for action in sched.tick(now) {
        match action {
            Action::Request { conn, range } => {
                // issue `GET` with `Range: bytes=lo-(hi-1)` on `conn`
                let _ = (conn, range);
            }
            Action::Cancel { conn } => {
                // stop reading this connection; its range was reclaimed
                let _ = conn;
            }
        }
    }

    // 2. Report what the network delivered (offset-credited variant:
    //    `on_bytes_at(conn, off, n, now, dt)` for out-of-order safety).
    sched.on_bytes(0, 65_536, now, 0.05);

    // 3. Invariants are cheap enough to assert in a loop.
    debug_assert!(sched.coverage_holds());
    debug_assert!(sched.liveness_holds());

    now += 0.05;
}
```

There is no step 4: the scheduler never blocks, sleeps, or reads a clock.
`now` is whatever timebase the caller has — simulated seconds in tests,
`Instant`-derived seconds in production.

The libraries are deliberately permissive so they remain usable as dependencies;
only the assembled tool is copyleft. See
[LICENSING.md](https://github.com/ja7ad/hydra/blob/main/LICENSING.md) for the
reasoning.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/ja7ad/hydra/blob/main/LICENSE-APACHE))
- MIT license ([LICENSE-MIT](https://github.com/ja7ad/hydra/blob/main/LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
