use std::collections::HashMap;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::ops::{Deref, DerefMut};
use std::os;
use std::path::{Path, PathBuf};

use log::debug;
use log::warn;
use walkdir::WalkDir;

use super::path_buf_ext::PathBufExt;
use super::path_ext::PathExt;
use super::unit_data::SystemdUnitData;
use super::INSTALL_SECTION;

#[derive(Debug, thiserror::Error)]
pub enum IoError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Unit(#[from] super::Error),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SystemdUnitFile {
    pub(crate) path: PathBuf,
    data: SystemdUnitData,
}

impl Default for SystemdUnitFile {
    fn default() -> Self {
        Self {
            path: Default::default(),
            data: Default::default(),
        }
    }
}

impl Deref for SystemdUnitFile {
    type Target = SystemdUnitData;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl DerefMut for SystemdUnitFile {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

impl SystemdUnitFile {
    // This parses the `Install` section of the unit file and creates the required
    // symlinks to get systemd to start the newly generated file as needed.
    // In a traditional setup this is done by "systemctl enable", but that doesn't
    // work for auto-generated files like these.
    pub fn enable_service_file(&self, output_path: &Path) {
        let mut symlinks: Vec<PathBuf> = Vec::new();

        let mut alias: Vec<PathBuf> = self
            .lookup_all_strv(super::INSTALL_SECTION, "Alias")
            .iter()
            .map(|s| PathBuf::from(s).cleaned())
            .collect();
        symlinks.append(&mut alias);

        let mut service_name = self.file_name().to_os_string();
        let (template_base, template_instance) = self.path().file_name_template_parts();

        // For non-instantiated template service we only support installs if a
        // DefaultInstance is given. Otherwise we ignore the Install group, but
        // it is still useful when instantiating the unit via a symlink.
        if let Some(template_base) = template_base {
            if template_instance.is_none() {
                if let Some(default_instance) = self.lookup(INSTALL_SECTION, "DefaultInstance") {
                    service_name = OsString::from(format!(
                        "{template_base}@{default_instance}.{}",
                        self.unit_type()
                    ));
                } else {
                    service_name = OsString::default();
                }
            }
        }

        if !service_name.is_empty() {
            symlinks.append(
                &mut self.gather_dependent_symlinks(
                    "WantedBy",
                    "wants",
                    service_name
                        .to_str()
                        .expect("service_name is not valid UTF-8"),
                ),
            );
            symlinks.append(
                &mut self.gather_dependent_symlinks(
                    "RequiredBy",
                    "requires",
                    service_name
                        .to_str()
                        .expect("service_name is not valid UTF-8"),
                ),
            );
            symlinks.append(
                &mut self.gather_dependent_symlinks(
                    "UpheldBy",
                    "upholds",
                    service_name
                        .to_str()
                        .expect("service_name is not valid UTF-8"),
                ),
            );
        }

        // construct relative symlink targets so that <output_path>/<symlink_rel (aka. foo/<service_name>)>
        // links to <output_path>/<service_name>
        for symlink_rel in symlinks {
            let mut target = PathBuf::new();

            // At this point the symlinks are all relative, canonicalized
            // paths, so the number of slashes corresponds to its path depth
            // i.e. number of slashes == components - 1
            for _ in 1..symlink_rel.components().count() {
                target.push("..");
            }
            target.push(self.file_name());

            let symlink_path = output_path.join(symlink_rel);
            let symlink_dir = symlink_path.parent().unwrap();
            if let Err(e) = fs::create_dir_all(symlink_dir) {
                warn!("Can't create dir {:?}: {e}", symlink_dir.to_str().unwrap());
                continue;
            }

            debug!("Creating symlink {symlink_path:?} -> {target:?}");
            fs::remove_file(&symlink_path).unwrap_or_default(); // overwrite existing symlinks
            if let Err(e) = os::unix::fs::symlink(target, &symlink_path) {
                warn!("Failed creating symlink {:?}: {e}", symlink_path.to_str());
                continue;
            }
        }
    }

    pub fn file_name(&self) -> &OsStr {
        self.path().file_name().expect("should have a file name")
    }

    fn gather_dependent_symlinks(&self, key: &str, dir_ext: &str, file_name: &str) -> Vec<PathBuf> {
        self.lookup_all_strv(INSTALL_SECTION, key)
            .iter()
            .filter(|s| !s.contains('/')) // Only allow filenames, not paths
            .map(|group_by_unit| PathBuf::from(format!("{group_by_unit}.{dir_ext}/{file_name}")))
            .collect()
    }

    pub fn is_plain_unit(&self) -> bool {
        !self.path().file_stem().unwrap_or_default().as_encoded_bytes().contains(&b'@')
    }

    pub fn is_template_instance_unit(&self) -> bool {
        match self.path().file_name_template_parts() {
            (Some(_), Some(_)) => true,
            _ => false,
        }
    }

    pub fn is_template_unit(&self) -> bool {
        match self.path().file_name_template_parts() {
            (Some(_), None) => true,
            _ => false,
        }
    }

    pub fn load_from_path(path: &Path) -> Result<Self, IoError> {
        let buf = fs::read_to_string(&path)?;

        Ok(SystemdUnitFile {
            path: path.into(),
            data: SystemdUnitData::load_from_str(buf.as_str())?,
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
                let template_dropin_dir = self
                    .path()
                    .with_file_name(format!("{template_base}@.{}.d", self.unit_type()));
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
            data: SystemdUnitData::new(),
        }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn unit_type(&self) -> &str {
        self.path.systemd_unit_type()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod file_name {
        use super::*;

        #[test]
        #[should_panic] // FIXME
        fn with_empty_path() {
            let unit_file = SystemdUnitFile {
                path: PathBuf::new(),
                ..Default::default()
            };

            assert_eq!(unit_file.file_name(), "");
        }

        #[test]
        fn with_simple_path() {
            let unit_file = SystemdUnitFile {
                path: PathBuf::from("foo.timer"),
                ..Default::default()
            };

            assert_eq!(unit_file.file_name(), "foo.timer");
        }

        #[test]
        fn with_long_path() {
            let unit_file = SystemdUnitFile {
                path: PathBuf::from("foo/bar.netdev"),
                ..Default::default()
            };

            assert_eq!(unit_file.file_name(), "bar.netdev");
        }
    }

    mod impl_default {
        use super::*;

        #[test]
        fn values() {
            let unit_file = SystemdUnitFile::default();

            assert_eq!(unit_file.path(), &PathBuf::from(""));
            assert_eq!(unit_file.data, SystemdUnitData::new());
        }
    }

    mod is_plain_unit {
        use super::*;

        #[test]
        fn with_empty_path() {
            let unit_file = SystemdUnitFile {
                path: PathBuf::new(),
                ..Default::default()
            };

            assert!(unit_file.is_plain_unit());
        }

        #[test]
        fn with_simple_path() {
            let unit_file = SystemdUnitFile {
                path: PathBuf::from("foo/bar.timer"),
                ..Default::default()
            };

            assert!(unit_file.is_plain_unit());
        }

        #[test]
        fn with_template_base_path() {
            let unit_file = SystemdUnitFile {
                path: PathBuf::from("foo/bar@.netdev"),
                ..Default::default()
            };

            assert!(!unit_file.is_plain_unit());
        }

        #[test]
        fn with_template_instance_path() {
            let unit_file = SystemdUnitFile {
                path: PathBuf::from("foo/bar@baz.netdev"),
                ..Default::default()
            };

            assert!(!unit_file.is_plain_unit());
        }
    }

    mod is_template_instance_unit {
        use super::*;

        #[test]
        fn with_empty_path() {
            let unit_file = SystemdUnitFile {
                path: PathBuf::new(),
                ..Default::default()
            };

            assert!(!unit_file.is_template_instance_unit());
        }

        #[test]
        fn with_simple_path() {
            let unit_file = SystemdUnitFile {
                path: PathBuf::from("foo/bar.timer"),
                ..Default::default()
            };

            assert!(!unit_file.is_template_instance_unit());
        }

        #[test]
        fn with_template_base_path() {
            let unit_file = SystemdUnitFile {
                path: PathBuf::from("foo/bar@.netdev"),
                ..Default::default()
            };

            assert!(!unit_file.is_template_instance_unit());
        }

        #[test]
        fn with_template_instance_path() {
            let unit_file = SystemdUnitFile {
                path: PathBuf::from("foo/bar@baz.netdev"),
                ..Default::default()
            };

            assert!(unit_file.is_template_instance_unit());
        }
    }

    mod is_template_unit {
        use super::*;

        #[test]
        fn with_empty_path() {
            let unit_file = SystemdUnitFile {
                path: PathBuf::new(),
                ..Default::default()
            };

            assert!(!unit_file.is_template_unit());
        }

        #[test]
        fn with_simple_path() {
            let unit_file = SystemdUnitFile {
                path: PathBuf::from("foo/bar.timer"),
                ..Default::default()
            };

            assert!(!unit_file.is_template_unit());
        }

        #[test]
        fn with_template_base_path() {
            let unit_file = SystemdUnitFile {
                path: PathBuf::from("foo/bar@.netdev"),
                ..Default::default()
            };

            assert!(unit_file.is_template_unit());
        }

        #[test]
        fn with_template_instance_path() {
            let unit_file = SystemdUnitFile {
                path: PathBuf::from("foo/bar@baz.netdev"),
                ..Default::default()
            };

            assert!(!unit_file.is_template_unit());
        }
    }

    mod unit_type {
        use super::*;

        #[test]
        #[should_panic] // FIXME
        fn with_empty_path() {
            let unit_file = SystemdUnitFile {
                path: PathBuf::new(),
                ..Default::default()
            };

            assert_eq!(unit_file.unit_type(), "");
        }

        #[test]
        fn is_same_as_extension() {
            let unit_file = SystemdUnitFile {
                path: PathBuf::from("foo.timer"),
                ..Default::default()
            };

            assert_eq!(unit_file.unit_type(), "timer");
        }
    }
}
