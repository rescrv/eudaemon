use crate::{Environment, Error, Stderr, Stdin, Stdout};

pub fn bin<SI, SO, SE>(env: &Environment<SI, SO, SE>) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
{
    Ok(())
}
