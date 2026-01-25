use std::ffi::OsStr;
use std::io::ErrorKind;
use std::os::unix::prelude::OsStrExt;
use std::path::{Path, PathBuf};
use std::{env, fs};

use log::{debug, info};
use walkdir::WalkDir;

use super::constants::*;

use super::RuntimeError;

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
            rootless: false,
        }
    }

    pub(crate) fn from_env_or_system() -> UnitSearchDirsBuilder {
        if let Some(quadlet_unit_dirs) = env::var("QUADLET_UNIT_DIRS").ok() {
            if !quadlet_unit_dirs.is_empty() {
                return Self::from_env();
            }
        }

        UnitSearchDirsBuilder {
            dirs: None,
            rootless: false,
        }
    }

    pub(crate) fn new(dirs: Vec<PathBuf>) -> UnitSearchDirsBuilder {
        UnitSearchDirsBuilder {
            dirs: Some(dirs),
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
    rootless: bool,
}

type FilterFn = Box<dyn Fn(&walkdir::DirEntry, bool) -> bool>;

impl UnitSearchDirsBuilder {
    pub(crate) fn build(mut self) -> UnitSearchDirs {
        if let Some(dirs) = self.dirs.take() {
            self.build_from_dirs(dirs)
        } else {
            self.build_from_system()
        }
    }

    pub(crate) fn build_from_dirs(self, dirs: Vec<PathBuf>) -> UnitSearchDirs {
        UnitSearchDirs(
            dirs.into_iter()
                .filter(|p| {
                    if p.is_absolute() {
                        true
                    } else {
                        info!("{p:?} is not a valid file path");
                        false
                    }
                })
                .flat_map(|p| self.subdirs_for_search_dir(p, None))
                .collect(),
        )
    }

    pub(crate) fn build_from_system(self) -> UnitSearchDirs {
        let resolved_unit_dir_admin_user = Self::resolve_unit_dir_admin_user();
        let user_level_filter = get_user_level_filter_func(resolved_unit_dir_admin_user.clone());

        if self.rootless {
            let system_user_dir_level = resolved_unit_dir_admin_user.components().count();
            let non_numeric_filter = get_non_numeric_filter_func(
                resolved_unit_dir_admin_user.clone(),
                system_user_dir_level,
            );

            return UnitSearchDirs(self.get_rootless_dirs(&non_numeric_filter, &user_level_filter));
        }

        UnitSearchDirs(self.get_root_dirs(&user_level_filter))
    }

    fn get_root_dirs(&self, user_level_filter: &FilterFn) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = Vec::with_capacity(4);

        dirs.extend(
            self.subdirs_for_search_dir(PathBuf::from(UNIT_DIR_TEMP), Some(user_level_filter)),
        );
        dirs.extend(
            self.subdirs_for_search_dir(PathBuf::from(UNIT_DIR_ADMIN), Some(user_level_filter)),
        );
        dirs.extend(self.subdirs_for_search_dir(PathBuf::from(UNIT_DIR_DISTRO), None));

        dirs
    }

    fn get_rootless_dirs(
        &self,
        non_numeric_filter: &FilterFn,
        user_level_filter: &FilterFn,
    ) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = Vec::with_capacity(4);

        if let Some(runtime_dir) = dirs::runtime_dir() {
            dirs.extend(self.subdirs_for_search_dir(runtime_dir.join("containers/systemd"), None));
        }

        if let Some(config_dir) = dirs::config_dir() {
            dirs.extend(self.subdirs_for_search_dir(config_dir.join("containers/systemd"), None));
        }

        dirs.extend(self.subdirs_for_search_dir(
            PathBuf::from(UNIT_DIR_ADMIN).join("users"),
            Some(non_numeric_filter),
        ));
        dirs.extend(
            self.subdirs_for_search_dir(
                PathBuf::from(UNIT_DIR_ADMIN)
                    .join("users")
                    .join(users::get_current_uid().to_string()),
                Some(user_level_filter),
            ),
        );

        dirs.push(PathBuf::from(UNIT_DIR_ADMIN).join("users"));

        dirs
    }

    pub(crate) fn rootless(mut self, rootless: bool) -> Self {
        self.rootless = rootless;
        self
    }

    fn resolve_unit_dir_admin_user() -> PathBuf {
        let unit_dir_admin_user = PathBuf::from(UNIT_DIR_ADMIN).join("users");

        if unit_dir_admin_user.is_symlink() {
            match unit_dir_admin_user.read_link() {
                Ok(resolved_path) => resolved_path,
                Err(err) => {
                    if err.kind() != ErrorKind::NotFound {
                        debug!(
                            "Error occurred resolving path {:?}: {err}",
                            &unit_dir_admin_user
                        );
                    }
                    unit_dir_admin_user
                }
            }
        } else {
            unit_dir_admin_user
        }
    }

    fn subdirs_for_search_dir(
        &self,
        path: PathBuf,
        filter_fn: Option<&Box<dyn Fn(&walkdir::DirEntry, bool) -> bool>>,
    ) -> Vec<PathBuf> {
        let path = if path.is_symlink() {
            match path.read_link() {
                Ok(path) => path,
                Err(err) => {
                    if err.kind() != ErrorKind::NotFound {
                        debug!("Error occurred resolving path {path:?}: {err}");
                    }
                    // Despite the failure add the path to the list for logging purposes
                    return vec![path];
                }
            }
        } else {
            path
        };

        let mut dirs = Vec::new();

        if !path.exists() {
            return dirs;
        }

        for entry in WalkDir::new(&path)
            .into_iter()
            .filter_entry(|e| e.path().is_dir())
        {
            match entry {
                Err(e) => debug!("Error occurred walking sub directories {path:?}: {e}"),
                Ok(entry) => {
                    if let Some(filter_fn) = &filter_fn {
                        if filter_fn(&entry, self.rootless) {
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

fn get_non_numeric_filter_func<'a>(
    resolved_unit_dir_admin_user: PathBuf,
    system_user_dir_level: usize,
) -> Box<dyn Fn(&walkdir::DirEntry, bool) -> bool + 'a> {
    return Box::new(move |entry, _rootless| -> bool {
        // when running in rootless, only recrusive walk directories that are non numeric
        // ignore sub dirs under the user directory that may correspond to a user id
        if entry
            .path()
            .starts_with(resolved_unit_dir_admin_user.clone())
        {
            if entry.path().components().count() > system_user_dir_level {
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
    });
}

fn get_user_level_filter_func<'a>(
    resolved_unit_dir_admin_user: PathBuf,
) -> Box<dyn Fn(&walkdir::DirEntry, bool) -> bool + 'a> {
    return Box::new(move |entry, rootless| -> bool {
        // if quadlet generator is run rootless, do not recurse other user sub dirs
        // if quadlet generator is run as root, ignore users sub dirs
        if entry
            .path()
            .starts_with(resolved_unit_dir_admin_user.clone())
        {
            if rootless {
                return true;
            }
        } else {
            return true;
        }

        false
    });
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
                    UnitSearchDirs::from_env().rootless(false).build().0,
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
                    format!("/etc/containers/systemd/users/{}", users::get_current_uid()),
                    format!("/etc/containers/systemd/users"),
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
                    UnitSearchDirs::from_env().rootless(true).build().0,
                    expected
                )
            }

            #[test]
            #[serial_test::serial]
            fn use_dirs_from_env_var() {
                // remember global state
                let _quadlet_unit_dirs = env::var("QUADLET_UNIT_DIRS");

                let temp_dir = tempfile::tempdir().expect("cannot create temp dir");
                // SAFETY: test ist run serially with other tests
                unsafe { env::set_var("QUADLET_UNIT_DIRS", temp_dir.path()) };

                let expected = [temp_dir.path()];

                assert_eq!(UnitSearchDirs::from_env().build().0, expected);

                // restore global state
                match _quadlet_unit_dirs {
                    // SAFETY: test ist run serially with other tests
                    Ok(val) => unsafe { env::set_var("QUADLET_UNIT_DIRS", val) },
                    // SAFETY: test ist run serially with other tests
                    Err(_) => unsafe { env::remove_var("QUADLET_UNIT_DIRS") },
                }
            }

            #[test]
            #[serial_test::serial]
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

                // SAFETY: test ist run serially with other tests
                unsafe { env::set_var("QUADLET_UNIT_DIRS", symlink) };

                let expected = [actual_dir.as_path(), inner_dir.as_path()];

                assert_eq!(UnitSearchDirs::from_env().build().0, expected);

                // cleanup
                fs::remove_dir_all(temp_dir.path()).expect("cannot remove temp dir");

                // restore global state
                match _quadlet_unit_dirs {
                    // SAFETY: test ist run serially with other tests
                    Ok(val) => unsafe { env::set_var("QUADLET_UNIT_DIRS", val) },
                    // SAFETY: test ist run serially with other tests
                    Err(_) => unsafe { env::remove_var("QUADLET_UNIT_DIRS") },
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
