extern crate git2;

use std::error::Error;

pub struct Config {
    pub dry_run: bool,
    pub force: bool,
    pub verbose: bool,
}

pub fn run(config: &Config) -> Result<(), Box<Error>> {
    let repo = git2::Repository::open_from_env()?;
    println!("{:?}", repo.path());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run() {
        assert!(
            run(&Config {
                dry_run: false,
                force: false,
                verbose: false,
            }).is_ok()
        );
    }
}
