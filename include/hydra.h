/*
 * libhydra - a stable C ABI over the hydra download engine.
 *
 * Copyright (C) 2026 Javad Rajabzadeh
 * SPDX-License-Identifier: MIT OR Apache-2.0
 *
 * GENERATED FILE - do not edit. Regenerate with `make header`.
 * The source of truth is crates/hydra-ffi/src/{abi,exports}.rs.
 *
 * ---------------------------------------------------------------------------
 * THE SPECIFICATION IS docs/ffi/ABI.md
 * ---------------------------------------------------------------------------
 *
 * This header is the machine-readable half of the ABI: the declarations your
 * compiler reads, each carrying its own threading, blocking and allocation
 * behaviour. docs/ffi/ABI.md is the other half - the design principles, the
 * stability policy, the event queue's ordering and drop guarantees, and the
 * rules for credentials, rate limits, persistence and destinations. Read it
 * once before writing a binding.
 *
 * https://github.com/ja7ad/hydra/blob/main/docs/ffi/ABI.md
 *
 * What follows is the short form, for the reader who is already here.
 *
 * ABI VERSION
 *   Compare HYDRA_FFI_ABI_VERSION against hydra_ffi_abi_version() at startup
 *   and refuse to continue on a mismatch. Nothing below means anything if the
 *   two disagree. The ABI version is independent of the library version.
 *
 *   Within one ABI version: existing fields never move and never change width
 *   or meaning, enumerator values are never reassigned or reused, exported
 *   symbols never disappear, ownership rules never change, and new fields are
 *   appended only to the two size-prefixed configuration structs. Anything
 *   else becomes ABI 2. See ABI.md section 3; the frozen layout lives in
 *   crates/hydra-ffi/abi/abi-1.manifest and is enforced in CI.
 *
 * OWNERSHIP
 *   Memory allocated by hydra is freed by hydra, through the matching *_free
 *   function and NEVER through free(). A statically linked libhydra may not
 *   share an allocator with your program, and on Windows it may not share a
 *   CRT; a cross-allocator free corrupts the heap intermittently and far from
 *   the call that caused it.
 *   Strings you pass IN are borrowed for the duration of that call only. hydra
 *   copies whatever it needs before returning, so you never have to keep a
 *   buffer alive on hydra's behalf.
 *   user_data handed to a callback is never owned and never freed by hydra; it
 *   must outlive the registration.
 *
 * ENCODING
 *   Every string crossing this boundary is UTF-8. Invalid UTF-8 supplied by a
 *   caller is HYDRA_ERR_INVALID_ARGUMENT, never a lossy conversion.
 *
 * LANGUAGE BASELINE
 *   C11 or later, or C++11 or later. Fixed-width integer types and static
 *   assertions throughout; every language this ABI targets can meet that.
 *
 * ENUM REPRESENTATION
 *   Every ABI-visible enum VALUE is a uint32_t. That is a statement about
 *   values rather than about enumeration types: under C++ and C23 the typedef
 *   names a real enum with uint32_t as its fixed underlying type, while under
 *   C11 the typedef IS uint32_t and the enumerators are ordinary constants.
 *   The assertions at the foot of this header check it in whichever mode you
 *   compile.
 *
 *   Hence the asymmetry in struct fields: a field hydra WRITES (an event's
 *   kind, a snapshot's state, an error's code) is declared as the enum, because
 *   hydra constructs it. A field hydra READS from you is a uint32_t and is
 *   validated, because you can put any bit pattern in a struct field.
 *
 * ERRORS
 *   Every fallible call returns hydra_error_code_t. The detail behind it -
 *   message, errno, HTTP status - is in a THREAD-LOCAL slot readable with
 *   hydra_last_error(), cleared at the start of every call. Branch on the
 *   code; never parse the message.
 *
 *   HYDRA_ERR_AGAIN is not a failure. It means "nothing to report right now",
 *   and the non-blocking event calls return it constantly. Use HYDRA_IS_ERROR()
 *   rather than a bare != HYDRA_OK when you mean "something went wrong".
 *
 * PANICS
 *   No Rust panic crosses this boundary. An internal failure becomes
 *   HYDRA_ERR_INTERNAL with the detail preserved.
 *
 * THREADS
 *   hydra_engine_t*      thread-safe
 *   job operations       thread-safe
 *   event consumption    thread-safe, but intended for ONE consumer
 *   hydra_engine_destroy synchronisation-sensitive: must not race with any
 *                        other call on the same engine
 *
 * RUNTIME
 *   The engine owns its own threads. Your program needs no async runtime, no
 *   event loop and no particular thread. No Rust future appears in this ABI.
 *
 * EVENTS
 *   The bounded queue is the fundamental mechanism; callbacks are an optional
 *   convenience and are EXPERIMENTAL. Progress events coalesce, terminal
 *   events (COMPLETED, FAILED, CANCELLED, ENGINE_SHUTDOWN) are never dropped,
 *   life-cycle events drop oldest-first and every drop is counted in
 *   hydra_event_t.dropped_events. Ordering is guaranteed WITHIN one job only,
 *   and every event is delivered exactly once - drain in one place and
 *   dispatch from there. The full order is in ABI.md section 5.
 *
 * BYTES
 *   File data never crosses this ABI. hydra writes the object directly to its
 *   destination by positioned writes; this interface carries control, state,
 *   progress and errors only. That is what keeps resident memory independent
 *   of file size.
 *
 * CREDENTIALS
 *   Passwords, proxy passwords and the Authorization, Proxy-Authorization and
 *   Cookie headers never appear in a snapshot, an event, an error message, the
 *   log sink, the metrics, or the persisted state file. Userinfo in a URL is
 *   stripped at hydra_job_create(). A restored job comes back without its
 *   credentials by design - re-arm it with hydra_job_set_credentials().
 *
 * MACROS
 *   The helpers below are thin wrappers over static inline functions, so an
 *   argument with side effects is evaluated exactly once. HYDRA_IS_ERROR(f())
 *   calls f() one time, which a naive macro would not.
 *
 * CRATE NAMES
 *   The engine crates are named hya-core and hya-net (in directories
 *   crates/hydra-core and crates/hydra-net). That prefix is deliberate and is
 *   not a typo where it appears below.
 *
 * STABILITY
 *   Everything here is STABLE except hydra_job_get_sources,
 *   hydra_source_info_t and hydra_source_array_t, which are EXPERIMENTAL and
 *   may change within ABI 1.
 */

#ifndef HYDRA_H
#define HYDRA_H



#include <stdint.h>
#include <stddef.h>
/* --------------------------------------------------------------------------
 * These macros are defined here, before the declarations they wrap, because
 * this is the hook that lands INSIDE the include guard. A macro is expanded at
 * its use site, so it does not need the function to be declared yet - and
 * being inside the guard is what makes a second #include of this header
 * harmless.
 * -------------------------------------------------------------------------- */

/* Initialise a configuration struct.
 *
 * These pass sizeof(*(c)) as the caller's header declares it, which is what
 * makes initialisation safe when your header is OLDER than the library you
 * link against: the library writes at most the number of bytes your struct
 * actually has, and everything past that keeps its documented default. Always
 * prefer these to calling the underlying function with a hand-written size.
 *
 * `c` appears twice but is evaluated once: sizeof does not evaluate its
 * operand. */
#define HYDRA_ENGINE_CONFIG_INIT(c) \
    hydra_engine_config_init((c), (uint32_t)sizeof(*(c)))
#define HYDRA_JOB_CONFIG_INIT(c) \
    hydra_job_config_init((c), (uint32_t)sizeof(*(c)))

/* Static assertions, spelled for whichever language is compiling this. */
#if defined(__cplusplus)
  #define HYDRA_STATIC_ASSERT(cond, msg) static_assert(cond, msg)
#elif defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
  #define HYDRA_STATIC_ASSERT(cond, msg) _Static_assert(cond, msg)
#else
  /* Pre-C11, which is what MSVC compiles a .c file as unless it is given
   * /std:c11 or later. Declaring an array with a negative length fails just as
   * loudly, and the layout checks below are the point rather than the spelling
   * of the assertion. The message survives as the typedef's neighbour in the
   * error, which is as much as this construct can carry. */
  #define HYDRA_STATIC_ASSERT_JOIN_(a, b) a##b
  #define HYDRA_STATIC_ASSERT_NAME_(a, b) HYDRA_STATIC_ASSERT_JOIN_(a, b)
  #define HYDRA_STATIC_ASSERT(cond, msg) typedef char HYDRA_STATIC_ASSERT_NAME_(hydra_static_assert_, __LINE__)[(cond) ? 1 : -1]
#endif

/* The code classifiers (hydra_succeeded / hydra_failed / hydra_is_error and
 * their uppercase aliases) are at the FOOT of this header, not here: they are
 * inline functions rather than bare macros, so their bodies need HYDRA_OK to
 * be declared first. */

/* Import/export decoration. A static build needs none; a shared build on
 * Windows needs both halves, and which half depends on who is compiling. */
#if defined(_WIN32)
  #if defined(HYDRA_BUILD_SHARED)
    #define HYDRA_API __declspec(dllexport)
  #elif defined(HYDRA_USE_SHARED)
    #define HYDRA_API __declspec(dllimport)
  #else
    #define HYDRA_API
  #endif
#else
  #define HYDRA_API
#endif


/**
 * The ABI version implemented by this library.
 */
#define HYDRA_FFI_ABI_VERSION 1

/**
 * The version field value stamped by `hydra_engine_config_init`.
 */
#define HYDRA_ENGINE_CONFIG_VERSION 1

/**
 * The version field value stamped by `hydra_job_config_init`.
 */
#define HYDRA_JOB_CONFIG_VERSION 1

/**
 * Value indicating an indefinite wait for `hydra_event_wait`.
 */
#define HYDRA_WAIT_FOREVER UINT32_MAX

/**
 * Major version component of `HYDRA_FFI_VERSION`.
 */
#define HYDRA_FFI_VERSION_MAJOR 0

/**
 * Minor version component of `HYDRA_FFI_VERSION`.
 */
#define HYDRA_FFI_VERSION_MINOR 3

/**
 * Patch version component of `HYDRA_FFI_VERSION`.
 */
#define HYDRA_FFI_VERSION_PATCH 0

/**
 * The library version this header was generated from.
 *
 * The LIBRARY version, not the ABI version: it moves on every release,
 * including ones that change nothing a binding can observe. Compare
 * HYDRA_FFI_ABI_VERSION to decide whether a header and a library are
 * compatible; use this to report what you linked against.
 *
 * hydra_ffi_version_string() returns the value compiled into the library.
 * If the two disagree, this header is not the one that library was built
 * from.
 */
#define HYDRA_FFI_VERSION "0.3.0"

/**
 * Numeric version encoded as `major * 1_000_000 + minor * 1_000 + patch` for preprocessor checks.
 */
#define HYDRA_FFI_VERSION_NUMBER (((HYDRA_FFI_VERSION_MAJOR * 1000000) + (HYDRA_FFI_VERSION_MINOR * 1000)) + HYDRA_FFI_VERSION_PATCH)

/**
 * Status and error codes returned by ABI functions.
 */
enum hydra_error_code_t
#if defined(__cplusplus) || __STDC_VERSION__ >= 202311L
  : uint32_t
#endif // defined(__cplusplus) || __STDC_VERSION__ >= 202311L
 {
  /**
   * Operation completed successfully.
   */
  HYDRA_OK = 0,
  /**
   * Invalid argument, NULL pointer, or malformed parameter.
   */
  HYDRA_ERR_INVALID_ARGUMENT = 1,
  /**
   * URL parsing failed or unsupported scheme.
   */
  HYDRA_ERR_INVALID_URL = 2,
  /**
   * Operation is invalid for the job's current state.
   */
  HYDRA_ERR_INVALID_STATE = 3,
  /**
   * Requested feature or scheme is unsupported.
   */
  HYDRA_ERR_UNSUPPORTED = 4,
  /**
   * Non-blocking call has no data available right now.
   */
  HYDRA_ERR_AGAIN = 5,
  /**
   * General network or transport failure.
   */
  HYDRA_ERR_NETWORK = 6,
  /**
   * Connection establishment failed or connection reset.
   */
  HYDRA_ERR_CONNECTION = 7,
  /**
   * Network or operation timeout expired.
   */
  HYDRA_ERR_TIMEOUT = 8,
  /**
   * Protocol error encountered.
   */
  HYDRA_ERR_PROTOCOL = 9,
  /**
   * Filesystem or I/O error.
   */
  HYDRA_ERR_IO = 10,
  /**
   * Filesystem permission denied.
   */
  HYDRA_ERR_PERMISSION = 11,
  /**
   * Target disk or partition has insufficient space.
   */
  HYDRA_ERR_NO_SPACE = 12,
  /**
   * Computed checksum does not match expected digest.
   */
  HYDRA_ERR_CHECKSUM = 13,
  /**
   * Integrity verification failed to read or verify target file.
   */
  HYDRA_ERR_VERIFICATION = 14,
  /**
   * Operation was cancelled by caller.
   */
  HYDRA_ERR_CANCELLED = 15,
  /**
   * Specified job, source, or file not found.
   */
  HYDRA_ERR_NOT_FOUND = 16,
  /**
   * Entity or job ID already exists.
   */
  HYDRA_ERR_ALREADY_EXISTS = 17,
  /**
   * System or configured resource limit reached.
   */
  HYDRA_ERR_RESOURCE_LIMIT = 18,
  /**
   * Engine is shutting down or shut down.
   */
  HYDRA_ERR_SHUTDOWN = 19,
  /**
   * Internal engine error or caught panic.
   */
  HYDRA_ERR_INTERNAL = 20,
};
#ifndef __cplusplus
#if __STDC_VERSION__ >= 202311L
typedef enum hydra_error_code_t hydra_error_code_t;
#else
typedef uint32_t hydra_error_code_t;
#endif // __STDC_VERSION__ >= 202311L
#endif // __cplusplus

/**
 * Download job lifecycle states.
 */
enum hydra_job_state_t
#if defined(__cplusplus) || __STDC_VERSION__ >= 202311L
  : uint32_t
#endif // defined(__cplusplus) || __STDC_VERSION__ >= 202311L
 {
  /**
   * Created and pending initial start.
   */
  HYDRA_JOB_CREATED = 0,
  /**
   * Queued and waiting for an execution slot.
   */
  HYDRA_JOB_QUEUED = 1,
  /**
   * Probing source capabilities and metadata.
   */
  HYDRA_JOB_RESOLVING = 2,
  /**
   * Actively transferring data.
   */
  HYDRA_JOB_DOWNLOADING = 3,
  /**
   * Paused; partial downloads and range maps preserved.
   */
  HYDRA_JOB_PAUSED = 4,
  /**
   * Transfer complete; verifying checksum.
   */
  HYDRA_JOB_VERIFYING = 5,
  /**
   * Download successfully completed and verified.
   */
  HYDRA_JOB_COMPLETED = 6,
  /**
   * Job encountered an error and stopped.
   */
  HYDRA_JOB_FAILED = 7,
  /**
   * Job was cancelled by request.
   */
  HYDRA_JOB_CANCELLED = 8,
};
#ifndef __cplusplus
#if __STDC_VERSION__ >= 202311L
typedef enum hydra_job_state_t hydra_job_state_t;
#else
typedef uint32_t hydra_job_state_t;
#endif // __STDC_VERSION__ >= 202311L
#endif // __cplusplus

/**
 * Event types emitted on state transitions and progress ticks.
 */
enum hydra_event_type_t
#if defined(__cplusplus) || __STDC_VERSION__ >= 202311L
  : uint32_t
#endif // defined(__cplusplus) || __STDC_VERSION__ >= 202311L
 {
  /**
   * Job was created.
   */
  HYDRA_EVENT_JOB_CREATED = 0,
  /**
   * Job was admitted to the wait queue.
   */
  HYDRA_EVENT_JOB_QUEUED = 1,
  /**
   * Job started execution.
   */
  HYDRA_EVENT_JOB_STARTED = 2,
  /**
   * Sources resolved (size and capabilities determined).
   */
  HYDRA_EVENT_RESOLVED = 3,
  /**
   * Periodic download progress sample.
   */
  HYDRA_EVENT_PROGRESS = 4,
  /**
   * Job was paused.
   */
  HYDRA_EVENT_PAUSED = 5,
  /**
   * Job was resumed from paused state.
   */
  HYDRA_EVENT_RESUMED = 6,
  /**
   * Transfer failed and is retrying.
   */
  HYDRA_EVENT_RETRYING = 7,
  /**
   * Source list or active mirrors changed.
   */
  HYDRA_EVENT_SOURCE_CHANGED = 8,
  /**
   * Transfer stalled due to lack of progress.
   */
  HYDRA_EVENT_STALLED = 9,
  /**
   * Checksum verification started.
   */
  HYDRA_EVENT_VERIFYING = 10,
  /**
   * Job finished successfully.
   */
  HYDRA_EVENT_COMPLETED = 11,
  /**
   * Job failed.
   */
  HYDRA_EVENT_FAILED = 12,
  /**
   * Job was cancelled.
   */
  HYDRA_EVENT_CANCELLED = 13,
  /**
   * Engine is shutting down.
   */
  HYDRA_EVENT_ENGINE_SHUTDOWN = 14,
};
#ifndef __cplusplus
#if __STDC_VERSION__ >= 202311L
typedef enum hydra_event_type_t hydra_event_type_t;
#else
typedef uint32_t hydra_event_type_t;
#endif // __STDC_VERSION__ >= 202311L
#endif // __cplusplus

/**
 * Which dialect a document was written in.
 */
enum hydra_metalink_version_t
#if defined(__cplusplus) || __STDC_VERSION__ >= 202311L
  : uint32_t
#endif // defined(__cplusplus) || __STDC_VERSION__ >= 202311L
 {
  /**
   * The document has no recognisable Metalink namespace.
   */
  HYDRA_METALINK_UNKNOWN = 0,
  /**
   * Metalink 3.0, `http://www.metalinker.org/`. Preference is 0-100, higher
   * is better — the reader converts it, so nothing downstream sees the
   * inverted scale.
   */
  HYDRA_METALINK_V3 = 3,
  /**
   * Metalink 4 / RFC 5854, `urn:ietf:params:xml:ns:metalink`. Priority is
   * 1-999999, lower is better.
   */
  HYDRA_METALINK_V4 = 4,
};
#ifndef __cplusplus
#if __STDC_VERSION__ >= 202311L
typedef enum hydra_metalink_version_t hydra_metalink_version_t;
#else
typedef uint32_t hydra_metalink_version_t;
#endif // __STDC_VERSION__ >= 202311L
#endif // __cplusplus

/**
 * Disposition of partial download files upon cancellation.
 */
enum hydra_cancel_mode_t
#if defined(__cplusplus) || __STDC_VERSION__ >= 202311L
  : uint32_t
#endif // defined(__cplusplus) || __STDC_VERSION__ >= 202311L
 {
  /**
   * Keep partial files on disk.
   */
  HYDRA_CANCEL_KEEP_PARTIAL = 0,
  /**
   * Remove partial files from disk.
   */
  HYDRA_CANCEL_REMOVE_PARTIAL = 1,
};
#ifndef __cplusplus
#if __STDC_VERSION__ >= 202311L
typedef enum hydra_cancel_mode_t hydra_cancel_mode_t;
#else
typedef uint32_t hydra_cancel_mode_t;
#endif // __STDC_VERSION__ >= 202311L
#endif // __cplusplus

/**
 * Job queue priority level.
 */
enum hydra_priority_t
#if defined(__cplusplus) || __STDC_VERSION__ >= 202311L
  : uint32_t
#endif // defined(__cplusplus) || __STDC_VERSION__ >= 202311L
 {
  /**
   * Low priority.
   */
  HYDRA_PRIORITY_LOW = 0,
  /**
   * Normal priority (default).
   */
  HYDRA_PRIORITY_NORMAL = 1,
  /**
   * High priority.
   */
  HYDRA_PRIORITY_HIGH = 2,
};
#ifndef __cplusplus
#if __STDC_VERSION__ >= 202311L
typedef enum hydra_priority_t hydra_priority_t;
#else
typedef uint32_t hydra_priority_t;
#endif // __STDC_VERSION__ >= 202311L
#endif // __cplusplus

/**
 * Network interface usage policy.
 */
enum hydra_network_policy_t
#if defined(__cplusplus) || __STDC_VERSION__ >= 202311L
  : uint32_t
#endif // defined(__cplusplus) || __STDC_VERSION__ >= 202311L
 {
  /**
   * Allow transfers over any available network.
   */
  HYDRA_NETWORK_ANY = 0,
  /**
   * Allow transfers over unmetered connections only.
   */
  HYDRA_NETWORK_UNMETERED = 1,
  /**
   * Allow transfers over Wi-Fi connections only.
   */
  HYDRA_NETWORK_WIFI_ONLY = 2,
};
#ifndef __cplusplus
#if __STDC_VERSION__ >= 202311L
typedef enum hydra_network_policy_t hydra_network_policy_t;
#else
typedef uint32_t hydra_network_policy_t;
#endif // __STDC_VERSION__ >= 202311L
#endif // __cplusplus

/**
 * Power consumption profile for the engine.
 */
enum hydra_power_mode_t
#if defined(__cplusplus) || __STDC_VERSION__ >= 202311L
  : uint32_t
#endif // defined(__cplusplus) || __STDC_VERSION__ >= 202311L
 {
  /**
   * Full performance mode.
   */
  HYDRA_POWER_NORMAL = 0,
  /**
   * Reduced concurrency and lower event frequency.
   */
  HYDRA_POWER_BATTERY_SAVER = 1,
  /**
   * Single-connection minimal power mode.
   */
  HYDRA_POWER_RESTRICTED = 2,
};
#ifndef __cplusplus
#if __STDC_VERSION__ >= 202311L
typedef enum hydra_power_mode_t hydra_power_mode_t;
#else
typedef uint32_t hydra_power_mode_t;
#endif // __STDC_VERSION__ >= 202311L
#endif // __cplusplus

/**
 * Forward proxy protocol type.
 */
enum hydra_proxy_type_t
#if defined(__cplusplus) || __STDC_VERSION__ >= 202311L
  : uint32_t
#endif // defined(__cplusplus) || __STDC_VERSION__ >= 202311L
 {
  /**
   * Direct connection without proxy.
   */
  HYDRA_PROXY_NONE = 0,
  /**
   * HTTP/HTTPS forward proxy.
   */
  HYDRA_PROXY_HTTP = 1,
  /**
   * SOCKS4 proxy.
   */
  HYDRA_PROXY_SOCKS4 = 2,
  /**
   * SOCKS4a proxy with remote DNS resolution.
   */
  HYDRA_PROXY_SOCKS4A = 3,
  /**
   * SOCKS5 proxy.
   */
  HYDRA_PROXY_SOCKS5 = 4,
};
#ifndef __cplusplus
#if __STDC_VERSION__ >= 202311L
typedef enum hydra_proxy_type_t hydra_proxy_type_t;
#else
typedef uint32_t hydra_proxy_type_t;
#endif // __STDC_VERSION__ >= 202311L
#endif // __cplusplus

/**
 * Checksum digest algorithm.
 */
enum hydra_checksum_algorithm_t
#if defined(__cplusplus) || __STDC_VERSION__ >= 202311L
  : uint32_t
#endif // defined(__cplusplus) || __STDC_VERSION__ >= 202311L
 {
  /**
   * No checksum verification.
   */
  HYDRA_CHECKSUM_NONE = 0,
  /**
   * MD5 digest (16 bytes).
   */
  HYDRA_CHECKSUM_MD5 = 1,
  /**
   * SHA-1 digest (20 bytes).
   */
  HYDRA_CHECKSUM_SHA1 = 2,
  /**
   * SHA-256 digest (32 bytes).
   */
  HYDRA_CHECKSUM_SHA256 = 3,
  /**
   * SHA-512 digest (64 bytes).
   */
  HYDRA_CHECKSUM_SHA512 = 4,
  /**
   * BLAKE3 digest (32 bytes).
   */
  HYDRA_CHECKSUM_BLAKE3 = 5,
};
#ifndef __cplusplus
#if __STDC_VERSION__ >= 202311L
typedef enum hydra_checksum_algorithm_t hydra_checksum_algorithm_t;
#else
typedef uint32_t hydra_checksum_algorithm_t;
#endif // __STDC_VERSION__ >= 202311L
#endif // __cplusplus

/**
 * Logging verbosity levels.
 */
enum hydra_log_level_t
#if defined(__cplusplus) || __STDC_VERSION__ >= 202311L
  : uint32_t
#endif // defined(__cplusplus) || __STDC_VERSION__ >= 202311L
 {
  /**
   * Error messages.
   */
  HYDRA_LOG_ERROR = 0,
  /**
   * Warning messages.
   */
  HYDRA_LOG_WARN = 1,
  /**
   * Informational messages.
   */
  HYDRA_LOG_INFO = 2,
  /**
   * Debug messages.
   */
  HYDRA_LOG_DEBUG = 3,
  /**
   * Trace-level messages.
   */
  HYDRA_LOG_TRACE = 4,
};
#ifndef __cplusplus
#if __STDC_VERSION__ >= 202311L
typedef enum hydra_log_level_t hydra_log_level_t;
#else
typedef uint32_t hydra_log_level_t;
#endif // __STDC_VERSION__ >= 202311L
#endif // __cplusplus

/**
 * Opaque engine instance handle type.
 */
typedef struct hydra_engine_t hydra_engine_t;

/**
 * An opaque, parsed Metalink document.
 *
 * Created by `hydra_metalink_parse`, `hydra_metalink_open` or
 * `hydra_metalink_fetch`, and released with `hydra_metalink_free`. Immutable
 * once created, so it may be read from several threads at once.
 */
typedef struct hydra_metalink_t hydra_metalink_t;

/**
 * Owned, NUL-terminated UTF-8 string allocated by hydra.
 *
 * Must be freed with `hydra_string_free` and never with `free()`.
 */
typedef struct {
  /**
   * UTF-8 bytes, NUL-terminated. NULL when absent.
   */
  char *data;
  /**
   * Length in bytes, excluding the NUL terminator.
   */
  size_t len;
} hydra_string_t;

/**
 * Detailed error container with OS and HTTP status metadata.
 *
 * Owned strings must be released with `hydra_error_free`.
 */
typedef struct {
  /**
   * Primary error classification.
   */
  hydra_error_code_t code;
  /**
   * Platform error code (errno / GetLastError), or 0.
   */
  int32_t os_error;
  /**
   * HTTP response status code, or 0.
   */
  int32_t http_status;
  /**
   * Human-readable error message.
   */
  hydra_string_t message;
} hydra_error_t;

/**
 * Global engine configuration.
 *
 * Initialise with `hydra_engine_config_init` before setting fields.
 */
typedef struct {
  /**
   * Struct size in bytes for version compatibility.
   */
  uint32_t size;
  /**
   * Struct version (`HYDRA_ENGINE_CONFIG_VERSION`).
   */
  uint32_t version;
  /**
   * Maximum number of concurrent active jobs (default 4).
   */
  uint32_t max_jobs;
  /**
   * Maximum connections per job ceiling (default 8).
   */
  uint32_t max_connections;
  /**
   * Default retry attempts for failed transfers (default 3).
   */
  uint32_t max_retries;
  /**
   * Minimum interval in milliseconds between progress events (default 250).
   */
  uint32_t progress_interval_ms;
  /**
   * Event queue capacity (default 1024).
   */
  uint32_t event_queue_capacity;
  /**
   * Internal runtime worker threads (0 for auto).
   */
  uint32_t worker_threads;
  /**
   * Engine-wide download rate limit in bytes/sec (0 = unlimited).
   */
  uint64_t max_bytes_per_second;
  /**
   * Nonzero to enable dynamic connection scaling (default 1).
   */
  uint8_t adaptive_concurrency;
  /**
   * Nonzero to enable range work stealing (default 1).
   */
  uint8_t range_stealing;
  /**
   * Nonzero to disable TLS certificate verification (testing only).
   */
  uint8_t allow_insecure_tls;
  /**
   * Reserved; must be zero.
   */
  uint8_t reserved0;
  /**
   * Initial network policy (`hydra_network_policy_t`).
   */
  uint32_t network_policy;
  /**
   * Initial power mode (`hydra_power_mode_t`).
   */
  uint32_t power_mode;
  /**
   * File path for durable state storage, or NULL.
   */
  const char *state_path;
  /**
   * Custom HTTP User-Agent string, or NULL for default.
   */
  const char *user_agent;
  /**
   * Reserved for future fields; must be zero.
   */
  uint8_t reserved[32];
} hydra_engine_config_t;

/**
 * HTTP request header key-value pair.
 */
typedef struct {
  /**
   * Header name (excluding trailing colon).
   */
  const char *name;
  /**
   * Header value.
   */
  const char *value;
} hydra_header_t;

/**
 * Proxy server configuration.
 */
typedef struct {
  /**
   * Proxy protocol (`hydra_proxy_type_t`).
   */
  uint32_t kind;
  /**
   * Proxy port.
   */
  uint16_t port;
  /**
   * Reserved; must be zero.
   */
  uint8_t reserved[2];
  /**
   * Proxy hostname or IP address.
   */
  const char *host;
  /**
   * Optional username, or NULL.
   */
  const char *username;
  /**
   * Optional password, or NULL.
   */
  const char *password;
} hydra_proxy_config_t;

/**
 * Checksum verification specification.
 */
typedef struct {
  /**
   * Digest algorithm (`hydra_checksum_algorithm_t`).
   */
  uint32_t algorithm;
  /**
   * Reserved; must be zero.
   */
  uint32_t reserved;
  /**
   * Pointer to raw expected digest bytes, or NULL.
   */
  const uint8_t *digest;
  /**
   * Expected digest byte length.
   */
  size_t digest_len;
} hydra_checksum_t;

/**
 * Download job configuration.
 *
 * Initialise with `hydra_job_config_init` before setting fields.
 */
typedef struct {
  /**
   * Struct size in bytes for version compatibility.
   */
  uint32_t size;
  /**
   * Struct version (`HYDRA_JOB_CONFIG_VERSION`).
   */
  uint32_t version;
  /**
   * Array of source URLs (mirrors).
   */
  const char *const *urls;
  /**
   * Number of URLs in `urls` (minimum 1).
   */
  size_t url_count;
  /**
   * Destination file path.
   */
  const char *output_path;
  /**
   * Array of custom HTTP headers, or NULL.
   */
  const hydra_header_t *headers;
  /**
   * Number of headers in `headers`.
   */
  size_t header_count;
  /**
   * Optional HTTP Basic / FTP username, or NULL.
   */
  const char *username;
  /**
   * Optional HTTP Basic / FTP password, or NULL.
   */
  const char *password;
  /**
   * Proxy configuration for this job, or NULL.
   */
  const hydra_proxy_config_t *proxy;
  /**
   * Checksum verification specification.
   */
  hydra_checksum_t checksum;
  /**
   * Max connections for this job (0 inherits engine default).
   */
  uint32_t max_connections;
  /**
   * Max retries for this job (0 inherits engine default).
   */
  uint32_t max_retries;
  /**
   * Job priority (`hydra_priority_t`).
   */
  uint32_t priority;
  /**
   * Reserved; must be zero.
   */
  uint32_t reserved0;
  /**
   * Per-job download rate limit in bytes/sec (0 = unlimited).
   */
  uint64_t max_bytes_per_second;
  /**
   * Nonzero to resume from existing file ranges (default 1).
   */
  uint8_t resume;
  /**
   * Nonzero to allow adaptive concurrency (default 1).
   */
  uint8_t adaptive;
  /**
   * Nonzero to start transfer immediately upon creation.
   */
  uint8_t auto_start;
  /**
   * Reserved; must be zero.
   */
  uint8_t reserved1;
  /**
   * Reserved for future fields; must be zero.
   */
  uint8_t reserved[32];
} hydra_job_config_t;

/**
 * Runtime network and execution policies applied by host platform.
 */
typedef struct {
  /**
   * Network policy enum value (`hydra_network_policy_t`).
   */
  uint32_t network_policy;
  /**
   * Power mode enum value (`hydra_power_mode_t`).
   */
  uint32_t power_mode;
  /**
   * Nonzero to allow cellular networks.
   */
  uint8_t allow_cellular;
  /**
   * Nonzero to allow metered connections.
   */
  uint8_t allow_metered;
  /**
   * Nonzero to pause active transfers on low battery.
   */
  uint8_t pause_on_low_battery;
  /**
   * Nonzero to pause active transfers when app is in background.
   */
  uint8_t pause_when_backgrounded;
  /**
   * Reserved for future expansion; must be zero.
   */
  uint8_t reserved[4];
} hydra_runtime_policy_t;

/**
 * Engine-wide operational counters and metrics.
 */
typedef struct {
  /**
   * Total network bytes received.
   */
  uint64_t bytes_received;
  /**
   * Total bytes written to disk.
   */
  uint64_t bytes_written;
  /**
   * Total range requests issued.
   */
  uint64_t request_count;
  /**
   * Total transfer retries attempted.
   */
  uint64_t retry_count;
  /**
   * Total errors encountered across all jobs.
   */
  uint64_t error_count;
  /**
   * Total ranges reclaimed from connections that stopped delivering, over
   * this engine's lifetime. Both causes; see `hydra_progress_t::stall_count`.
   */
  uint64_t stall_count;
  /**
   * Total jobs created over engine lifetime.
   */
  uint64_t jobs_created;
  /**
   * Total jobs successfully completed.
   */
  uint64_t jobs_completed;
  /**
   * Total jobs failed.
   */
  uint64_t jobs_failed;
  /**
   * Low-priority events dropped under queue load.
   */
  uint64_t events_dropped;
} hydra_metrics_t;

/**
 * Durable download job identifier (nonzero).
 */
typedef uint64_t hydra_job_id_t;

/**
 * Owned list of job IDs. Release with `hydra_job_id_array_free`.
 */
typedef struct {
  /**
   * Array items, or NULL if empty.
   */
  hydra_job_id_t *items;
  /**
   * Number of items in array.
   */
  size_t len;
} hydra_job_id_array_t;

/**
 * Download progress metrics and transfer statistics.
 */
typedef struct {
  /**
   * Total bytes downloaded to disk.
   */
  uint64_t bytes_downloaded;
  /**
   * Total content size in bytes (0 if unknown).
   */
  uint64_t total_bytes;
  /**
   * Smoothed instantaneous transfer rate in bytes/sec.
   */
  uint64_t bytes_per_second;
  /**
   * Average transfer rate over this session in bytes/sec.
   */
  uint64_t average_bytes_per_second;
  /**
   * Estimated time remaining in seconds (0 if unknown).
   */
  uint64_t eta_seconds;
  /**
   * Number of active network connections.
   */
  uint32_t active_connections;
  /**
   * Number of active mirror sources.
   */
  uint32_t active_sources;
  /**
   * Number of completed byte ranges.
   */
  uint32_t completed_ranges;
  /**
   * Total number of tracked byte ranges.
   */
  uint32_t total_ranges;
  /**
   * Total range retry requests issued.
   */
  uint64_t retry_count;
  /**
   * Ranges reclaimed from a connection that stopped delivering.
   *
   * Counts both causes, because to a range they are the same event: a
   * connection graded stalled by the no-progress timeout, and a connection
   * whose fetch returned an error (a socket the peer had closed, a truncated
   * body, a refused request). A failure the transport retries internally on a
   * fresh connection is NOT counted — nothing was reclaimed and the range
   * never went back to the scheduler.
   */
  uint64_t stall_count;
} hydra_progress_t;

/**
 * Point-in-time snapshot of job status and metadata.
 *
 * Owned strings must be released with `hydra_job_snapshot_free`.
 */
typedef struct {
  /**
   * Job identifier.
   */
  hydra_job_id_t id;
  /**
   * Current job lifecycle state.
   */
  hydra_job_state_t state;
  /**
   * Error code if job failed.
   */
  hydra_error_code_t error_code;
  /**
   * Progress statistics.
   */
  hydra_progress_t progress;
  /**
   * Resolved target URL.
   */
  hydra_string_t url;
  /**
   * Output file destination path.
   */
  hydra_string_t output_path;
  /**
   * Suggested or inferred file name.
   */
  hydra_string_t file_name;
  /**
   * Failure detail message, if any.
   */
  hydra_string_t error_message;
  /**
   * Job creation timestamp (ms since epoch).
   */
  uint64_t created_at_ms;
  /**
   * Last start timestamp (ms since epoch), or 0.
   */
  uint64_t started_at_ms;
  /**
   * Completion or termination timestamp (ms since epoch), or 0.
   */
  uint64_t finished_at_ms;
} hydra_job_snapshot_t;

/**
 * Source (mirror) identifier, unique within an engine instance.
 */
typedef uint64_t hydra_source_id_t;

/**
 * Source mirror statistics and status.
 */
typedef struct {
  /**
   * Source ID.
   */
  hydra_source_id_t id;
  /**
   * Source URL.
   */
  hydra_string_t url;
  /**
   * Total bytes downloaded from this source.
   */
  uint64_t bytes_downloaded;
  /**
   * Current transfer rate in bytes/sec.
   */
  uint64_t bytes_per_second;
  /**
   * Measured setup latency in microseconds.
   */
  uint64_t latency_us;
  /**
   * Active connections assigned to this source.
   */
  uint32_t active_connections;
  /**
   * Errors charged to this source.
   */
  uint32_t error_count;
  /**
   * Nonzero if source is currently active.
   */
  uint8_t active;
  /**
   * Reserved; must be ignored.
   */
  uint8_t reserved[7];
} hydra_source_info_t;

/**
 * Owned list of source statistics. Release with `hydra_source_array_free`.
 */
typedef struct {
  /**
   * Array items, or NULL if empty.
   */
  hydra_source_info_t *items;
  /**
   * Number of items in array.
   */
  size_t len;
} hydra_source_array_t;

/**
 * Event notification emitted by the engine event queue.
 */
typedef struct {
  /**
   * Event type classification.
   */
  hydra_event_type_t kind;
  /**
   * Job state at event generation time.
   */
  hydra_job_state_t state;
  /**
   * Target job ID (0 for engine-level events).
   */
  hydra_job_id_t job_id;
  /**
   * Progress snapshot at event generation time.
   */
  hydra_progress_t progress;
  /**
   * Error code if this is a failure event.
   */
  hydra_error_code_t error;
  /**
   * HTTP status code, or 0.
   */
  int32_t http_status;
  /**
   * Platform error code, or 0.
   */
  int32_t os_error;
  /**
   * Reserved; must be ignored.
   */
  uint32_t reserved;
  /**
   * Timestamp in milliseconds since Unix epoch.
   */
  uint64_t timestamp_ms;
  /**
   * Count of coalesced or dropped low-priority events.
   */
  uint64_t dropped_events;
} hydra_event_t;

/**
 * Optional callback function for event notifications.
 */
typedef void (*hydra_event_callback)(const hydra_event_t *event,
                                     void *user_data);

/**
 * Optional callback function for engine log messages.
 */
typedef void (*hydra_log_callback)(uint32_t level,
                                   const char *message,
                                   void *user_data);

/**
 * One file entry from a document.
 */
typedef struct {
  /**
   * The document's `name`, as a relative path.
   *
   * Absent when the name would escape the output directory (`../`, an
   * absolute path, a Windows drive letter or an alternate data stream);
   * `name_usable` is then zero and the entry cannot be downloaded.
   */
  hydra_string_t name;
  /**
   * The strongest digest the document published, as `algorithm:hex`, or an
   * absent string.
   */
  hydra_string_t digest;
  /**
   * The entry's `version`, or an absent string.
   */
  hydra_string_t version;
  /**
   * Object size in bytes, or 0 if the document stated none.
   *
   * A stated size is what admits a mirror to a multi-source transfer: it is
   * evidence from the publisher rather than from the mirrors, so it replaces
   * the `ETag` agreement independent mirror operators cannot satisfy.
   */
  uint64_t size;
  /**
   * Piece length in bytes, or 0 if the document published no `<pieces>`.
   */
  uint64_t piece_length;
  /**
   * Number of piece digests published.
   */
  size_t piece_count;
  /**
   * Mirrors listed, including ones this build cannot fetch from.
   */
  size_t mirror_count;
  /**
   * Mirrors this build has a transport for.
   */
  size_t fetchable_count;
  /**
   * The publisher's DEFAULT per-mirror connection ceiling, or 0 for none.
   *
   * Metalink 3.0 `<resources maxconnections>`. Read as the default each
   * mirror inherits rather than as a cap across the whole file: mirrormanager
   * emits `maxconnections="1"` beside a seventeen-mirror list, and the
   * aggregate reading would make that list useless. A mirror stating its own
   * overrides it; `hydra_metalink_url_t.max_connections` reports what
   * actually governs each one.
   */
  uint32_t max_connections;
  /**
   * Nonzero when the piece list tiles the stated size exactly.
   *
   * Zero means the two disagree, so the pieces describe a different object
   * and are not used — the whole-file digest is then the only check.
   */
  uint8_t pieces_tile;
  /**
   * Nonzero if the entry carries an OpenPGP `<signature>`.
   *
   * Recorded, never verified: hydra does not check it, and reporting it as
   * verified would be worse than not reporting it at all.
   */
  uint8_t signed_;
  /**
   * Nonzero when `name` is a usable relative path.
   */
  uint8_t name_usable;
  /**
   * Reserved; must be ignored.
   */
  uint8_t reserved[5];
} hydra_metalink_file_t;

/**
 * Owned list of file entries. Release with `hydra_metalink_file_array_free`.
 */
typedef struct {
  /**
   * Array items, or NULL if empty.
   */
  hydra_metalink_file_t *items;
  /**
   * Number of items in array.
   */
  size_t len;
} hydra_metalink_file_array_t;

/**
 * One mirror named by a document, as the ranking the engine would use.
 */
typedef struct {
  /**
   * The mirror URL.
   */
  hydra_string_t url;
  /**
   * ISO 3166-1 alpha-2 country code, lowercased, or an absent string.
   */
  hydra_string_t location;
  /**
   * URL scheme: `http`, `https`, `ftp`, or whatever the document named.
   */
  hydra_string_t protocol;
  /**
   * Rank after the engine's own ordering has been applied: 1 is best.
   *
   * Always in the RFC 5854 direction whichever dialect the document used.
   */
  uint32_t priority;
  /**
   * Connection ceiling the mirror stated for itself, or 0 if it stated none.
   *
   * A stated value NARROWS the client's own per-host ceiling and never
   * widens it.
   */
  uint32_t max_connections;
  /**
   * Nonzero if this build has a transport for the URL's scheme.
   *
   * A document may name `rsync://` mirrors; they are visible here and are
   * never given to a transfer.
   */
  uint8_t fetchable;
  /**
   * Reserved; must be ignored.
   */
  uint8_t reserved[7];
} hydra_metalink_url_t;

/**
 * Owned list of mirrors. Release with `hydra_metalink_url_array_free`.
 */
typedef struct {
  /**
   * Array items, or NULL if empty.
   */
  hydra_metalink_url_t *items;
  /**
   * Number of items in array.
   */
  size_t len;
} hydra_metalink_url_array_t;

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

/**
 * The ABI version this library implements.
 *
 * Compare against `HYDRA_FFI_ABI_VERSION` from the header you compiled with;
 * a mismatch means the header and the library disagree about every struct
 * below and the program should refuse to continue.
 *
 * Thread-safe. Non-blocking. Does not allocate.
 */
HYDRA_API
uint32_t hydra_ffi_abi_version(void);

/**
 * The library's own version, as a static NUL-terminated string.
 *
 * Never freed by the caller: it points into the library's read-only data.
 *
 * Thread-safe. Non-blocking. Does not allocate.
 */
HYDRA_API
const char *hydra_ffi_version_string(void);

/**
 * The stable spelling of an error code, as a static NUL-terminated string.
 *
 * Never freed by the caller. Unknown codes return `"HYDRA_ERR_UNKNOWN"` rather
 * than NULL, so a caller can print the result unconditionally.
 *
 * Thread-safe. Non-blocking. Does not allocate.
 */
HYDRA_API
const char *hydra_error_name(uint32_t code);

/**
 * The detail behind this thread's most recent failure.
 *
 * The slot is **thread-local**: it describes the last hydra call made on the
 * calling thread and is cleared at the start of every call, so reading it after
 * a success reports `HYDRA_OK` rather than a stale failure.
 *
 * Returns `HYDRA_ERR_NOT_FOUND` when nothing has failed on this thread.
 *
 * Thread-safe. Non-blocking. **Allocates**: `out->message` is owned by the
 * caller and must be released with hydra_error_free.
 *
 * # Safety
 *
 * `out` must point to a writable hydra_error_t.
 */
HYDRA_API
hydra_error_code_t hydra_last_error(hydra_error_t *out);

/**
 * Release the owned parts of an error.
 *
 * Safe to call on a zeroed struct and safe to call twice in the sense that the
 * second call sees a NULL message — but the second call is still a bug, and the
 * pointer is cleared here to make it a harmless one.
 *
 * Thread-safe. Non-blocking.
 *
 * # Safety
 *
 * `e` must be NULL or point to an hydra_error_t this library produced.
 */
HYDRA_API
void hydra_error_free(hydra_error_t *e);

/**
 * Release a string this library produced.
 *
 * Never call `free()` on `value.data`: a static libhydra may not share an
 * allocator with the host program.
 *
 * Thread-safe. Non-blocking.
 *
 * # Safety
 *
 * `value` must be a string this library produced and not yet freed, or the
 * NULL value.
 */
HYDRA_API
void hydra_string_free(hydra_string_t value);

/**
 * Fill an engine configuration with defaults.
 *
 * `struct_size` is `sizeof(hydra_engine_config_t)` **as the caller's header
 * declares it**, and passing it is what makes this safe in both directions: a
 * program built against an older header has a smaller struct, and a library
 * that wrote its own `sizeof` would run off the end of it. Use the
 * `HYDRA_ENGINE_CONFIG_INIT` convenience macro from the header, which supplies
 * it from `sizeof`.
 *
 * Thread-safe. Non-blocking. Does not allocate.
 *
 * # Safety
 *
 * `config` must point to at least `struct_size` writable bytes.
 */
HYDRA_API
hydra_error_code_t hydra_engine_config_init(hydra_engine_config_t *config,
                                            uint32_t struct_size);

/**
 * Fill a job configuration with defaults.
 *
 * See hydra_engine_config_init for why `struct_size` is a parameter. Use
 * the `HYDRA_JOB_CONFIG_INIT` macro from the header.
 *
 * Thread-safe. Non-blocking. Does not allocate.
 *
 * # Safety
 *
 * `config` must point to at least `struct_size` writable bytes.
 */
HYDRA_API
hydra_error_code_t hydra_job_config_init(hydra_job_config_t *config,
                                         uint32_t struct_size);

/**
 * Fill a runtime policy with the permissive defaults: any network, full power.
 *
 * Thread-safe. Non-blocking. Does not allocate.
 *
 * # Safety
 *
 * `policy` must point to a writable hydra_runtime_policy_t.
 */
HYDRA_API
hydra_error_code_t hydra_runtime_policy_init(hydra_runtime_policy_t *policy);

/**
 * Create an engine.
 *
 * Returns NULL on failure; call hydra_last_error on the same thread for the
 * reason. The engine starts its own threads and is ready to accept jobs when
 * this returns.
 *
 * Thread-safe. **Blocking** only in the sense that it builds a thread pool and
 * a TLS root store, which is milliseconds. Allocates.
 *
 * # Safety
 *
 * `config` must have been initialised by hydra_engine_config_init and all
 * its string pointers must be valid for this call.
 */
HYDRA_API
hydra_engine_t *hydra_engine_create(const hydra_engine_config_t *config);

/**
 * Stop every running job and stop accepting new work.
 *
 * Running jobs are paused, not cancelled: their partial data and range maps
 * survive, and if the engine has a `state_path` they are written to it before
 * this returns. A final `HYDRA_EVENT_ENGINE_SHUTDOWN` is published, after which
 * the queue is closed and every blocked hydra_event_wait returns.
 *
 * Post-conditions, which are deterministic and worth stating exactly:
 *
 * * no job is `HYDRA_JOB_CANCELLED` merely because a shutdown happened;
 * * every job that was running is `HYDRA_JOB_PAUSED`;
 * * incomplete work is retained in durable state when `state_path` is set;
 * * the event queue is closed and every blocked hydra_event_wait returns.
 *
 * Returns `HYDRA_OK` when every transfer stopped within `timeout_ms`, and in
 * that case no network operation is still in flight. Returns
 * `HYDRA_ERR_TIMEOUT` when one did not: the engine is still shut down and the
 * post-conditions above still hold, but a socket may still be draining until
 * hydra_engine_destroy tears the runtime down. The distinction exists so a
 * host that needs "nothing is running" as a fact can tell whether it has one.
 *
 * In **both** cases, once this returns, **no new network work can start**.
 * Every job has been told to stop, no job can be started, and a transfer that
 * is still unwinding will not issue another request. That is the guarantee
 * that matters on a platform which is about to suspend the process: a timeout
 * means "something has not finished letting go", never "something may still
 * begin".
 *
 * `timeout_ms` bounds only that wait, not the whole call.
 *
 * Calling this before hydra_engine_destroy is optional but is the way to
 * get a deterministic stop. Idempotent: a second call returns `HYDRA_OK`.
 *
 * Thread-safe. **Blocking** for up to roughly `timeout_ms`.
 *
 * # Safety
 *
 * `engine` must be a valid handle.
 */
HYDRA_API
hydra_error_code_t hydra_engine_shutdown(hydra_engine_t *engine,
                                         uint32_t timeout_ms);

/**
 * Destroy an engine and release everything it owns.
 *
 * hydra_engine_shutdown is the lifecycle transition; this is resource
 * release. Prefer calling them in that order, and treat this as the
 * destructor it is — a C++ `~Engine`, a Swift `deinit`, a Go finalizer or a
 * JNI `close()` should not be where a network lifecycle happens.
 *
 * If shutdown was not called, this performs a **best-effort emergency
 * shutdown** first — with a fixed internal grace period rather than one you
 * choose — so that the simple path (create, use, destroy) is still correct and
 * still writes state. That is a safety net, not the intended sequence.
 *
 * **Synchronisation-sensitive.** This must not race with any other call on the
 * same engine, and the handle must not be used afterwards. Passing NULL is a
 * no-op.
 *
 * Thread-safe with respect to *other* engines. Blocking for up to a few hundred
 * milliseconds while runtime threads are joined.
 *
 * # Safety
 *
 * `engine` must be NULL or a handle from hydra_engine_create that has not
 * already been destroyed, and no other thread may be inside a hydra call on it.
 */
HYDRA_API
void hydra_engine_destroy(hydra_engine_t *engine);

/**
 * Replace the platform policy.
 *
 * Takes effect for jobs started after the call; a running transfer keeps the
 * connection count it was admitted with, because tearing down live connections
 * to satisfy a new ceiling costs more than it saves.
 *
 * Thread-safe. Non-blocking. Does not allocate.
 *
 * # Safety
 *
 * `engine` must be valid and `policy` must point to a readable
 * hydra_runtime_policy_t.
 */
HYDRA_API
hydra_error_code_t hydra_engine_set_policy(hydra_engine_t *engine,
                                           const hydra_runtime_policy_t *policy);

/**
 * Read the active platform policy.
 *
 * Thread-safe. Non-blocking. Does not allocate.
 *
 * # Safety
 *
 * `engine` must be valid and `out` must be writable.
 */
HYDRA_API
hydra_error_code_t hydra_engine_get_policy(hydra_engine_t *engine,
                                           hydra_runtime_policy_t *out);

/**
 * Change the engine-wide rate ceiling, in bytes per second. 0 = unlimited.
 *
 * Applies immediately, including to transfers already running — to every job,
 * whether or not it has a ceiling of its own, since the engine-wide limiter is
 * an aggregate over all of them. See the note on `max_bytes_per_second` in the
 * header.
 *
 * Thread-safe. Non-blocking. Does not allocate.
 *
 * # Safety
 *
 * `engine` must be valid.
 */
HYDRA_API
hydra_error_code_t hydra_engine_set_max_bytes_per_second(hydra_engine_t *engine,
                                                         uint64_t bytes_per_second);

/**
 * Change how many jobs may execute at once.
 *
 * Raising it admits queued jobs immediately. Lowering it never preempts a
 * running job; the new ceiling takes hold as jobs finish.
 *
 * Thread-safe. Non-blocking. Does not allocate.
 *
 * # Safety
 *
 * `engine` must be valid.
 */
HYDRA_API
hydra_error_code_t hydra_engine_set_max_jobs(hydra_engine_t *engine,
                                             uint32_t max_jobs);

/**
 * Read the engine's counters.
 *
 * Thread-safe. Non-blocking. Does not allocate.
 *
 * # Safety
 *
 * `engine` must be valid and `out` writable.
 */
HYDRA_API
hydra_error_code_t hydra_engine_get_metrics(hydra_engine_t *engine,
                                            hydra_metrics_t *out);

/**
 * List every job the engine knows about, in creation order.
 *
 * Thread-safe. Non-blocking. **Allocates**: release with
 * hydra_job_id_array_free.
 *
 * # Safety
 *
 * `engine` must be valid and `out` writable.
 */
HYDRA_API
hydra_error_code_t hydra_engine_list_jobs(hydra_engine_t *engine,
                                          hydra_job_id_array_t *out);

/**
 * Release a job-id array.
 *
 * Thread-safe. Non-blocking.
 *
 * # Safety
 *
 * `a` must be NULL or an array this library produced and not yet freed.
 */
HYDRA_API
void hydra_job_id_array_free(hydra_job_id_array_t *a);

/**
 * Write every job's durable state to the engine's `state_path`.
 *
 * The write is atomic — a temporary file and a rename — so a process death
 * during it cannot leave a truncated state file. Returns
 * `HYDRA_ERR_INVALID_STATE` when the engine was created without a `state_path`.
 *
 * This also happens automatically whenever a job reaches a terminal state; call
 * it explicitly when the platform tells you the process is about to be
 * suspended.
 *
 * Thread-safe. **Blocking**: performs file I/O. Allocates internally.
 *
 * # Safety
 *
 * `engine` must be valid.
 */
HYDRA_API
hydra_error_code_t hydra_engine_snapshot(hydra_engine_t *engine);

/**
 * Load persisted jobs from the engine's `state_path`.
 *
 * Restores identities and range maps, not execution: every job that was
 * running when the state was written comes back as `HYDRA_JOB_PAUSED`, and
 * nothing starts until the application calls hydra_job_resume. On a phone
 * that is the correct division — whether work may run now is the platform
 * layer's decision, not the engine's.
 *
 * Credentials are never persisted. A job whose configuration included a
 * password or a credential-bearing header comes back without them; use
 * hydra_job_set_credentials before resuming it.
 *
 * `out_restored` receives how many jobs were added; it may be NULL. Ids already
 * present are skipped rather than overwritten.
 *
 * Thread-safe. **Blocking**: performs file I/O. Allocates internally.
 *
 * # Safety
 *
 * `engine` must be valid; `out_restored` must be NULL or writable.
 */
HYDRA_API
hydra_error_code_t hydra_engine_restore(hydra_engine_t *engine,
                                        size_t *out_restored);

/**
 * Create a job and return its durable id.
 *
 * Nothing happens on the network until the job is started, unless
 * `config->auto_start` is set. The id is stable for the life of the engine and,
 * with persistence enabled, across process restarts.
 *
 * Every string in `config` is borrowed for this call only.
 *
 * Thread-safe. Non-blocking. Allocates internally.
 *
 * # Safety
 *
 * `engine` must be valid, `config` must have been initialised by
 * hydra_job_config_init, and every pointer in it must be valid for this
 * call.
 */
HYDRA_API
hydra_error_code_t hydra_job_create(hydra_engine_t *engine,
                                    const hydra_job_config_t *config,
                                    hydra_job_id_t *out_job_id);

/**
 * Start a job.
 *
 * Legal from `HYDRA_JOB_CREATED`, and also from `HYDRA_JOB_PAUSED`,
 * `HYDRA_JOB_FAILED` and `HYDRA_JOB_CANCELLED` — restarting a failed job is a
 * thing applications legitimately do, and refusing it would only make them
 * recreate the job and lose its range map. A job already running returns
 * `HYDRA_ERR_INVALID_STATE`; a completed one does too.
 *
 * What "start" means for a job that has already been somewhere is decided by
 * the range map, not by the previous state, and the rule is one sentence:
 * **whatever spans hydra still records as present are reused; everything else
 * is fetched.** So
 *
 * * a paused or failed job continues from where it stopped;
 * * a job cancelled with `HYDRA_CANCEL_KEEP_PARTIAL` continues too, because
 *   the file and its range map both survived;
 * * a job cancelled with `HYDRA_CANCEL_REMOVE_PARTIAL` starts from zero,
 *   because cancelling that way cleared both.
 *
 * A job created with `resume = 0` always starts from zero.
 *
 * Returns as soon as the job is queued. The transfer runs on engine threads and
 * reports through the event queue.
 *
 * Thread-safe. Non-blocking. Allocates internally.
 *
 * # Safety
 *
 * `engine` must be valid.
 */
HYDRA_API
hydra_error_code_t hydra_job_start(hydra_engine_t *engine,
                                   hydra_job_id_t job_id);

/**
 * Stop a running job, preserving everything needed to resume it.
 *
 * The sockets are closed; the partial file, the range map and the source
 * information all survive. The job reaches `HYDRA_JOB_PAUSED` asynchronously
 * and publishes `HYDRA_EVENT_PAUSED` when it gets there — this call only asks.
 *
 * Thread-safe. Non-blocking. Does not allocate.
 *
 * # Safety
 *
 * `engine` must be valid.
 */
HYDRA_API
hydra_error_code_t hydra_job_pause(hydra_engine_t *engine,
                                   hydra_job_id_t job_id);

/**
 * Resume a paused job.
 *
 * Only legal from `HYDRA_JOB_PAUSED`; use hydra_job_start for anything
 * else. The transfer picks up from the recorded range map rather than from
 * zero, which is what makes an interrupted 4 GB download cost the remainder
 * and not the whole thing.
 *
 * Thread-safe. Non-blocking. Allocates internally.
 *
 * # Safety
 *
 * `engine` must be valid.
 */
HYDRA_API
hydra_error_code_t hydra_job_resume(hydra_engine_t *engine,
                                    hydra_job_id_t job_id);

/**
 * Cancel a job.
 *
 * `mode` is one of hydra_cancel_mode_t and decides what happens to the
 * partial file. Cancelling is safe from every non-terminal state — queued,
 * resolving, downloading, verifying, paused — and always ends at
 * `HYDRA_JOB_CANCELLED`.
 *
 * A job that is not running is cancelled synchronously; a running one is asked
 * to stop and reaches the terminal state when its transfer unwinds.
 *
 * Thread-safe. Non-blocking. May remove a file when `mode` is
 * `HYDRA_CANCEL_REMOVE_PARTIAL`.
 *
 * # Safety
 *
 * `engine` must be valid.
 */
HYDRA_API
hydra_error_code_t hydra_job_cancel(hydra_engine_t *engine,
                                    hydra_job_id_t job_id,
                                    uint32_t mode);

/**
 * Forget a job.
 *
 * Only legal once the job is in a terminal state or has never started;
 * removing a running job would leave a transfer writing to a file nobody is
 * tracking.
 *
 * The **file is not touched** — deleting a completed download because the
 * application stopped tracking it would be the wrong default, and
 * `HYDRA_CANCEL_REMOVE_PARTIAL` exists for when deletion is what you want.
 *
 * The job's **persisted metadata is removed** along with it, so a later
 * hydra_engine_restore cannot resurrect it. That write is best effort, on
 * the same terms as every other automatic snapshot: call
 * hydra_engine_snapshot and read the code if you need to know it
 * succeeded.
 *
 * Thread-safe. Non-blocking. Does not allocate.
 *
 * # Safety
 *
 * `engine` must be valid.
 */
HYDRA_API
hydra_error_code_t hydra_job_remove(hydra_engine_t *engine,
                                    hydra_job_id_t job_id);

/**
 * Set or clear a job's credentials.
 *
 * Used for HTTP basic authentication and for the `ftp://` login. Pass NULL for
 * both to clear them. Takes effect on the next attempt, so a running job must
 * be paused and resumed for a change to apply.
 *
 * This exists mainly for restored jobs: credentials are deliberately never
 * written to a state file, so a job that comes back from disk needs them
 * supplied again before it can authenticate.
 *
 * Thread-safe. Non-blocking. Allocates internally.
 *
 * # Safety
 *
 * `engine` must be valid; `username` and `password` must each be NULL or a
 * NUL-terminated UTF-8 string valid for this call.
 */
HYDRA_API
hydra_error_code_t hydra_job_set_credentials(hydra_engine_t *engine,
                                             hydra_job_id_t job_id,
                                             const char *username,
                                             const char *password);

/**
 * Re-aim where the object is written.
 *
 * Legal only from `HYDRA_JOB_CREATED`, `HYDRA_JOB_PAUSED`,
 * `HYDRA_JOB_FAILED` and `HYDRA_JOB_CANCELLED`. A job that is queued,
 * resolving, downloading or verifying returns `HYDRA_ERR_INVALID_STATE`;
 * pause it first.
 *
 * That restriction is not caution, it is correctness. A running transfer has
 * connections writing at absolute offsets into a file it opened, and a range
 * map that describes *that* file. Letting the destination move underneath it
 * would leave the finished ranges in the old path, the retried ranges in the
 * new one, and a range map claiming both are present in a single object —
 * two partial files, each looking complete by length. This call used to take
 * effect "on the next attempt", which is exactly that bug.
 *
 * A destination is a filesystem path in this ABI version and nothing else. It
 * covers desktop and an app-private directory on Android or iOS; it does not
 * cover a content URI, a security-scoped resource or a document-provider
 * handle. A future ABI may add other destination kinds alongside the path —
 * the path will keep working, but do not build on the assumption that it is
 * permanently the only storage model.
 *
 * Thread-safe. Non-blocking. Allocates internally.
 *
 * # Safety
 *
 * `engine` must be valid and `path` must be a NUL-terminated UTF-8 string valid
 * for this call.
 */
HYDRA_API
hydra_error_code_t hydra_job_set_output_path(hydra_engine_t *engine,
                                             hydra_job_id_t job_id,
                                             const char *path);

/**
 * Change a job's rate ceiling, in bytes per second. 0 = unlimited.
 *
 * Applies immediately, including to a transfer already running that started
 * with no ceiling of its own. The engine-wide cap still applies on top: the
 * job moves at the lower of the two.
 *
 * Thread-safe. Non-blocking. Does not allocate.
 *
 * # Safety
 *
 * `engine` must be valid.
 */
HYDRA_API
hydra_error_code_t hydra_job_set_max_bytes_per_second(hydra_engine_t *engine,
                                                      hydra_job_id_t job_id,
                                                      uint64_t bytes_per_second);

/**
 * Read a job's state.
 *
 * Thread-safe. Non-blocking. Does not allocate.
 *
 * # Safety
 *
 * `engine` must be valid and `out_state` writable.
 */
HYDRA_API
hydra_error_code_t hydra_job_get_state(hydra_engine_t *engine,
                                       hydra_job_id_t job_id,
                                       hydra_job_state_t *out_state);

/**
 * Read a job's progress.
 *
 * Thread-safe. Non-blocking. Does not allocate.
 *
 * # Safety
 *
 * `engine` must be valid and `out` writable.
 */
HYDRA_API
hydra_error_code_t hydra_job_get_progress(hydra_engine_t *engine,
                                          hydra_job_id_t job_id,
                                          hydra_progress_t *out);

/**
 * Take a consistent, owned picture of a job.
 *
 * Everything in the result is copied. It stays valid no matter what the engine
 * does next, which is what makes it safe to hand to a UI thread.
 *
 * Thread-safe. Non-blocking. **Allocates**: release with
 * hydra_job_snapshot_free.
 *
 * # Safety
 *
 * `engine` must be valid and `out` writable.
 */
HYDRA_API
hydra_error_code_t hydra_job_get_snapshot(hydra_engine_t *engine,
                                          hydra_job_id_t job_id,
                                          hydra_job_snapshot_t *out);

/**
 * Release a snapshot's owned strings.
 *
 * Thread-safe. Non-blocking.
 *
 * # Safety
 *
 * `s` must be NULL or a snapshot this library produced and not yet freed.
 */
HYDRA_API
void hydra_job_snapshot_free(hydra_job_snapshot_t *s);

/**
 * What each source is contributing to a job.
 *
 * **Experimental**: this call and hydra_source_info_t may change within
 * ABI 1. It exists because hydra's multi-source behaviour is worth making
 * visible — an application can show which mirror is carrying the transfer and
 * which one has stalled, instead of a single opaque rate.
 *
 * Thread-safe. Non-blocking. **Allocates**: release with
 * hydra_source_array_free.
 *
 * # Safety
 *
 * `engine` must be valid and `out` writable.
 */
HYDRA_API
hydra_error_code_t hydra_job_get_sources(hydra_engine_t *engine,
                                         hydra_job_id_t job_id,
                                         hydra_source_array_t *out);

/**
 * Release a source array and the strings inside it.
 *
 * Thread-safe. Non-blocking.
 *
 * # Safety
 *
 * `a` must be NULL or an array this library produced and not yet freed.
 */
HYDRA_API
void hydra_source_array_free(hydra_source_array_t *a);

/**
 * Take the next event, if one is pending.
 *
 * Returns `HYDRA_ERR_AGAIN` when the queue is empty — not a failure, just
 * nothing to report — and `HYDRA_ERR_SHUTDOWN` once the engine has stopped and
 * drained.
 *
 * Life-cycle events are delivered before pending progress events, so a
 * completion never waits behind a progress sample. Progress events **are**
 * coalesced: at most one per job is ever pending, and a newer sample replaces
 * an older one. Terminal events are **never** dropped.
 *
 * The event is copied into `out`; there is nothing to free and nothing that
 * expires.
 *
 * Thread-safe (intended for one consumer). Non-blocking. Does not allocate.
 *
 * # Safety
 *
 * `engine` must be valid and `out` writable.
 */
HYDRA_API
hydra_error_code_t hydra_event_next(hydra_engine_t *engine,
                                    hydra_event_t *out);

/**
 * Wait up to `timeout_ms` for an event.
 *
 * `0` polls without waiting; `HYDRA_WAIT_FOREVER` waits indefinitely. Returns
 * `HYDRA_ERR_AGAIN` on timeout and `HYDRA_ERR_SHUTDOWN` once the engine has
 * stopped — a shutdown releases every waiter immediately rather than making
 * them sit out their timeouts.
 *
 * This is the call a dedicated consumer thread should sit in: it costs nothing
 * while idle, where a polling loop costs a wake-up per interval on a device
 * with a battery.
 *
 * Thread-safe (intended for one consumer). **Blocking**. Does not allocate.
 *
 * # Safety
 *
 * `engine` must be valid and `out` writable.
 */
HYDRA_API
hydra_error_code_t hydra_event_wait(hydra_engine_t *engine,
                                    uint32_t timeout_ms,
                                    hydra_event_t *out);

/**
 * Release every thread blocked in hydra_event_wait.
 *
 * The woken calls return `HYDRA_ERR_AGAIN`. This is how a host tells its own
 * consumer thread to look at something else — a flag of its own, a request to
 * exit — without having to shut the engine down first.
 *
 * Thread-safe. Non-blocking. Does not allocate.
 *
 * # Safety
 *
 * `engine` must be valid.
 */
HYDRA_API
hydra_error_code_t hydra_event_wake(hydra_engine_t *engine);

/**
 * Install or clear the optional event callback.
 *
 * **Experimental.** The queue is the stable mechanism; this is a convenience
 * layer over it and may change within ABI 1. Getting a foreign callback right
 * differs sharply between the JVM (the thread must be attached), .NET (the
 * delegate must be pinned), Go (a cgo callback lands on a non-Go stack), Swift
 * concurrency and Python (the GIL), and freezing an interface across all of
 * them before any of those bindings exist would be guessing.
 *
 * Pass NULL to clear. The callback runs on an engine-owned thread, immediately
 * after the event is queued, and the event is **also** delivered to the queue
 * — installing a callback supplements polling rather than replacing it.
 *
 * The callback **must not block and must not call back into the engine**.
 *
 * **`user_data` is never owned by hydra and is never freed by hydra.** It is
 * stored, never dereferenced, and handed back verbatim. Freeing it while the
 * callback is installed is a use-after-free in your program, not in hydra's.
 *
 * Thread-safe. Non-blocking. Does not allocate.
 *
 * # Safety
 *
 * `engine` must be valid. `callback` must remain a valid function pointer, and
 * `user_data` a valid token, until it is cleared or the engine is destroyed.
 */
HYDRA_API
hydra_error_code_t hydra_event_set_callback(hydra_engine_t *engine,
                                            hydra_event_callback callback,
                                            void *user_data);

/**
 * Install or clear this engine's log sink.
 *
 * Per engine, not per process. Two independent consumers can live in one
 * process — a host application and a plugin, two frameworks in one iOS app,
 * two libraries in one JVM — and a global sink would let the second one
 * silently reconfigure the first one's diagnostics.
 *
 * Logs are not events. An event is a state transition your logic acts on and
 * is delivered through the queue with delivery guarantees; a log line is a
 * diagnostic for whoever is debugging, is fire-and-forget, and losing one
 * costs nothing. Do not build application behaviour on this.
 *
 * Nothing is written anywhere unless you install a sink: a library that prints
 * to `stderr` on its own initiative corrupts the output of every program that
 * embeds it.
 *
 * `max_level` is one of hydra_log_level_t; messages above it are discarded
 * before they are formatted. Pass NULL as `callback` to clear.
 *
 * **`user_data` is never owned by hydra and is never freed by hydra.** It is
 * stored, never dereferenced, and handed back to your function verbatim. It
 * must stay valid until the callback is cleared or the engine is destroyed —
 * freeing it while a sink is installed is a use-after-free in your program,
 * not in hydra's.
 *
 * Thread-safe. Non-blocking. Allocates per delivered message.
 *
 * # Safety
 *
 * `engine` must be valid. `callback` must remain a valid function pointer, and
 * `user_data` a valid token, until the sink is cleared or the engine is
 * destroyed, and the callback must tolerate being called from any thread.
 */
HYDRA_API
hydra_error_code_t hydra_engine_set_log_callback(hydra_engine_t *engine,
                                                 hydra_log_callback callback,
                                                 void *user_data,
                                                 uint32_t max_level);

/**
 * Parse a Metalink document held in memory.
 *
 * `xml` is the document text, NUL-terminated UTF-8. Both dialects are read:
 * Metalink 3.0 (`.metalink`, what mirrormanager and most distribution
 * redirectors emit) and Metalink 4 / RFC 5854 (`.meta4`). The two spell mirror
 * preference on scales that run in OPPOSITE directions; the reader normalises
 * them, so every priority this ABI reports has 1 as best.
 *
 * On success `*out_document` owns a document that must be released with
 * hydra_metalink_free.
 *
 * Thread-safe. Non-blocking. Allocates internally.
 *
 * # Safety
 *
 * `xml` must be a valid NUL-terminated string and `out_document` must be
 * writable.
 */
HYDRA_API
hydra_error_code_t hydra_metalink_parse(const char *xml,
                                        hydra_metalink_t **out_document);

/**
 * Read a Metalink document from a local file.
 *
 * Thread-safe. Blocking (one file read). Allocates internally.
 *
 * # Safety
 *
 * `path` must be a valid NUL-terminated string and `out_document` must be
 * writable.
 */
HYDRA_API
hydra_error_code_t hydra_metalink_open(const char *path,
                                       hydra_metalink_t **out_document);

/**
 * Fetch a Metalink document over HTTP and read it.
 *
 * Runs on the engine's own runtime and **blocks the calling thread** until the
 * document arrives or the fetch fails — a mirror list is kilobytes, and an
 * application that wants it off the UI thread has its own thread pool for that.
 * Redirects are followed, because mirror redirectors use them constantly.
 *
 * The body is capped at 4 MiB: it is fetched before anything about it is known,
 * and an unbounded read of a body chosen by whoever answers is a
 * memory-exhaustion primitive no amount of care in the parser can fix.
 *
 * Thread-safe. Blocking. Allocates internally.
 *
 * # Safety
 *
 * `engine` must be valid, `url` must be a valid NUL-terminated string, and
 * `out_document` must be writable.
 */
HYDRA_API
hydra_error_code_t hydra_metalink_fetch(hydra_engine_t *engine,
                                        const char *url,
                                        hydra_metalink_t **out_document);

/**
 * Release a parsed document.
 *
 * Thread-safe. Non-blocking.
 *
 * # Safety
 *
 * `document` must be NULL or a handle this library produced and not yet freed.
 */
HYDRA_API
void hydra_metalink_free(hydra_metalink_t *document);

/**
 * Which dialect a document was written in.
 *
 * Returns `HYDRA_METALINK_UNKNOWN` for an invalid handle, which is also what a
 * document with no recognisable namespace reports — the distinction is not one
 * a caller can act on differently.
 *
 * Thread-safe. Non-blocking.
 *
 * # Safety
 *
 * `document` must be a valid handle.
 */
HYDRA_API
hydra_metalink_version_t hydra_metalink_version(hydra_metalink_t *document);

/**
 * Every file entry a document describes.
 *
 * This is what a host application shows a user before anything is fetched: the
 * names, the sizes, whether a digest and a piece list are published, and how
 * many of the listed mirrors this build can actually fetch from. A mirror list
 * that silently loses two thirds of its entries to an unsupported scheme is
 * worth seeing before a multi-gigabyte download rather than after.
 *
 * Release with hydra_metalink_file_array_free.
 *
 * Thread-safe. Non-blocking. Allocates internally.
 *
 * # Safety
 *
 * `document` must be a valid handle and `out` must be writable.
 */
HYDRA_API
hydra_error_code_t hydra_metalink_files(hydra_metalink_t *document,
                                        hydra_metalink_file_array_t *out);

/**
 * Release a file array and the strings inside it.
 *
 * Thread-safe. Non-blocking.
 *
 * # Safety
 *
 * `a` must be NULL or an array this library produced and not yet freed.
 */
HYDRA_API
void hydra_metalink_file_array_free(hydra_metalink_file_array_t *a);

/**
 * The mirrors of one file entry, in the order hydra would use them.
 *
 * Best first, with `priority` renumbered densely from 1 whichever dialect the
 * document used — so a caller never has to know that Metalink 3.0's scale runs
 * the other way. Only mirrors this build has a transport for are returned;
 * `hydra_metalink_file_t.mirror_count` against `fetchable_count` is how many
 * were dropped.
 *
 * Release with hydra_metalink_url_array_free.
 *
 * Thread-safe. Non-blocking. Allocates internally.
 *
 * # Safety
 *
 * `document` must be a valid handle and `out` must be writable.
 */
HYDRA_API
hydra_error_code_t hydra_metalink_mirrors(hydra_metalink_t *document,
                                          size_t file_index,
                                          hydra_metalink_url_array_t *out);

/**
 * Release a mirror array and the strings inside it.
 *
 * Thread-safe. Non-blocking.
 *
 * # Safety
 *
 * `a` must be NULL or an array this library produced and not yet freed.
 */
HYDRA_API
void hydra_metalink_url_array_free(hydra_metalink_url_array_t *a);

/**
 * Create a job for one entry of a Metalink document.
 *
 * `config` is the ordinary job configuration and supplies everything about how
 * the transfer should behave — output path, headers, proxy, rate cap, retries,
 * priority. Its `urls` and `url_count` are IGNORED and may be NULL/0: the
 * document supplies the sources. Its `checksum` is honoured when set and
 * otherwise filled in from the document's strongest published digest, so a
 * caller with a digest from a signed announcement keeps it and a caller with
 * none still gets verification.
 *
 * What the document adds beyond the URLs is the point of this call:
 *
 * * the **size**, which admits every agreeing mirror to a multi-source transfer
 *   without the `ETag` match independent mirror operators cannot produce;
 * * the **ranking**, which decides the first split and the reserve order;
 * * the **reserve bench** — mirrors past the connection budget, substituted in
 *   place when a source dies, so nineteen mirrors are worth more than four;
 * * **`<pieces>`**, verified after the transfer with a failing chunk refetched
 *   from a different mirror instead of the whole object being downloaded again.
 *
 * A `<signature>` in the document is recorded and NOT verified. Verify it
 * yourself before trusting the digests it covers.
 *
 * Thread-safe. Non-blocking. Allocates internally.
 *
 * # Safety
 *
 * `engine` and `document` must be valid, `config` must have been initialised by
 * hydra_job_config_init, and `out_job_id` must be writable.
 */
HYDRA_API
hydra_error_code_t hydra_job_create_from_metalink(hydra_engine_t *engine,
                                                  hydra_metalink_t *document,
                                                  size_t file_index,
                                                  const hydra_job_config_t *config,
                                                  hydra_job_id_t *out_job_id);

/**
 * Find a file entry by name.
 *
 * Matches either the document's full relative name or just the base name,
 * because an application passes on what a user picked from a listing and a
 * listing generally shows the base name.
 *
 * Returns `HYDRA_ERR_NOT_FOUND` when no entry matches, leaving `*out_index`
 * untouched.
 *
 * Thread-safe. Non-blocking.
 *
 * # Safety
 *
 * `document` must be a valid handle, `name` must be a valid NUL-terminated
 * string, and `out_index` must be writable.
 */
HYDRA_API
hydra_error_code_t hydra_metalink_find_file(hydra_metalink_t *document,
                                            const char *name,
                                            size_t *out_index);

/* ==========================================================================
 * Classifying a returned code.
 *
 * HYDRA_ERR_AGAIN deliberately means "nothing to report right now", not
 * "something went wrong" - the non-blocking event calls return it constantly.
 * hydra_is_error() excludes it, so a caller cannot accidentally treat an empty
 * queue as a failure; hydra_failed() is the strict negation of success, for the
 * cases where you really do want every non-OK code.
 *
 * Inline functions rather than bare macros so that the argument is evaluated
 * exactly once: HYDRA_IS_ERROR(do_something()) must not call do_something()
 * twice. The uppercase spellings are aliases, kept because they read like the
 * rest of this header.
 *
 * These live at the foot of the header because an inline function body needs
 * the enumerators to have been declared; a macro would not.
 * ========================================================================== */
static inline int hydra_succeeded(int code) { return code == (int)HYDRA_OK; }
static inline int hydra_failed(int code) { return code != (int)HYDRA_OK; }
static inline int hydra_is_error(int code)
{
    return code != (int)HYDRA_OK && code != (int)HYDRA_ERR_AGAIN;
}

#define HYDRA_SUCCEEDED(code) hydra_succeeded((int)(code))
#define HYDRA_FAILED(code)    hydra_failed((int)(code))
#define HYDRA_IS_ERROR(code)  hydra_is_error((int)(code))

/* ==========================================================================
 * ABI layout, asserted at compile time.
 *
 * These are the numbers a compiled binding has baked into it. A field
 * reordered, a type widened, a reserved array resized: each is an ABI break
 * that compiles cleanly, links cleanly, and then hands a caller the wrong
 * bytes at run time.
 *
 * The matching table lives in crates/hydra-ffi/src/abi.rs and is checked by
 * the Rust compiler. This copy is checked by YOUR compiler, which is the half
 * that matters — a padding rule or an enum width that differs on your toolchain
 * is caught at your build rather than at your customer's run time.
 *
 * If one of these fires, the header and the library you are compiling against
 * disagree about memory. Do not silence it.
 * ========================================================================== */

/* Representation of the fixed-width types the whole ABI is built from. */
HYDRA_STATIC_ASSERT(sizeof(uint8_t) == 1, "uint8_t must be 8-bit");
HYDRA_STATIC_ASSERT(sizeof(uint16_t) == 2, "uint16_t must be 16-bit");
HYDRA_STATIC_ASSERT(sizeof(uint32_t) == 4, "uint32_t must be 32-bit");
HYDRA_STATIC_ASSERT(sizeof(uint64_t) == 8, "uint64_t must be 64-bit");
HYDRA_STATIC_ASSERT(sizeof(int32_t) == 4, "int32_t must be 32-bit");

/* Every ABI-visible enum VALUE is a uint32_t. Under C++ and C23 these typedefs
 * name enums with uint32_t as their fixed underlying type; under C11 they ARE
 * uint32_t. Either way the representation is the same, which is the only thing
 * this ABI promises about them. */
HYDRA_STATIC_ASSERT(sizeof(hydra_error_code_t) == 4, "enum value width changed");
HYDRA_STATIC_ASSERT(sizeof(hydra_job_state_t) == 4, "enum value width changed");
HYDRA_STATIC_ASSERT(sizeof(hydra_event_type_t) == 4, "enum value width changed");
HYDRA_STATIC_ASSERT(sizeof(hydra_cancel_mode_t) == 4, "enum value width changed");
HYDRA_STATIC_ASSERT(sizeof(hydra_priority_t) == 4, "enum value width changed");
HYDRA_STATIC_ASSERT(sizeof(hydra_network_policy_t) == 4, "enum value width changed");
HYDRA_STATIC_ASSERT(sizeof(hydra_power_mode_t) == 4, "enum value width changed");
HYDRA_STATIC_ASSERT(sizeof(hydra_proxy_type_t) == 4, "enum value width changed");
HYDRA_STATIC_ASSERT(sizeof(hydra_checksum_algorithm_t) == 4, "enum value width changed");
HYDRA_STATIC_ASSERT(sizeof(hydra_log_level_t) == 4, "enum value width changed");

HYDRA_STATIC_ASSERT(sizeof(hydra_job_id_t) == 8, "job ids must be 64-bit");
HYDRA_STATIC_ASSERT(sizeof(hydra_source_id_t) == 8, "source ids must be 64-bit");

/* Struct layout. Guarded on pointer width, because pointers and size_t change
 * size and one table cannot describe both; UINTPTR_MAX is the portable test
 * (MSVC does not define __SIZEOF_POINTER__). A 32-bit port adds its own table
 * rather than weakening this one. */
#if defined(UINTPTR_MAX) && UINTPTR_MAX == 0xFFFFFFFFFFFFFFFFULL

HYDRA_STATIC_ASSERT(sizeof(hydra_string_t) == 16, "hydra_string_t layout changed");
HYDRA_STATIC_ASSERT(sizeof(hydra_error_t) == 32, "hydra_error_t layout changed");
HYDRA_STATIC_ASSERT(sizeof(hydra_header_t) == 16, "hydra_header_t layout changed");
HYDRA_STATIC_ASSERT(sizeof(hydra_proxy_config_t) == 32, "hydra_proxy_config_t layout changed");
HYDRA_STATIC_ASSERT(sizeof(hydra_checksum_t) == 24, "hydra_checksum_t layout changed");
HYDRA_STATIC_ASSERT(sizeof(hydra_runtime_policy_t) == 16, "hydra_runtime_policy_t layout changed");
HYDRA_STATIC_ASSERT(sizeof(hydra_engine_config_t) == 104, "hydra_engine_config_t layout changed");
HYDRA_STATIC_ASSERT(sizeof(hydra_job_config_t) == 160, "hydra_job_config_t layout changed");
HYDRA_STATIC_ASSERT(sizeof(hydra_progress_t) == 72, "hydra_progress_t layout changed");
HYDRA_STATIC_ASSERT(sizeof(hydra_event_t) == 120, "hydra_event_t layout changed");
HYDRA_STATIC_ASSERT(sizeof(hydra_job_snapshot_t) == 176, "hydra_job_snapshot_t layout changed");
HYDRA_STATIC_ASSERT(sizeof(hydra_source_info_t) == 64, "hydra_source_info_t layout changed");
HYDRA_STATIC_ASSERT(sizeof(hydra_source_array_t) == 16, "hydra_source_array_t layout changed");
HYDRA_STATIC_ASSERT(sizeof(hydra_job_id_array_t) == 16, "hydra_job_id_array_t layout changed");
HYDRA_STATIC_ASSERT(sizeof(hydra_metrics_t) == 80, "hydra_metrics_t layout changed");

/* Offsets, which catch the reordering that sizes alone would not. */
HYDRA_STATIC_ASSERT(offsetof(hydra_error_t, message) == 16, "field moved: ABI break");
HYDRA_STATIC_ASSERT(offsetof(hydra_progress_t, bytes_downloaded) == 0, "field moved: ABI break");
HYDRA_STATIC_ASSERT(offsetof(hydra_progress_t, total_bytes) == 8, "field moved: ABI break");
HYDRA_STATIC_ASSERT(offsetof(hydra_progress_t, retry_count) == 56, "field moved: ABI break");
HYDRA_STATIC_ASSERT(offsetof(hydra_event_t, job_id) == 8, "field moved: ABI break");
HYDRA_STATIC_ASSERT(offsetof(hydra_event_t, progress) == 16, "field moved: ABI break");
HYDRA_STATIC_ASSERT(offsetof(hydra_event_t, timestamp_ms) == 104, "field moved: ABI break");
HYDRA_STATIC_ASSERT(offsetof(hydra_engine_config_t, size) == 0, "field moved: ABI break");
HYDRA_STATIC_ASSERT(offsetof(hydra_engine_config_t, version) == 4, "field moved: ABI break");
HYDRA_STATIC_ASSERT(offsetof(hydra_engine_config_t, max_bytes_per_second) == 32, "field moved: ABI break");
HYDRA_STATIC_ASSERT(offsetof(hydra_engine_config_t, state_path) == 56, "field moved: ABI break");
HYDRA_STATIC_ASSERT(offsetof(hydra_engine_config_t, reserved) == 72, "field moved: ABI break");
HYDRA_STATIC_ASSERT(offsetof(hydra_job_config_t, size) == 0, "field moved: ABI break");
HYDRA_STATIC_ASSERT(offsetof(hydra_job_config_t, version) == 4, "field moved: ABI break");
HYDRA_STATIC_ASSERT(offsetof(hydra_job_config_t, checksum) == 72, "field moved: ABI break");
HYDRA_STATIC_ASSERT(offsetof(hydra_job_config_t, reserved) == 124, "field moved: ABI break");
HYDRA_STATIC_ASSERT(offsetof(hydra_job_snapshot_t, url) == 88, "field moved: ABI break");

#endif /* 64-bit */

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* HYDRA_H */
