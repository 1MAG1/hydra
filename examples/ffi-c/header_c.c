/*
 * The header, compiled as a bare C translation unit and included twice.
 *
 * Copyright (C) 2026 Javad Rajabzadeh
 * SPDX-License-Identifier: MIT OR Apache-2.0
 *
 * The point is the *absence* of anything else: no library, no linking, no
 * behaviour. If this compiles under -Wall -Wextra -Wpedantic -Werror then the
 * header is self-contained, its include guard covers the convenience macros,
 * its static assertions hold on this toolchain, and it does not depend on
 * anything a consumer has to include first.
 *
 * The companion file header_cxx.cpp does the same for C++.
 */
#include "hydra.h"
#include "hydra.h"

/* Included AFTER hydra.h on purpose: the header must not depend on it. */
#include <string.h>

/* The classifiers must evaluate their argument exactly once: a macro that
 * expanded `code` twice would call this function twice, and the counter would
 * end at 2. */
static int hydra_header_c_calls;

static hydra_error_code_t hydra_header_c_counted(void)
{
    hydra_header_c_calls++;
    return HYDRA_ERR_AGAIN;
}

int hydra_header_c_check(void);

int hydra_header_c_check(void)
{
    hydra_engine_config_t cfg;
    hydra_job_config_t job;

    memset(&cfg, 0, sizeof cfg);
    memset(&job, 0, sizeof job);
    if (HYDRA_ENGINE_CONFIG_INIT(&cfg) != HYDRA_OK) {
        return 0;
    }
    if (HYDRA_JOB_CONFIG_INIT(&job) != HYDRA_OK) {
        return 0;
    }

    hydra_header_c_calls = 0;
    if (HYDRA_IS_ERROR(hydra_header_c_counted())) {
        return 0; /* HYDRA_ERR_AGAIN is not an error */
    }
    return hydra_header_c_calls == 1;
}
