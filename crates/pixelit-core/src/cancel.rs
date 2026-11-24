//! Cancellation shared by every long operation.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::error::{Error, Result};

/// A cheap, clonable flag. Long loops call [`CancelToken::check`] between rows
/// or files and stop with the `cancelled` error once the flag is raised.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::cancelled())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_the_flag() {
        let token = CancelToken::new();
        let other = token.clone();
        assert!(token.check().is_ok());
        other.cancel();
        assert!(token.is_cancelled());
        assert!(token.check().unwrap_err().is_cancelled());
    }
}
