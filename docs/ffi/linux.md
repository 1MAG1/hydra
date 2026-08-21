# libhydra on Linux

## Which archive

| Archive | Use it when |
|---|---|
| `libhydra-<v>-x86_64-unknown-linux-gnu` | Ordinary x86-64 distributions |
| `libhydra-<v>-aarch64-unknown-linux-gnu` | arm64 servers, Raspberry Pi 64-bit, Graviton |
| `libhydra-<v>-armv7-unknown-linux-gnueabihf` | 32-bit ARM with hardware float |
| `libhydra-<v>-x86_64-unknown-linux-musl` | Alpine, scratch containers, anything that must not depend on the host glibc |
| `libhydra-<v>-aarch64-unknown-linux-musl` | The same, on arm64 |

**glibc archives are built on the oldest distribution the release CI runs**, and
a glibc binary will not start on a system with an *older* glibc than the one it
was built against. If you are targeting an old distribution, or you do not know
what you are targeting, use musl: it is statically self-contained and runs
anywhere.

## Linking

The static library is the primary artifact, and static linking is not free of
system dependencies — an archive is not an executable. `native-static-libs.txt`
in every archive records exactly what `rustc` says the archive needs, which is
better than a list someone typed once and never revisited:

```bash
cat native-static-libs.txt
# -lgcc_s -lutil -lrt -lpthread -lm -ldl -lc
```

Then:

```bash
cc -std=c11 -I include myapp.c lib/libhydra.a \
   $(grep -v '^#' native-static-libs.txt) -o myapp
```

Or with pkg-config, which is what the bundled `hydra.pc` is for:

```bash
# Install the archive somewhere on the pkg-config path first, e.g.
sudo cp -r include/hydra.h /usr/local/include/
sudo cp lib/libhydra.a lib/libhydra.so /usr/local/lib/
sudo cp lib/pkgconfig/hydra.pc /usr/local/lib/pkgconfig/

cc myapp.c $(pkg-config --cflags --libs hydra) -o myapp          # shared
cc myapp.c $(pkg-config --cflags --libs --static hydra) -o myapp # static
```

`Libs.private` in `hydra.pc` carries the same system libraries, so
`--static` resolves them for you.

### CMake

```cmake
cmake_minimum_required(VERSION 3.16)
project(myapp C)

add_library(hydra STATIC IMPORTED)
set_target_properties(hydra PROPERTIES
    IMPORTED_LOCATION "${CMAKE_SOURCE_DIR}/libhydra/lib/libhydra.a"
    INTERFACE_INCLUDE_DIRECTORIES "${CMAKE_SOURCE_DIR}/libhydra/include")

add_executable(myapp main.c)
# The system libraries from native-static-libs.txt. Read them from the file
# rather than hardcoding: they differ between glibc and musl.
file(STRINGS "${CMAKE_SOURCE_DIR}/libhydra/native-static-libs.txt" _libs
     REGEX "^-")
separate_arguments(_libs UNIX_COMMAND "${_libs}")
target_link_libraries(myapp PRIVATE hydra ${_libs})
```

### Shared library

`lib/libhydra.so` exports the same symbols and works with the same header. At
run time it must be findable:

```bash
# For a program you are running from a build tree
LD_LIBRARY_PATH=$PWD/libhydra/lib ./myapp

# Or bake the path in at link time
cc myapp.c -Ilibhydra/include -Llibhydra/lib -lhydra \
   -Wl,-rpath,'$ORIGIN/../lib' -o myapp
```

Prefer the static library unless you have a specific reason not to. There is no
plugin boundary here to keep replaceable, and static linking removes a class of
deployment problem entirely.

## TLS and the certificate store

hydra uses `rustls` with a **compiled-in** copy of the Mozilla root store. It
does **not** read `/etc/ssl/certs`, and it does not link OpenSSL. Two
consequences worth knowing before you deploy:

- A container with no CA bundle installed still validates certificates
  correctly. That is usually a relief.
- A private CA that you added to the system trust store is **not** trusted.
  There is no ABI for adding one yet; for testing against a private CA there is
  `allow_insecure_tls` in `hydra_engine_config_t`, which disables validation
  entirely and must never be shipped enabled.

## DNS

Resolution goes through the standard resolver, so `/etc/resolv.conf`,
`/etc/hosts` and NSS behave as you expect. In a `scratch` container remember
that neither file exists by default and the musl build has no NSS at all —
either copy a `resolv.conf` in, or use an image with one.

## Containers

The musl archive plus a `scratch` or `distroless` base gives an image whose only
moving parts are your binary and the engine:

```dockerfile
FROM alpine:3 AS build
RUN apk add --no-cache gcc musl-dev
COPY libhydra /libhydra
COPY myapp.c .
RUN cc -static -std=c11 -I/libhydra/include myapp.c /libhydra/lib/libhydra.a \
      -o /myapp

FROM scratch
COPY --from=build /myapp /myapp
# hydra writes to the destination path you give it, so make sure it exists and
# is writable — there is no filesystem in a scratch image otherwise.
COPY --from=build /etc/ssl /etc/ssl
ENTRYPOINT ["/myapp"]
```

## systemd and long-running services

The engine owns its own threads and needs no event loop from you, so a service
that creates one engine at startup and keeps it for the process lifetime is the
natural shape. Two things to wire up:

- **Set `state_path`** to somewhere under `StateDirectory=`, and call
  `hydra_engine_snapshot()` on `SIGTERM` before you shut the engine down.
  Restored jobs come back paused; resume them when you are ready to run.
- **`hydra_engine_shutdown(engine, timeout_ms)`** returns `HYDRA_ERR_TIMEOUT`
  if a transfer had not stopped in time. Either way, no new network work can
  start once it returns, so it is safe to call from a signal-handling thread
  before exiting.

## Threads and file descriptors

Each connection is one socket, and the ceiling is
`max_jobs × max_connections`. The default (4 × 8) is comfortable under the usual
1024 descriptor limit; if you raise either substantially, raise `LimitNOFILE`
to match.
