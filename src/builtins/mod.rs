use crate::{Environment, Error, ExitCode, Filesystem, Stderr, Stdin, Stdout};

mod cat;
pub mod sh;

/// Look up a builtin binary by name.
#[allow(clippy::type_complexity)]
pub fn lookup_bin<SI, SO, SE, FS>(
    bin: &str,
) -> Result<fn(&Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    match bin {
        "cat" | "/bin/cat" => Ok(cat::bin),
        "echo" | "/bin/echo" => Ok(echo::bin),
        "sh" | "/bin/sh" => Ok(sh::bin),
        _ => Err(Error::UnknownBinary(bin.to_string())),
    }
}
