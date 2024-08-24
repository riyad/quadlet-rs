use std::collections::HashMap;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};

use log::debug;
use walkdir::WalkDir;

use super::path_buf_ext::PathBufExt;
use super::unit::SystemdUnit;

#[derive(Debug, thiserror::Error)]
pub enum IoError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Unit(#[from] super::Error),
}

#[derive(Debug, PartialEq)]
pub struct SystemdUnitFile {
    pub(crate) path: PathBuf,
    unit: SystemdUnit,
}

impl Default for SystemdUnitFile {
    fn default() -> Self {
        Self {
            path: Default::default(),
            unit: Default::default(),
        }
    }
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
    pub fn file_name(&self) -> &OsStr {
        self.path().file_name().expect("should have a file name")
    }

    pub fn is_template_unit(&self) -> bool {
        match self.path().file_name_template_parts() {
            (Some(_), _) => true,
            _ => false,
        }
    }

    pub fn load_from_path(path: &Path) -> Result<Self, IoError> {
        let buf = fs::read_to_string(&path)?;

        Ok(SystemdUnitFile {
            path: path.into(),
            unit: SystemdUnit::load_from_str(buf.as_str())?,
        })
    }

    pub fn load_dropins_from<'i, I: IntoIterator<Item = &'i Path>>(
        self: &mut SystemdUnitFile,
        source_paths: I,
    ) -> Result<(), IoError> {
        let source_paths = Vec::from_iter(source_paths);

        let mut dropin_dirs: Vec<PathBuf> = Vec::new();

        for source_path in &source_paths {
            let mut unit_dropin_dir = self.path().as_os_str().to_os_string();
            unit_dropin_dir.push(".d");
            dropin_dirs.push(source_path.join(unit_dropin_dir));
        }

        // For instantiated templates, also look in the non-instanced template dropin dirs
        if let (Some(template_base), Some(_)) = self.path().file_name_template_parts() {
            for source_path in &source_paths {
                let template_dropin_dir = self.path().with_file_name(format!("{template_base}@.{}.d", self.unit_type()));
                dropin_dirs.push(source_path.join(template_dropin_dir));
            }
        }

        let mut dropin_paths: HashMap<OsString, PathBuf> = HashMap::new();
        for dropin_dir in dropin_dirs {
            for entry in WalkDir::new(&dropin_dir) {
                let dropin_file = match entry {
                    Ok(entry) => entry,
                    Err(e) => {
                        if let Some(io_error) = e.io_error() {
                            match io_error.kind() {
                                io::ErrorKind::NotFound => {} // ignore missing drop-in directories
                                _ => {
                                    return Err(IoError::Io(
                                        //format!("error reading directory {dropin_dir:?}"),
                                        e.into(),
                                    ));
                                }
                            }
                        }
                        continue;
                    }
                };

                let dropin_name = dropin_file.file_name();
                if dropin_file.path().extension().unwrap_or_default() != "conf" {
                    // Only *.conf supported
                    continue;
                }

                if dropin_paths.contains_key(dropin_name) {
                    // We already saw this name
                    continue;
                }

                dropin_paths.insert(dropin_name.to_owned(), dropin_dir.join(dropin_name));
            }
        }

        let mut dropin_files: Vec<&OsString> = dropin_paths.keys().collect();

        // Merge in alpha-numerical order
        dropin_files.sort_unstable();

        for dropin_file in dropin_files {
            let dropin_path = dropin_paths
                .get(dropin_file.as_os_str())
                .expect("drop-in should be there");

            debug!("Loading source drop-in file {dropin_path:?}");

            match SystemdUnitFile::load_from_path(dropin_path) {
                Ok(dropin_unit_file) => self.merge_from(&dropin_unit_file),
                Err(e) => {
                    return Err(
                        //format!("error loading {dropin_path:?}"),
                        e,
                    );
                }
            }
        }

        Ok(())
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

    pub fn unit_type(&self) -> &str {
        self.path.extension().expect("should have an extension").to_str().expect("path is not a valid UTF-8 string")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod file_name {
        use super::*;

        #[test]
        #[should_panic]  // FIXME
        fn with_empty_path() {
            let unit_file = SystemdUnitFile { path: PathBuf::new(), ..Default::default() };

            assert_eq!(unit_file.file_name(), "");
        }

        #[test]
        fn with_simple_path() {
            let unit_file = SystemdUnitFile { path: PathBuf::from("foo.timer"), ..Default::default() };

            assert_eq!(unit_file.file_name(), "foo.timer");
        }

        #[test]
        fn with_long_path() {
            let unit_file = SystemdUnitFile { path: PathBuf::from("foo/bar.netdev"), ..Default::default() };

            assert_eq!(unit_file.file_name(), "bar.netdev");
        }
    }

    mod impl_default {
        use super::*;

        #[test]
        fn values() {
            let unit_file = SystemdUnitFile::default();

            assert_eq!(unit_file.path(), &PathBuf::from(""));
            assert_eq!(unit_file.unit, SystemdUnit::new());
        }
    }

    mod is_template_unit {
        use super::*;

            #[test]
        fn with_empty_path() {
            let unit_file = SystemdUnitFile { path: PathBuf::new(), ..Default::default() };

            assert!(!unit_file.is_template_unit());
        }

        #[test]
        fn with_simple_path() {
            let unit_file = SystemdUnitFile { path: PathBuf::from("foo/bar.timer"), ..Default::default() };

            assert!(!unit_file.is_template_unit());
        }

        #[test]
        fn with_template_base_path() {
            let unit_file = SystemdUnitFile { path: PathBuf::from("foo/bar@.netdev"), ..Default::default() };

            assert!(unit_file.is_template_unit());
        }

        #[test]
        fn with_template_instance_path() {
            let unit_file = SystemdUnitFile { path: PathBuf::from("foo/bar@baz.netdev"), ..Default::default() };

            assert!(unit_file.is_template_unit());
        }
    }

    mod unit_type {
        use super::*;

        #[test]
        #[should_panic]  // FIXME
        fn with_empty_path() {
            let unit_file = SystemdUnitFile { path: PathBuf::new(), ..Default::default() };

            assert_eq!(unit_file.unit_type(), "");
        }

        #[test]
        fn is_same_as_extension() {
            let unit_file = SystemdUnitFile { path: PathBuf::from("foo.timer"), ..Default::default() };

            assert_eq!(unit_file.unit_type(), "timer");
        }
    }
}
