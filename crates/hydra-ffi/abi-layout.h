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
