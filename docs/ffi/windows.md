# libhydra on Windows

## Which archive

| Archive | Use it when |
|---|---|
| `libhydra-<v>-x86_64-pc-windows-msvc.zip` | 64-bit Intel/AMD |
| `libhydra-<v>-aarch64-pc-windows-msvc.zip` | Windows on ARM |

Both are built with the **MSVC ABI**. They link into MSVC and clang-cl projects.
They are not compatible with a MinGW/GCC toolchain — if you need GNU-ABI
binaries, build from source with `--target x86_64-pc-windows-gnu` (see
[other-platforms.md](other-platforms.md)).

## What is in `lib/`

| File | What it is |
|---|---|
| `hydra.lib` | the static library — link this and nothing ships beside your exe |
| `hydra.dll` | the shared library |
| `hydra.dll.lib` | the import library for `hydra.dll` |

The same `hydra.h` works with either. When you link against the DLL, define
`HYDRA_USE_SHARED` before including the header so the declarations get
`__declspec(dllimport)`:

```c
#define HYDRA_USE_SHARED
#include "hydra.h"
```

Omit it for the static library. Getting this wrong produces link errors about
`__imp_hydra_*` symbols.

## The C runtime — the thing that actually bites

`libhydra` is built with the **static** CRT (`/MT`), matching the rest of this
project: a DLL-linked CRT would make the library depend on the Visual C++
Redistributable being installed, which a clean Windows install does not have.

**Your program must use the static CRT too.** Mixing `/MT` and `/MD` in one
binary produces link errors at best and two heaps at worst.

```bat
cl /nologo /MT /std:c11 /I include myapp.c lib\hydra.lib ^
   ws2_32.lib userenv.lib ntdll.lib advapi32.lib bcrypt.lib kernel32.lib
```

The exact list is in `native-static-libs.txt` in the archive — read it from
there rather than copying the line above, because it changes with the Rust
version.

In CMake:

```cmake
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded")   # /MT, not /MD

add_library(hydra STATIC IMPORTED)
set_target_properties(hydra PROPERTIES
    IMPORTED_LOCATION "${CMAKE_SOURCE_DIR}/libhydra/lib/hydra.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${CMAKE_SOURCE_DIR}/libhydra/include")

add_executable(myapp main.c)
target_link_libraries(myapp PRIVATE hydra
    ws2_32 userenv ntdll advapi32 bcrypt)
```

If you must use `/MD`, build the library from source to match:

```bat
set RUSTFLAGS=-C target-feature=-crt-static
bash scripts/build-ffi.sh --target x86_64-pc-windows-msvc
```

## Paths

`output_path` is UTF-8, like every string in this ABI. Windows APIs are UTF-16,
so hydra converts internally — you pass ordinary UTF-8 and it works, including
for non-ASCII paths.

Two limits worth knowing:

- **Long paths.** Paths beyond `MAX_PATH` (260 characters) work only if long
  path support is enabled for the system *and* your executable's manifest opts
  in with `longPathAware`. Without that, a deep destination fails with
  `HYDRA_ERR_IO` and an `os_error` of `ERROR_PATH_NOT_FOUND` (3).
- **Reserved names.** `CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`
  are not usable as file names, whatever extension you add. If your destination
  name comes from a URL or a server-supplied header, sanitise it before handing
  it over — hydra will not invent a different name for you.

## Antivirus and Defender

A newly written executable being scanned mid-transfer can slow positioned
writes noticeably. If you are downloading large executables and see write
throughput far below the network rate, that is usually the cause. Downloading
to a directory excluded from real-time scanning, then moving the finished file,
is the standard mitigation — and moving it after `HYDRA_EVENT_COMPLETED` is
safe, because the object is only complete at that point.

## TLS

hydra uses `rustls` with a compiled-in Mozilla root store. It does **not** use
Schannel or the Windows certificate store, so an enterprise root you deployed
by group policy is not trusted here. That is a deliberate trade for identical
behaviour on every platform.

## Console applications and Ctrl-C

The engine runs on its own threads and does not install signal handlers. Your
`SetConsoleCtrlHandler` (or `signal(SIGINT, ...)`) can set a flag, and the main
loop can call `hydra_job_pause()` and then `hydra_engine_shutdown()` — pausing
rather than cancelling, so an interrupted multi-gigabyte download resumes on
the next run. `examples/download.c` does exactly that.
