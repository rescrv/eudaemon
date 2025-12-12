use crate::{Environment, Error, ExitCode, Stderr, Stdin, Stdout};

mod cat;

#[allow(clippy::type_complexity)]
pub fn lookup_bin<SI, SO, SE>(
    bin: &str,
) -> Result<fn(&Environment<SI, SO, SE>) -> Result<ExitCode, Error>, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
{
    match bin {
        "cat" => Ok(cat::bin),
        _ => todo!("TODO(claude):  Return error"),
    }
}
