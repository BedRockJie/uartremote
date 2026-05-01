use crate::error::{CoreError, Result};

#[derive(Debug, Default)]
pub struct WriterLease {
    owner: Option<String>,
}

impl WriterLease {
    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    pub fn claim(&mut self, client_id: &str) -> Result<()> {
        match self.owner.as_deref() {
            None => {
                self.owner = Some(client_id.to_string());
                Ok(())
            }
            Some(owner) if owner == client_id => Ok(()),
            Some(owner) => Err(CoreError::WriterAlreadyClaimed(owner.to_string())),
        }
    }

    pub fn release(&mut self, client_id: &str) -> Result<bool> {
        match self.owner.as_deref() {
            Some(owner) if owner == client_id => {
                self.owner = None;
                Ok(true)
            }
            None => Ok(false),
            Some(_) => Err(CoreError::WriterNotOwned(client_id.to_string())),
        }
    }

    pub fn release_if_owner(&mut self, client_id: &str) -> bool {
        if self.owner.as_deref() == Some(client_id) {
            self.owner = None;
            true
        } else {
            false
        }
    }

    pub fn can_write(&self, client_id: &str) -> bool {
        self.owner.as_deref() == Some(client_id)
    }
}

#[cfg(test)]
mod tests {
    use super::WriterLease;

    #[test]
    fn only_one_client_can_claim_writer() {
        let mut lease = WriterLease::default();

        assert!(lease.claim("a").is_ok());
        assert!(lease.claim("b").is_err());
        assert!(lease.can_write("a"));
        assert!(!lease.can_write("b"));
    }

    #[test]
    fn owner_disconnect_releases_writer() {
        let mut lease = WriterLease::default();

        lease.claim("a").unwrap();
        assert!(lease.release_if_owner("a"));
        assert_eq!(lease.owner(), None);
        assert!(lease.claim("b").is_ok());
    }
}
