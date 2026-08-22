# The `libhydra` ABI

This is the canonical specification of the C ABI that `libhydra` publishes.
[`include/hydra.h`](../../include/hydra.h) is the machine-readable half — the
declarations your compiler reads — and this document is the human-readable
half: what the declarations mean, what may change, and what may not.

The header is generated from
[`crates/hydra-ffi/src/abi.rs`](../../crates/hydra-ffi/src/abi.rs) and
[`exports.rs`](../../crates/hydra-ffi/src/exports.rs), so it can never describe
a library that does not exist. This document is written by hand, so it can say
things a `.h` file cannot.

```text
crates/hydra-ffi/src/{abi,exports}.rs      the implementation
              │        cbindgen
              ▼
       include/hydra.h                     the published API surface
              │
              ▼
       docs/ffi/ABI.md                     what it means and what is frozen
              │
              ▼
   crates/hydra-ffi/abi/abi-1.manifest     the frozen layout, checked in CI
```

**Current ABI version: 1.** `HYDRA_FFI_ABI_VERSION` in the header,
`hydra_ffi_abi_version()` in the library. Compare them at startup and refuse to
continue on a mismatch — everything below is void if they disagree.

---

## 1. Design principles

`libhydra` is an **embeddable engine**, not a Rust-to-C wrapper. The difference
shows up in almost every decision below, so it is worth stating plainly. A
wrapper exposes whatever the Rust API happens to look like and asks the caller
to cope; an embeddable engine picks a shape that a Go runtime, a JVM, a .NET
host, a Swift actor and a plain C program can all hold on to, and then keeps
that shape still.

The ABI prioritises, in this order:

**Stable binary compatibility.** A program compiled against an older header
keeps working against a newer library, without recompiling. This is the
constraint that outranks the others; where elegance and stability disagree,
stability wins and the ugliness is documented.

**Explicit ownership.** Every allocation has exactly one owner and one named
way to release it. Nothing is freed implicitly, nothing is freed twice, and no
buffer's lifetime depends on a rule the caller has to remember.

**Panic isolation.** No Rust panic crosses the boundary. Unwinding into a
foreign frame is undefined behaviour in every host language; an internal
failure becomes `HYDRA_ERR_INTERNAL` with the detail preserved.

**Runtime independence.** The engine owns its threads. It needs no host event
loop, no async runtime, no particular thread, and no cooperation from the
caller's scheduler. No Rust `Future` appears anywhere in this ABI.

**Thread safety.** The engine handle and every job operation are safe to call
concurrently. The one exception — `hydra_engine_destroy` — is named, not
implied.

**Bounded asynchronous events.** State reaches the caller through a queue with
a fixed capacity and defined drop semantics, so a slow consumer cannot make the
engine allocate without limit and cannot miss the events that matter.

**Cross-language interoperability.** Opaque handles, integer identities,
fixed-width integers, UTF-8 strings, a return-code discipline, and no
callbacks on the critical path. Every one of those is chosen because it is
cheap in *all* the target languages rather than elegant in one.

**Platform independence.** The same header, the same layout and the same
semantics on Linux, macOS, Windows, Android and iOS. Where a platform genuinely
differs, the difference is in a policy call the host makes, not in the ABI.

---

## 2. Two products

Hydra ships two things that are easy to confuse and should not be.

```text
hydra          the application  — CLI, desktop GUI, browser host   GPL-3.0-or-later
libhydra       the engine       — stable C ABI, embeddable         MIT OR Apache-2.0
```

`hydra` is a program you run. `libhydra` is a library you build into a program
of your own; it is a deliverable in its own right, with its own version, its
own release archives, its own compatibility promise and its own licence. It
must never gain a dependency on `hya-cli`, `hya-gui` or `hya-host` — Rust links
statically, and a copyleft engine would propagate into every application that
embeds it. See [LICENSING.md](../../LICENSING.md).

The layering underneath:

```text
                    ┌───────────────┐   ┌───────────────┐
                    │   hydra-gui   │   │   hydra-cli   │
                    └───────┬───────┘   └───────┬───────┘
                            │                   │
              ┌─────────────▼───────────────────▼──────┐
              │               libhydra                  │
              │             stable C ABI                │
              └─────────────────────┬───────────────────┘
                                    │
                          ┌─────────▼─────────┐
                          │      hya-core     │
                          │  adaptive engine  │
                          └─────────┬─────────┘
                                    │
                          ┌─────────▼─────────┐
                          │      hya-net      │
                          │  network engine   │
                          └───────────────────┘
```

Language bindings sit *above* `libhydra` and are independent of it. A binding
consumes the published header and a prebuilt archive; it does not need this
repository, a Rust toolchain, or any knowledge of `hya-core`:

```text
libhydra
   ├── hydra-go
   ├── hydra-python
   ├── hydra-swift
   ├── hydra-kotlin
   └── hydra-dotnet
```

That is deliberate. A binding that has to be built in lockstep with the engine
is a binding only this project can maintain. See
[bindings.md](bindings.md) for what writing one involves.

---

## 3. The ABI 1 stability policy

This section is the contract. It is not a statement of intent; it is enforced
mechanically, and the enforcement is described in [§6](#6-compatibility-testing).

### 3.1 What is frozen

Within ABI version 1, for as long as ABI 1 exists:

```text
ABI 1
 ├── Existing fields      NEVER move          (offset is frozen)
 ├── Existing fields      NEVER change width  (size is frozen)
 ├── Existing fields      NEVER change meaning
 ├── Enumerators          NEVER change value, and a value is NEVER reused
 ├── Exported symbols     NEVER disappear, and NEVER change signature
 ├── Ownership rules      NEVER change
 ├── Struct layout        NEVER changes alignment or packing
 └── New fields           APPEND ONLY, and only to a size-prefixed struct
```

"Never changes meaning" is the one a compiler cannot check and the one that
matters most. If `hydra_progress_t::bytes_per_second` were to switch from an
instantaneous sample to a lifetime average, nothing would move, every assertion
would pass, and every caller's speed readout would quietly start saying
something else — `average_bytes_per_second` sits right next to it precisely so
that neither has to. A field whose meaning must change gets a new field and a
documented deprecation, not a redefinition.

### 3.2 What may be added

Additions are allowed exactly where they cannot be observed by a program
compiled against an older header:

| Addition | Allowed | Why |
|---|---|---|
| A new exported function | yes | An old program does not call it. |
| A new enumerator, with a value never used before | yes | An old program never receives it — see below. |
| A new field appended to `hydra_engine_config_t` | yes | Size-prefixed; the library writes at most `size` bytes. |
| A new field appended to `hydra_job_config_t` | yes | Same. |
| A new field appended to any other struct | **no** | The caller allocates it; growing it would overrun the caller's buffer. |
| A new struct type | yes | Nothing existing refers to it. |
| Reusing one of the `reserved` bytes | yes | Documented as "must be zero"; a conforming caller wrote zeros. |

The asymmetry between the two configuration structs and everything else is the
whole forward-compatibility mechanism. `hydra_engine_config_t` and
`hydra_job_config_t` begin with a `size` field that the caller sets to
`sizeof` *their* struct — which is what `HYDRA_ENGINE_CONFIG_INIT` and
`HYDRA_JOB_CONFIG_INIT` do for you. The library reads and writes at most that
many bytes and defaults everything past it. Every other struct is either
allocated by the library and handed to you, or allocated by you and filled in
by the library at a size it must not guess at; neither can grow.

**New enumerators and old programs.** A new enumerator is only safe because the
library never *sends* one to a caller who cannot know it. A new `hydra_event_type_t`
is not delivered to a queue drained by a program built against a header that
predates it; a new `hydra_error_code_t` is only returned from a call that
predates nothing, i.e. a new function. Where that cannot be arranged, the value
does not get added inside ABI 1. In the other direction — values you pass *in* —
the library validates every enum-typed field it reads and returns
`HYDRA_ERR_INVALID_ARGUMENT` rather than trusting the bit pattern.

### 3.3 What forces ABI 2

Any of the following makes the change ABI 2 rather than an amendment to ABI 1:

- moving, widening, narrowing or removing an existing field;
- renumbering an enumerator, or reusing a retired value for something else;
- changing what an existing field or enumerator means;
- removing an exported function, or changing its signature or its ownership
  semantics;
- changing who frees what, or how;
- changing the thread-safety guarantee of an existing call;
- changing the event queue's drop or ordering guarantees;
- growing a struct that is not size-prefixed.

When that happens, `HYDRA_FFI_ABI_VERSION` becomes `2`. It does **not** happen
by gradually amending ABI 1 until the old rules no longer hold. A new ABI
version gets its own frozen baseline
(`crates/hydra-ffi/abi/abi-2.manifest`) and the ABI 1 baseline is kept, so that
what ABI 1 promised remains inspectable after ABI 2 exists.

`HYDRA_FFI_ABI_VERSION` is **independent of the library version**. `libhydra`
0.9 and 1.4 may both implement ABI 1; the library version tells you what
features exist, the ABI version tells you what layout they have.

### 3.4 The startup check

```c
if (hydra_ffi_abi_version() != HYDRA_FFI_ABI_VERSION) {
    /* The header this was compiled against and the library that loaded
       disagree about the layout of every struct below. Nothing else in this
       program means anything. */
    return EXIT_FAILURE;
}
```

Do this before the first `hydra_engine_create`. A binding should do it in its
module initialiser and refuse to load.

---

## 4. The contract

### Ownership

Memory allocated by hydra is freed by hydra, through the matching `*_free`
function and **never** through `free()`. A statically linked `libhydra` may not
share an allocator with your program, and on Windows it may not share a CRT; a
cross-allocator free corrupts the heap intermittently and far from the call
that caused it.

Strings you pass **in** are borrowed for the duration of that call only. hydra
copies whatever it needs before returning, so you never have to keep a buffer
alive on hydra's behalf.

Every `*_free` accepts the null value, so a binding's destructor can be
unconditional.

### Encoding

Every string crossing this boundary is UTF-8. Invalid UTF-8 supplied by a
caller is `HYDRA_ERR_INVALID_ARGUMENT`, never a lossy conversion.

### Language baseline

C11 or later, or C++11 or later. The header uses fixed-width integer types and
static assertions and does not attempt to support older dialects; every
language this ABI targets can meet that baseline. Under a pre-C11 compiler the
layout assertions fall back to a negative-length typedef, which fails just as
loudly — that path is compiled in CI too, because MSVC treats a `.c` file as
pre-C11 unless given `/std:c11`.

### Enum representation

Every ABI-visible enum **value** is represented as a `uint32_t`. That is
deliberately a statement about values rather than about enumeration types:
under C++ and C23 the typedef names a real enum with `uint32_t` as its fixed
underlying type, while under C11 the typedef *is* `uint32_t` and the
enumerators are ordinary constants. C11 says nothing about the width of an
enumeration type itself, so nothing here depends on it.

Note the asymmetry in struct fields. A field hydra **writes** — an event's
kind, a snapshot's state, an error's code — is declared as the enum, because
hydra constructs it. A field hydra **reads** from you is a `uint32_t` and is
validated, because you can put any bit pattern in a struct field and an
out-of-range enum value would otherwise be undefined behaviour on the Rust
side.

### Errors

Every fallible call returns `hydra_error_code_t`. The detail behind it —
message, errno, HTTP status — is in a **thread-local** slot readable with
`hydra_last_error()`, cleared at the start of every call. Branch on the code;
never parse the message.

`HYDRA_ERR_AGAIN` is not a failure. It means "nothing to report right now", and
the non-blocking event calls return it constantly. Use `HYDRA_IS_ERROR()`
rather than a bare `!= HYDRA_OK` when you mean "something went wrong".

### Panics

No Rust panic crosses this boundary. An internal failure becomes
`HYDRA_ERR_INTERNAL` with the detail preserved.

### Threads

| | |
|---|---|
| `hydra_engine_t *` | thread-safe |
| job operations | thread-safe |
| event consumption | thread-safe, but intended for ONE consumer |
| `hydra_engine_destroy` | synchronisation-sensitive: must not race with any other call on the same engine |

Each function's own comment in the header states whether it blocks and whether
it allocates.

### Runtime

The engine owns its own threads. Your program needs no async runtime, no event
loop and no particular thread. No Rust future appears in this ABI.

### Bytes

File data never crosses this ABI. hydra writes the object directly to its
destination by positioned writes; this interface carries control, state,
progress and errors only. That is what keeps resident memory independent of
file size — a 40 GB download costs the same memory as a 4 MB one.

### Credentials

Passwords, proxy passwords and the `Authorization`, `Proxy-Authorization` and
`Cookie` headers never appear in a job snapshot, an event, an error message,
the log sink, the metrics, or the persisted state file. Userinfo embedded in a
URL (`ftp://user:pass@host/path`) is stripped at `hydra_job_create()` and moved
into the job's credentials, so the URL hydra stores and reports carries no
secret either. A restored job therefore comes back without its credentials by
design — re-arm it with `hydra_job_set_credentials()`.

### Rate limits

```text
engine_max = hydra_engine_config_t.max_bytes_per_second   (0 = none)
job_max    = hydra_job_config_t.max_bytes_per_second      (0 = none)

job_max == 0 and engine_max == 0  ->  unlimited
job_max  > 0                      ->  that job alone is capped at job_max
engine_max  > 0                   ->  every job shares ONE limiter at
                                      engine_max, so the cap is a true
                                      aggregate across all of them
```

Both apply at once when both are set: a job under an engine-wide cap *and* one
of its own moves at whichever is lower at that moment, and the aggregate
guarantee is not given up by the jobs that set their own.

Every cap is read live, on every read. `hydra_engine_set_max_bytes_per_second`
and `hydra_job_set_max_bytes_per_second` therefore bind a transfer that is
**already running**, in both directions, including a job that started with no
cap at all.

### Destinations

In this ABI version a destination is a filesystem path, and that is all. It is
enough for Linux, macOS, Windows and for an app-private directory on Android or
iOS. It is **not** enough for a content URI, a security-scoped resource or a
document-provider handle, and a future ABI may add other destination kinds
alongside the path. Do not assume a path is permanently the only storage model;
do assume it will keep working.

### Persistence

The state file is written **atomically** — a temporary file and a rename — so a
process death mid-write cannot leave a truncated one. Atomic is not the same as
durable: hydra does not `fsync` the file or its directory, so a sudden power
loss may lose the most recent snapshot entirely, and you get the previous
consistent one instead. Nothing is ever half-written; the newest write may
simply not be there. Do not read more into "atomic" than that.

`state_path` is this version's persistence mechanism: hydra owns the file and
its format. A future ABI may add host-managed persistence, so that an
application can put job state in Room, Core Data or its own database. Do not
depend on the file's contents or its format — only on the API.

### Job identity

A job is a `uint64_t`, not a pointer. Job 1842 survives an app restart, a UI
rebuild, a killed Android service and an engine recreated from a state file.
The application owns identity, hydra owns download state, the operating system
owns execution.

### Stability of individual items

Everything in the header is **stable** except `hydra_job_get_sources`,
`hydra_source_info_t` and `hydra_source_array_t`, which are **experimental**
and may change within ABI 1, and the event callback (see below), which is
experimental for the same reason.

---

## 5. The event queue

### Why the queue is the primitive

The queue is the fundamental integration mechanism and callbacks are an
optional convenience. That ordering is not an accident of implementation; it is
the single decision that makes this ABI usable from more than one language.

A foreign callback behaves differently in every runtime that would consume one.
Go must cross the cgo boundary and cannot safely call into Go from an arbitrary
C thread without care; the JVM requires the thread to be attached before any
JNI call and detached afterwards; .NET needs the delegate rooted for as long as
native code can reach it; Swift and Dart have their own rules about which
thread may touch which object; Python has to acquire the GIL. A callback-first
ABI would push all five of those problems onto the binding author, and each of
them is a class of crash that only appears under load.

A queue has none of that. The host calls in on a thread it already owns, at a
time it chooses, and gets a value:

```text
                    Hydra engine
                          │
                          ▼
                 bounded event queue
                          │
      ┌───────────┬───────┴───┬───────────┬──────────┐
      ▼           ▼           ▼           ▼          ▼
  Go channel  Kotlin Flow  Swift       Dart      C# IAsyncEnumerable
                           AsyncStream Stream
```

Each of those is the idiomatic asynchronous type of its language, and each is a
loop around `hydra_event_wait()` on a thread the host already manages. That is
the whole binding.

Callbacks remain available — `hydra_event_set_callback()` — and remain marked
experimental. They are a convenience layer over the queue, not the foundation
of it, and nothing in this ABI requires you to use one.

### Bounds and drops

The queue is bounded, so:

- progress events **coalesce**: at most one pending per job, and a newer sample
  replaces an older one;
- terminal events (`COMPLETED`, `FAILED`, `CANCELLED`, `ENGINE_SHUTDOWN`) are
  **never dropped**;
- life-cycle events drop oldest-first once the bound is reached, and every drop
  is counted in `hydra_event_t.dropped_events`.

A consumer that falls behind therefore loses resolution, never outcomes.

### Ordering

Guaranteed **within a single job**, and only there. It is a *partial* order,
not a single sequence — a job can fail while resolving, so `RESOLVED` is not
something every run reaches:

```text
JOB_CREATED -> JOB_QUEUED -> JOB_STARTED

after JOB_STARTED, in this relative order where they occur at all:
    RESOLVED        at most once per attempt, once size and range support
                    are known
    PROGRESS        zero or more times, only after RESOLVED
    RETRYING        zero or more times
    STALLED         zero or more times
    SOURCE_CHANGED  zero or more times
    VERIFYING       at most once, and always before COMPLETED

PAUSED -> RESUMED, and a RESUMED job re-enters at JOB_QUEUED

COMPLETED, FAILED and CANCELLED are terminal: each is that job's last event
until the job is started again, and exactly one of them ends an attempt that
was not paused. Any of them may follow JOB_STARTED directly — a bad URL fails
before RESOLVED, a cancel can land at any point.
```

There is **no** ordering guarantee between events belonging to different jobs.
Two jobs run concurrently on threads hydra owns, so an application must treat
each job's stream as independent — which is also what makes the queue easy to
demultiplex into one stream per job in a binding.

### One consumer

Several threads may call the event functions safely, but every event is
delivered **exactly once**. A thread draining the queue while waiting for job A
will consume and discard job B's completion unless it keeps it. Drain in one
place and dispatch from there.

### Callback pointers

`user_data` is **never** owned by hydra and is **never** freed by hydra. It is
stored, never dereferenced, and handed back to your function verbatim. It must
outlive the callback registration; freeing it while a callback is installed is a
use-after-free in your program, not in hydra's. This applies to both
`hydra_event_set_callback()` and `hydra_engine_set_log_callback()`.

---

## 6. Compatibility testing

The promises in [§3](#3-the-abi-1-stability-policy) are worth exactly as much
as the machinery that refuses to let them be broken. Four independent checks
run on every pull request, on Linux, macOS and Windows.

### 6.1 The header cannot drift from the implementation

`scripts/gen-ffi-header.sh --check` regenerates `include/hydra.h` from the Rust
definitions and fails if the committed file would change. A header that
describes a library that does not exist is worse than no header.

```bash
make header-check
```

### 6.2 Both sides agree about layout

Two independent tables of struct sizes and field offsets:
`crates/hydra-ffi/src/abi.rs` is checked by `rustc`, and
`crates/hydra-ffi/abi-layout.h` — appended to every generated header — is
checked by *your* compiler. Duplication is the point. A padding rule or an enum
width that differs on a particular toolchain is invisible to either table
alone, and shows up at run time as a caller reading the wrong bytes out of a
struct that compiled and linked perfectly.

### 6.3 The layout has not moved since ABI 1 was published

`crates/hydra-ffi/abi/abi-1.manifest` is the frozen ABI: every enumerator's
value, every field's offset and width, every struct's size, every exported
symbol. `scripts/ffi-abi-compat.sh` derives the same facts from the current
header and enforces the rules of §3.1 and §3.2 against it — additions pass,
movements fail.

```bash
make ffi-compat
```

It deliberately does not diff. A diff would reject the appends §3.2 permits,
and then nobody could ever add a field. The check is asymmetric: everything in
the baseline must still be true, and the current header may say more.

The manifest is regenerated with `scripts/ffi-abi-compat.sh --update`, which is
for two occasions only — appending within the rules, and starting a new ABI
version. It is never the way to make a failure go away.

### 6.4 Old header, new library

Every other program in this repository is compiled against the header sitting
next to the library it links, which is the one arrangement a stable ABI does
not need to be stable for. The interesting case is the program somebody
compiled two releases ago and has not rebuilt: its struct sizes, its enumerator
values and its inline helpers are frozen inside its object file, and today's
library has to still fit them.

`scripts/ffi-c-example.sh` extracts `include/hydra.h` from **every release tag
that published one**, builds `examples/ffi-c/compat_probe.c` against each, links
each against the archive built from the working tree, and runs them. The probe
puts a guard wall of known bytes immediately after every struct the caller
allocates, so a library that wrote one byte past where the old header's struct
ended is caught there rather than in somebody's corrupted stack frame months
later.

```bash
make ffi-test
```

### 6.5 The matrix

| | Linux | macOS | Windows |
|---|---|---|---|
| compiler | GCC and Clang | Apple Clang | MSVC `cl` |
| C dialects | `c99`, `c11`, `c17` | `c99`, `c11`, `c17` | default (pre-C11), `c11`, `c17` |
| C++ dialects | `c++11`, `c++17` | `c++11`, `c++17` | `c++14`, `c++17` |
| `sizeof` / `offsetof` / enum values | ✓ | ✓ | ✓ |
| symbols present in the archive | ✓ | ✓ | ✓ |
| frozen-manifest conformance | ✓ | ✓ | ✓ |
| old header + new library | ✓ | ✓ | ✓ |

Three platforms because the failures this exists to catch are platform-specific
by nature: a struct that packs differently, a symbol the linker cannot find, a
system library the static archive needs and does not declare. A Linux-only run
would find none of them. The pre-C11 column is not redundant either — it is the
dialect a consumer's existing MSVC project most likely uses, and the only one
that compiles the header's fallback assertion path.

**Not covered: new header + old library.** There is nothing to test. A newer
header may declare a function the older library does not define, and the link
fails — loudly, at build time, which is the correct outcome. The ABI makes no
promise in that direction and the startup version check exists to catch the
runtime form of it.

### 6.6 Everything at once

```bash
make header-check   # the header matches the Rust
make ffi-compat     # the layout matches the frozen ABI 1 baseline
make ffi-test       # the Rust suite, the C/C++ conformance program, and
                    # every published header against this library
```

---

## 7. A checklist for binding authors

1. Check `hydra_ffi_abi_version()` against `HYDRA_FFI_ABI_VERSION` before
   anything else, and refuse to load on a mismatch.
2. Initialise configuration structs with `HYDRA_ENGINE_CONFIG_INIT` /
   `HYDRA_JOB_CONFIG_INIT`, never with a hand-written size.
3. Build your asynchronous type on `hydra_event_wait()`, on a thread you own.
   Drain in exactly one place.
4. Treat `HYDRA_ERR_AGAIN` as "nothing yet", not as an error.
5. Read the detail with `hydra_last_error()` on the same thread that made the
   failing call, and branch on the code rather than the message.
6. Free everything hydra gave you with the matching `*_free`, and nothing with
   `free()`.
7. Keep `user_data` alive for as long as a callback is installed.
8. Surface job identity as an integer your caller can store, not as a handle.
9. Never log a `hydra_error_t` message expecting it to be secret-free — it is,
   but do not add credentials of your own to it.
10. Pin the ABI version you support, and say so in your binding's README.

See [bindings.md](bindings.md) for worked examples in Go, Python, C#, Dart, Zig
and C++.
