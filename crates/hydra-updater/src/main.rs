// Copyright (C) 2026 Javad Rajabzadeh
// SPDX-License-Identifier: GPL-3.0-or-later

//! hydra-updater: the finisher that completes an update after Hydra exits.
//!
//! The GUI downloads and extracts the new release itself, copies THIS binary
//! out of the new bundle into the staging directory (so it never overwrites
//! itself while running), launches it detached, and exits. This process then:
//!
//! 1. waits a beat for the parent to finish exiting,
//! 2. copies the new files over the installed ones (retrying while the old
//!    executables are still locked — the retry loop is the "wait for exit"
//!    on Windows, where a running exe cannot be replaced but can be renamed),
//! 3. relaunches the application,
//! 4. logs everything to `<staging>/update.log` for post-mortems.
//!
//! It is deliberately headless: by the time it runs there is no UI process
//! left to host a window, and a failed swap leaves the old install intact
//! (each file is renamed aside before its replacement is copied in).

use clap::Parser;
use std::io::Write;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "hydra-updater", about = "Hydra update finisher", version)]
struct Args {
    /// Directory holding the new release's files (an extracted bundle root).
    #[arg(long = "src-dir", value_name = "DIR")]
    src_dir: PathBuf,

    /// Directory of the running install to update (where hydra-gui lives).
    #[arg(long = "install-dir", value_name = "DIR")]
    install_dir: PathBuf,

    /// Executable to launch once the swap is done.
    #[arg(long = "relaunch", value_name = "EXE")]
    relaunch: Option<PathBuf>,

    /// Extra arguments for the relaunched executable.
    #[arg(long = "relaunch-arg", value_name = "ARG")]
    relaunch_args: Vec<String>,
}

fn main() -> std::process::ExitCode {
    let args = Args::parse();
    let mut log = Log::open();
    log.line(&format!(
        "hydra-updater {} starting: src={} install={}",
        env!("CARGO_PKG_VERSION"),
        args.src_dir.display(),
        args.install_dir.display()
    ));

    // Give the parent a moment to leave its main loop; file locks (Windows)
    // are then handled by the per-file retry inside `apply`.
    std::thread::sleep(std::time::Duration::from_millis(1200));

    match hya_updater::apply(&args.src_dir, &args.install_dir) {
        Ok(report) => {
            log.line(&format!("replaced: {}", report.replaced.join(", ")));
            if !report.skipped.is_empty() {
                log.line(&format!("skipped:  {}", report.skipped.join(", ")));
            }
        }
        Err(e) => {
            log.line(&format!("update FAILED: {e}"));
            // Relaunch anyway: the rename-aside dance rolls back per file,
            // so whatever is in the install dir is runnable.
            relaunch(&args, &mut log);
            return std::process::ExitCode::FAILURE;
        }
    }

    relaunch(&args, &mut log);
    log.line("done");
    std::process::ExitCode::SUCCESS
}

fn relaunch(args: &Args, log: &mut Log) {
    let Some(exe) = &args.relaunch else {
        return;
    };
    match std::process::Command::new(exe)
        .args(&args.relaunch_args)
        .current_dir(&args.install_dir)
        .spawn()
    {
        Ok(_) => log.line(&format!("relaunched {}", exe.display())),
        Err(e) => log.line(&format!("relaunch failed: {e}")),
    }
}

/// Append-only log in the staging directory; the updater has no other way to
/// report (its parent is gone and it may have no console).
struct Log(Option<std::fs::File>);

impl Log {
    fn open() -> Log {
        let dir = hya_updater::staging_dir();
        let _ = std::fs::create_dir_all(&dir);
        Log(std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("update.log"))
            .ok())
    }

    fn line(&mut self, msg: &str) {
        if let Some(f) = &mut self.0 {
            let _ = writeln!(f, "[{}] {msg}", std::process::id());
        }
    }
}
