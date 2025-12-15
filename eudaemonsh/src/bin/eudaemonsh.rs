//! Eudaemon Shell - An interactive shell using an eudaemonfs filesystem.
//!
//! # Usage
//!
//! ```text
//! eudaemonsh /path/to/filesystem_image.bin
//! ```
//!
//! Opens or creates a filesystem image at the given path and drops into an
//! interactive shell that uses that filesystem as its root.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use eudaemonfs::DeviceId;
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use utf8path::Path;

use eudaemonsh::Environment;
use eudaemonsh::FileBackedEudaemonFilesystem;
use eudaemonsh::sh;

/// Returns the current time in milliseconds since UNIX epoch.
fn current_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!("usage: eudaemonsh /path/to/filesystem_image.bin");
        std::process::exit(1);
    }

    let image_path = &args[1];
    let fs = match FileBackedEudaemonFilesystem::open_or_create(
        Path::new(image_path),
        DeviceId::new(1),
        current_time_ms,
    ) {
        Ok(fs) => fs,
        Err(e) => {
            eprintln!("eudaemonsh: failed to open filesystem: {:?}", e);
            std::process::exit(1);
        }
    };

    let env: Environment<std::io::Stdin, std::io::Stdout, std::io::Stderr, _> = Environment {
        stdin: std::io::stdin(),
        stdout: std::io::stdout(),
        stderr: std::io::stderr(),
        fs,
        env: HashMap::from_iter([
            ("COLUMNS".to_string(), "120".to_string()),
            ("HOME".to_string(), "/home/assistant".to_string()),
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            ("PWD".to_string(), "/".to_string()),
            ("SHELL".to_string(), "eudaemonsh".to_string()),
            ("TMPDIR".to_string(), "/tmp".to_string()),
            ("USER".to_string(), "assistant".to_string()),
        ]),
        args: vec!["/bin/eudaemonsh".to_string()],
        cwd: Path::from("/"),
        exit_signaled: Arc::new(AtomicBool::new(false)),
    };

    let mut rl = match DefaultEditor::new() {
        Ok(rl) => rl,
        Err(e) => {
            eprintln!("eudaemonsh: failed to initialize editor: {}", e);
            std::process::exit(1);
        }
    };

    loop {
        if env.exit_signaled.load(Ordering::SeqCst) {
            break;
        }

        let prompt = format!("{}$ ", env.cwd.as_str());
        match rl.readline(&prompt) {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let _ = rl.add_history_entry(line);

                match sh::run(line.to_string(), &env) {
                    Ok(_exit_code) => {}
                    Err(e) => {
                        eprintln!("eudaemonsh: {:?}", e);
                    }
                }

                if let Err(e) = env.fs.sync() {
                    eprintln!("eudaemonsh: sync failed: {:?}", e);
                }
            }
            Err(ReadlineError::Interrupted) => {
                continue;
            }
            Err(ReadlineError::Eof) => {
                break;
            }
            Err(e) => {
                eprintln!("eudaemonsh: readline error: {}", e);
                break;
            }
        }
    }
}
