use crate::{Environment, Error, ExitCode, Stderr, Stdin, Stdout};

pub fn bin<SI, SO, SE>(env: &Environment<SI, SO, SE>) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
{
    Ok(ExitCode::from(-13))
}
