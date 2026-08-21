// Copyright (C) 2026 Javad Rajabzadeh
// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Permissive on purpose, and NOT GPL like the `hydra` CLI that ships beside it.
// The entire point of this crate is that a third party can embed the engine in
// an application of their own; copyleft here would defeat that, and Rust's
// static linking would propagate it to every downstream binary. This crate must
// therefore never gain a dependency on hya-cli, hya-gui or hya-host.
// See LICENSING.md.

//! # libhydra — C ABI for the hydra download engine
//!
//! Provides a stable C ABI over `hya-core` and `hya-net`, enabling hydra
//! to be embedded into applications across C, Go, Swift, Kotlin, Dart, C#, and Python.
//!
//! ## Key Principles
//! - **Canonical C ABI**: Exposes stable C symbols with opaque handles (`hydra_engine_t`) and integer job IDs (`hydra_job_id_t`).
//! - **Isolated Memory**: Hydra manages its own allocations; callers free returned objects via matching `*_free` functions.
//! - **Asynchronous Queue**: Non-blocking, bounded event queue with coalescing progress events and guaranteed terminal delivery.
//! - **Panic Safety**: All exported functions catch panics internally and translate them to error codes.
//! - **Self-Contained Runtime**: The engine manages its internal Tokio worker threads without requiring a host event loop.

#![allow(non_camel_case_types)]
#![warn(missing_docs)]
#![deny(clippy::undocumented_unsafe_blocks)]

pub mod abi;
mod convert;
mod driver;
mod engine;
mod err;
mod event;
mod exports;
mod gate;
mod guard;
mod log;
mod mem;
mod persist;
mod url;
mod verify;

pub use abi::*;
pub use exports::*;

/// The ABI version implemented by this library.
pub const HYDRA_FFI_ABI_VERSION: u32 = 1;

/// The version field value stamped by `hydra_engine_config_init`.
pub const HYDRA_ENGINE_CONFIG_VERSION: u32 = 1;

/// The version field value stamped by `hydra_job_config_init`.
pub const HYDRA_JOB_CONFIG_VERSION: u32 = 1;

/// Value indicating an indefinite wait for `hydra_event_wait`.
pub const HYDRA_WAIT_FOREVER: u32 = u32::MAX;

/// Library version string corresponding to `Cargo.toml`.
pub const HYDRA_FFI_VERSION: &str = "0.1.0";

/// Major version component of `HYDRA_FFI_VERSION`.
pub const HYDRA_FFI_VERSION_MAJOR: u32 = 0;
/// Minor version component of `HYDRA_FFI_VERSION`.
pub const HYDRA_FFI_VERSION_MINOR: u32 = 1;
/// Patch version component of `HYDRA_FFI_VERSION`.
pub const HYDRA_FFI_VERSION_PATCH: u32 = 0;

/// Numeric version encoded as `major * 1_000_000 + minor * 1_000 + patch` for preprocessor checks.
pub const HYDRA_FFI_VERSION_NUMBER: u32 =
    HYDRA_FFI_VERSION_MAJOR * 1_000_000 + HYDRA_FFI_VERSION_MINOR * 1_000 + HYDRA_FFI_VERSION_PATCH;

const _: () = assert!(
    HYDRA_FFI_VERSION_MINOR < 1_000 && HYDRA_FFI_VERSION_PATCH < 1_000,
    "version component exceeds 3-digit limit for HYDRA_FFI_VERSION_NUMBER encoding"
);

#[cfg(test)]
mod version_tests {
    /// The numeric components are literals, because cbindgen emits `#define`s
    /// from literal expressions and not from anything it has to evaluate. That
    /// makes them capable of drifting from `Cargo.toml`, so the build refuses
    /// to let them.
    #[test]
    fn the_version_string_matches_cargo_toml() {
        assert_eq!(
            super::HYDRA_FFI_VERSION,
            env!("CARGO_PKG_VERSION"),
            "HYDRA_FFI_VERSION in src/lib.rs is out of date with version in \
             crates/hydra-ffi/Cargo.toml; update it and run scripts/gen-ffi-header.sh"
        );
    }

    #[test]
    fn the_numeric_version_matches_cargo_toml() {
        let parts: Vec<u32> = super::HYDRA_FFI_VERSION
            // A pre-release suffix (`0.2.0-rc1`) is not part of the numeric
            // triple.
            .split('-')
            .next()
            .unwrap()
            .split('.')
            .map(|p| p.parse().expect("numeric version component"))
            .collect();
        assert_eq!(
            parts,
            vec![
                super::HYDRA_FFI_VERSION_MAJOR,
                super::HYDRA_FFI_VERSION_MINOR,
                super::HYDRA_FFI_VERSION_PATCH
            ],
            "HYDRA_FFI_VERSION_{{MAJOR,MINOR,PATCH}} in src/lib.rs are out of date \
             with version in crates/hydra-ffi/Cargo.toml; update them and run \
             scripts/gen-ffi-header.sh"
        );
    }
}
