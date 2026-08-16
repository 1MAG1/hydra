# hya-net

[![crates.io](https://img.shields.io/crates/v/hya-net.svg?style=flat-square)](https://crates.io/crates/hya-net)
[![docs.rs](https://img.shields.io/docsrs/hya-net?style=flat-square)](https://docs.rs/hya-net)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue?style=flat-square)](#license)

Range transport driving the [`hya-core`](https://crates.io/crates/hya-core)
scheduler: HTTP/1.1 and FTP over `tokio`, TLS via `rustls`, proxy support,
positioned writes, and chunk-level integrity.

[HYDRA](https://github.com/ja7ad/hydra) splits a multi-source downloader along
one seam: `hya-core` is an I/O-free state machine that decides which connection
fetches which byte range; **this crate contains all the syscalls**. It is
deliberately minimal — a hand-rolled HTTP/1.1 client over `tokio::net::TcpStream`
— because the point is to show the scheduler needs nothing from the transport
but *bytes arrived* and *when*.

## Two properties the design holds

- **Memory is independent of object size.** Bytes are written to their final
  file offset as they arrive (`SparseSink`, positioned `pwrite`-style writes) —
  never buffered whole, never reassembled. Resident memory is
  `O(connections × 64 KiB)` whether the object is 4 MB or 1 GB.
- **The scheduler stays I/O-free.** Everything in `hya-core` is driven by
  `on_bytes` / `tick`; the transfer loop here translates its `Action`s into
  range requests and cancellations.

## What's inside

- **HTTP/1.1 range client** — request construction, response-head parsing,
  capability probing (ranges honoured? validator present?), chunked decoding
  via an O(1)-consume frame buffer (`framebuf`).
- **HTTPS** — `rustls` with the `ring` provider, Mozilla roots
  (`webpki-roots`), SNI, session-resumption caching. Certificate verification
  can be disabled for self-signed internal mirrors — loudly, with a warning on
  every use.
- **FTP** — RFC 959 plus `SIZE`/`REST` (RFC 3659), behind the same `Fetcher`
  seam as HTTP, so protocol choice never touches the scheduler.
- **Proxies** — HTTP forward proxy (absolute-form requests, `CONNECT`
  tunneling) and SOCKS4/4a/5.
- **Politeness** — per-host connection ceilings, `Retry-After` parsing
  (delta-seconds and HTTP-date) with exponential backoff and jitter,
  token-bucket rate limiting, bounded redirect handling.
- **Integrity** — per-chunk digest manifests (`manifest`), streaming digests of
  out-of-order arrivals without buffering (`stream_digest`), header-advertised
  digest extraction (`digest`), and local Reed–Solomon parity for offline
  bitrot repair (`parity`).
- **Test origin** — a rate-shapeable in-process HTTP origin (`origin`) with
  real `Range`/`206` handling, programmable rate caps, and failure injection,
  so adversarial scenarios run hermetically over `tokio::io::duplex` instead
  of being asserted.

## Example

The high-level entry point is the transfer loop: give it targets, a scheduler,
and an output path, and it drives everything to completion.

```rust,no_run
use hya_core::{Capability, Scheduler, Source};
use hya_net::{transfer::run_transfer, Target, TcpConnector};
use std::sync::Arc;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let size = 100_000_000; // learned from a probe in practice
    let targets = vec![
        Target::direct_tls("mirror-a.example.com", 443, "/big.iso"),
        Target::direct_tls("mirror-b.example.com", 443, "/big.iso"),
    ];
    let sources = targets
        .iter()
        .map(|_| Source { caps: Capability::Full, ..Source::default() })
        .collect();
    let sched = Scheduler::new(size, sources, &[4, 4]);

    let (elapsed, requests) =
        run_transfer(Arc::new(TcpConnector), targets, &[4, 4], size, "big.iso", sched).await?;
    println!("done in {elapsed:.2}s over {requests} range requests");
    Ok(())
}
```

Variants expose more control: `run_transfer_tick` (explicit tick period, which
bounds repair latency), `run_transfer_observed` (per-connection state callback
for rendering progress — this crate owns sockets, the caller owns pixels),
`run_transfer_paced` (rate limiting), and `run_transfer_into` (existing sink).

The `Connector` trait is the transport seam: `TcpConnector` for real networks,
or an in-process duplex connector (see `origin::OriginSet`) for hermetic tests
— same scheduler, same transfer loop, no network.

## Where SIMD is (and is not) used

Byte-parallel paths get vectorization from measured, maintained sources rather
than hand-written intrinsics: `memchr` for protocol byte search (runtime
AVX2/SSE2/NEON dispatch, no `unsafe` here), an LLVM-autovectorized table lookup
for hex encoding (measured 11.8× over the `write!` form it replaced), and
delegation to `sha2` / `blake3` / `reed-solomon-simd` for the genuinely
compute-bound hashing and erasure coding. Vectorized paths are covered by
differential tests against scalar references.

## Module map

| Module | Contents |
|---|---|
| `transfer` | Drive a `Scheduler` over live connections to completion |
| `sink` | `SparseSink`: positioned writes, no reassembly buffer |
| `http` / `http_scheme` | HTTP/1.1 request/response, probing, per-range fetch |
| `ftp` / `ftp_origin` | FTP protocol and its test origin |
| `scheme` | The `Fetcher` seam: what a protocol must provide |
| `tls` | rustls transport, roots, session resumption |
| `socks` | SOCKS4/4a/5 and HTTP proxy support |
| `polite` | Backoff, `Retry-After`, rate limiting, redirects |
| `framebuf` | O(1)-consume read buffer for protocol framing |
| `digest` / `stream_digest` / `manifest` / `parity` | Integrity: header digests, out-of-order streaming digests, chunk manifests, Reed–Solomon parity |
| `origin` | Rate-shapeable in-process HTTP origin for hermetic tests |

## Relationship to the HYDRA workspace

| Crate | Role | License |
|---|---|---|
| [`hya-core`](https://crates.io/crates/hya-core) | Scheduler state machine, no I/O | MIT OR Apache-2.0 |
| **`hya-net`** (this crate) | All the syscalls: transports, TLS, proxies, integrity | MIT OR Apache-2.0 |
| `hya-cli` | The `hydra` binary: CLI, TUI queue manager, measurement harness | GPL-3.0-or-later |

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
