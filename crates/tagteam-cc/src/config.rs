use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tagteam_provider::atomic::{remove_target, write_atomic_with};
use tagteam_provider::splice::{self, render_nested};
use tagteam_provider::{Identity, LiveLocks, ProviderError, Read, ReadError, Undo};

use crate::live::Fence;

use crate::paths::CcPaths;
use crate::provider::CONFIG_REMEDY;
use crate::shape::identity_from_oauth_account;

pub fn read_bytes(path: &Path) -> Read<Vec<u8>> {
    match fs::read(path) {
        Ok(b) => Read::Present(b),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Read::Absent,
        Err(e) => Read::Unreadable(ReadError::new(path.display().to_string(), e.to_string())),
    }
}

/// `Absent` means no live login: no file, no `oauthAccount`, or no email (Appendix A.6).
pub fn live_identity(paths: &CcPaths) -> Read<Identity> {
    match get_key(&paths.global_config, "oauthAccount") {
        Read::Present(Some(v)) => {
            identity_from_oauth_account(&v).map_or(Read::Absent, Read::Present)
        }
        Read::Present(None) | Read::Absent => Read::Absent,
        Read::Unreadable(e) => Read::Unreadable(e),
    }
}

pub fn get_key(path: &Path, key: &str) -> Read<Option<Value>> {
    match read_bytes(path) {
        Read::Present(b) => match splice::get_top_level(&b, key) {
            Ok(v) => Read::Present(v),
            Err(e) => Read::Unreadable(ReadError::new(path.display().to_string(), e.to_string())),
        },
        Read::Absent => Read::Absent,
        Read::Unreadable(e) => Read::Unreadable(e),
    }
}

/// Restores the exact bytes a splice replaced, or removes a file the splice created. The
/// bytes may hold `primaryApiKey`, so `Debug` shows the path only.
pub struct ConfigUndo {
    path: PathBuf,
    before: Option<Vec<u8>>,
}

impl fmt::Debug for ConfigUndo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfigUndo")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Undo for ConfigUndo {
    fn undo(self: Box<Self>, locks: &LiveLocks<'_>) -> Result<(), ProviderError> {
        let fence = || locks.check_owned().map_err(ProviderError::from);
        match &self.before {
            Some(b) => write_atomic_with(&self.path, b, 0o600, fence)?,
            None => {
                // The splice created the file, possibly through a dangling symlink: remove
                // what it created and keep the link.
                fence()?;
                remove_target(&self.path)?;
            }
        }
        Ok(())
    }

    fn what(&self) -> String {
        format!("restore {}", self.path.display())
    }
}

/// §9.5: replaces (`Some`) or removes (`None`) one top-level key, changing no other byte.
/// A torn or non-object file is never replaced. `fence` runs immediately before the new file
/// is published (§9.1).
pub fn splice_key(
    path: &Path,
    key: &str,
    value: Option<&Value>,
    fence: Fence<'_>,
) -> Result<ConfigUndo, ProviderError> {
    let before = read_bytes(path);
    let unsplicable = || ProviderError::ConfigUnsplicable {
        path: path.to_path_buf(),
        remedy: CONFIG_REMEDY,
    };
    let new = match (&before, value) {
        (Read::Unreadable(_), _) => return Err(unsplicable()),
        (Read::Absent, None) => {
            return Ok(ConfigUndo {
                path: path.to_path_buf(),
                before: None,
            });
        }
        (Read::Absent, Some(v)) => {
            let key_json = serde_json::to_string(key).expect("a string always serializes");
            format!("{{\n  {key_json}: {}\n}}\n", render_nested(v, 1)).into_bytes()
        }
        (Read::Present(b), Some(v)) => {
            splice::replace_top_level(b, key, v).map_err(|_| unsplicable())?
        }
        (Read::Present(b), None) => splice::remove_top_level(b, key).map_err(|_| unsplicable())?,
    };
    let before = before.present();
    if before.as_deref() != Some(new.as_slice()) {
        write_atomic_with(path, &new, 0o600, fence)?;
    }
    Ok(ConfigUndo {
        path: path.to_path_buf(),
        before,
    })
}
