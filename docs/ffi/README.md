# Embedding HYDRA — `libhydra`

`libhydra` is the HYDRA download engine with a stable C ABI in front of it. One
static library, one header, and a contract that a program written in C, C++,
Go, Swift, Kotlin, Dart, C# or Python can hold on to across releases.

It is the same engine the `hydra` CLI and the desktop app run: adaptive
concurrency, multi-source retrieval, range stealing, stall detection, resume,
and integrity verification. What the ABI adds is durable job identity,
persistence, and a platform-policy boundary that makes the engine usable on a
phone.

## Pick your platform

| | |
|---|---|
| [Linux](linux.md) | glibc and musl, x86-64 / arm64 / armv7, pkg-config, containers |
| [macOS](macos.md) | universal binaries, Xcode, Homebrew-style layouts |
| [Windows](windows.md) | MSVC, static CRT, `hydra.lib` vs `hydra.dll` |
| [Android](android.md) | jniLibs, JNI, CMake, and who owns background execution |
| [iOS](ios.md) | `Hydra.xcframework`, Swift Package Manager, background transfers |
| [Any other platform](other-platforms.md) | building for a triple that is not in the release matrix |
| [Language bindings](bindings.md) | Go, Python, C#, Dart, Zig, C++, and how to write your own |

## What a release archive contains

Every platform archive on the [releases page](https://github.com/ja7ad/hydra/releases)
has the same shape:

```text
libhydra-<version>-<triple>/
    include/hydra.h            the published ABI
    lib/libhydra.a             static library
    lib/libhydra.so|.dylib     shared library, where the platform has one
    lib/pkgconfig/hydra.pc     pkg-config metadata (Unix)
    docs/                      these guides
    examples/                  a complete C client and the conformance program
    native-static-libs.txt     system libraries the static archive needs
    NOTICE.md LICENSE-MIT LICENSE-APACHE THIRD-PARTY-NOTICES.md
```

Android and Apple archives differ, because those platforms consume native code
differently — see their pages.

## Sixty seconds

```c
#include "hydra.h"

/* A header from one ABI and a library from another disagree about the layout
   of every struct below. Check before trusting anything. */
if (hydra_ffi_abi_version() != HYDRA_FFI_ABI_VERSION) { return 1; }

hydra_engine_config_t cfg;
HYDRA_ENGINE_CONFIG_INIT(&cfg);
cfg.max_connections = 8;              /* a ceiling, not a target */
cfg.state_path = "hydra-state.json";  /* jobs survive a process restart */

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
    if (ev.kind == HYDRA_EVENT_PROGRESS)  { /* render ev.progress */ }
    if (ev.kind == HYDRA_EVENT_COMPLETED) break;
    if (ev.kind == HYDRA_EVENT_FAILED)    break;
}

hydra_engine_shutdown(engine, 5000);
hydra_engine_destroy(engine);
```

A complete version — mirrors, pause on Ctrl-C, resume, error reporting — is in
`examples/download.c`.

## The rules that matter most

**Memory allocated by hydra is freed by hydra**, through the matching `*_free`
function and never through `free()`. Strings you pass *in* are borrowed for that
call only.

**Job identity is a `uint64_t`, not a pointer.** Job 1842 survives an app
restart, a UI rebuild, a killed Android service and an engine recreated from a
state file. The application owns identity, hydra owns download state, the
operating system owns execution.

**The event queue is the interface.** Callbacks exist and are experimental. A
queue you drain becomes a Go channel, a Kotlin `Flow`, a Swift `AsyncStream`, a
Dart `Stream`. Progress events coalesce under load; terminal events are never
dropped.

**File bytes never cross the ABI.** hydra writes the object directly to its
destination by positioned writes. That is what keeps resident memory
independent of file size — a 40 GB download costs the same memory as a 4 MB one.

**No Rust panic crosses the boundary**, every string is UTF-8, and every
fallible call returns a code with the detail available from
`hydra_last_error()`.

The full contract is at the top of [`hydra.h`](../../include/hydra.h) itself,
and every function restates its own threading, blocking and allocation
behaviour.

## Licence

`libhydra` is **MIT OR Apache-2.0** — deliberately not the GPL-3.0-or-later of
the `hydra` command-line tool. Rust links statically, so a copyleft engine would
propagate into every application that embeds it. A libhydra archive contains no
GPL code. See [LICENSING.md](../../LICENSING.md).
