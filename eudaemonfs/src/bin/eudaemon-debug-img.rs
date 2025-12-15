//! Debug REPL for interacting with LFS filesystem images.
//!
//! This tool provides an interactive Lisp environment for exploring and manipulating
//! log-structured filesystem images. It supports both creating new filesystems and
//! opening existing ones.
//!
//! # Usage
//!
//! Create a new filesystem image:
//! ```text
//! eudaemon-debug-img --new --size 1M image.lfs
//! ```
//!
//! Open an existing filesystem image:
//! ```text
//! eudaemon-debug-img image.lfs
//! ```
//!
//! # Available Commands
//!
//! - `:help` - Show available commands
//! - `:quit` - Exit the REPL
//! - `:tree` - Show filesystem tree
//! - `:ls [path]` - List directory contents
//! - `:cat path` - Show file contents
//! - `:stat path` - Show file metadata
//! - `:usage` - Show filesystem usage statistics
//! - `:sync` - Sync filesystem to disk
//!
//! # Lisp Functions
//!
//! All `lfs-*` functions from the lisp module are available:
//! - `(lfs-read path)` - Read file contents
//! - `(lfs-write path content)` - Write to a file
//! - `(lfs-mkdir path)` - Create a directory
//! - `(lfs-remove path)` - Remove a file
//! - `(lfs-tree)` - Get filesystem tree as S-expression
//! - And many more...

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use arrrg::CommandLine;
use arrrg_derive::CommandLine;

use eudaemonfs::lisp::{DebugImgRepl, DebugImgReplConfig, LfsLisp, parse_size};
use eudaemonfs::{DeviceId, FileBlockDevice, Lfs};

/// Command line arguments for eudaemon-debug-img.
#[derive(CommandLine, Debug, Default, Eq, PartialEq)]
struct Args {
    /// Create a new filesystem image instead of opening an existing one.
    #[arrrg(flag, "Create a new filesystem image")]
    new: bool,

    /// Size of the new filesystem (e.g., 1M, 64K, 1G). Only used with --new.
    #[arrrg(optional, "Size of new filesystem (e.g., 1M, 64K)")]
    size: Option<String>,

    /// Use an in-memory filesystem instead of a file.
    #[arrrg(flag, "Use in-memory filesystem (no persistence)")]
    memory: bool,

    /// Read-only mode (don't sync changes to disk).
    #[arrrg(flag, "Read-only mode")]
    read_only: bool,
}

/// Returns current time in milliseconds since UNIX epoch.
fn current_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn main() {
    let (args, free) =
        Args::from_command_line_relaxed("eudaemon-debug-img: debug LFS filesystem images");

    if free.is_empty() && !args.memory {
        eprintln!("Usage: eudaemon-debug-img [OPTIONS] <image-file>");
        eprintln!("       eudaemon-debug-img --memory [--size SIZE]");
        eprintln!();
        eprintln!("Options:");
        eprintln!("  --new           Create a new filesystem image");
        eprintln!("  --size SIZE     Size of new filesystem (e.g., 1M, 64K)");
        eprintln!("  --memory        Use in-memory filesystem (no persistence)");
        eprintln!("  --read-only     Read-only mode");
        std::process::exit(1);
    }

    if args.memory {
        // In-memory filesystem
        let size = args
            .size
            .as_deref()
            .map(|s| {
                parse_size(s).unwrap_or_else(|e| {
                    eprintln!("Error parsing size: {}", e);
                    std::process::exit(1);
                })
            })
            .unwrap_or(1024 * 1024); // Default 1MB

        let data = vec![0u8; size];
        let lfs = match LfsLisp::from_vec(data, DeviceId::new(1), current_time_ms) {
            Ok(lfs) => lfs,
            Err(e) => {
                eprintln!("Error creating filesystem: {:?}", e);
                std::process::exit(1);
            }
        };

        let config = DebugImgReplConfig {
            read_only: args.read_only,
            sync_fn: None,
            banner: Some(format!("Created in-memory filesystem ({} bytes)", size)),
        };

        let repl = DebugImgRepl::new(lfs, config);
        repl.run();
    } else {
        // File-backed filesystem
        let path = &free[0];

        if args.new {
            // Create new filesystem
            let size = args
                .size
                .as_deref()
                .map(|s| {
                    parse_size(s).unwrap_or_else(|e| {
                        eprintln!("Error parsing size: {}", e);
                        std::process::exit(1);
                    })
                })
                .unwrap_or(1024 * 1024); // Default 1MB

            let total_blocks = (size / 4096) as u64;
            if total_blocks < 16 {
                eprintln!("Error: Filesystem too small (minimum 64KB)");
                std::process::exit(1);
            }

            let device = match FileBlockDevice::create(path, total_blocks) {
                Ok(dev) => dev,
                Err(e) => {
                    eprintln!("Error creating file {}: {}", path, e);
                    std::process::exit(1);
                }
            };

            let lfs = match Lfs::new(device, total_blocks, DeviceId::new(1), current_time_ms) {
                Ok(lfs) => lfs,
                Err(e) => {
                    eprintln!("Error creating filesystem: {:?}", e);
                    std::process::exit(1);
                }
            };

            let lfs_lisp = LfsLisp::new(lfs);

            // Sync function for file-backed filesystem
            let sync_fn: Box<dyn Fn()> = Box::new(|| {
                // The filesystem is synced through the FileBlockDevice
                let _ = std::io::stdout().flush();
            });

            let config = DebugImgReplConfig {
                read_only: args.read_only,
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
                    eprintln!("Error opening file {}: {}", path, e);
                    std::process::exit(1);
                }
            };

            let total_blocks = device.total_blocks();
            let lfs = match Lfs::open(device, total_blocks, DeviceId::new(1), current_time_ms) {
                Ok(lfs) => lfs,
                Err(e) => {
                    eprintln!("Error opening filesystem: {:?}", e);
                    std::process::exit(1);
                }
            };

            let lfs_lisp = LfsLisp::new(lfs);

            // Sync function
            let sync_fn: Box<dyn Fn()> = Box::new(|| {
                let _ = std::io::stdout().flush();
            });

            let config = DebugImgReplConfig {
                read_only: args.read_only,
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
}
