use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::Command;

/// see https://www.freedesktop.org/software/systemd/man/latest/systemd.unit.html#User%20Unit%20Search%20Path
/// and https://www.freedesktop.org/software/systemd/man/latest/systemd.unit.html#Unit%20File%20Load%20Path
/// $XDG_CONFIG_HOME defaults to "$HOME/.config"
/// $XDG_CONFIG_DIRS defaults to "/etc/xdg"
/// $XDG_DATA_DIRS defaults to "/usr/local/share" and "/usr/share"
/// $XDG_DATA_HOME defaults to "$HOME/.local/share"
// TODO: init with `systemd-analyze --user unit-paths`
pub static DEFAULT_USER_SEARCH_PATHS: &[&str] = &[
    "$XDG_CONFIG_HOME/systemd/user.control/",
    "$XDG_RUNTIME_DIR/systemd/user.control/",
    "$XDG_RUNTIME_DIR/systemd/transient/",
    "$XDG_RUNTIME_DIR/systemd/generator.early/",
    "$XDG_CONFIG_HOME/systemd/user/",
    "$XDG_CONFIG_DIRS/systemd/user/",
    "/etc/systemd/user/",
    "$XDG_RUNTIME_DIR/systemd/user/",
    "/run/systemd/user/",
    "$XDG_RUNTIME_DIR/systemd/generator/",
    "$XDG_DATA_HOME/systemd/user/",
    "$XDG_DATA_DIRS/systemd/user/",
    // ...
    "/usr/local/lib/systemd/user/",
    "/usr/lib/systemd/user/",
    "$XDG_RUNTIME_DIR/systemd/generator.late/",
];

/// see https://www.freedesktop.org/software/systemd/man/latest/systemd.unit.html#System%20Unit%20Search%20Path
/// and https://www.freedesktop.org/software/systemd/man/latest/systemd.unit.html#Unit%20File%20Load%20Path
// TODO: init with `systemd-analyze --system unit-paths`
pub static DEFAULT_SYSTEM_SEARCH_PATHS: &[&str] = &[
    "/etc/systemd/system.control/",
    "/run/systemd/system.control/",
    "/run/systemd/transient/",
    "/run/systemd/generator.early/",
    "/etc/systemd/system/",
    //"/etc/systemd/system.attached/",
    "/run/systemd/system/",
    //"/run/systemd/system.attached/",
    "/run/systemd/generator/",
    // ...
    "/usr/local/lib/systemd/system/",
    "/usr/lib/systemd/system/",
    "/run/systemd/generator.late/",
];

pub static UNIT_PATH_ENV: &str = "SYSTEMD_UNIT_PATH";

/// Directory for global Systemd units (sysadmin owned)
pub static UNIT_DIR_ADMIN: &str = "/etc/systemd/system";
/// Directory for global Systemd units (distro owned)
pub static UNIT_DIR_DISTRO: &str = "/usr/lib/systemd/system";
/// Directory for temporary Systemd units (sysadmin owned)
pub static UNIT_DIR_TEMP: &str = "/run/systemd/system";

pub struct UnitSearchPaths(Vec<PathBuf>);

impl UnitSearchPaths {
    pub fn dirs(&self) -> &Vec<PathBuf> {
        &self.0
    }

    pub fn new(dirs: Vec<PathBuf>) -> Self {
        Self(dirs)
    }

    pub fn iter(&self) -> UnitSearchPathsIterator<'_> {
        UnitSearchPathsIterator {
            inner: self.0.iter(),
        }
    }
}

pub struct UnitSearchPathsBuilder {
    dirs: Vec<PathBuf>,
}

impl UnitSearchPathsBuilder {
    pub fn build(self) -> UnitSearchPaths {
        UnitSearchPaths(
            self.dirs
                .into_iter()
                .filter(|p| {
                    #[cfg(feature = "log")]
                    if !p.is_absolute() {
                        info!("{p:?} is not a valid file path");
                    }
                    p.is_absolute()
                })
                .flat_map(Self::subdirs_for_search_dir)
                .collect(),
        )
    }

    pub fn from_env(env_var_name: &str) -> Self {
        Self {
            // Allow overdiding source dir, this is mainly for the CI tests
            dirs: env::var(env_var_name)
                .ok()
                .map(|unit_dirs_env| env::split_paths(&unit_dirs_env).collect())
                .unwrap_or_default(),
        }
    }

    pub fn from_env_or_system(env_var_name: &str) -> Self {
        if let Ok(quadlet_unit_dirs) = env::var(env_var_name)
            && !quadlet_unit_dirs.is_empty() {
                return Self::from_env(env_var_name);
            }

        Self {
            dirs: Self::get_system_unit_paths().dirs().to_owned(),
        }
    }

    pub fn get_system_unit_paths() -> UnitSearchPaths {
        let output = Command::new("systemd-analyze")
            .arg("--system")
            .arg("unit-paths")
            .output()
            .expect("failed to execute process");
        UnitSearchPaths(
            output
                .stdout
                .split(|b| *b == b'\n')
                .map(|slice| PathBuf::from(OsStr::from_bytes(slice)))
                .collect(),
        )
    }

    pub fn get_user_unit_paths() -> UnitSearchPaths {
        let output = Command::new("systemd-analyze")
            .arg("--user")
            .arg("unit-paths")
            .output()
            .expect("failed to execute process");
        UnitSearchPaths(
            output
                .stdout
                .split(|b| *b == b'\n')
                .map(|slice| PathBuf::from(OsStr::from_bytes(slice)))
                .collect(),
        )
    }

    fn new(dirs: Vec<PathBuf>) -> Self {
        Self { dirs }
    }

    fn subdirs_for_search_dir(path: PathBuf) -> Vec<PathBuf> {
        let path = if path.is_symlink() {
            match path.read_link() {
                Ok(path) => path,
                Err(err) => {
                    if err.kind() != ErrorKind::NotFound {
                        #[cfg(feature = "log")]
                        debug!("Error occurred resolving path {path:?}: {err}")
                    }
                    // Despite the failure add the path to the list for logging purposes
                    return vec![path];
                }
            }
        } else {
            path
        };

        let mut dirs = Vec::new();

        if !path.exists() || !path.is_dir() {
            return dirs;
        }

        dirs.push(path.clone());

        for entry in fs::read_dir(&path).expect("cannot access search directory") {
            match entry {
                #[allow(unused_variables)]
                Err(e) => {
                    #[cfg(feature = "log")]
                    debug!("Error occurred walking sub directories of {path:?}: {e}")
                }
                Ok(entry) => {
                    // only iterate over directories
                    // skip drop-in directories
                    if entry.path().is_dir() && !entry.file_name().as_bytes().ends_with(b".d") {
                        dirs.push(entry.path())
                    }
                }
            }
        }

        dirs
    }
}

pub struct UnitSearchPathsIterator<'a> {
    inner: std::slice::Iter<'a, PathBuf>,
}

impl<'a> Iterator for UnitSearchPathsIterator<'a> {
    type Item = &'a PathBuf;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod unit_search_paths {
        use super::*;
    }

    mod unit_search_paths_builder {
        use super::*;

        mod build {
            use super::*;
            use std::os;
            use tempfile;

            #[test]
            #[serial_test::parallel]
            fn ignores_relative_paths() {
                let expected: Vec<PathBuf> = vec![];

                let dirs = vec![PathBuf::from("./foo/bar")];
                assert_eq!(UnitSearchPathsBuilder::new(dirs).build().dirs(), &expected)
            }

            #[test]
            #[serial_test::parallel]
            fn should_follow_symlinks() {
                // setup
                let temp_dir = tempfile::tempdir().expect("cannot create temp dir");
                let actual_dir = temp_dir.path().join("actual");
                let inner_dir = actual_dir.as_path().join("inner");
                let symlink = temp_dir.path().join("symlink");
                fs::create_dir(&actual_dir).expect("cannot create actual dir");
                fs::create_dir(&inner_dir).expect("cannot create inner dir");
                os::unix::fs::symlink(&actual_dir, &symlink).expect("cannot create symlink");

                let expected = [actual_dir.as_path(), inner_dir.as_path()];

                let dirs = vec![symlink];
                assert_eq!(UnitSearchPathsBuilder::new(dirs).build().dirs(), &expected);

                // cleanup
                fs::remove_dir_all(temp_dir.path()).expect("cannot remove temp dir");
            }

            #[test]
            #[serial_test::parallel]
            fn should_not_recurse_beyond_one_level() {
                // setup
                let temp_dir = tempfile::tempdir().expect("cannot create temp dir");
                let actual_dir = temp_dir.path().join("actual");
                let inner_dir = temp_dir.path().join("actual/inner");
                let ignored_dir = temp_dir.path().join("actual/inner/ignored");
                fs::create_dir(&actual_dir).expect("cannot create actual dir");
                fs::create_dir(&inner_dir).expect("cannot create inner dir");
                fs::create_dir(&ignored_dir).expect("cannot create inner dir");

                let expected = vec![actual_dir.as_path(), inner_dir.as_path()];

                let dirs = vec![actual_dir.to_owned()];
                assert_eq!(UnitSearchPathsBuilder::new(dirs).build().dirs(), &expected)
            }
        }

        mod from_env {
            use super::*;
            use tempfile;

            #[test]
            #[serial_test::parallel]
            //#[ignore = "fails when run as ordinary user, because /run/containers is only root-accessible"]
            fn falls_back_to_empty_list() {
                let expected: Vec<PathBuf> = vec![];

                assert_eq!(
                    UnitSearchPathsBuilder::from_env("DOES.NOT.EXIST")
                        .build()
                        .dirs(),
                    &expected,
                )
            }

            #[test]
            #[serial_test::serial]
            fn use_dirs_from_env_var() {
                // remember global state
                let _quadlet_unit_dirs = env::var(UNIT_PATH_ENV);

                let temp_dir = tempfile::tempdir().expect("cannot create temp dir");
                // SAFETY: test ist run serially with other tests
                unsafe { env::set_var(UNIT_PATH_ENV, temp_dir.path()) };

                let expected = [temp_dir.path()];

                assert_eq!(
                    UnitSearchPathsBuilder::from_env(UNIT_PATH_ENV)
                        .build()
                        .dirs(),
                    &expected
                );

                // restore global setate
                match _quadlet_unit_dirs {
                    // SAFETY: test ist run serially with other tests
                    Ok(val) => unsafe { env::set_var(UNIT_PATH_ENV, val) },
                    // SAFETY: test ist run serially with other tests
                    Err(_) => unsafe { env::remove_var(UNIT_PATH_ENV) },
                }
            }
        }

        mod new {
            use super::*;

            #[test]
            #[serial_test::parallel]
            fn specify_dirs() {
                let temp_dir = tempfile::tempdir().expect("cannot create temp dir");

                let dirs = vec![temp_dir.path().into()];

                let expected = [temp_dir.path()];

                assert_eq!(UnitSearchPathsBuilder::new(dirs).dirs, &expected);
            }
        }

        mod get_system_unit_paths {
            use super::*;

            #[test]
            #[serial_test::parallel]
            fn paths_match_systemd_analyze_output() {
                let output = Command::new("systemd-analyze")
                    .arg("--system")
                    .arg("unit-paths")
                    .output()
                    .expect("failed to execute process");

                let expected: Vec<PathBuf> = output
                    .stdout
                    .split(|b| *b == b'\n')
                    .map(|slice| PathBuf::from(OsStr::from_bytes(slice)))
                    .collect();

                assert_eq!(
                    UnitSearchPathsBuilder::get_system_unit_paths().dirs(),
                    &expected
                );
            }
        }

        mod get_user_unit_paths {
            use super::*;

            #[test]
            #[serial_test::parallel]
            fn paths_match_systemd_analyze_output() {
                let output = Command::new("systemd-analyze")
                    .arg("--user")
                    .arg("unit-paths")
                    .output()
                    .expect("failed to execute process");

                let expected: Vec<PathBuf> = output
                    .stdout
                    .split(|b| *b == b'\n')
                    .map(|slice| PathBuf::from(OsStr::from_bytes(slice)))
                    .collect();

                assert_eq!(
                    UnitSearchPathsBuilder::get_user_unit_paths().dirs(),
                    &expected
                );
            }
        }
    }

    mod unit_search_paths_iterator {
        use super::*;
    }
}
