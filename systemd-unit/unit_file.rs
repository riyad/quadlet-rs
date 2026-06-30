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

use crate::{Error, IoError};

use super::INSTALL_SECTION;
use super::path_buf_ext::PathBufExt;
use super::path_ext::PathExt;
use super::unit_data::SystemdUnitData;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemdUnitFile {
    pub path: PathBuf,
    data: SystemdUnitData,
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
        if let Some(template_base) = template_base
            && template_instance.is_none()
        {
            if let Some(default_instance) = self.lookup(INSTALL_SECTION, "DefaultInstance") {
                service_name = OsString::from(format!(
                    "{template_base}@{default_instance}.{}",
                    self.unit_type()
                ));
            } else {
                service_name = OsString::default();
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
        !self
            .path()
            .file_stem()
            .unwrap_or_default()
            .as_encoded_bytes()
            .contains(&b'@')
    }

    pub fn is_template_instance_unit(&self) -> bool {
        matches!(self.path().file_name_template_parts(), (Some(_), Some(_)))
    }

    pub fn is_template_unit(&self) -> bool {
        matches!(self.path().file_name_template_parts(), (Some(_), None))
    }

    pub fn load_from_path(path: &Path) -> Result<Self, IoError> {
        let buf = fs::read_to_string(path)?;

        Ok(SystemdUnitFile {
            path: path.into(),
            data: SystemdUnitData::parse(buf.as_str()).map_err(Error::Unit)?,
        })
    }

    pub fn load_dropins_from<'i, I: IntoIterator<Item = &'i Path>>(
        self: &mut SystemdUnitFile,
        source_paths: I,
    ) -> Result<(), IoError> {
        let source_paths = Vec::from_iter(source_paths);

        let mut dropin_dirs: Vec<PathBuf> = Vec::new();

        let mut unit_dropin_dir_name = self.file_name().to_os_string();
        unit_dropin_dir_name.push(".d");
        for source_path in &source_paths {
            dropin_dirs.push(source_path.join(&unit_dropin_dir_name));
        }

        // For instantiated templates, also look in the non-instanced template dropin dirs
        if let (Some(template_base), Some(_)) = self.path().file_name_template_parts() {
            let template_dropin_dir_name = format!("{template_base}@.{}.d", self.unit_type());
            for source_path in &source_paths {
                dropin_dirs.push(source_path.join(&template_dropin_dir_name));
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

    mod load_dropins_from {
        use super::*;
        use tempfile::tempdir;

        #[test]
        fn with_no_source_paths_does_nothing() {
            let mut unit_file = SystemdUnitFile {
                path: PathBuf::from("foo.container"),
                ..Default::default()
            };
            unit_file.add("Container", "Image", "example.com/image");

            let result = unit_file.load_dropins_from(std::iter::empty::<&Path>());

            assert!(result.is_ok());
            assert_eq!(
                unit_file.lookup("Container", "Image"),
                Some("example.com/image".to_string())
            );
        }

        #[test]
        fn with_missing_dropin_dir_returns_ok() {
            let source_dir = tempdir().expect("cannot create temp dir");

            let mut unit_file = SystemdUnitFile {
                path: PathBuf::from("foo.container"),
                ..Default::default()
            };

            let result = unit_file.load_dropins_from([source_dir.path()]);

            assert!(result.is_ok());
            assert!(unit_file.is_empty());
        }

        #[test]
        fn loads_single_dropin_conf_file() {
            let source_dir = tempdir().expect("cannot create temp dir");
            let dropin_dir = source_dir.path().join("foo.container.d");
            fs::create_dir_all(&dropin_dir).expect("cannot create dropin dir");
            fs::write(
                dropin_dir.join("10-override.conf"),
                "[Container]\nImage=example.com/image\n",
            )
            .expect("cannot write dropin file");

            let mut unit_file = SystemdUnitFile {
                path: PathBuf::from("foo.container"),
                ..Default::default()
            };

            let result = unit_file.load_dropins_from([source_dir.path()]);

            assert!(result.is_ok());
            assert_eq!(
                unit_file.lookup("Container", "Image"),
                Some("example.com/image".to_string())
            );
        }

        #[test]
        fn ignores_files_without_conf_extension() {
            let source_dir = tempdir().expect("cannot create temp dir");
            let dropin_dir = source_dir.path().join("foo.container.d");
            fs::create_dir_all(&dropin_dir).expect("cannot create dropin dir");
            fs::write(
                dropin_dir.join("10-override.txt"),
                "[Container]\nImage=example.com/image\n",
            )
            .expect("cannot write dropin file");

            let mut unit_file = SystemdUnitFile {
                path: PathBuf::from("foo.container"),
                ..Default::default()
            };

            let result = unit_file.load_dropins_from([source_dir.path()]);

            assert!(result.is_ok());
            assert!(unit_file.is_empty());
        }

        #[test]
        fn merges_multiple_dropins_in_alphanumeric_order() {
            let source_dir = tempdir().expect("cannot create temp dir");
            let dropin_dir = source_dir.path().join("foo.container.d");
            fs::create_dir_all(&dropin_dir).expect("cannot create dropin dir");
            fs::write(
                dropin_dir.join("10-first.conf"),
                "[Container]\nImage=first\n",
            )
            .expect("cannot write dropin file");
            fs::write(
                dropin_dir.join("20-second.conf"),
                "[Container]\nImage=second\n",
            )
            .expect("cannot write dropin file");

            let mut unit_file = SystemdUnitFile {
                path: PathBuf::from("foo.container"),
                ..Default::default()
            };

            let result = unit_file.load_dropins_from([source_dir.path()]);

            assert!(result.is_ok());
            // entries are merged in, not overwritten
            assert_eq!(
                unit_file.lookup_all("Container", "Image"),
                vec!["first", "second"]
            );
            // last merged-in value wins for single-value lookups
            assert_eq!(
                unit_file.lookup("Container", "Image"),
                Some("second".to_string())
            );
        }

        #[test]
        fn first_source_path_wins_for_a_dropin_filename_present_in_multiple_source_paths() {
            // Same as how a higher-priority unit search dir masks a whole unit
            // file of the same name in a lower-priority dir, a drop-in file
            // masks a same-named drop-in file from a lower-priority source
            // path; it does not get merged in as well.
            let source_dir1 = tempdir().expect("cannot create temp dir");
            let source_dir2 = tempdir().expect("cannot create temp dir");
            let dropin_dir1 = source_dir1.path().join("foo.container.d");
            let dropin_dir2 = source_dir2.path().join("foo.container.d");
            fs::create_dir_all(&dropin_dir1).expect("cannot create dropin dir");
            fs::create_dir_all(&dropin_dir2).expect("cannot create dropin dir");
            fs::write(
                dropin_dir1.join("10-override.conf"),
                "[Container]\nImage=from-dir1\n",
            )
            .expect("cannot write dropin file");
            fs::write(
                dropin_dir2.join("10-override.conf"),
                "[Container]\nImage=from-dir2\n",
            )
            .expect("cannot write dropin file");

            let mut unit_file = SystemdUnitFile {
                path: PathBuf::from("foo.container"),
                ..Default::default()
            };

            let result = unit_file.load_dropins_from([source_dir1.path(), source_dir2.path()]);

            assert!(result.is_ok());
            assert_eq!(
                unit_file.lookup_all("Container", "Image"),
                vec!["from-dir1"]
            );
        }

        #[test]
        fn instantiated_template_unit_also_loads_generic_template_dropin_dir() {
            let source_dir = tempdir().expect("cannot create temp dir");
            let generic_dropin_dir = source_dir.path().join("foo@.container.d");
            fs::create_dir_all(&generic_dropin_dir).expect("cannot create dropin dir");
            fs::write(
                generic_dropin_dir.join("10-override.conf"),
                "[Container]\nImage=generic\n",
            )
            .expect("cannot write dropin file");

            let mut unit_file = SystemdUnitFile {
                path: PathBuf::from("foo@bar.container"),
                ..Default::default()
            };

            let result = unit_file.load_dropins_from([source_dir.path()]);

            assert!(result.is_ok());
            assert_eq!(
                unit_file.lookup("Container", "Image"),
                Some("generic".to_string())
            );
        }

        #[test]
        fn instance_specific_dropin_takes_precedence_over_generic_template_dropin() {
            let source_dir = tempdir().expect("cannot create temp dir");
            let instance_dropin_dir = source_dir.path().join("foo@bar.container.d");
            let generic_dropin_dir = source_dir.path().join("foo@.container.d");
            fs::create_dir_all(&instance_dropin_dir).expect("cannot create dropin dir");
            fs::create_dir_all(&generic_dropin_dir).expect("cannot create dropin dir");
            fs::write(
                instance_dropin_dir.join("10-override.conf"),
                "[Container]\nImage=instance\n",
            )
            .expect("cannot write dropin file");
            fs::write(
                generic_dropin_dir.join("10-override.conf"),
                "[Container]\nImage=generic\n",
            )
            .expect("cannot write dropin file");

            let mut unit_file = SystemdUnitFile {
                path: PathBuf::from("foo@bar.container"),
                ..Default::default()
            };

            let result = unit_file.load_dropins_from([source_dir.path()]);

            assert!(result.is_ok());
            assert_eq!(
                unit_file.lookup("Container", "Image"),
                Some("instance".to_string())
            );
            assert_eq!(unit_file.lookup_all("Container", "Image"), vec!["instance"]);
        }

        #[test]
        fn instance_specific_dropin_ignores_other_instances_dropins() {
            let source_dir = tempdir().expect("cannot create temp dir");
            let instance_dropin_dir = source_dir.path().join("foo@bar.container.d");
            let other_instance_dropin_dir = source_dir.path().join("foo@other.container.d");
            fs::create_dir_all(&instance_dropin_dir).expect("cannot create dropin dir");
            fs::create_dir_all(&other_instance_dropin_dir).expect("cannot create dropin dir");
            fs::write(
                instance_dropin_dir.join("10-override.conf"),
                "[Container]\nImage=instance\n",
            )
            .expect("cannot write dropin file");
            fs::write(
                other_instance_dropin_dir.join("10-override.conf"),
                "[Container]\nImage=other\n",
            )
            .expect("cannot write dropin file");

            let mut unit_file = SystemdUnitFile {
                path: PathBuf::from("foo@bar.container"),
                ..Default::default()
            };

            let result = unit_file.load_dropins_from([source_dir.path()]);

            assert!(result.is_ok());
            assert_eq!(unit_file.lookup_all("Container", "Image"), vec!["instance"]);
        }

        #[test]
        fn non_template_unit_does_not_check_template_dropin_dirs() {
            let source_dir = tempdir().expect("cannot create temp dir");
            let generic_dropin_dir = source_dir.path().join("foo@.container.d");
            fs::create_dir_all(&generic_dropin_dir).expect("cannot create dropin dir");
            fs::write(
                generic_dropin_dir.join("10-override.conf"),
                "[Container]\nImage=generic\n",
            )
            .expect("cannot write dropin file");

            let mut unit_file = SystemdUnitFile {
                path: PathBuf::from("foo.container"),
                ..Default::default()
            };

            let result = unit_file.load_dropins_from([source_dir.path()]);

            assert!(result.is_ok());
            assert!(unit_file.is_empty());
        }

        #[test]
        fn returns_error_for_invalid_dropin_file() {
            let source_dir = tempdir().expect("cannot create temp dir");
            let dropin_dir = source_dir.path().join("foo.container.d");
            fs::create_dir_all(&dropin_dir).expect("cannot create dropin dir");
            fs::write(
                dropin_dir.join("10-broken.conf"),
                // `Key_One` is not a legal key (underscores are not allowed)
                "[Container]\nKey_One=value\n",
            )
            .expect("cannot write dropin file");

            let mut unit_file = SystemdUnitFile {
                path: PathBuf::from("foo.container"),
                ..Default::default()
            };

            let result = unit_file.load_dropins_from([source_dir.path()]);

            assert!(matches!(result, Err(IoError::Unit(_))));
        }

        #[test]
        fn with_absolute_unit_path_still_searches_dropin_dirs_in_every_source_path() {
            let root = tempdir().expect("cannot create temp dir");
            let source_dir1 = root.path().join("dir1");
            let source_dir2 = root.path().join("dir2");
            fs::create_dir_all(&source_dir1).expect("cannot create source dir");
            fs::create_dir_all(&source_dir2).expect("cannot create source dir");

            let dropin_dir1 = source_dir1.join("foo.container.d");
            let dropin_dir2 = source_dir2.join("foo.container.d");
            fs::create_dir_all(&dropin_dir1).expect("cannot create dropin dir");
            fs::create_dir_all(&dropin_dir2).expect("cannot create dropin dir");
            fs::write(
                dropin_dir1.join("10-first.conf"),
                "[Container]\nImage=from-dir1\n",
            )
            .expect("cannot write dropin file");
            fs::write(
                dropin_dir2.join("20-second.conf"),
                "[Container]\nImage=from-dir2\n",
            )
            .expect("cannot write dropin file");

            // the unit file itself lives in a third, unrelated directory
            let mut unit_file = SystemdUnitFile {
                path: root.path().join("elsewhere").join("foo.container"),
                ..Default::default()
            };

            let result =
                unit_file.load_dropins_from([source_dir1.as_path(), source_dir2.as_path()]);

            assert!(result.is_ok());
            assert_eq!(
                unit_file.lookup_all("Container", "Image"),
                vec!["from-dir1", "from-dir2"]
            );
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
