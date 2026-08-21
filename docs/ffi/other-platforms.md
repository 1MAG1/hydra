# Building libhydra for any other platform

The release matrix covers Linux, macOS, Windows, Android and iOS on the
architectures most people need. It is not a list of what the engine *can* run
on — nothing in the ABI or the implementation is tied to those targets. If Rust
has a std-capable target for your platform and a C toolchain exists for it,
`libhydra` should build.

## The one script

```bash
git clone https://github.com/ja7ad/hydra
cd hydra
scripts/build-ffi.sh --target <triple>
```

This is the identical script the release workflow runs — CI has no private
steps. You get the same archive layout, in `target/ffi-dist/`.

```bash
scripts/build-ffi.sh --list          # what the release matrix builds
scripts/build-ffi.sh --help
```

Useful flags:

| Flag | Effect |
|---|---|
| `--target <triple>` | Cross-compile. Defaults to the host. |
| `--static-only` | Skip the shared library. Use when no linker is configured for the target. |
| `--profile dist` | Smaller binary via `panic = "abort"`. Read the caveat below. |
| `--out DIR` | Where the archive lands. |

## What a target actually needs

Three things, in decreasing order of how often they are the problem:

**1. A C compiler for the target.** `ring` (TLS) and `blake3` compile C and
assembly, so a Rust target alone is not enough — cross-compiling to
`aarch64-unknown-linux-musl` from macOS fails with
`failed to find tool "aarch64-linux-musl-gcc"` until a cross toolchain is
installed. Point cc-rs at it:

```bash
export CC_aarch64_unknown_linux_musl=aarch64-linux-musl-gcc
export AR_aarch64_unknown_linux_musl=aarch64-linux-musl-ar
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-musl-gcc
scripts/build-ffi.sh --target aarch64-unknown-linux-musl
```

Or let [`cross`](https://github.com/cross-rs/cross) supply the toolchain in a
container:

```bash
cargo install cross --locked
cross rustc --locked -p hya-ffi --target aarch64-unknown-linux-musl \
      --release --crate-type staticlib
```

**2. `std`.** The engine needs threads, sockets and files. `no_std` targets and
bare-metal targets are out of scope, and no amount of feature-gating changes
that — this is a download engine.

**3. A linker, but only for the shared library.** Building a `staticlib` never
invokes one, which is why `--static-only` succeeds on targets where a full
build does not. The static archive is the primary deliverable anyway.

```bash
rustup target add <triple>
scripts/build-ffi.sh --target <triple> --static-only
```

## Targets that should work but are not in the matrix

Not tested by CI, so treat "should" literally — but there is no known reason
these fail:

| Triple | Notes |
|---|---|
| `x86_64-unknown-freebsd` | Build on FreeBSD; Rust's std supports it fully. |
| `x86_64-unknown-netbsd`, `*-openbsd` | Same. |
| `x86_64-unknown-illumos` | Same. |
| `riscv64gc-unknown-linux-gnu` | Needs a riscv64 cross toolchain. |
| `powerpc64le-unknown-linux-gnu` | Needs a ppc64le cross toolchain. |
| `s390x-unknown-linux-gnu` | Big-endian; the ABI uses fixed-width types, so this is a build question rather than a correctness one. |
| `x86_64-pc-windows-gnu` | MinGW ABI, for a GCC-based Windows toolchain. |
| `aarch64-linux-android` with a newer API level | `scripts/package-ffi-android.sh --api 24` |

If you build one of these and it works, a note on the issue tracker is welcome —
that is how a target moves into the matrix.

## Proving it works on your platform

The ABI conformance program is the check that matters, and it is the same one CI
runs. It compiles the header, asserts struct layouts against *your* compiler,
and exercises create/destroy and the refusal paths:

```bash
scripts/ffi-c-example.sh
```

That builds `examples/ffi-c/abi_smoke.c` and runs it. If the header's
`_Static_assert`s fire, your platform lays out one of the ABI structs
differently and the archive is not usable as-is — please report that, because it
is precisely what those assertions exist to catch.

The Rust suite is worth running too:

```bash
cargo test -p hya-ffi
```

## 32-bit platforms

The ABI is written in fixed-width types and works on 32-bit targets, but the
header's struct-layout assertions are guarded on 64-bit pointer width, because
one table cannot describe both. On a 32-bit target the sizes and offsets are
simply not checked. If you are shipping to one, generating a second table is
worthwhile — the numbers come from `crates/hydra-ffi/src/abi.rs`, whose
`layout` module is the Rust-side mirror of the same values.

## Big-endian platforms

Nothing in the ABI has an endianness: every field is a fixed-width integer
passed in native order across a same-process boundary. The persisted state file
is JSON, so it is portable across endianness too. The engine's own byte handling
is endianness-agnostic. This should just work; it is untested only because CI has
no big-endian runner.

## The `dist` profile

`--profile dist` adds `panic = "abort"`, which is worth roughly 600 KB. Read the
comment on `[profile.dist]` in the workspace `Cargo.toml` before using it: hydra
spawns one task per connection, and under `panic = "abort"` a panic in one fetch
task takes the whole process down, killing every unrelated in-flight transfer.
Under unwinding, that task's handle reports the failure, the scheduler reclaims
its range, and the other transfers continue.

For a phone app that runs one or two downloads, the trade is often worth it. For
a service managing many concurrent transfers, it is not.
