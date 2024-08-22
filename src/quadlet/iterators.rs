use std::cell::LazyCell;
use std::ffi::OsStr;
use std::os::unix::prelude::OsStrExt;
use std::path::{Path, PathBuf};
use std::{env, fs};

use walkdir::WalkDir;

use super::constants::*;
use super::logger::*;

use super::RuntimeError;

const UNIT_DIR_ADMIN_USER: LazyCell<PathBuf> =
    LazyCell::new(|| PathBuf::from(UNIT_DIR_ADMIN).join("users"));
const RESOLVED_UNIT_DIR_ADMIN_USER: LazyCell<PathBuf> =
    LazyCell::new(|| match UNIT_DIR_ADMIN_USER.read_link() {
        Ok(resolved_path) => resolved_path,
        Err(err) => {
            debug!(
                "Error occurred resolving path {:?}: {err}",
                &UNIT_DIR_ADMIN_USER
            );
            UNIT_DIR_ADMIN_USER.clone()
        }
    });
const SYSTEM_USER_DIR_LEVEL: LazyCell<usize> =
    LazyCell::new(|| RESOLVED_UNIT_DIR_ADMIN_USER.components().count());

pub(crate) struct UnitFiles {
    inner: Box<dyn Iterator<Item = Result<fs::DirEntry, RuntimeError>>>,
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
            inner: Box::new(iter),
        })
    }
}

impl Iterator for UnitFiles {
    type Item = Result<fs::DirEntry, RuntimeError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

pub(crate) struct UnitSearchDirs(Vec<PathBuf>);

impl UnitSearchDirs {
    pub(crate) fn dirs(&self) -> &Vec<PathBuf> {
        &self.0
    }

    pub(crate) fn from_env() -> UnitSearchDirsBuilder {
        UnitSearchDirsBuilder {
            // Allow overdiding source dir, this is mainly for the CI tests
            dirs: env::var("QUADLET_UNIT_DIRS").ok().map(|unit_dirs_env| {
                env::split_paths(&unit_dirs_env)
                    .map(PathBuf::from)
                    .collect()
            }),
            recursive: false,
            rootless: false,
        }
    }

    pub(crate) fn new(dirs: Vec<PathBuf>) -> UnitSearchDirsBuilder {
        UnitSearchDirsBuilder {
            dirs: Some(dirs),
            recursive: false,
            rootless: false,
        }
    }

    pub(crate) fn iter(&self) -> UnitSearchDirsIterator {
        UnitSearchDirsIterator {
            inner: self.0.iter(),
        }
    }
}

pub(crate) struct UnitSearchDirsBuilder {
    dirs: Option<Vec<PathBuf>>,
    recursive: bool,
    rootless: bool,
}

impl UnitSearchDirsBuilder {
    pub(crate) fn build(mut self) -> UnitSearchDirs {
        if let Some(dirs) = self.dirs.take() {
            self.build_from_dirs(dirs)
        } else {
            self.build_from_env()
        }
    }

    pub(crate) fn build_from_dirs(self, dirs: Vec<PathBuf>) -> UnitSearchDirs {
        UnitSearchDirs(
            dirs.into_iter()
                .filter(|p| {
                    if p.is_absolute() {
                        return true;
                    }

                    log!("{p:?} is not a valid file path");
                    false
                })
                .flat_map(|p| self.subdirs_for_search_dir(p, None))
                .collect(),
        )
    }

    pub(crate) fn build_from_env(self) -> UnitSearchDirs {
        let mut dirs: Vec<PathBuf> = Vec::with_capacity(4);
        if self.rootless {
            let runtime_dir = dirs::runtime_dir().expect("could not determine runtime dir");
            dirs.extend(self.subdirs_for_search_dir(runtime_dir.join("containers/systemd"), None));
            let config_dir = dirs::config_dir().expect("could not determine config dir");
            dirs.extend(self.subdirs_for_search_dir(config_dir.join("containers/systemd"), None));
            dirs.push(PathBuf::from(UNIT_DIR_ADMIN).join("users"));
            dirs.extend(self.subdirs_for_search_dir(
                PathBuf::from(UNIT_DIR_ADMIN).join("users"),
                Some(Box::new(_non_numeric_filter)),
            ));
            dirs.extend(
                self.subdirs_for_search_dir(
                    PathBuf::from(UNIT_DIR_ADMIN)
                        .join("users")
                        .join(users::get_current_uid().to_string()),
                    Some(Box::new(_user_level_filter)),
                ),
            );
        } else {
            dirs.extend(self.subdirs_for_search_dir(
                PathBuf::from(UNIT_DIR_TEMP),
                Some(Box::new(_user_level_filter)),
            ));
            dirs.extend(self.subdirs_for_search_dir(
                PathBuf::from(UNIT_DIR_ADMIN),
                Some(Box::new(_user_level_filter)),
            ));
            dirs.extend(self.subdirs_for_search_dir(PathBuf::from(UNIT_DIR_DISTRO), None));
        }

        UnitSearchDirs(dirs)
    }

    pub(crate) fn recursive(mut self, recursive: bool) -> Self {
        self.recursive = recursive;
        self
    }

    pub(crate) fn rootless(mut self, rootless: bool) -> Self {
        self.rootless = rootless;
        self
    }

    fn subdirs_for_search_dir(
        &self,
        path: PathBuf,
        filter_fn: Option<Box<dyn Fn(&UnitSearchDirsBuilder, &walkdir::DirEntry) -> bool>>,
    ) -> Vec<PathBuf> {
        let path = if path.is_symlink() {
            match fs::read_link(&path) {
                Ok(path) => path,
                Err(e) => {
                    debug!("Error occurred resolving path {path:?}: {e}");
                    // Despite the failure add the path to the list for logging purposes
                    return vec![path];
                }
            }
        } else {
            path
        };

        let mut dirs = Vec::new();

        for entry in WalkDir::new(&path)
            .into_iter()
            .filter_entry(|e| e.path().is_dir())
        {
            match entry {
                Err(e) => debug!("Error occurred walking sub directories {path:?}: {e}"),
                Ok(entry) => {
                    if let Some(filter_fn) = &filter_fn {
                        if filter_fn(&self, &entry) {
                            dirs.push(entry.path().to_owned())
                        }
                    } else {
                        dirs.push(entry.path().to_owned())
                    }
                }
            }
        }

        dirs
    }
}

fn _non_numeric_filter(
    _unit_search_dirs: &UnitSearchDirsBuilder,
    entry: &walkdir::DirEntry,
) -> bool {
    // when running in rootless, only recrusive walk directories that are non numeric
    // ignore sub dirs under the user directory that may correspond to a user id
    if entry.path().starts_with(&*RESOLVED_UNIT_DIR_ADMIN_USER) {
        if entry.path().components().count() > *SYSTEM_USER_DIR_LEVEL {
            if !entry
                .path()
                .components()
                .last()
                .expect("path should have enough components")
                .as_os_str()
                .as_bytes()
                .iter()
                .all(|b| b.is_ascii_digit())
            {
                return true;
            }
        }
    } else {
        return true;
    }

    false
}

fn _user_level_filter(unit_search_dirs: &UnitSearchDirsBuilder, entry: &walkdir::DirEntry) -> bool {
    // if quadlet generator is run rootless, do not recurse other user sub dirs
    // if quadlet generator is run as root, ignore users sub dirs
    if entry.path().starts_with(&*RESOLVED_UNIT_DIR_ADMIN_USER) {
        if unit_search_dirs.rootless {
            return true;
        }
    } else {
        return true;
    }

    false
}

pub(crate) struct UnitSearchDirsIterator<'a> {
    inner: std::slice::Iter<'a, PathBuf>,
}

impl<'a> Iterator for UnitSearchDirsIterator<'a> {
    type Item = &'a PathBuf;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod unit_search_dirs {
        use super::*;

        mod from_env {
            use super::*;
            use std::os;
            use tempfile;

            #[test]
            #[serial_test::parallel]
            #[ignore = "fails when run as ordinary user, because /run/containers is only root-accessible"]
            fn rootful() {
                let expected = [
                    "/run/containers/systemd", // might only be accessible for root :/
                    "/etc/containers/systemd",
                    "/usr/share/containers/systemd",
                ]
                .iter()
                .map(PathBuf::from)
                .collect::<Vec<_>>();

                // NOTE: directories must exists and be reachable
                for path in &expected {
                    if !path.exists() {
                        panic!("{path:?} must exist and be reachable to run tests");
                    }
                }

                assert_eq!(
                    UnitSearchDirs::from_env()
                        .rootless(false)
                        .recursive(false)
                        .build()
                        .0,
                    expected,
                )
            }

            #[test]
            #[serial_test::parallel]
            fn rootless() {
                let expected = [
                    format!(
                        "{}/containers/systemd",
                        dirs::runtime_dir()
                            .expect("could not determine runtime dir")
                            .to_str()
                            .expect("runtime dir is not a valid UTF-8 string")
                    ),
                    format!(
                        "{}/containers/systemd",
                        dirs::config_dir()
                            .expect("could not determine config dir")
                            .to_str()
                            .expect("config dir is not a valid UTF-8 string")
                    ),
                    format!("/etc/containers/systemd/users"),
                    format!("/etc/containers/systemd/users/{}", users::get_current_uid()),
                ]
                .iter()
                .map(PathBuf::from)
                .collect::<Vec<_>>();

                // NOTE: directories must exists
                for path in &expected {
                    if !path.exists() {
                        panic!("{path:?} must exist to run tests");
                    }
                }

                assert_eq!(
                    UnitSearchDirs::from_env()
                        .rootless(true)
                        .recursive(false)
                        .build()
                        .0,
                    expected
                )
            }

            #[test]
            #[serial_test::serial]
            fn use_dirs_from_env_var() {
                // remember global state
                let _quadlet_unit_dirs = env::var("QUADLET_UNIT_DIRS");

                let temp_dir = tempfile::tempdir().expect("cannot create temp dir");
                env::set_var("QUADLET_UNIT_DIRS", temp_dir.path());

                let expected = [temp_dir.path()];

                assert_eq!(UnitSearchDirs::from_env().build().0, expected);

                // restore global setate
                match _quadlet_unit_dirs {
                    Ok(val) => env::set_var("QUADLET_UNIT_DIRS", val),
                    Err(_) => env::remove_var("QUADLET_UNIT_DIRS"),
                }
            }

            #[test]
            #[serial_test::parallel]
            fn should_follow_symlinks() {
                // remember global state
                let _quadlet_unit_dirs = env::var("QUADLET_UNIT_DIRS");

                // setup
                let temp_dir = tempfile::tempdir().expect("cannot create temp dir");
                let actual_dir = &temp_dir.path().join("actual");
                let inner_dir = &actual_dir.as_path().join("inner");
                let symlink = &temp_dir.path().join("symlink");
                fs::create_dir(actual_dir).expect("cannot create actual dir");
                fs::create_dir(inner_dir).expect("cannot create inner dir");
                os::unix::fs::symlink(actual_dir, symlink).expect("cannot create symlink");

                env::set_var("QUADLET_UNIT_DIRS", symlink);

                let expected = [actual_dir.as_path(), inner_dir.as_path()];

                assert_eq!(UnitSearchDirs::from_env().build().0, expected);

                // cleanup
                fs::remove_dir_all(temp_dir.path()).expect("cannot remove temp dir");

                // restore global setate
                match _quadlet_unit_dirs {
                    Ok(val) => env::set_var("QUADLET_UNIT_DIRS", val),
                    Err(_) => env::remove_var("QUADLET_UNIT_DIRS"),
                }
            }
        }

        mod new {
            use super::*;
            use std::os;

            #[test]
            fn specify_dirs() {
                let temp_dir = tempfile::tempdir().expect("cannot create temp dir");

                let dirs = vec![temp_dir.path().into()];

                let expected = [temp_dir.path()];

                assert_eq!(UnitSearchDirs::new(dirs).build().0, expected);
            }

            #[test]
            fn should_follow_symlinks() {
                // setup
                let temp_dir = tempfile::tempdir().expect("cannot create temp dir");
                let actual_dir = &temp_dir.path().join("actual");
                let inner_dir = &actual_dir.as_path().join("inner");
                let symlink = &temp_dir.path().join("symlink");
                fs::create_dir(actual_dir).expect("cannot create actual dir");
                fs::create_dir(inner_dir).expect("cannot create inner dir");
                os::unix::fs::symlink(actual_dir, symlink).expect("cannot create symlink");

                let dirs = vec![symlink.into()];

                let expected = [actual_dir.as_path(), inner_dir.as_path()];

                assert_eq!(UnitSearchDirs::new(dirs).build().0, expected);

                // cleanup
                fs::remove_dir_all(temp_dir.path()).expect("cannot remove temp dir");
            }
        }
    }
}
