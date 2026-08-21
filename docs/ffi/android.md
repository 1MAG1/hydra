# libhydra on Android

## The division of responsibility

This is the part to get right before any code. On a phone there are two
schedulers, and confusing them is the classic mistake:

| hydra decides | Android decides |
|---|---|
| How to split byte ranges | Whether your process may run at all |
| Which mirror is faster | Whether the network is available or metered |
| Whether to reclaim a stalled range | Whether the battery allows it |
| How many connections to open | Whether a service owns the work |
| How to verify the finished file | Whether a notification is shown |
| | Where a file may be written |

hydra never asks `ConnectivityManager` or `BatteryManager` anything. Your Kotlin
layer answers those questions and translates the answers into
`hydra_runtime_policy_t`; the engine adjusts its connection ceiling, event rate
and retry aggressiveness accordingly. **The engine must never be assumed to keep
a background thread alive** — Android will stop your process, and that is the
platform working correctly, not a failure.

```text
Android app
├── Kotlin UI
├── Foreground service / UIDT job / WorkManager   ← owns execution
├── Notification
├── Room or DataStore                             ← your own bookkeeping
└── libhydra                                      ← owns the download
```

## What is in the archive

```text
libhydra-<version>-android/
    include/hydra.h
    jniLibs/arm64-v8a/libhydra.so
    jniLibs/armeabi-v7a/libhydra.so
    jniLibs/x86_64/libhydra.so
    jniLibs/x86/libhydra.so
    static/<abi>/libhydra.a          for your own native target
    cmake/hydra-config.cmake
    docs/  examples/  NOTICE.md  LICENSE-*
```

Built against **API level 21** (Android 5.0), so it runs on anything Google Play
still accepts.

## Wiring it into a Gradle project

The `.so` files go where the Android Gradle Plugin already looks:

```text
app/src/main/jniLibs/arm64-v8a/libhydra.so
app/src/main/jniLibs/armeabi-v7a/libhydra.so
app/src/main/jniLibs/x86_64/libhydra.so
app/src/main/jniLibs/x86/libhydra.so
```

or point Gradle at the archive without copying:

```kotlin
android {
    sourceSets["main"].jniLibs.srcDir("$rootDir/third_party/libhydra/jniLibs")

    // Ship every ABI in one App Bundle; Play splits per device automatically.
    // If you distribute an APK directly, use splits { abi { ... } } instead.
    packaging {
        jniLibs.useLegacyPackaging = false
    }
}
```

`libhydra.so` exports **C symbols, not JNI symbols**, so
`System.loadLibrary("hydra")` on its own gives you nothing callable from Kotlin.
You need one of:

- **A thin JNI shim of your own** — a small C or C++ library that links
  `static/<abi>/libhydra.a` and exposes `Java_...` functions. This is the usual
  choice and gives you exactly the Kotlin API you want.
- **A Rust JNI crate** that depends on `hya-ffi` (or on `hya-core`/`hya-net`
  directly) and uses the `jni` crate.

### The shim, with CMake

```cmake
# app/src/main/cpp/CMakeLists.txt
cmake_minimum_required(VERSION 3.22)
project(hydra_jni C)

list(APPEND CMAKE_PREFIX_PATH "${CMAKE_SOURCE_DIR}/../../../../third_party/libhydra/cmake")
find_package(hydra REQUIRED)          # resolves the slice for ANDROID_ABI

add_library(hydra_jni SHARED hydra_jni.c)
target_link_libraries(hydra_jni PRIVATE hydra::hydra log)
```

```kotlin
android {
    externalNativeBuild {
        cmake { path = file("src/main/cpp/CMakeLists.txt") }
    }
}
```

Then `System.loadLibrary("hydra_jni")` from Kotlin.

## Turning the event queue into a Flow

This is the shape worth aiming for. hydra's queue exists precisely so each
language can build its own idiom on top; on Android that idiom is `Flow`.

```kotlin
class HydraEngine(config: Config) : AutoCloseable {
    private val handle: Long = nativeCreate(config)   // hydra_engine_create

    val events: Flow<HydraEvent> = callbackFlow {
        val thread = Thread {
            while (!Thread.currentThread().isInterrupted) {
                // nativeEventWait wraps hydra_event_wait with a 250 ms timeout,
                // returning null on HYDRA_ERR_AGAIN and closing on SHUTDOWN.
                val ev = nativeEventWait(handle, 250) ?: continue
                trySend(ev)
            }
        }.apply { name = "hydra-events"; start() }

        awaitClose {
            // hydra_event_wake releases the blocked wait so the thread can
            // exit promptly, without shutting the engine down.
            nativeEventWake(handle)
            thread.interrupt()
        }
    }.flowOn(Dispatchers.IO)

    override fun close() {
        nativeShutdown(handle, 5_000)   // hydra_engine_shutdown
        nativeDestroy(handle)           // hydra_engine_destroy
    }
}
```

Drain the queue in **one** place. Events are delivered exactly once, so two
collectors racing on the native queue will each miss what the other took —
fan out in Kotlin from a single reader, which `callbackFlow` plus `shareIn`
does naturally.

Use the queue rather than `hydra_event_set_callback`. A callback arrives on a
thread the JVM does not know about, so it would have to `AttachCurrentThread`
on every event; the queue reads from a thread you created and already own. The
callback API is marked experimental for exactly this reason.

## Background execution

Android's rules changed several times and will change again, so the engine
deliberately holds no opinion. Current guidance, roughly:

| Situation | Use |
|---|---|
| The user pressed Download and is watching | **Foreground service** with a progress notification |
| A long user-initiated transfer that should survive leaving the app | **User-initiated data transfer job** (`JobService`, API 34+) |
| Deferrable, constraint-driven work ("sync when charging on Wi-Fi") | **WorkManager** |

Whichever you choose, the pattern is the same:

1. Your service starts and creates (or restores) the engine.
2. It calls `hydra_engine_restore()`, gets jobs back as `HYDRA_JOB_PAUSED`, and
   resumes the ones it wants running now.
3. It collects the event flow and updates the notification.
4. When Android signals that execution is ending — `onStopJob`, `onDestroy`,
   `onStopCurrentWork` — it calls `hydra_engine_snapshot()` and then
   `hydra_engine_shutdown()`.
5. Next time it runs, back to step 2.

Step 4 is the one people skip, and it is the one that makes the difference
between resuming a 3 GB download and starting it again.

## Persistence

Set `state_path` to a file in your app's private storage:

```kotlin
val statePath = File(context.filesDir, "hydra-state.json").absolutePath
```

hydra records the **range map** — which byte spans are already on disk — not the
data. A partially downloaded file written by positioned writes is full of holes,
so its length tells you nothing about which bytes are present. Only the recorded
spans do, which is why resume needs this file and not just the partial download.

Credentials are never written to it. A restored job that needs a password comes
back without one; re-arm it with `hydra_job_set_credentials()` before starting.

## Destinations

`output_path` is a filesystem path. That covers app-private storage
(`filesDir`, `getExternalFilesDir`) completely, and those need no permission on
any supported API level.

It does **not** cover a `content://` URI from the Storage Access Framework or
`MediaStore`. If the user picks a destination through a document picker, the
workable pattern today is:

1. Download to a private file with hydra.
2. On `HYDRA_EVENT_COMPLETED`, copy the finished file into the content URI with
   `ContentResolver.openOutputStream()`.
3. Delete the private file.

A destination abstraction that covers content URIs directly is a planned ABI
extension, not something to work around with a fake path.

## Policy

Translate platform conditions into the engine's generic model:

```kotlin
val caps = connectivityManager.getNetworkCapabilities(activeNetwork)
val unmetered = caps?.hasCapability(NET_CAPABILITY_NOT_METERED) == true

policy.networkPolicy = if (userWantsWifiOnly) HYDRA_NETWORK_UNMETERED else HYDRA_NETWORK_ANY
policy.allowMetered  = if (unmetered) 1 else 0
policy.powerMode     = when {
    powerManager.isPowerSaveMode -> HYDRA_POWER_BATTERY_SAVER
    batteryLow                   -> HYDRA_POWER_RESTRICTED
    else                         -> HYDRA_POWER_NORMAL
}
// hydra_engine_set_policy
```

`HYDRA_POWER_BATTERY_SAVER` halves the connection ceiling and coarsens progress
events to at least one second; `HYDRA_POWER_RESTRICTED` drops to one connection
and two seconds. Both take effect for jobs started after the call — a running
transfer keeps the connections it was admitted with, because tearing them down
to satisfy a new ceiling costs more than it saves.

## Size

The stripped `arm64-v8a` library is the one most of your users download. If
binary size matters more than fault isolation, build from source with the
`dist` profile, which adds `panic = "abort"`:

```bash
scripts/package-ffi-android.sh --profile dist
```

Read the comment on `[profile.dist]` in the workspace `Cargo.toml` first: under
`panic = "abort"` a panic in one fetch task takes the whole process with it,
rather than being reported through that task's handle while other transfers
continue.

## Licence obligations

`libhydra` is MIT OR Apache-2.0 and contains no GPL code. `THIRD-PARTY-NOTICES.md`
lists the dependency terms, and those must be reproduced in your app's
open-source-licences screen.
