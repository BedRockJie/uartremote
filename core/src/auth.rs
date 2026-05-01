use crate::error::{CoreError, Result};

#[derive(Debug, Clone)]
pub struct TokenAuth {
    token: String,
}

impl TokenAuth {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }

    pub fn verify(&self, token: &str) -> Result<()> {
        if constant_time_eq(self.token.as_bytes(), token.as_bytes()) {
            Ok(())
        } else {
            Err(CoreError::AuthFailed)
        }
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }

    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::TokenAuth;

    #[test]
    fn accepts_matching_token() {
        assert!(TokenAuth::new("secret").verify("secret").is_ok());
    }

    #[test]
    fn rejects_wrong_token() {
        assert!(TokenAuth::new("secret").verify("wrong").is_err());
    }
}
