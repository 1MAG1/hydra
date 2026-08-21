# hya-ffi — `libhydra`

A stable C ABI over the HYDRA download engine.

`hya-core` and `hya-net` are a strong Rust download implementation. This crate
turns them into an **embeddable engine**: one static or shared library, one
header, and a contract a program written in C, Go, Swift, Kotlin, Dart, C# or
Python can hold on to across releases.

```text
                         Applications
                              │
              ┌───────────────┼────────────────┐
           Desktop         Android            iOS
           C/C++/Go       Kotlin/Java        Swift
              └───────────────┼────────────────┘
                              │
                         HYDRA FFI
                         libhydra
                              │
                 ┌────────────┴────────────┐
             hya-core                   hya-net
                 └────────── Rust ─────────┘
```

## Building

```sh
make ffi          # libhydra.a, libhydra.dylib/.so, and the header
make header       # regenerate include/hydra.h from the Rust definitions
make ffi-test     # the Rust ABI suite plus the C conformance program
```

The deliverable is:

```text
include/hydra.h
target/release/libhydra.a          # and libhydra.so / libhydra.dylib / hydra.dll
```

The same header works against the static and the shared library. On Windows,
define `HYDRA_USE_SHARED` when compiling against the DLL.

## Hello, download

```c
#include "hydra.h"

hydra_engine_config_t cfg;
HYDRA_ENGINE_CONFIG_INIT(&cfg);
cfg.max_connections = 8;              /* a ceiling, not a target */
cfg.state_path = "hydra-state.json";  /* durable jobs across restarts */

hydra_engine_t *engine = hydra_engine_create(&cfg);

const char *urls[] = { "https://example.com/big.iso" };
hydra_job_config_t job;
HYDRA_JOB_CONFIG_INIT(&job);
job.urls = urls;
job.url_count = 1;
job.output_path = "big.iso";

hydra_job_id_t id;
hydra_job_create(engine, &job, &id);
hydra_job_start(engine, id);

for (;;) {
    hydra_event_t ev;
    if (hydra_event_wait(engine, 250, &ev) != HYDRA_OK) continue;
    if (ev.kind == HYDRA_EVENT_PROGRESS) { /* render ev.progress */ }
    if (ev.kind == HYDRA_EVENT_COMPLETED) break;
    if (ev.kind == HYDRA_EVENT_FAILED)   break;
}

hydra_engine_shutdown(engine, 5000);
hydra_engine_destroy(engine);
```

A complete, working version of this is in [`examples/ffi-c/download.c`](../../examples/ffi-c/download.c) —
mirrors, pause on Ctrl-C, resume, error reporting and all.

## The five decisions everything else follows from

**1. The C ABI is canonical.** Not the Rust ABI, not C++, not JNI. Every other
binding is a thin idiomatic layer over the same symbols, so there is one
implementation of the engine and not five.

**2. Rust internals are opaque.** The header names `hydra_engine_t` and a
`uint64_t`. No `Vec`, `String`, `Option`, `Arc` or future appears anywhere, so
the implementation can be rewritten without breaking a single binding.

**3. Job identity is a durable integer, not a pointer.** A pointer dies with the
process. Job `1842` survives an app restart, a UI rebuild, a killed Android
service, and an engine recreated from a state file. The application owns
identity, hydra owns download state, the OS owns execution.

**4. The event queue is the async interface.** Callbacks are optional and
marked experimental. A callback means hydra calls into your runtime from a
thread it created, at a moment you did not choose — the shape that breaks on
the JVM, on .NET, in cgo and under Swift concurrency. A queue you drain becomes
a Go channel, a Kotlin `Flow`, a Swift `AsyncStream`, a Dart `Stream`, a Python
iterator.

**5. Bytes never cross the boundary.** The ABI carries control, state, progress
and errors. The object is written straight to its destination by positioned
writes, which is what keeps resident memory independent of file size. There is
deliberately no `hydra_read_data`.

## The contract

| | |
|---|---|
| **Ownership** | Memory allocated by hydra is freed by hydra, through the matching `*_free` and never through `free()`. Strings you pass *in* are borrowed for that call only. |
| **Encoding** | Everything is UTF-8. Invalid UTF-8 is `HYDRA_ERR_INVALID_ARGUMENT`, never a lossy conversion. |
| **Errors** | Every fallible call returns a code. Detail — message, `errno`, HTTP status — is in a thread-local slot readable with `hydra_last_error()`. Branch on the code; never parse the message. |
| **Panics** | No Rust panic crosses the boundary. Every export is wrapped; an escape becomes `HYDRA_ERR_INTERNAL`. |
| **Threads** | Engine and job calls are thread-safe. Event consumption is safe but intended for one consumer. `hydra_engine_destroy` must not race with anything. |
| **Runtime** | The engine owns its threads. Your program needs no async runtime and no particular thread. |
| **Versioning** | `HYDRA_FFI_ABI_VERSION` is independent of the library version. Within one ABI version, fields are added only to the end of versioned structs, existing fields never move, enum values never change meaning, ownership rules never change. The header carries `_Static_assert`s for every struct size and key offset, so *your* compiler checks the layout too. |
| **Language** | C11 or C++11 and later. Every ABI-visible enum *value* is represented as a `uint32_t` in every mode. |
| **Callbacks** | `user_data` is never owned and never freed by hydra. It must outlive the registration. |

Every function in `hydra.h` restates its own threading, blocking and allocation
behaviour. That is the artifact binding authors read.

## Backpressure

The event queue is bounded, because an unbounded one turns a slow UI into an
out-of-memory kill on a phone. Bounding alone is not enough either, so the queue
is split by priority:

- **Progress events coalesce.** At most one is pending per job; a newer sample
  replaces an older one. Five hundred queued progress events for one job are
  not five hundred facts.
- **Terminal events are never dropped.** `COMPLETED`, `FAILED`, `CANCELLED` and
  `ENGINE_SHUTDOWN` are what your state machine turns on; losing one strands a
  job in the UI forever.
- **Life-cycle events drop oldest-first** once the bound is reached, and every
  drop is counted in `hydra_event_t.dropped_events`, so a consumer can see it is
  falling behind instead of guessing.

Ordering is guaranteed **within a single job** and only there — `JOB_STARTED`
before `RESOLVED` before `PROGRESS`, `VERIFYING` before `COMPLETED`, a terminal
event last. There is no ordering guarantee between jobs, because they run
concurrently on threads hydra owns.

There is **one consumer**. Several threads may call the event functions safely,
but each event is delivered exactly once — a thread that drains while waiting
for job A will consume and discard job B's completion unless it keeps it. Drain
in one place and dispatch from there.

## Persistence, and why it is the mobile feature

A desktop app can keep the engine alive. A mobile app cannot — the process is
suspended, the service is stopped, the app is killed and rebuilt, and none of
that is a failure. So set `state_path` and the engine records what it needs to
resume.

What is stored is **engine state, not download data**: the bytes stay in the
destination file, and what is persisted is the identity, the sources, the
destination, the size and — the part that actually matters — the **range map**.
A partially downloaded file written by positioned writes is full of holes, so
its length says nothing about which bytes are present. Only the recorded spans
do.

The write is atomic — a temporary file and a rename — so no state file is ever
half-written. That is not the same as durable: nothing is fsynced, so a sudden
power loss can lose the newest snapshot and leave the previous consistent one.

`hydra_engine_restore` brings jobs back as `HYDRA_JOB_PAUSED` and starts
nothing. That is deliberate: whether work may run *now* is the platform layer's
decision, not the engine's.

**Credentials never leave the process.** Passwords, proxy passwords and the
`Authorization`, `Proxy-Authorization` and `Cookie` headers appear in no
snapshot, no event, no error message, no log line, no metric and no state file —
that last one matters because a state file is an ordinary file that frequently
gets backed up. Userinfo embedded in a URL (`ftp://user:pass@host/path`) is
stripped at `hydra_job_create` and moved into the job's credentials, so the URL
hydra stores and reports carries no secret either. Re-arm a restored job with
`hydra_job_set_credentials` before starting it; the names of the headers that
were withheld are recorded so you can tell that you have to.

## Where the line is

hydra owns the download. The platform owns execution.

| hydra decides | the platform decides |
|---|---|
| How to split ranges | Whether the app may run at all |
| Which source is faster | Whether the network is available or metered |
| Whether to steal a stalled range | Whether the battery allows it |
| How many connections to open | Whether a service owns the work |
| How to verify the file | Whether a notification is shown |
| | Where a file may be written |

The engine never reads a battery gauge or asks `ConnectivityManager` anything.
The platform layer translates those conditions into `hydra_runtime_policy_t` and
the engine adjusts its connection ceiling, event rate and retry aggressiveness
accordingly.

## Multi-source

Passing several URLs means several mirrors of *the same object*. This is a
correctness gate, not an optimisation: two mirrors that disagree produce a file
assembled from both, of exactly the right length, that is not either object — a
corruption every length check passes. Mirrors that do not agree on size and on a
**strong** validator are dropped rather than mixed in, and a weak validator is
not evidence of agreement, because the specification lets one compare equal
across representations that are merely equivalent.

`hydra_job_get_sources` (experimental) makes the result observable:

```text
Mirror A   13.8 MB/s   3 connections
Mirror B    7.4 MB/s   2 connections
Mirror C    0.4 MB/s   stalled
```

## A note on `output_path`

A destination can be re-aimed only while a job is created, paused, failed or
cancelled — an active one returns `HYDRA_ERR_INVALID_STATE`. A running transfer
has connections writing at absolute offsets into a file it opened and a range
map describing *that* file; moving the destination underneath it would leave the
finished ranges in one file, the retried ranges in another, and a range map
claiming a single complete object.

## What is here, and what is not

Implemented and tested:

- the engine, job life cycle, and the priority-ordered admission gate;
- the bounded, coalescing event queue, plus the optional callback;
- HTTP/HTTPS with redirects, multi-source mirrors, and the single-stream
  fallback for servers with no ranges;
- `ftp://` on one connection, resuming from the contiguous prefix;
- HTTP and SOCKS4/4a/5 proxies, custom headers, basic auth;
- MD5 / SHA-1 / SHA-256 / SHA-512 / BLAKE3 verification after the transfer;
- rate limiting, per job and engine-wide;
- durable state: snapshot, restore, resume across a process restart;
- the platform policy hooks (network policy, power mode);
- a C example, an ABI conformance program, and header-only translation units in
  C and C++ (each including the header twice) — all built in CI on Linux, macOS
  and Windows.

Not here yet, and deliberately not faked:

- **Packaging for mobile.** The ABI is designed for an Android AAR and an iOS
  XCFramework and nothing in it prevents either, but neither is built by this
  repository yet.
- **Language bindings.** C is the canonical ABI and the only binding shipped.
  Go, Swift, Kotlin, Dart, C# and Python layers belong in `bindings/` when they
  are written; each should be a thin idiomatic wrapper, never a second engine.
- **A custom storage backend.** hydra writes to a path. A callback-driven
  storage interface — for a content URI, a security-scoped resource, a
  platform-managed destination — is a real requirement on mobile and a bad
  thing to design in a hurry, so it is a later extension rather than a v1 knob.

## Stability

Experimental, and subject to change within ABI 1:

- `hydra_job_get_sources`, `hydra_source_info_t`, `hydra_source_array_t` — the
  scheduler is still evolving and `latency_us`, `active_connections` and
  `error_count` may come to mean something slightly different;
- `hydra_event_set_callback` — the queue is the stable mechanism, and freezing a
  callback interface before any of the JVM, .NET, Go, Swift or Python bindings
  exist would be guessing at their constraints.

Everything else is stable.

## Licence

MIT **or** Apache-2.0, matching `hya-core` and `hya-net` and deliberately *not*
the GPL of the `hydra` CLI. Rust links statically, so copyleft here would
propagate into every application that embeds the engine — which would defeat the
point. This crate must therefore never gain a dependency on `hya-cli`,
`hya-gui` or `hya-host`. See [LICENSING.md](../../LICENSING.md).
