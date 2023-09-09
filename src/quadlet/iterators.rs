use std::ffi::OsStr;
use std::fs::DirEntry;
use std::path::Path;

use crate::SUPPORTED_EXTENSIONS;

use super::RuntimeError;

pub(crate) struct UnitFiles {
    iter: Box<dyn Iterator<Item = Result<DirEntry, RuntimeError>>>,
}

impl UnitFiles {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, RuntimeError> {
        let path = path.as_ref();

        let entries = match path.read_dir() {
            Ok(entries) => entries,
            Err(e) => return Err(RuntimeError::Io(format!("Can't read {path:?}"), e)),
        };

        let iter = entries.filter_map(|entry| {
            let file = match entry {
                Ok(file) => file,
                Err(e) => {
                    return Some(Err(RuntimeError::Io(
                        format!("Can't read directory entry"),
                        e,
                    )))
                }
            };

            if SUPPORTED_EXTENSIONS
                .map(OsStr::new)
                .contains(&file.path().extension().unwrap_or(OsStr::new("")))
            {
                Some(Ok(file))
            } else {
                None
            }
        });

        Ok(UnitFiles {
            iter: Box::new(iter),
        })
    }
}

impl Iterator for UnitFiles {
    type Item = Result<DirEntry, RuntimeError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}
