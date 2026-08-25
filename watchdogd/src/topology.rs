//! Production startup topology verifier for the sealed H-04 watchdog store.

use crate::error::TopologyError;
use crate::interfaces::TopologyProbe;
use std::fs;
use std::path::Path;

/// Verifies the watchdog dedicated store path before arming or authority work.
#[derive(Debug, Default)]
pub struct ProductionTopologyProbe;

impl ProductionTopologyProbe {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl TopologyProbe for ProductionTopologyProbe {
    fn verify_startup(&self, store_root: &Path) -> Result<(), TopologyError> {
        if !store_root.exists() {
            return Err(TopologyError::MissingMount);
        }
        let metadata = fs::metadata(store_root).map_err(|_| TopologyError::UnavailableStore)?;
        if metadata.file_type().is_symlink() {
            return Err(TopologyError::SymlinkComponent);
        }
        if !metadata.is_dir() {
            return Err(TopologyError::OrdinaryDirectoryFallback);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = metadata.permissions().mode() & 0o777;
            if mode != 0o700 {
                return Err(TopologyError::WrongOwnershipOrMode);
            }
        }
        Ok(())
    }
}
