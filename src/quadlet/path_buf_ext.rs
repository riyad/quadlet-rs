use std::env;
use std::path::{PathBuf, Path};

use crate::systemd_unit::SystemdUnit;

pub(crate) trait PathBufExt<T> {
    fn absolute_from(&self, new_root: &Path) -> T;
    fn absolute_from_unit(&self, unit_file: &SystemdUnit) -> T;
    fn cleaned(&self) -> T;
}

impl PathBufExt<PathBuf> for PathBuf {
    fn absolute_from(&self, new_root: &Path) -> PathBuf {
        if !self.is_absolute() {
            if !new_root.as_os_str().is_empty() {
                return new_root.join(self).cleaned()
            } else {
                return env::current_dir().expect("current directory").join(self).cleaned()
            }
        }

        self.cleaned()
    }

    fn absolute_from_unit(&self, unit_file: &SystemdUnit) -> Self {
        let current_dir = env::current_dir().expect("current dir");
        let unit_file_dir = unit_file.path()
            .map_or_else(|| current_dir.as_path(), |p| p.parent().unwrap_or(current_dir.as_path()));

        self.absolute_from(unit_file_dir)
    }

    /// This function normalizes relative the paths by dropping multiple slashes,
    /// removing "." elements and making ".." drop the parent element as long
    /// as there is not (otherwise the .. is just removed).
    /// Symlinks are not handled in any way.
    /// TODO: we could use std::path::absolute() here, but it's nightly-only ATM
    /// see https://doc.rust-lang.org/std/path/fn.absolute.html
    fn cleaned(&self) -> PathBuf {
        // normalized path could be shorter, but never longer
        let mut normalized = PathBuf::with_capacity(self.as_os_str().len());

        for element in self.components() {
            if element.as_os_str().is_empty() || element.as_os_str() == "." {
                continue;
            } else if element.as_os_str() == ".." {
                if normalized.components().count() > 0 {
                    normalized.pop();
                } else {
                    normalized.push(element);
                }
            } else {
                normalized.push(element);
            }
        }

        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod absolute_from {
        use super::*;

        #[test]
        fn with_absolute_target_path() {
            let target_path = PathBuf::from("/x/y/z");
            let inputs = vec![
                ("", "/x/y/z"),
                ("/", "/"),
                (".", "/x/y/z"),
                ("..", "/x/y"),
                ("/foo/bar/baz.js", "/foo/bar/baz.js"),
                ("/foo/bar/baz", "/foo/bar/baz"),
                ("/foo/bar/baz/", "/foo/bar/baz/"),
                ("/dirty//path///", "/dirty//path///"),
                ("dev.txt", "/x/y/z/dev.txt"),
                ("../todo.txt", "/x/y/todo.txt"),
                ("/a/b/c", "/a/b/c"),
                ("/b/c", "/b/c"),
                ("./b/c", "/x/y/z/b/c"),
            ];

            for input in inputs {
                let base_path = PathBuf::from(input.0);
                let expected = PathBuf::from(input.1);
                assert_eq!(
                    base_path.absolute_from(target_path.as_path()),
                    expected,
                    "{input:?}"
                );
            }
        }

        #[test]
        fn with_empty_target_path() {
            let empty_path = PathBuf::from("");
            let current_dir = env::current_dir().expect("current dir");
            let inputs = vec![
                (""),
                ("/"),
                ("."),
                (".."),
                ("/foo/bar/baz.js"),
                ("/foo/bar/baz"),
                ("/foo/bar/baz/"),
                ("/dirty//path///"),
                ("dev.txt"),
                ("../todo.txt"),
                ("/a/b/c"),
                ("/b/c"),
                ("./b/c"),
            ];

            for input in inputs {
                let base_path = PathBuf::from(input);
                assert_eq!(
                    base_path.absolute_from(empty_path.as_path()),
                    base_path.absolute_from(current_dir.as_path()),
                    "{input:?}"
                );
            }
        }

        #[test]
        fn with_relative_target_path() {
            let target_path = PathBuf::from("x/y/z");
            let inputs = vec![
                ("", "x/y/z"),
                ("/", "/"),
                (".", "x/y/z"),
                ("..", "x/y"),
                ("/foo/bar/baz.js", "/foo/bar/baz.js"),
                ("/foo/bar/baz", "/foo/bar/baz"),
                ("/foo/bar/baz/", "/foo/bar/baz/"),
                ("/dirty//path///", "/dirty//path///"),
                ("dev.txt", "x/y/z/dev.txt"),
                ("../todo.txt", "x/y/todo.txt"),
                ("/a/b/c", "/a/b/c"),
                ("/b/c", "/b/c"),
                ("./b/c", "x/y/z/b/c"),
            ];

            for input in inputs {
                let base_path = PathBuf::from(input.0);
                let expected = PathBuf::from(input.1);
                assert_eq!(
                    base_path.absolute_from(target_path.as_path()),
                    expected,
                    "{input:?}"
                );
            }
        }
    }

    mod absolute_from_unit {
        use super::*;

        #[test]
        fn with_no_path_targets_current_dir() {
            let inputs = vec![
                (""),
                ("/"),
                ("."),
                (".."),
                ("/foo/bar/baz.js"),
                ("/foo/bar/baz"),
                ("/foo/bar/baz/"),
                ("/dirty//path///"),
                ("dev.txt"),
                ("../todo.txt"),
                ("/a/b/c"),
                ("/b/c"),
                ("./b/c"),
            ];
            let target_path = env::current_dir().expect("current dir");

            let mut unit = SystemdUnit::new();
            unit.path = None;

            for input in inputs {
                let base_path = PathBuf::from(input);
                assert_eq!(
                    base_path.absolute_from_unit(&unit),
                    base_path.absolute_from(target_path.as_path()),
                    "{input:?}"
                );
            }
        }

        #[test]
        fn with_empty_path_targets_current_dir() {
            let inputs = vec![
                (""),
                ("/"),
                ("."),
                (".."),
                ("/foo/bar/baz.js"),
                ("/foo/bar/baz"),
                ("/foo/bar/baz/"),
                ("/dirty//path///"),
                ("dev.txt"),
                ("../todo.txt"),
                ("/a/b/c"),
                ("/b/c"),
                ("./b/c"),
            ];
            let target_path = env::current_dir().expect("current dir");

            let mut unit = SystemdUnit::new();
            unit.path = Some(PathBuf::from(""));

            for input in inputs {
                let base_path = PathBuf::from(input);
                assert_eq!(
                    base_path.absolute_from_unit(&unit),
                    base_path.absolute_from(target_path.as_path()),
                    "{input:?}"
                );
            }
        }

        #[test]
        fn with_only_file_name_targets_current_dir() {
            let inputs = vec![
                (""),
                ("/"),
                ("."),
                (".."),
                ("/foo/bar/baz.js"),
                ("/foo/bar/baz"),
                ("/foo/bar/baz/"),
                ("/dirty//path///"),
                ("dev.txt"),
                ("../todo.txt"),
                ("/a/b/c"),
                ("/b/c"),
                ("./b/c"),
            ];
            let target_path = env::current_dir().expect("current dir");

            let mut unit = SystemdUnit::new();
            unit.path = Some(PathBuf::from("z.service"));

            for input in inputs {
                let base_path = PathBuf::from(input);
                assert_eq!(
                    base_path.absolute_from_unit(&unit),
                    base_path.absolute_from(target_path.as_path()),
                    "{input:?}"
                );
            }
        }

        #[test]
        fn with_absolute_path_targets_parent() {
            let inputs = vec![
                (""),
                ("/"),
                ("."),
                (".."),
                ("/foo/bar/baz.js"),
                ("/foo/bar/baz"),
                ("/foo/bar/baz/"),
                ("/dirty//path///"),
                ("dev.txt"),
                ("../todo.txt"),
                ("/a/b/c"),
                ("/b/c"),
                ("./b/c"),
            ];
            let target_path = PathBuf::from("/x/y");

            let mut unit = SystemdUnit::new();
            unit.path = Some(PathBuf::from("/x/y/z.service"));

            for input in inputs {
                let base_path = PathBuf::from(input);
                assert_eq!(
                    base_path.absolute_from_unit(&unit),
                    base_path.absolute_from(target_path.as_path()),
                    "{input:?}"
                );
            }
        }

        #[test]
        fn with_relative_path_targets_parent() {
            let inputs = vec![
                (""),
                ("/"),
                ("."),
                (".."),
                ("/foo/bar/baz.js"),
                ("/foo/bar/baz"),
                ("/foo/bar/baz/"),
                ("/dirty//path///"),
                ("dev.txt"),
                ("../todo.txt"),
                ("/a/b/c"),
                ("/b/c"),
                ("./b/c"),
            ];
            let target_path = PathBuf::from("x/y");

            let mut unit = SystemdUnit::new();
            unit.path = Some(PathBuf::from("x/y/z.service"));

            for input in inputs {
                let base_path = PathBuf::from(input);
                assert_eq!(
                    base_path.absolute_from_unit(&unit),
                    base_path.absolute_from(target_path.as_path()),
                    "{input:?}"
                );
            }
        }
    }

    mod cleaned {
        use super::*;

        #[test]
        fn test_cases() {
            let inputs = vec![
                ("", ""),
                ("/", "/"),
                (".", ""),
                ("..", ".."),
                ("/foo/bar/baz.js", "/foo/bar/baz.js"),
                ("/foo/bar/baz", "/foo/bar/baz"),
                ("/foo/bar/baz/", "/foo/bar/baz/"),
                ("/dirty//path///", "/dirty//path///"),
                ("dev.txt", "dev.txt"),
                ("../todo.txt", "../todo.txt"),
                ("a/b/../../../xyz", "../xyz"),
                ("/a/b/../../../xyz", "/xyz"),
                ("a/./b/.././../../xyz", "../xyz"),
            ];

            for input in inputs {
                let base_path = PathBuf::from(input.0);
                let expected = PathBuf::from(input.1);
                assert_eq!(
                    base_path.cleaned(),
                    PathBuf::from(expected),
                    "{base_path:?}"
                );
            }
        }

        // TODO: test cases from https://pkg.go.dev/path/filepath#Dir
        // TODO: test cases from https://pkg.go.dev/path/filepath#Clean

    }
}