extern crate git2;

use std::error::Error;

pub fn run() -> Result<(), Box<Error>> {
    let repo = git2::Repository::open_from_env()?;
    println!("{:?}", repo.path());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run() {
        assert!(run().is_ok());
    }
}
