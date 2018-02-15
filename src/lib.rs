use std::error::Error;

pub fn run() -> Result<(), Box<Error>> {
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
