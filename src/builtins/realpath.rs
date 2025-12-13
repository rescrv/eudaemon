//! The realpath builtin: return resolved physical path.

use getopts::Options;
use utf8path::Component;
use utf8path::Path;

use crate::{Environment, Error, ExitCode, FileType, Filesystem, Stderr, Stdin, Stdout};

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag("q", "", "Suppress warnings when realpath fails");
    opts
}

/// Build a path string from resolved components.
fn build_path(components: &[String]) -> String {
    if components.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", components.join("/"))
    }
}

/// Process path components from a utf8path, resolving symlinks along the way.
/// This is recursive to handle symlinks that themselves contain symlinks.
fn resolve_components<FS: Filesystem>(
    input_path: &Path<'_>,
    resolved: &mut Vec<String>,
    fs: &FS,
    original_path: &str,
    depth: usize,
) -> Result<(), String> {
    if depth > 40 {
        return Err(format!(
            "{}: Too many levels of symbolic links",
            original_path
        ));
    }

    for component in input_path.components() {
        match component {
            Component::RootDir | Component::AppDefined => {
                // Absolute path - clear what we have and start fresh
                resolved.clear();
            }
            Component::CurDir => {
                // Skip "." components
            }
            Component::ParentDir => {
                // Go up one directory, but don't go above root
                resolved.pop();
            }
            Component::Normal(name) => {
                let name_str = name.as_str().to_string();
                resolved.push(name_str);

                // Build current path and check for symlinks
                let current_path = build_path(resolved);

                match fs.lstat(&current_path) {
                    Ok(entry) if entry.file_type == FileType::Symlink => {
                        // Read the symlink target
                        let target = fs
                            .readlink(&current_path)
                            .map_err(|_| format!("{}: No such file or directory", original_path))?;

                        // Remove the symlink component we just added
                        resolved.pop();

                        // Recursively resolve the symlink target
                        let target_path = if target.starts_with('/') {
                            Path::from(target)
                        } else {
                            // Relative symlink - join with parent directory
                            let parent = Path::from(build_path(resolved));
                            parent.join(&target).into_owned()
                        };

                        resolve_components(&target_path, resolved, fs, original_path, depth + 1)?;
                    }
                    Ok(_) => {
                        // Regular file or directory - keep the component
                    }
                    Err(_) => {
                        return Err(format!("{}: No such file or directory", original_path));
                    }
                }
            }
        }
    }

    Ok(())
}

/// Resolve a path to its canonical absolute form.
///
/// This function:
/// 1. Makes the path absolute (relative to cwd if needed)
/// 2. Resolves `.` and `..` components
/// 3. Follows symbolic links
fn resolve_path<FS: Filesystem>(path: &str, cwd: &Path<'_>, fs: &FS) -> Result<String, String> {
    // Make the path absolute
    let abs_path = if Path::from(path).is_abs() {
        Path::from(path).into_owned()
    } else {
        cwd.join(path).into_owned()
    };

    let mut resolved: Vec<String> = Vec::new();
    resolve_components(&abs_path, &mut resolved, fs, path, 0)?;

    Ok(build_path(&resolved))
}

/// The realpath builtin: return resolved physical path.
///
/// Usage:
///   realpath [-q] [path ...]
///
/// Resolves all symbolic links, extra `/` characters and references to
/// `/./` and `/../` in path. If path is absent, the current working
/// directory (`.`) is assumed.
///
/// If `-q` is specified, warnings will not be printed when resolution fails.
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
            env.stderr.write_line(&format!("realpath: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let qflag = matches.opt_present("q");

    // If no paths provided, default to "."
    let paths: Vec<&str> = if matches.free.is_empty() {
        vec!["."]
    } else {
        matches.free.iter().map(|s| s.as_str()).collect()
    };

    let mut rval = 0i8;

    for path in paths {
        match resolve_path(path, &env.cwd, &env.fs) {
            Ok(resolved) => {
                env.stdout.write_line(&resolved)?;
            }
            Err(msg) => {
                if !qflag {
                    env.stderr.write_line(&format!("realpath: {}", msg))?;
                }
                rval = 1;
            }
        }
    }

    Ok(ExitCode::from(rval))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MockFilesystem, StringStderr, StringStdin, StringStdout};

    fn make_env(
        args: Vec<&str>,
    ) -> Environment<StringStdin, StringStdout, StringStderr, MockFilesystem> {
        Environment {
            stdin: StringStdin::new(""),
            stdout: StringStdout::new(),
            stderr: StringStderr::new(),
            fs: MockFilesystem::new(),
            env: std::collections::HashMap::new(),
            args: args.into_iter().map(|s| s.to_string()).collect(),
            cwd: Path::from("/home/user"),
            exit_signaled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    fn make_env_with_cwd(
        args: Vec<&str>,
        cwd: &str,
    ) -> Environment<StringStdin, StringStdout, StringStderr, MockFilesystem> {
        Environment {
            stdin: StringStdin::new(""),
            stdout: StringStdout::new(),
            stderr: StringStderr::new(),
            fs: MockFilesystem::new(),
            env: std::collections::HashMap::new(),
            args: args.into_iter().map(|s| s.to_string()).collect(),
            cwd: Path::from(cwd).into_owned(),
            exit_signaled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    #[test]
    fn absolute_path_file() {
        let env = make_env(vec!["realpath", "/usr/bin/ls"]);
        env.fs.add_directory("/usr");
        env.fs.add_directory("/usr/bin");
        env.fs.add_file("/usr/bin/ls", "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/usr/bin/ls\n", env.stdout.into_string());
    }

    #[test]
    fn absolute_path_directory() {
        let env = make_env(vec!["realpath", "/usr/bin"]);
        env.fs.add_directory("/usr");
        env.fs.add_directory("/usr/bin");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/usr/bin\n", env.stdout.into_string());
    }

    #[test]
    fn relative_path() {
        let env = make_env_with_cwd(vec!["realpath", "foo/bar"], "/home/user");
        env.fs.add_directory("/home");
        env.fs.add_directory("/home/user");
        env.fs.add_directory("/home/user/foo");
        env.fs.add_file("/home/user/foo/bar", "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/home/user/foo/bar\n", env.stdout.into_string());
    }

    #[test]
    fn dot_component() {
        let env = make_env(vec!["realpath", "/usr/./bin"]);
        env.fs.add_directory("/usr");
        env.fs.add_directory("/usr/bin");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/usr/bin\n", env.stdout.into_string());
    }

    #[test]
    fn dotdot_component() {
        let env = make_env(vec!["realpath", "/usr/bin/../lib"]);
        env.fs.add_directory("/usr");
        env.fs.add_directory("/usr/bin");
        env.fs.add_directory("/usr/lib");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/usr/lib\n", env.stdout.into_string());
    }

    #[test]
    fn dotdot_at_root() {
        let env = make_env(vec!["realpath", "/../.."]);
        env.fs.add_directory("/");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/\n", env.stdout.into_string());
    }

    #[test]
    fn no_args_defaults_to_cwd() {
        let env = make_env_with_cwd(vec!["realpath"], "/home/user");
        env.fs.add_directory("/home");
        env.fs.add_directory("/home/user");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/home/user\n", env.stdout.into_string());
    }

    #[test]
    fn nonexistent_path_error() {
        let env = make_env(vec!["realpath", "/nonexistent"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("No such file or directory"));
    }

    #[test]
    fn nonexistent_path_quiet() {
        let env = make_env(vec!["realpath", "-q", "/nonexistent"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        assert_eq!("", env.stderr.into_string());
    }

    #[test]
    fn multiple_paths() {
        let env = make_env(vec!["realpath", "/usr", "/bin"]);
        env.fs.add_directory("/usr");
        env.fs.add_directory("/bin");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/usr\n/bin\n", env.stdout.into_string());
    }

    #[test]
    fn multiple_paths_one_fails() {
        let env = make_env(vec!["realpath", "/usr", "/nonexistent", "/bin"]);
        env.fs.add_directory("/usr");
        env.fs.add_directory("/bin");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        assert_eq!("/usr\n/bin\n", env.stdout.into_string());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("nonexistent"));
    }

    #[test]
    fn root_path() {
        let env = make_env(vec!["realpath", "/"]);
        env.fs.add_directory("/");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/\n", env.stdout.into_string());
    }

    #[test]
    fn symlink_resolution() {
        let env = make_env(vec!["realpath", "/link"]);
        env.fs.add_directory("/usr");
        env.fs.add_directory("/usr/bin");
        env.fs.symlink("/usr/bin", "/link").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/usr/bin\n", env.stdout.into_string());
    }

    #[test]
    fn symlink_chain() {
        let env = make_env(vec!["realpath", "/link1"]);
        env.fs.add_directory("/actual");
        env.fs.symlink("/link2", "/link1").unwrap();
        env.fs.symlink("/actual", "/link2").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/actual\n", env.stdout.into_string());
    }

    #[test]
    fn relative_symlink() {
        let env = make_env(vec!["realpath", "/home/link"]);
        env.fs.add_directory("/home");
        env.fs.add_directory("/home/user");
        env.fs.symlink("user", "/home/link").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/home/user\n", env.stdout.into_string());
    }

    #[test]
    fn trailing_slashes_normalized() {
        let env = make_env(vec!["realpath", "/usr/bin///"]);
        env.fs.add_directory("/usr");
        env.fs.add_directory("/usr/bin");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/usr/bin\n", env.stdout.into_string());
    }

    #[test]
    fn complex_path_normalization() {
        let env = make_env(vec!["realpath", "/usr/./bin/../lib/./gcc/../.."]);
        env.fs.add_directory("/usr");
        env.fs.add_directory("/usr/bin");
        env.fs.add_directory("/usr/lib");
        env.fs.add_directory("/usr/lib/gcc");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/usr\n", env.stdout.into_string());
    }
}
