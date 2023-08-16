use std::fs;
use std::io;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};

use super::unit::SystemdUnit;

pub enum IoError {
    Io(io::Error),
    Unit(super::Error),
}

impl From<super::Error> for IoError {
    fn from(e: super::Error) -> Self {
        IoError::Unit(e)
    }
}

impl From<io::Error> for IoError {
    fn from(e: io::Error) -> Self {
        IoError::Io(e)
    }
}

#[derive(Debug, PartialEq)]
pub struct SystemdUnitFile {
    pub(crate) path: PathBuf,
    unit: SystemdUnit,
}

impl Deref for SystemdUnitFile {
    type Target = SystemdUnit;

    fn deref(&self) -> &Self::Target {
        &self.unit
    }
}

impl DerefMut for SystemdUnitFile {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.unit
    }
}

impl SystemdUnitFile {

    pub fn load_from_path(path: &Path) -> Result<Self, IoError> {
        let buf = fs::read_to_string(&path)?;

        Ok(SystemdUnitFile {
            path: path.into(),
            unit: SystemdUnit::load_from_str(buf.as_str())?,
        })
    }

    pub fn new() -> Self {
        SystemdUnitFile {
            path: PathBuf::new(),
            unit: SystemdUnit::new(),
        }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}