/*
 * A complete download client in C, using libhydra.
 *
 * Copyright (C) 2026 Javad Rajabzadeh
 * SPDX-License-Identifier: MIT OR Apache-2.0
 *
 *   ./download <url> <output-path> [more-mirror-urls...]
 *
 * This is the program the FFI exists for, and it is deliberately the FIRST
 * consumer written rather than the last: a C client that can drive the whole
 * life cycle is what proves the ABI is genuinely language-neutral, in a way
 * that a Rust test of Rust functions never can.
 *
 * It shows the shape every binding should follow:
 *
 *   - check the ABI version before trusting anything else;
 *   - configure with an initialised struct, never a positional constructor;
 *   - create a job, get back a durable id, start it;
 *   - drain the event queue and render from the events;
 *   - free everything the library handed over.
 */
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "hydra.h"

static volatile sig_atomic_t interrupted = 0;

static void on_sigint(int sig)
{
    (void)sig;
    interrupted = 1;
}

/* Print whatever detail the library recorded for this thread's last failure. */
static void print_last_error(const char *what)
{
    hydra_error_t e;
    memset(&e, 0, sizeof e);
    if (hydra_last_error(&e) == HYDRA_OK && e.message.data) {
        fprintf(stderr, "%s: %s (%s", what, e.message.data, hydra_error_name(e.code));
        if (e.http_status) {
            fprintf(stderr, ", HTTP %d", e.http_status);
        }
        if (e.os_error) {
            fprintf(stderr, ", errno %d", e.os_error);
        }
        fprintf(stderr, ")\n");
        /* Allocated by hydra, freed by hydra. Never free(e.message.data). */
        hydra_error_free(&e);
    } else {
        fprintf(stderr, "%s: no detail available\n", what);
    }
}

static void human(uint64_t bytes, char *out, size_t n)
{
    static const char *unit[] = { "B", "KiB", "MiB", "GiB", "TiB" };
    double v = (double)bytes;
    size_t i = 0;
    while (v >= 1024.0 && i + 1 < sizeof unit / sizeof *unit) {
        v /= 1024.0;
        i++;
    }
    snprintf(out, n, "%.1f %s", v, unit[i]);
}

static void render(const hydra_progress_t *p)
{
    char done[32], total[32], rate[32];
    human(p->bytes_downloaded, done, sizeof done);
    human(p->total_bytes, total, sizeof total);
    human(p->bytes_per_second, rate, sizeof rate);
    if (p->total_bytes) {
        double pct = 100.0 * (double)p->bytes_downloaded / (double)p->total_bytes;
        printf("\r  %5.1f%%  %s / %s  %s/s  %u conn  %u src  ETA %llus     ", pct,
               done, total, rate, p->active_connections, p->active_sources,
               (unsigned long long)p->eta_seconds);
    } else {
        printf("\r  %s  %s/s     ", done, rate);
    }
    fflush(stdout);
}

int main(int argc, char **argv)
{
    if (argc < 3) {
        fprintf(stderr, "usage: %s <url> <output-path> [mirror-url...]\n", argv[0]);
        return 2;
    }

    /* A header from one ABI and a library from another disagree about every
     * struct below. Refuse rather than find out field by field. */
    if (hydra_ffi_abi_version() != HYDRA_FFI_ABI_VERSION) {
        fprintf(stderr, "ABI mismatch: header %u, library %u\n",
                (unsigned)HYDRA_FFI_ABI_VERSION, hydra_ffi_abi_version());
        return 1;
    }

    signal(SIGINT, on_sigint);

    hydra_engine_config_t cfg;
    memset(&cfg, 0, sizeof cfg);
    if (HYDRA_ENGINE_CONFIG_INIT(&cfg) != HYDRA_OK) {
        print_last_error("engine config");
        return 1;
    }
    cfg.max_connections = 8;      /* a ceiling, not a target */
    cfg.progress_interval_ms = 100;
    cfg.state_path = "hydra-state.json";

    hydra_engine_t *engine = hydra_engine_create(&cfg);
    if (!engine) {
        print_last_error("engine create");
        return 1;
    }

    /* Mirrors: every extra argument is another source for the SAME object.
     * hydra drops any that disagree about size or validator rather than
     * splicing bytes from two different files together. */
    const char **urls = calloc((size_t)argc, sizeof *urls);
    size_t url_count = 0;
    urls[url_count++] = argv[1];
    for (int i = 3; i < argc; i++) {
        urls[url_count++] = argv[i];
    }

    hydra_job_config_t job;
    memset(&job, 0, sizeof job);
    if (HYDRA_JOB_CONFIG_INIT(&job) != HYDRA_OK) {
        print_last_error("job config");
        hydra_engine_destroy(engine);
        free(urls);
        return 1;
    }
    job.urls = urls;
    job.url_count = url_count;
    job.output_path = argv[2];

    hydra_job_id_t id = 0;
    if (hydra_job_create(engine, &job, &id) != HYDRA_OK) {
        print_last_error("job create");
        hydra_engine_destroy(engine);
        free(urls);
        return 1;
    }
    /* The strings above were borrowed for that call only; hydra has its own
     * copies now, so this is safe here rather than at the end. */
    free(urls);

    printf("job %llu: %s -> %s\n", (unsigned long long)id, argv[1], argv[2]);

    if (hydra_job_start(engine, id) != HYDRA_OK) {
        print_last_error("job start");
        hydra_engine_destroy(engine);
        return 1;
    }

    int exit_code = 1;
    int running = 1;
    while (running) {
        if (interrupted) {
            /* Pause rather than cancel: Ctrl-C should not throw away 3 GB.
             * The state file written at shutdown lets a later run resume. */
            interrupted = 0;
            printf("\ninterrupted; pausing\n");
            hydra_job_pause(engine, id);
        }

        hydra_event_t ev;
        memset(&ev, 0, sizeof ev);
        /* A bounded wait rather than an infinite one, so the SIGINT flag above
         * is noticed promptly. The event itself is plain data: nothing to free,
         * nothing that expires. */
        hydra_error_code_t rc = hydra_event_wait(engine, 250, &ev);
        if (rc == HYDRA_ERR_AGAIN) {
            continue;
        }
        if (rc == HYDRA_ERR_SHUTDOWN) {
            break;
        }
        if (rc != HYDRA_OK) {
            print_last_error("event wait");
            break;
        }
        if (ev.job_id != id) {
            continue;
        }

        switch ((hydra_event_type_t)ev.kind) {
        case HYDRA_EVENT_RESOLVED: {
            hydra_job_snapshot_t snap;
            memset(&snap, 0, sizeof snap);
            if (hydra_job_get_snapshot(engine, id, &snap) == HYDRA_OK) {
                printf("resolved: %s, %llu bytes\n",
                       snap.file_name.data ? snap.file_name.data : "(unnamed)",
                       (unsigned long long)snap.progress.total_bytes);
                hydra_job_snapshot_free(&snap);
            }
            break;
        }
        case HYDRA_EVENT_PROGRESS:
            render(&ev.progress);
            break;
        case HYDRA_EVENT_STALLED:
            printf("\nstalled; the engine is reassigning ranges\n");
            break;
        case HYDRA_EVENT_RETRYING:
            printf("\nretrying after %s\n", hydra_error_name(ev.error));
            break;
        case HYDRA_EVENT_VERIFYING:
            printf("\nverifying...\n");
            break;
        case HYDRA_EVENT_PAUSED:
            printf("paused at %llu bytes; run again to resume\n",
                   (unsigned long long)ev.progress.bytes_downloaded);
            running = 0;
            exit_code = 0;
            break;
        case HYDRA_EVENT_COMPLETED: {
            char done[32];
            human(ev.progress.bytes_downloaded, done, sizeof done);
            printf("\ncompleted: %s -> %s\n", done, argv[2]);
            running = 0;
            exit_code = 0;
            break;
        }
        case HYDRA_EVENT_FAILED: {
            hydra_job_snapshot_t snap;
            memset(&snap, 0, sizeof snap);
            printf("\nfailed: %s", hydra_error_name(ev.error));
            if (hydra_job_get_snapshot(engine, id, &snap) == HYDRA_OK) {
                if (snap.error_message.data && snap.error_message.len) {
                    printf(" - %s", snap.error_message.data);
                }
                hydra_job_snapshot_free(&snap);
            }
            putchar('\n');
            running = 0;
            break;
        }
        case HYDRA_EVENT_CANCELLED:
            printf("\ncancelled\n");
            running = 0;
            break;
        case HYDRA_EVENT_ENGINE_SHUTDOWN:
            running = 0;
            break;
        default:
            break;
        }
    }

    /* Shutdown before destroy so the state file is written; destroy alone would
     * still be correct, this just makes the sequence explicit. */
    hydra_engine_shutdown(engine, 5000);
    hydra_engine_destroy(engine);
    return exit_code;
}
