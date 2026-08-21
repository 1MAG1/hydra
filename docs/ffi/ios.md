# libhydra on iOS

## The division of responsibility

iOS decides when your app runs. hydra decides how the download is performed
while it is allowed to run. Do not try to make the engine emulate the platform's
lifecycle — an app that assumes a Rust thread will keep working after the user
switches away is an app that loses transfers.

| hydra decides | iOS decides |
|---|---|
| How to split byte ranges | Whether the app is running at all |
| Which mirror is faster | How much background time you get |
| Whether to reclaim a stalled range | Whether the network is expensive |
| How many connections to open | Whether Low Power Mode is on |
| How to verify the finished file | Where a file may be written |

## What is in the archive

```text
libhydra-<version>-apple.zip
    Hydra.xcframework/         iOS device (arm64), iOS simulator
                               (arm64 + x86_64), macOS (arm64 + x86_64)
    Package.swift              consume it as a binary Swift package
    docs/  NOTICE.md  LICENSE-*  README.md
```

Static libraries, not dynamic. Apple's guidance for embedded third-party code is
static linking, and there is no reason for a download engine to be separately
replaceable at run time.

Why an XCFramework and not a universal `.a`: iOS-device arm64 and
iOS-simulator arm64 are the same *architecture* for different *platforms*.
`lipo` refuses to put them in one file, and that refusal is the reason the
XCFramework format exists.

## Adding it to an Xcode project

1. Drag `Hydra.xcframework` into the project navigator.
2. Target → **General** → *Frameworks, Libraries, and Embedded Content*.
3. Set it to **Do Not Embed**. It is static; embedding is for dynamic
   frameworks and will fail code signing.

Or, as a binary Swift package:

```swift
.package(path: "third_party/libhydra-apple")
// or, for a released archive:
.binaryTarget(
    name: "Hydra",
    url: "https://github.com/ja7ad/hydra/releases/download/v0.3.1/libhydra-0.1.0-apple.zip",
    checksum: "<swift package compute-checksum libhydra-0.1.0-apple.zip>")
```

The framework carries a module map, so Swift can `import Hydra` with no bridging
header.

## Calling it from Swift

The C API is directly usable. Three details do most of the work.

**Strings are borrowed for the call.** `withCString` gives you a pointer valid
for exactly the right lifetime:

```swift
import Hydra

var cfg = hydra_engine_config_t()
_ = hydra_engine_config_init(&cfg, UInt32(MemoryLayout<hydra_engine_config_t>.size))
cfg.max_connections = 6

let statePath = FileManager.default
    .urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
    .appendingPathComponent("hydra-state.json").path

let engine: OpaquePointer? = statePath.withCString { p in
    cfg.state_path = p
    return hydra_engine_create(&cfg)          // copies what it needs
}
```

**Everything hydra hands back must be freed by hydra.** Never `free()` it:

```swift
func snapshot(_ engine: OpaquePointer, _ job: hydra_job_id_t) -> (String, UInt64)? {
    var snap = hydra_job_snapshot_t()
    guard hydra_job_get_snapshot(engine, job, &snap) == HYDRA_OK else { return nil }
    defer { hydra_job_snapshot_free(&snap) }        // not free(), ever
    let name = snap.file_name.data.map(String.init(cString:)) ?? ""
    return (name, snap.progress.bytes_downloaded)
}
```

**The event queue becomes an `AsyncStream`.** This is what the queue is for:

```swift
func events(of engine: OpaquePointer) -> AsyncStream<hydra_event_t> {
    AsyncStream { continuation in
        let thread = Thread {
            while !Thread.current.isCancelled {
                var ev = hydra_event_t()
                switch hydra_event_wait(engine, 250, &ev) {
                case HYDRA_OK:            continuation.yield(ev)
                case HYDRA_ERR_SHUTDOWN:  continuation.finish(); return
                default:                  continue        // HYDRA_ERR_AGAIN
                }
            }
            continuation.finish()
        }
        thread.name = "hydra-events"
        thread.start()
        continuation.onTermination = { _ in
            hydra_event_wake(engine)   // release the blocked wait promptly
            thread.cancel()
        }
    }
}

for await ev in events(of: engine) {
    switch ev.kind {
    case HYDRA_EVENT_PROGRESS:  update(ev.progress)
    case HYDRA_EVENT_COMPLETED: finish(ev.job_id); 
    case HYDRA_EVENT_FAILED:    fail(ev.job_id, ev.error)
    default: break
    }
}
```

Drain it in **one** place. Events are delivered exactly once, so two readers
would each miss what the other took. Prefer this to
`hydra_event_set_callback` — a callback runs on a thread hydra created, at a
moment Swift concurrency did not choose, which is why that API is marked
experimental.

## The app lifecycle

The pattern that works:

```text
didFinishLaunching / scene active
    create the engine
    hydra_engine_restore()          jobs come back PAUSED
    resume the ones that should run now

willResignActive / didEnterBackground
    hydra_engine_snapshot()         write the range map
    request a background task assertion if you want a few more seconds
    hydra_engine_shutdown(engine, 5000) when the time is nearly up

willTerminate
    hydra_engine_snapshot()
    hydra_engine_shutdown() then hydra_engine_destroy()
```

`hydra_engine_restore()` deliberately does **not** start anything. Whether work
may run now is your decision, informed by the platform — not the engine's.

### Background time

```swift
var bg = UIBackgroundTaskIdentifier.invalid
bg = UIApplication.shared.beginBackgroundTask(withName: "hydra") {
    hydra_engine_snapshot(engine)
    hydra_engine_shutdown(engine, 2_000)
    UIApplication.shared.endBackgroundTask(bg)
    bg = .invalid
}
```

That buys tens of seconds, not minutes. For a transfer that must continue with
the app suspended, the platform mechanism is `URLSession` with a background
configuration, which hands the transfer to the system daemon — and a
system-daemon transfer is not a hydra transfer. Choose per download:

- **In-app, user is watching, wants multi-source speed** → hydra.
- **Must complete while suspended, single source is fine** → background
  `URLSession`.

Snapshot before you suspend either way, so whatever did not finish resumes
rather than restarts.

## Destinations and the sandbox

`output_path` is a POSIX path. For app-private storage, that is exactly what
`FileManager` gives you:

```swift
let dest = FileManager.default
    .urls(for: .documentsDirectory, in: .userDomainMask)[0]
    .appendingPathComponent("big.iso").path
```

For a user-chosen location from `UIDocumentPickerViewController`, resolve the
security-scoped URL first and keep the access alive until the job reaches a
terminal state:

```swift
guard url.startAccessingSecurityScopedResource() else { return }
// pass url.path to hydra; stop accessing on COMPLETED / FAILED / CANCELLED
```

hydra takes a path, not a URL, and holds no security scope of its own. A
destination model that understands security-scoped resources directly is a
planned ABI extension.

Set `isExcludedFromBackupKey` on partial downloads unless you want them in
iCloud backups — a half-finished 4 GB file is not something users want backed
up, and App Review has rejected apps for less.

## Policy

```swift
var policy = hydra_runtime_policy_t()
hydra_runtime_policy_init(&policy)
policy.power_mode = ProcessInfo.processInfo.isLowPowerModeEnabled
    ? UInt32(HYDRA_POWER_BATTERY_SAVER.rawValue)
    : UInt32(HYDRA_POWER_NORMAL.rawValue)
policy.allow_cellular = allowCellularSetting ? 1 : 0
// NWPathMonitor's isExpensive / isConstrained map onto allow_metered
hydra_engine_set_policy(engine, &policy)
```

The engine reads no iOS API itself; that is what keeps the core free of platform
code and identical on every target.

## App Transport Security

hydra does not use `URLSession` or `NSURLConnection`, so **ATS does not apply**
to its traffic and no `Info.plist` exception is needed — including for plain
`http://`. It uses `rustls` with a compiled-in Mozilla root store and does not
consult the system trust store, so an enterprise root profile installed on the
device is not trusted for hydra's connections.

That also means the responsibility for refusing cleartext is yours. If your app
should be HTTPS-only, validate the scheme before calling `hydra_job_create`.

## App Review

Nothing in the engine requires an entitlement. There is no JIT, no dynamic code
loading, and no use of private API. `libhydra` is MIT OR Apache-2.0 and contains
no GPL code; `THIRD-PARTY-NOTICES.md` lists the dependency terms for your
licences screen.
