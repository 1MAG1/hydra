# Writing a language binding

The C ABI is the single contract. Every language layer is a thin idiomatic
wrapper over the same symbols — never a second implementation of the engine.

```text
Go     → cgo            Python → ctypes / cffi
Swift  → Clang module   C#     → P/Invoke
Kotlin → JNI            Dart   → dart:ffi
C++    → thin wrapper   Zig    → @cImport
```

Read [**ABI.md**](ABI.md) first. It is the specification this page assumes:
what is frozen and what may change, who owns which allocation, and exactly what
the event queue guarantees. Section 7 is a checklist you can work through.

A binding does not need this repository. It needs the published `hydra.h`, a
prebuilt archive from a [release](https://github.com/ja7ad/hydra/releases), and
the ABI version it targets — no Rust toolchain, no knowledge of `hya-core`, and
no coupling to hydra's release cadence. That is deliberate: a binding that has
to be rebuilt in lockstep with the engine is a binding only this project can
maintain. Bindings are expected to live as independent projects, and the ABI
version they support belongs in their README.

## The five things every binding must get right

**1. Free with the matching function.** Never `free()` anything hydra returned.
A static `libhydra` may not share an allocator with the host runtime, and on
Windows may not share a CRT. Wrap the ownership in whatever your language uses
for it — `defer`, `using`, `__del__`, a `Drop`, a finaliser — so a user of your
binding cannot get it wrong.

**2. Keep input strings alive for exactly the call.** Every `const char *` is
borrowed for the duration of the call and copied internally. In Go that means
`C.CString` plus `defer C.free`; in Python, keeping the `bytes` object alive
across the call; in C# a `fixed` block or a marshalled string.

**3. Drain the event queue in one place.** Events are delivered exactly once.
Two goroutines, two threads or two tasks reading the queue will each miss what
the other took. Read from one dedicated thread and fan out in your own language.

**4. Check the ABI version at load time.** `hydra_ffi_abi_version()` against
`HYDRA_FFI_ABI_VERSION` from the header you generated bindings from. A mismatch
means the two disagree about the layout of every struct; fail loudly rather than
discover it field by field.

**5. Initialise config structs through the init call.** Never zero a struct and
fill in fields — `hydra_engine_config_init` stamps `size` and `version`, and
that is what makes an old binding work against a new library.

## Go

```go
package hydra

/*
#cgo CFLAGS: -I${SRCDIR}/../include
#cgo LDFLAGS: ${SRCDIR}/../lib/libhydra.a -lm -ldl -lpthread
#include "hydra.h"
#include <stdlib.h>
*/
import "C"
import (
    "errors"
    "runtime"
    "unsafe"
)

type Engine struct{ h *C.hydra_engine_t }
type JobID uint64

type Event struct {
    Kind     EventKind
    JobID    JobID
    Progress Progress
    Err      error
}

func New(cfg Config) (*Engine, error) {
    var c C.hydra_engine_config_t
    if rc := C.hydra_engine_config_init(&c, C.uint32_t(unsafe.Sizeof(c))); rc != C.HYDRA_OK {
        return nil, lastError()
    }
    c.max_connections = C.uint32_t(cfg.MaxConnections)

    // Borrowed for this call only; hydra copies what it keeps.
    if cfg.StatePath != "" {
        p := C.CString(cfg.StatePath)
        defer C.free(unsafe.Pointer(p))
        c.state_path = p
    }
    h := C.hydra_engine_create(&c)
    if h == nil {
        return nil, lastError()
    }
    e := &Engine{h: h}
    runtime.SetFinalizer(e, (*Engine).Close)
    return e, nil
}

// Events turns the queue into a channel. ONE reader goroutine: events are
// delivered exactly once, so a second reader would steal from the first.
func (e *Engine) Events(buf int) <-chan Event {
    ch := make(chan Event, buf)
    go func() {
        defer close(ch)
        for {
            var ev C.hydra_event_t
            switch C.hydra_event_wait(e.h, 250, &ev) {
            case C.HYDRA_OK:
                ch <- toEvent(&ev)
            case C.HYDRA_ERR_SHUTDOWN:
                return
            }
        }
    }()
    return ch
}
```

Notes specific to cgo: every call crosses the Go/C boundary and costs roughly a
function call plus scheduling bookkeeping, which is irrelevant at hydra's event
rates. Do not call into the engine from a `//export`ed callback — that is why
the queue exists.

## Python

```python
import ctypes, ctypes.util
from ctypes import c_char_p, c_uint32, c_uint64, POINTER, byref

lib = ctypes.CDLL("./libhydra.so")

class Progress(ctypes.Structure):
    _fields_ = [("bytes_downloaded", c_uint64), ("total_bytes", c_uint64),
                ("bytes_per_second", c_uint64), ("average_bytes_per_second", c_uint64),
                ("eta_seconds", c_uint64),
                ("active_connections", c_uint32), ("active_sources", c_uint32),
                ("completed_ranges", c_uint32), ("total_ranges", c_uint32),
                ("retry_count", c_uint64), ("stall_count", c_uint64)]

assert ctypes.sizeof(Progress) == 72, "struct layout mismatch"

class Hydra:
    def __init__(self, state_path=None, max_connections=8):
        cfg = EngineConfig()
        if lib.hydra_engine_config_init(byref(cfg), ctypes.sizeof(cfg)) != 0:
            raise HydraError.last()
        cfg.max_connections = max_connections
        # Keep the bytes object alive for the duration of the call.
        self._state = state_path.encode() if state_path else None
        cfg.state_path = self._state
        self._h = lib.hydra_engine_create(byref(cfg))
        if not self._h:
            raise HydraError.last()

    def events(self, timeout_ms=250):
        """A generator, so `for ev in engine.events()` reads naturally.

        hydra_event_wait releases the GIL for you only if you declare the
        function with ctypes' default behaviour — which does release it — so a
        blocking wait here does not freeze the interpreter.
        """
        ev = Event()
        while True:
            rc = lib.hydra_event_wait(self._h, timeout_ms, byref(ev))
            if rc == HYDRA_ERR_SHUTDOWN:
                return
            if rc == HYDRA_OK:
                yield ev

    def close(self):
        if self._h:
            lib.hydra_engine_shutdown(self._h, 5000)
            lib.hydra_engine_destroy(self._h)
            self._h = None

    __del__ = close
```

Assert the struct sizes at import time, as above. A `ctypes.Structure` whose
field list has drifted from the header fails silently and produces nonsense
numbers; one `assert` per struct turns that into an immediate, obvious error.

## C#

```csharp
using System.Runtime.InteropServices;

internal static partial class Native
{
    private const string Lib = "hydra";

    [LibraryImport(Lib)]
    internal static partial uint hydra_ffi_abi_version();

    [LibraryImport(Lib)]
    internal static partial IntPtr hydra_engine_create(in EngineConfig config);

    [LibraryImport(Lib)]
    internal static partial ErrorCode hydra_event_wait(IntPtr engine, uint timeoutMs, out Event ev);
}

[StructLayout(LayoutKind.Sequential)]
internal struct Progress
{
    public ulong BytesDownloaded, TotalBytes, BytesPerSecond, AverageBytesPerSecond, EtaSeconds;
    public uint ActiveConnections, ActiveSources, CompletedRanges, TotalRanges;
    public ulong RetryCount, StallCount;
}
```

`LayoutKind.Sequential` matches the C layout. Verify once at startup with
`Marshal.SizeOf<Progress>() == 72`. Run the event loop on a dedicated
long-running thread (`TaskCreationOptions.LongRunning`) rather than a thread-pool
task — `hydra_event_wait` blocks, and blocking a pool thread for the life of the
process is not what the pool is for.

## Dart / Flutter

```dart
final lib = DynamicLibrary.open(Platform.isAndroid
    ? 'libhydra.so'
    : 'Hydra.framework/Hydra');

// The event loop must not run on the UI isolate: hydra_event_wait blocks.
// Run it in a separate isolate and forward events over a SendPort, which is
// what turns the queue into a Stream<HydraEvent>.
```

On Android the `.so` goes in `jniLibs`; on iOS the XCFramework is linked into
the runner target. See [android.md](android.md) and [ios.md](ios.md).

## C++

The header is `extern "C"`-guarded and compiles as C++11 and later. A thin RAII
wrapper is usually all that is wanted:

```cpp
class Engine {
public:
    explicit Engine(const hydra_engine_config_t& cfg)
        : h_(hydra_engine_create(&cfg)) {
        if (!h_) throw std::runtime_error(last_error());
    }
    ~Engine() {
        if (h_) { hydra_engine_shutdown(h_, 5000); hydra_engine_destroy(h_); }
    }
    Engine(const Engine&) = delete;              // one owner, always
    Engine& operator=(const Engine&) = delete;
    Engine(Engine&& o) noexcept : h_(std::exchange(o.h_, nullptr)) {}

    hydra_engine_t* get() const noexcept { return h_; }
private:
    hydra_engine_t* h_;
};

// Snapshots own their strings; free through hydra, never delete.
struct SnapshotDeleter {
    void operator()(hydra_job_snapshot_t* s) const { hydra_job_snapshot_free(s); }
};
```

Do not make C++ the canonical layer for other languages to sit on. C is the base
ABI precisely because it is the one every language can reach.

## Zig

```zig
const c = @cImport({
    @cInclude("hydra.h");
});
```

Zig's `@cImport` handles the header directly, including the static assertions.

## Testing a binding

Port `examples/ffi-c/abi_smoke.c` first. It exercises version checking, config
initialisation, engine create/destroy, the refusal paths and every `*_free`
function without touching the network, so it runs anywhere and finishes
instantly. If your binding passes it, the plumbing is right and what remains is
API design.

Then check for leaks: create and destroy a thousand engines, take and free ten
thousand snapshots. Ownership mistakes at this boundary are silent until they
are not.

## Contributing one

Bindings live in `bindings/<language>/`. A binding is welcome when it is a thin
wrapper over the ABI, has the smoke test ported, and documents its ownership
rules in the language its users read. It is not welcome if it reimplements any
part of the engine — there is one engine, and it is in Rust.
