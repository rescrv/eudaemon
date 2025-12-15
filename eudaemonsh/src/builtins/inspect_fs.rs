//! The inspect-fs builtin: inspect LFS filesystem images using S-expressions.
//!
//! This builtin launches an interactive S-expression REPL that allows
//! inspection and manipulation of LFS filesystem images.

use std::io::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use getopts::Options;

use eudaemonfs::lisp::{DebugImgRepl, DebugImgReplConfig, LfsLisp, parse_size};
use eudaemonfs::{DeviceId, FileBlockDevice, Lfs};

use crate::{Environment, Error, ExitCode, Filesystem, Stderr, Stdin, Stdout};

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag("h", "help", "Print help information");
    opts.optflag("n", "new", "Create a new filesystem image");
    opts.optopt(
        "s",
        "size",
        "Size of new filesystem (e.g., 1M, 64K)",
        "SIZE",
    );
    opts.optflag("m", "memory", "Use in-memory filesystem (no persistence)");
    opts.optflag("r", "read-only", "Read-only mode");
    opts
}

fn print_usage<W: Stderr>(out: &W) -> Result<(), Error> {
    out.write_line("Usage: inspect-fs [OPTIONS] [IMAGE-FILE]")?;
    out.write_line("       inspect-fs --memory [--size SIZE]")?;
    out.write_line("")?;
    out.write_line("Inspect and manipulate LFS filesystem images interactively.")?;
    out.write_line("")?;
    out.write_line("Options:")?;
    out.write_line("  -h, --help        Print help information")?;
    out.write_line("  -n, --new         Create a new filesystem image")?;
    out.write_line("  -s, --size SIZE   Size of new filesystem (e.g., 1M, 64K)")?;
    out.write_line("  -m, --memory      Use in-memory filesystem (no persistence)")?;
    out.write_line("  -r, --read-only   Read-only mode")?;
    out.write_line("")?;
    out.write_line("REPL Commands:")?;
    out.write_line("  :help, :h, :?     Show help")?;
    out.write_line("  :quit, :q, :exit  Exit the REPL")?;
    out.write_line("  :tree             Show filesystem tree")?;
    out.write_line("  :ls [path]        List directory (default: /)")?;
    out.write_line("  :cat path         Show file contents")?;
    out.write_line("  :stat path        Show file metadata")?;
    out.write_line("  :usage            Show filesystem usage")?;
    out.write_line("  :sync             Sync changes to disk")?;
    out.write_line("  :fns              List available Lisp functions")?;
    out.write_line("")?;
    out.write_line("Lisp Functions:")?;
    out.write_line("  (lfs-read path)           Read file contents")?;
    out.write_line("  (lfs-write path content)  Write to file")?;
    out.write_line("  (lfs-mkdir path)          Create directory")?;
    out.write_line("  (lfs-remove path)         Remove file")?;
    out.write_line("  (lfs-tree)                Get filesystem tree")?;
    out.write_line("  (lfs-exists? path)        Check if path exists")?;
    out.write_line("  ... and many more (type :fns in REPL)")?;
    Ok(())
}

/// Returns current time in milliseconds since UNIX epoch.
fn current_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The inspect-fs builtin: inspect LFS filesystem images using S-expressions.
pub fn bin<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let opts_def = build_options();

    let matches = match opts_def.parse(&env.args[1..]) {
        Ok(m) => m,
        Err(e) => {
            env.stderr.write_line(&format!("inspect-fs: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    if matches.opt_present("h") {
        print_usage(&env.stderr)?;
        return Ok(ExitCode::from(0));
    }

    let is_new = matches.opt_present("n");
    let is_memory = matches.opt_present("m");
    let is_read_only = matches.opt_present("r");
    let size_str = matches.opt_str("s");

    // Validate arguments
    if matches.free.is_empty() && !is_memory {
        env.stderr
            .write_line("inspect-fs: missing image file argument")?;
        env.stderr
            .write_line("Use --memory for in-memory filesystem or provide a file path")?;
        return Ok(ExitCode::from(1));
    }

    if is_memory {
        // In-memory filesystem
        let size = match size_str {
            Some(s) => match parse_size(&s) {
                Ok(size) => size,
                Err(e) => {
                    env.stderr.write_line(&format!("inspect-fs: {}", e))?;
                    return Ok(ExitCode::from(1));
                }
            },
            None => 1024 * 1024, // Default 1MB
        };

        let data = vec![0u8; size];
        let lfs = match LfsLisp::from_vec(data, DeviceId::new(1), current_time_ms) {
            Ok(lfs) => lfs,
            Err(e) => {
                env.stderr
                    .write_line(&format!("inspect-fs: error creating filesystem: {:?}", e))?;
                return Ok(ExitCode::from(1));
            }
        };

        let config = DebugImgReplConfig {
            read_only: is_read_only,
            sync_fn: None,
            banner: Some(format!("Created in-memory filesystem ({} bytes)", size)),
        };

        let repl = DebugImgRepl::new(lfs, config);
        repl.run();
    } else {
        // File-backed filesystem
        let path = &matches.free[0];

        if is_new {
            // Create new filesystem
            let size = match size_str {
                Some(s) => match parse_size(&s) {
                    Ok(size) => size,
                    Err(e) => {
                        env.stderr.write_line(&format!("inspect-fs: {}", e))?;
                        return Ok(ExitCode::from(1));
                    }
                },
                None => 1024 * 1024, // Default 1MB
            };

            let total_blocks = (size / 4096) as u64;
            if total_blocks < 16 {
                env.stderr
                    .write_line("inspect-fs: filesystem too small (minimum 64KB)")?;
                return Ok(ExitCode::from(1));
            }

            let device = match FileBlockDevice::create(path, total_blocks) {
                Ok(dev) => dev,
                Err(e) => {
                    env.stderr
                        .write_line(&format!("inspect-fs: error creating {}: {}", path, e))?;
                    return Ok(ExitCode::from(1));
                }
            };

            let lfs = match Lfs::new(device, total_blocks, DeviceId::new(1), current_time_ms) {
                Ok(lfs) => lfs,
                Err(e) => {
                    env.stderr
                        .write_line(&format!("inspect-fs: error creating filesystem: {:?}", e))?;
                    return Ok(ExitCode::from(1));
                }
            };

            let lfs_lisp = LfsLisp::new(lfs);

            let sync_fn: Box<dyn Fn()> = Box::new(|| {
                let _ = std::io::stdout().flush();
            });

            let config = DebugImgReplConfig {
                read_only: is_read_only,
                sync_fn: Some(sync_fn),
                banner: Some(format!(
                    "Created new filesystem: {} ({} blocks)",
                    path, total_blocks
                )),
            };

            let repl = DebugImgRepl::new(lfs_lisp, config);
            repl.run();
        } else {
            // Open existing filesystem
            let device = match FileBlockDevice::open(path) {
                Ok(dev) => dev,
                Err(e) => {
                    env.stderr
                        .write_line(&format!("inspect-fs: error opening {}: {}", path, e))?;
                    return Ok(ExitCode::from(1));
                }
            };

            let total_blocks = device.total_blocks();
            let lfs = match Lfs::open(device, total_blocks, DeviceId::new(1), current_time_ms) {
                Ok(lfs) => lfs,
                Err(e) => {
                    env.stderr
                        .write_line(&format!("inspect-fs: error opening filesystem: {:?}", e))?;
                    return Ok(ExitCode::from(1));
                }
            };

            let lfs_lisp = LfsLisp::new(lfs);

            let sync_fn: Box<dyn Fn()> = Box::new(|| {
                let _ = std::io::stdout().flush();
            });

            let config = DebugImgReplConfig {
                read_only: is_read_only,
                sync_fn: Some(sync_fn),
                banner: Some(format!(
                    "Opened filesystem: {} ({} blocks)",
                    path, total_blocks
                )),
            };

            let repl = DebugImgRepl::new(lfs_lisp, config);
            repl.run();
        }
    }

    Ok(ExitCode::from(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::make_test_env;

    #[test]
    fn help_flag() {
        let env = make_test_env(vec!["inspect-fs", "-h"]);
        env.fs.add_directory("/");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Usage:"));
        assert!(stderr.contains("lfs-read"));
    }

    #[test]
    fn missing_file_argument() {
        let env = make_test_env(vec!["inspect-fs"]);
        env.fs.add_directory("/");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("missing image file"));
    }
}
