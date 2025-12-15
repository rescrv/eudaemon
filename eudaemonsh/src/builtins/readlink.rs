//! The readlink builtin: print the target of a symbolic link.

use getopts::Options;
use utf8path::Component;
use utf8path::Path;

use crate::{Environment, Error, ExitCode, FileType, Filesystem, Stderr, Stdin, Stdout};

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag("f", "", "Canonicalize by following symlinks recursively");
    opts.optflag("n", "", "Do not print trailing newline");
    opts
}

/// Canonicalize a path by following all symlinks (like realpath).
fn canonicalize<FS: Filesystem>(path: &str, cwd: &Path<'_>, fs: &FS) -> Result<String, String> {
    // Make the path absolute
    let abs_path = if Path::from(path).is_abs() {
        Path::from(path).into_owned()
    } else {
        cwd.join(path).into_owned()
    };

    let mut resolved: Vec<String> = Vec::new();
    canonicalize_components(&abs_path, &mut resolved, fs, path, 0)?;

    Ok(build_path(&resolved))
}

/// Build a path string from resolved components.
fn build_path(components: &[String]) -> String {
    if components.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", components.join("/"))
    }
}

/// Process path components, resolving symlinks along the way.
fn canonicalize_components<FS: Filesystem>(
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
                resolved.clear();
            }
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(name) => {
                let name_str = name.as_str().to_string();
                resolved.push(name_str);

                let current_path = build_path(resolved);

                match fs.lstat(&current_path) {
                    Ok(entry) if entry.file_type == FileType::Symlink => {
                        let target = fs
                            .readlink(&current_path)
                            .map_err(|_| format!("{}: No such file or directory", original_path))?;

                        resolved.pop();

                        let target_path = if target.starts_with('/') {
                            Path::from(target)
                        } else {
                            let parent = Path::from(build_path(resolved));
                            parent.join(&target).into_owned()
                        };

                        canonicalize_components(
                            &target_path,
                            resolved,
                            fs,
                            original_path,
                            depth + 1,
                        )?;
                    }
                    Ok(_) => {}
                    Err(_) => {
                        return Err(format!("{}: No such file or directory", original_path));
                    }
                }
            }
        }
    }

    Ok(())
}

/// The readlink builtin: print the target of a symbolic link.
///
/// Usage:
///   readlink [-fn] file ...
///
/// Prints the value of the symbolic link. If the `-f` option is specified,
/// the output is canonicalized by following every symlink in every component
/// of the given path recursively. If `-n` is specified, the trailing newline
/// is not printed.
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
            env.stderr.write_line(&format!("readlink: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let fflag = matches.opt_present("f");
    let nflag = matches.opt_present("n");

    if matches.free.is_empty() {
        env.stderr.write_line("readlink: missing operand")?;
        return Ok(ExitCode::from(1));
    }

    let mut rval = 0i8;

    for (i, path) in matches.free.iter().enumerate() {
        let result = if fflag {
            canonicalize(path, &env.cwd, &env.fs)
        } else {
            // For non-canonicalize mode, just read the immediate symlink target
            let abs_path = if Path::from(path.as_str()).is_abs() {
                path.clone()
            } else {
                env.cwd.join(path).to_string()
            };

            // Check if it's actually a symlink
            match env.fs.lstat(&abs_path) {
                Ok(entry) if entry.file_type == FileType::Symlink => env
                    .fs
                    .readlink(&abs_path)
                    .map_err(|_| format!("{}: No such file or directory", path)),
                Ok(_) => Err(format!("{}: Not a symbolic link", path)),
                Err(_) => Err(format!("{}: No such file or directory", path)),
            }
        };

        match result {
            Ok(target) => {
                // Print with or without newline based on -n flag
                // For multiple files with -n, we still need some separator
                if nflag {
                    if i > 0 {
                        env.stdout.write_str(" ")?;
                    }
                    env.stdout.write_str(&target)?;
                } else {
                    env.stdout.write_line(&target)?;
                }
            }
            Err(msg) => {
                env.stderr.write_line(&format!("readlink: {}", msg))?;
                rval = 1;
            }
        }
    }

    Ok(ExitCode::from(rval))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestFilesystemExt;
    use crate::test_utils::{TestEnvBuilder, TestFilesystem};
    use crate::{StringStderr, StringStdin, StringStdout};

    fn make_env(
        args: Vec<&str>,
    ) -> Environment<StringStdin, StringStdout, StringStderr, TestFilesystem> {
        let env = TestEnvBuilder::new().args(args).cwd("/home/user").build();
        env.fs.add_directory("/home");
        env.fs.add_directory("/home/user");
        env
    }

    #[test]
    fn simple_symlink() {
        let env = make_env(vec!["readlink", "/link"]);
        env.fs.add_directory("/target");
        env.fs.symlink("/target", "/link").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/target\n", env.stdout.into_string());
    }

    #[test]
    fn relative_symlink_target() {
        let env = make_env(vec!["readlink", "/home/link"]);
        // Note: make_env() already creates /home and /home/user
        env.fs.symlink("user", "/home/link").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // readlink without -f just prints the raw target
        assert_eq!("user\n", env.stdout.into_string());
    }

    #[test]
    fn not_a_symlink() {
        let env = make_env(vec!["readlink", "/regular"]);
        env.fs.add_file("/regular", "content");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Not a symbolic link"));
    }

    #[test]
    fn directory_not_a_symlink() {
        let env = make_env(vec!["readlink", "/dir"]);
        env.fs.add_directory("/dir");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Not a symbolic link"));
    }

    #[test]
    fn nonexistent_path() {
        let env = make_env(vec!["readlink", "/nonexistent"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("No such file or directory"));
    }

    #[test]
    fn no_trailing_newline() {
        let env = make_env(vec!["readlink", "-n", "/link"]);
        env.fs.add_directory("/target");
        env.fs.symlink("/target", "/link").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/target", env.stdout.into_string());
    }

    #[test]
    fn canonicalize_symlink() {
        let env = make_env(vec!["readlink", "-f", "/link"]);
        env.fs.add_directory("/actual");
        env.fs.symlink("/actual", "/link").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/actual\n", env.stdout.into_string());
    }

    #[test]
    fn canonicalize_chain() {
        let env = make_env(vec!["readlink", "-f", "/link1"]);
        env.fs.add_directory("/actual");
        env.fs.symlink("/link2", "/link1").unwrap();
        env.fs.symlink("/actual", "/link2").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/actual\n", env.stdout.into_string());
    }

    #[test]
    fn canonicalize_relative_symlink() {
        let env = make_env(vec!["readlink", "-f", "/home/link"]);
        // Note: make_env() already creates /home and /home/user
        env.fs.symlink("user", "/home/link").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/home/user\n", env.stdout.into_string());
    }

    #[test]
    fn canonicalize_regular_file() {
        let env = make_env(vec!["readlink", "-f", "/regular"]);
        env.fs.add_file("/regular", "content");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // -f works on regular files too, returns canonical path
        assert_eq!("/regular\n", env.stdout.into_string());
    }

    #[test]
    fn canonicalize_with_dotdot() {
        let env = make_env(vec!["readlink", "-f", "/usr/bin/../lib"]);
        env.fs.add_directory("/usr");
        env.fs.add_directory("/usr/bin");
        env.fs.add_directory("/usr/lib");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/usr/lib\n", env.stdout.into_string());
    }

    #[test]
    fn multiple_files() {
        let env = make_env(vec!["readlink", "/link1", "/link2"]);
        env.fs.add_directory("/target1");
        env.fs.add_directory("/target2");
        env.fs.symlink("/target1", "/link1").unwrap();
        env.fs.symlink("/target2", "/link2").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/target1\n/target2\n", env.stdout.into_string());
    }

    #[test]
    fn multiple_files_one_fails() {
        let env = make_env(vec!["readlink", "/link", "/regular"]);
        env.fs.add_directory("/target");
        env.fs.symlink("/target", "/link").unwrap();
        env.fs.add_file("/regular", "content");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        assert_eq!("/target\n", env.stdout.into_string());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Not a symbolic link"));
    }

    #[test]
    fn missing_operand() {
        let env = make_env(vec!["readlink"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("missing operand"));
    }

    #[test]
    fn canonicalize_nonexistent() {
        let env = make_env(vec!["readlink", "-f", "/nonexistent"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("No such file or directory"));
    }

    #[test]
    fn relative_path_input() {
        let env = make_env(vec!["readlink", "link"]);
        // Note: make_env() already creates /home and /home/user
        env.fs.add_directory("/target");
        env.fs.symlink("/target", "/home/user/link").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/target\n", env.stdout.into_string());
    }

    #[test]
    fn canonicalize_and_no_newline() {
        let env = make_env(vec!["readlink", "-f", "-n", "/link"]);
        env.fs.add_directory("/actual");
        env.fs.symlink("/actual", "/link").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/actual", env.stdout.into_string());
    }
}
