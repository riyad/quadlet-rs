use std::env;
use std::os::unix::prelude::OsStrExt;
use std::path::{Path, PathBuf};

use crate::systemd_unit::SystemdUnitFile;

pub trait PathBufExt<T> {
    fn absolute_from(&self, new_root: &Path) -> T;
    fn absolute_from_unit(&self, unit_file: &SystemdUnitFile) -> T;
    fn cleaned(&self) -> T;
    fn file_name_template_parts(&self) -> (Option<&str>, Option<&str>);
    fn starts_with_systemd_specifier(&self) -> bool;
    fn to_str(&self) -> &str;
}

impl PathBufExt<PathBuf> for PathBuf {
    fn absolute_from(&self, new_root: &Path) -> PathBuf {
        // When the path starts with a Systemd specifier do not resolve what looks like a relative address
        if !self.starts_with_systemd_specifier() && !self.is_absolute() {
            if !new_root.as_os_str().is_empty() {
                return new_root.join(self).cleaned();
            } else {
                return env::current_dir()
                    .expect("current directory")
                    .join(self)
                    .cleaned();
            }
        }

        self.cleaned()
    }

    fn absolute_from_unit(&self, unit_file: &SystemdUnitFile) -> Self {
        let current_dir = env::current_dir().expect("current dir");
        let unit_file_dir = unit_file.path().parent().unwrap_or(current_dir.as_path());

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

    /// splits the file name into Systemd template unit parts
    /// e.g. `"foo/template@instance.service"` would become `(Some("template"), Some("instance"))`
    fn file_name_template_parts(&self) -> (Option<&str>, Option<&str>) {
        let mut parts = self
            .file_stem()
            .unwrap_or_default()
            .to_str()
            .expect("path is not a valid UTF-8 string")
            .splitn(2, '@');

        // there's always a first part
        let template_base = parts.next().unwrap_or_default();
        let template_instance = parts.next();

        // '@' found
        if let Some(template_instance) = template_instance {
            if template_base.is_empty() {
                return (None, None);
            }

            if template_instance.is_empty() {
                (Some(template_base), None)
            } else {
                (Some(template_base), Some(template_instance.into()))
            }
        } else {
            (None, None)
        }
    }

    /// Systemd Specifiers start with % with the exception of %%
    fn starts_with_systemd_specifier(&self) -> bool {
        if self.as_os_str().len() <= 1 {
            return false;
        }
        // self has length of at least 2

        // if first component has length of 2, starts with %, but is not %%
        if self.components().next().unwrap().as_os_str().len() == 2 {
            if self.as_os_str().as_bytes().starts_with("%%".as_bytes()) {
                return false;
            } else if self.as_os_str().as_bytes().starts_with("%".as_bytes()) {
                return true;
            }
        }

        false
    }

    fn to_str(&self) -> &str {
        (self as &Path)
            .to_str()
            .expect("path is not a valid UTF-8 string")
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

            let unit = SystemdUnitFile::new();

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

            let mut unit = SystemdUnitFile::new();
            unit.path = PathBuf::from("");

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

            let mut unit = SystemdUnitFile::new();
            unit.path = PathBuf::from("z.service");

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

            let mut unit = SystemdUnitFile::new();
            unit.path = PathBuf::from("/x/y/z.service");

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

            let mut unit = SystemdUnitFile::new();
            unit.path = PathBuf::from("x/y/z.service");

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

    mod file_name_template_parts {
        use super::*;

        #[test]
        fn with_default_path() {
            let path = PathBuf::default();

            let (template_base, template_instance) = path.file_name_template_parts();

            assert_eq!(template_base, None);
            assert_eq!(template_instance, None);
        }

        #[test]
        fn with_simple_path() {
            let path = PathBuf::from("foo/simple-service_name.service");

            let (template_base, template_instance) = path.file_name_template_parts();

            assert_eq!(template_base, None);
            assert_eq!(template_instance, None);
        }

        #[test]
        fn with_base_template_path() {
            let path = PathBuf::from("foo/simple-base_template@.service");

            let (template_base, template_instance) = path.file_name_template_parts();

            assert_eq!(template_base, Some("simple-base_template"));
            assert_eq!(template_instance, None);
        }

        #[test]
        fn with_instance_template_path() {
            let path = PathBuf::from("foo/simple-base_template@some-instance_foo.service");

            let (template_base, template_instance) = path.file_name_template_parts();

            assert_eq!(template_base, Some("simple-base_template"));
            assert_eq!(template_instance, Some("some-instance_foo"));
        }

        #[test]
        fn must_have_a_base_template_path() {
            let path = PathBuf::from("foo/@broken-instance_template.service");

            let (template_base, template_instance) = path.file_name_template_parts();

            assert_eq!(template_base, None);
            assert_eq!(template_instance, None);
        }
    }

    mod starts_with_systemd_specifier {
        use super::*;

        #[test]
        fn test_cases() {
            let inputs = vec![
                ("", false),
                ("/", false),
                ("%", false),
                ("%%", false),
                ("%h", true),
                ("%%/", false),
                ("%t/todo.txt", true),
                ("%abc/todo.txt", false),
                ("/foo/bar/baz.js", false),
                ("../todo.txt", false),
            ];

            for input in inputs {
                let path = PathBuf::from(input.0);
                assert_eq!(path.starts_with_systemd_specifier(), input.1, "{path:?}");
            }
        }
    }
}
