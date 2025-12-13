use crate::{Environment, Error, ExitCode, Filesystem, Stderr, Stdin, Stdout};

/// The cat builtin: concatenate and print files or stdin.
pub fn bin<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let args = &env.args;

    // If no file arguments, read from stdin
    if args.len() <= 1 {
        cat_stdin(env)?;
    } else {
        // Process each file argument
        for arg in args.iter().skip(1) {
            if arg == "-" {
                cat_stdin(env)?;
            } else {
                cat_file(env, arg)?;
            }
        }
    }
    Ok(ExitCode::from(0))
}

fn cat_stdin<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    while let Some(line) = env.stdin.read_line()? {
        env.stdout.write_line(&line)?;
    }
    Ok(())
}

fn cat_file<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>, path: &str) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    match env.fs.read_to_string(path) {
        Ok(contents) => {
            env.stdout.write_str(&contents)?;
            // Add trailing newline if file doesn't end with one
            if !contents.ends_with('\n') && !contents.is_empty() {
                env.stdout.write_str("\n")?;
            }
        }
        Err(Error::Io(e)) => {
            env.stderr.write_line(&format!("cat: {}: {}", path, e))?;
        }
        Err(e) => return Err(e),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MockFilesystem, StringStderr, StringStdin, StringStdout};

    fn make_env(
        args: Vec<&str>,
        stdin: &str,
    ) -> Environment<StringStdin, StringStdout, StringStderr, MockFilesystem> {
        Environment {
            stdin: StringStdin::new(stdin),
            stdout: StringStdout::new(),
            stderr: StringStderr::new(),
            fs: MockFilesystem::new(),
            env: std::collections::HashMap::new(),
            args: args.into_iter().map(|s| s.to_string()).collect(),
            cwd: utf8path::Path::from("/"),
        }
    }

    #[test]
    fn empty_stdin() {
        let env = make_env(vec!["cat"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn single_line() {
        let env = make_env(vec!["cat"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\n", env.stdout.into_string());
    }

    #[test]
    fn multiple_lines() {
        let env = make_env(vec!["cat"], "line1\nline2\nline3");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("line1\nline2\nline3\n", env.stdout.into_string());
    }

    #[test]
    fn file_not_found() {
        let env = make_env(vec!["cat", "nonexistent.txt"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.stderr.into_string().contains("nonexistent.txt"));
    }

    #[test]
    fn single_file() {
        let env = make_env(vec!["cat", "file.txt"], "");
        env.fs.add_file("file.txt", "hello world");
        let result = bin(&env).unwrap();
        // Print stdout/stderr for debugging
        println!("stdout: {:?}", env.stdout.into_string());
        println!("stderr: {:?}", env.stderr.into_string());
        assert_eq!(0, result.code());
        assert_eq!("hello world\n", env.stdout.into_string());
    }

    #[test]
    fn multiple_files() {
        let env = make_env(vec!["cat", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "aaa\n");
        env.fs.add_file("b.txt", "bbb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("aaa\nbbb\n", env.stdout.into_string());
    }

    #[test]
    fn dash_means_stdin() {
        let env = make_env(vec!["cat", "-"], "from stdin");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("from stdin\n", env.stdout.into_string());
    }

    #[test]
    fn mixed_files_and_stdin() {
        let env = make_env(vec!["cat", "a.txt", "-", "b.txt"], "middle");
        env.fs.add_file("a.txt", "first\n");
        env.fs.add_file("b.txt", "last\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("first\nmiddle\nlast\n", env.stdout.into_string());
    }
}
