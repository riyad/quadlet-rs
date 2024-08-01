use std::ffi::OsStr;
use std::os::unix::prelude::OsStrExt;
use std::path::{Path, PathBuf};
use std::{env, fs};

use walkdir::WalkDir;

use super::constants::*;
use super::logger::*;

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

    pub(crate) fn new() -> UnitSearchDirsBuilder {
        UnitSearchDirsBuilder {
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
    recursive: bool,
    rootless: bool,
}

impl UnitSearchDirsBuilder {
    pub(crate) fn build(&self) -> UnitSearchDirs {
        // Allow overdiding source dir, this is mainly for the CI tests
        if let Ok(unit_dirs_env) = std::env::var("QUADLET_UNIT_DIRS") {
            let iter = env::split_paths(&unit_dirs_env)
                .map(PathBuf::from)
                .filter(|p| {
                    if p.is_absolute() {
                        return true;
                    }

                    log!("{p:?} is not a valid file path");
                    false
                })
                .flat_map(|p| self.subdirs_for_search_dir(p, None));

            return UnitSearchDirs(iter.collect());
        }

        let mut dirs: Vec<PathBuf> = Vec::with_capacity(4);
        if self.rootless {
            let runtime_dir = dirs::runtime_dir().expect("could not determine runtime dir");
            dirs.extend(self.subdirs_for_search_dir(runtime_dir.join("containers/systemd"), None));
            let config_dir = dirs::config_dir().expect("could not determine config dir");
            dirs.extend(self.subdirs_for_search_dir(config_dir.join("containers/systemd"), None));
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
            dirs.push(PathBuf::from(UNIT_DIR_ADMIN).join("users"));
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
    if entry
        .path()
        .starts_with(PathBuf::from(UNIT_DIR_ADMIN).join("users"))
    {
        if entry.path().components().count() > SYSTEM_USER_DIR_LEVEL {
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
    if entry
        .path()
        .starts_with(PathBuf::from(UNIT_DIR_ADMIN).join("users"))
    {
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

        #[test]
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
                UnitSearchDirs::new()
                    .rootless(false)
                    .recursive(false)
                    .build()
                    .0,
                expected,
            )
        }

        #[test]
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
                UnitSearchDirs::new()
                    .rootless(true)
                    .recursive(false)
                    .build()
                    .0,
                expected
            )
        }
    }
}
