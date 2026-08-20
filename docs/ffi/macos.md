# libhydra on macOS

## Which archive

| Archive | Contents |
|---|---|
| `libhydra-<v>-apple.zip` | `Hydra.xcframework` with macOS **and** iOS slices, plus `Package.swift` |
| `libhydra-<v>-aarch64-apple-darwin` | Apple silicon only, plain `include/` + `lib/` layout |
| `libhydra-<v>-x86_64-apple-darwin` | Intel only, same layout |

For an Xcode project, take the Apple archive. For a command-line tool, a
Homebrew formula or a build that is not Xcode-shaped, take the per-architecture
one — or build a universal library yourself:

```bash
lipo -create \
    libhydra-<v>-aarch64-apple-darwin/lib/libhydra.a \
    libhydra-<v>-x86_64-apple-darwin/lib/libhydra.a \
    -output libhydra.a
lipo -info libhydra.a
# Architectures in the fat file: libhydra.a are: x86_64 arm64
```

`scripts/package-ffi-apple.sh --no-ios` does exactly this from a source
checkout, if you would rather not do it by hand.

## Linking

```bash
cc -std=c11 -I include myapp.c lib/libhydra.a \
   $(grep -v '^#' native-static-libs.txt) -o myapp
```

On macOS that resolves to `-liconv -lSystem -lc -lm`. To build a universal
executable, pass both architectures and give the linker a universal archive:

```bash
cc -std=c11 -arch arm64 -arch x86_64 -I include \
   myapp.c libhydra.a -liconv -o myapp
```

`pkg-config` works the same as on Linux; `lib/pkgconfig/hydra.pc` is in the
archive.

## Xcode

1. Drag `Hydra.xcframework` into the project navigator.
2. Target → **General** → *Frameworks, Libraries, and Embedded Content* → add
   it with **Do Not Embed**. It is a static library; embedding is for dynamic
   frameworks and will fail code signing.
3. In Swift, `import Hydra` — the framework carries a module map, so no
   bridging header is needed. In Objective-C or C, `#import <hydra.h>`.

There is no Objective-C or Swift wrapper in the box. The C API is directly
usable from Swift; see [ios.md](ios.md) for the patterns, which are identical
on macOS.

## Deployment target

The release archives are built with the default deployment target of the CI
runner's toolchain. If you need to support an older macOS, build from source
with the target set:

```bash
MACOSX_DEPLOYMENT_TARGET=11.0 scripts/build-ffi.sh --target aarch64-apple-darwin
```

The engine itself has no macOS version requirements beyond what Rust's standard
library needs.

## App Sandbox and permissions

hydra writes to the `output_path` you give it and does nothing else with the
filesystem. Inside the App Sandbox that means:

- **Outgoing network** — enable `com.apple.security.network.client`. Without
  it every transfer fails with a connection error and the reason is not
  obvious from the error text.
- **File access** — a path the sandbox denies produces
  `HYDRA_ERR_PERMISSION`. For a user-chosen destination, resolve the
  security-scoped URL, call `startAccessingSecurityScopedResource()`, pass the
  resulting POSIX path to hydra, and keep the access alive until the job
  reaches a terminal state. hydra takes a path, not a URL — see the note on
  destinations in `hydra.h`.
- **Hardened Runtime** — nothing in the engine needs an exception. There is no
  JIT, no unsigned executable memory and no DYLD interposition.

## Notarisation

A static `libhydra.a` linked into your binary carries no separate signing
identity — you sign and notarise your own executable as usual, and there is
nothing extra to staple. If you ship `libhydra.dylib` instead, it must be
signed with your Developer ID and included in the notarisation submission.

## TLS

hydra uses `rustls` with a compiled-in Mozilla root store. It does **not** use
the system Keychain, so a certificate you trusted in Keychain Access is not
trusted here, and it does not link the platform TLS stack at all. That makes
behaviour identical across every platform hydra runs on, which is usually what
you want from a download engine.

## App Nap and background execution

A macOS app that is not visible can be throttled. If a transfer must keep
running while your app is in the background, hold an activity assertion for its
duration:

```swift
let token = ProcessInfo.processInfo.beginActivity(
    options: [.userInitiated, .idleSystemSleepDisabled],
    reason: "Downloading")
// ... run the job, drain events ...
ProcessInfo.processInfo.endActivity(token)
```

That is the platform's decision to make, not the engine's — hydra runs when it
is allowed to run. The same division is the whole design on iOS and Android.
