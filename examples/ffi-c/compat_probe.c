/*
 * Old header, new library.
 *
 * Copyright (C) 2026 Javad Rajabzadeh
 * SPDX-License-Identifier: MIT OR Apache-2.0
 *
 * Somebody compiled against include/hydra.h as it was two releases ago and
 * linked the libhydra they shipped with. Then they upgraded the library and
 * did NOT recompile - which is the entire point of a stable ABI, and the one
 * arrangement no test in this repository exercises by default, because every
 * other program here is compiled against the header sitting next to it.
 *
 * scripts/ffi-c-example.sh compiles this file once per published header,
 * extracted from its git tag, and links each of them against the archive built
 * from the working tree. What it is looking for:
 *
 *   - the library still answers the ABI version the old header expects
 *   - a size-prefixed config struct SHORTER than the library's own is filled
 *     in without one byte being written past where the caller's struct ends
 *   - a hydra_event_t the size the old header declared is filled in without
 *     the library writing past it either
 *   - the enumerators the old header compiled into this program still name the
 *     same things in the library
 *   - the ordinary lifecycle - create, job, drain, shut down - still runs
 *
 * Deliberately narrow: it uses only the surface that ABI 1 has had since it
 * was published, so that it keeps compiling against every old header rather
 * than only the recent ones. Do not add a call here that a later release
 * introduced; add it to abi_smoke.c, which is always compiled against the
 * current header.
 *
 * It touches no network. It creates no files: state_path stays NULL and no job
 * is ever started, so the output path is never opened.
 */
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "hydra.h"

static int failures = 0;

#define CHECK(cond, ...)                                                       \
    do {                                                                       \
        if (!(cond)) {                                                         \
            fprintf(stderr, "FAIL %s:%d: ", __FILE__, __LINE__);               \
            fprintf(stderr, __VA_ARGS__);                                      \
            fputc('\n', stderr);                                               \
            failures++;                                                        \
        }                                                                      \
    } while (0)

/* A struct the caller allocated, with a wall of known bytes immediately after
 * it. If the library writes past what the caller's header declared, it writes
 * into the wall and this program says so - which is the failure a caller would
 * otherwise meet as a corrupted local variable, in their code, months later. */
#define GUARD 64
#define GUARD_BYTE 0xA5

/* Sized in uint64_t rather than char, and static rather than automatic, so the
 * storage is 8-byte aligned - the alignment every struct in this ABI needs. A
 * char array would have satisfied the compiler and then been misaligned for
 * the cast on a platform that cares. */
#define GUARDED_WORDS(type) ((sizeof(type) + GUARD + 7u) / 8u)

static int wall_intact(const unsigned char *base, size_t used, const char *what)
{
    size_t i;
    for (i = 0; i < GUARD; i++) {
        if (base[used + i] != GUARD_BYTE) {
            fprintf(stderr,
                    "FAIL: the library wrote %zu byte(s) past the end of the "
                    "caller's %s (byte %zu of the guard)\n",
                    (size_t)(i + 1), what, i);
            return 0;
        }
    }
    return 1;
}

int main(void)
{
    static uint64_t engine_words[GUARDED_WORDS(hydra_engine_config_t)];
    static uint64_t job_words[GUARDED_WORDS(hydra_job_config_t)];
    static uint64_t event_words[GUARDED_WORDS(hydra_event_t)];
    unsigned char *engine_buf = (unsigned char *)engine_words;
    unsigned char *job_buf = (unsigned char *)job_words;
    unsigned char *event_buf = (unsigned char *)event_words;
    hydra_engine_config_t *cfg = (hydra_engine_config_t *)engine_words;
    hydra_job_config_t *job = (hydra_job_config_t *)job_words;
    hydra_event_t *ev = (hydra_event_t *)event_words;
    hydra_engine_t *engine = NULL;
    const char *urls[1];
    hydra_job_id_t id = 0;
    int drained = 0;
    int saw_created = 0;

    printf("compat probe: header ABI %u / v%s, library ABI %u / v%s\n",
           (unsigned)HYDRA_FFI_ABI_VERSION, HYDRA_FFI_VERSION,
           hydra_ffi_abi_version(), hydra_ffi_version_string());

    /* The gate. Everything below assumes the two sides agree about layout, and
     * this is the only thing that says they do. */
    CHECK(hydra_ffi_abi_version() == HYDRA_FFI_ABI_VERSION,
          "this header describes ABI %u; the library implements ABI %u",
          (unsigned)HYDRA_FFI_ABI_VERSION, hydra_ffi_abi_version());
    if (hydra_ffi_abi_version() != HYDRA_FFI_ABI_VERSION) {
        /* A layout disagreement makes every check after this meaningless and
         * some of them dangerous. Stop. */
        return 1;
    }
    CHECK(hydra_ffi_version_string() != NULL && hydra_ffi_version_string()[0] != '\0',
          "the library reports no version string");

    /* The enumerators this program compiled in. They are numbers baked into
     * the binary; the library has to still call them the same things. */
    CHECK(strcmp(hydra_error_name(HYDRA_OK), "HYDRA_OK") == 0,
          "HYDRA_OK (%d) is now called %s", (int)HYDRA_OK, hydra_error_name(HYDRA_OK));
    CHECK(strcmp(hydra_error_name(HYDRA_ERR_AGAIN), "HYDRA_ERR_AGAIN") == 0,
          "HYDRA_ERR_AGAIN (%d) is now called %s", (int)HYDRA_ERR_AGAIN,
          hydra_error_name(HYDRA_ERR_AGAIN));
    CHECK(strcmp(hydra_error_name(HYDRA_ERR_NOT_FOUND), "HYDRA_ERR_NOT_FOUND") == 0,
          "HYDRA_ERR_NOT_FOUND (%d) is now called %s", (int)HYDRA_ERR_NOT_FOUND,
          hydra_error_name(HYDRA_ERR_NOT_FOUND));
    CHECK(!HYDRA_IS_ERROR(HYDRA_ERR_AGAIN), "AGAIN must not read as an error");

    /* --------------------------------------------------------------------
     * A config struct exactly as large as THIS header says, no larger. If the
     * library has appended fields since, it must fill what is here and leave
     * the rest of our buffer alone.
     * ------------------------------------------------------------------ */
    memset(engine_buf, GUARD_BYTE, sizeof engine_words);
    memset(cfg, 0, sizeof(hydra_engine_config_t));
    CHECK(hydra_engine_config_init(cfg, (uint32_t)sizeof(hydra_engine_config_t)) == HYDRA_OK,
          "the library refused a configuration of this header's size (%zu bytes)",
          sizeof(hydra_engine_config_t));
    CHECK(cfg->size == (uint32_t)sizeof(hydra_engine_config_t),
          "init stamped size %u for a %zu-byte struct", cfg->size,
          sizeof(hydra_engine_config_t));
    CHECK(cfg->version == HYDRA_ENGINE_CONFIG_VERSION,
          "init stamped config version %u, this header expects %u", cfg->version,
          (unsigned)HYDRA_ENGINE_CONFIG_VERSION);
    CHECK(cfg->max_jobs > 0 && cfg->max_connections > 0,
          "init left this header's fields empty");
    if (!wall_intact(engine_buf, sizeof(hydra_engine_config_t), "hydra_engine_config_t")) {
        failures++;
    }

    memset(job_buf, GUARD_BYTE, sizeof job_words);
    memset(job, 0, sizeof(hydra_job_config_t));
    CHECK(hydra_job_config_init(job, (uint32_t)sizeof(hydra_job_config_t)) == HYDRA_OK,
          "the library refused a job configuration of this header's size");
    CHECK(job->size == (uint32_t)sizeof(hydra_job_config_t), "job init stamped the wrong size");
    CHECK(job->resume == 1, "resume no longer defaults on");
    CHECK(job->auto_start == 0, "creating a job must still not start it");
    if (!wall_intact(job_buf, sizeof(hydra_job_config_t), "hydra_job_config_t")) {
        failures++;
    }

    /* --------------------------------------------------------------------
     * The lifecycle, and the event struct - which the CALLER allocates, so a
     * library that grew hydra_event_t would overrun this buffer.
     * ------------------------------------------------------------------ */
    cfg->max_jobs = 2;
    cfg->max_connections = 2;
    cfg->state_path = NULL; /* no file is written by this program */
    engine = hydra_engine_create(cfg);
    CHECK(engine != NULL, "the library refused an engine built from this header's config");
    if (engine == NULL) {
        fprintf(stderr, "\n%d check(s) failed\n", failures + 1);
        return 1;
    }

    urls[0] = "https://example.invalid/compat-probe.bin";
    job->urls = urls;
    job->url_count = 1;
    job->output_path = "compat-probe.bin"; /* never opened: the job is not started */
    CHECK(hydra_job_create(engine, job, &id) == HYDRA_OK,
          "the library refused a job built from this header's config");
    CHECK(id != 0, "a created job must have a non-zero id");

    /* Drain whatever the creation produced, into a struct this header's size. */
    for (;;) {
        hydra_error_code_t rc;
        memset(event_buf, GUARD_BYTE, sizeof event_words);
        memset(ev, 0, sizeof(hydra_event_t));
        rc = hydra_event_next(engine, ev);
        if (rc == HYDRA_ERR_AGAIN) {
            break;
        }
        CHECK(rc == HYDRA_OK, "draining the queue returned %s", hydra_error_name(rc));
        if (rc != HYDRA_OK) {
            break;
        }
        if (!wall_intact(event_buf, sizeof(hydra_event_t), "hydra_event_t")) {
            failures++;
            break;
        }
        if (ev->kind == HYDRA_EVENT_JOB_CREATED) {
            saw_created = 1;
            CHECK(ev->job_id == id, "JOB_CREATED carried job %llu, expected %llu",
                  (unsigned long long)ev->job_id, (unsigned long long)id);
        }
        if (++drained > 64) {
            break;
        }
    }
    CHECK(saw_created, "creating a job produced no HYDRA_EVENT_JOB_CREATED");

    CHECK(hydra_job_get_state(engine, id, NULL) == HYDRA_ERR_INVALID_ARGUMENT,
          "a NULL out pointer should still be INVALID_ARGUMENT");
    CHECK(hydra_job_start(engine, id + 1000) == HYDRA_ERR_NOT_FOUND,
          "an unknown job id should still be NOT_FOUND");

    CHECK(hydra_engine_shutdown(engine, 2000) == HYDRA_OK, "shutdown failed");
    hydra_engine_destroy(engine);

    if (failures) {
        fprintf(stderr, "\n%d check(s) failed\n", failures);
        return 1;
    }
    printf("  ok - this header still works against libhydra %s\n",
           hydra_ffi_version_string());
    return 0;
}
