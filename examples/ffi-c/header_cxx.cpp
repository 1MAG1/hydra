/*
 * The header, compiled as C++ and included twice.
 *
 * Copyright (C) 2026 Javad Rajabzadeh
 * SPDX-License-Identifier: MIT OR Apache-2.0
 *
 * Two failures this catches that the C smoke test cannot:
 *
 *   - a convenience macro defined OUTSIDE the include guard, which a second
 *     #include then redefines. That was a real defect in an earlier draft of
 *     this header;
 *   - an enum whose representation differs between C and C++. Under C++ the
 *     typedefs name real enums with a fixed underlying type; under C11 they are
 *     uint32_t. The ABI promise is that the representation is identical either
 *     way, and only a C++ compiler can check the C++ half of it.
 *
 * Compiled, not linked: this file asserts about types, not behaviour.
 */
#include "hydra.h"
#include "hydra.h"

static_assert(sizeof(hydra_error_code_t) == 4, "enum width differs under C++");
static_assert(sizeof(hydra_job_state_t) == 4, "enum width differs under C++");
static_assert(sizeof(hydra_event_type_t) == 4, "enum width differs under C++");
static_assert(sizeof(hydra_event_t) == 120, "struct layout differs under C++");
static_assert(sizeof(hydra_engine_config_t) == 104, "struct layout differs under C++");
static_assert(sizeof(hydra_job_config_t) == 160, "struct layout differs under C++");
static_assert(sizeof(hydra_job_snapshot_t) == 176, "struct layout differs under C++");

/* The macros must be usable after a second include, and must classify
 * HYDRA_ERR_AGAIN as "nothing to report" rather than as a failure. */
bool hydra_header_cxx_check()
{
    hydra_engine_config_t cfg{};
    if (HYDRA_ENGINE_CONFIG_INIT(&cfg) != HYDRA_OK) {
        return false;
    }
    hydra_job_config_t job{};
    if (HYDRA_JOB_CONFIG_INIT(&job) != HYDRA_OK) {
        return false;
    }
    return HYDRA_SUCCEEDED(HYDRA_OK) && !HYDRA_IS_ERROR(HYDRA_ERR_AGAIN)
           && HYDRA_IS_ERROR(HYDRA_ERR_IO);
}
