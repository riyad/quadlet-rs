use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

pub trait PathExt {
    fn file_name_template_parts(&self) -> (Option<&str>, Option<&str>);
    fn starts_with_systemd_specifier(&self) -> bool;
    fn systemd_unit_type(&self) -> &str;
    fn to_unwrapped_str(&self) -> &str;
}

impl PathExt for Path {
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
                (Some(template_base), Some(template_instance))
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
            if self.as_os_str().as_bytes().starts_with(b"%%") {
                return false;
            } else if self.as_os_str().as_bytes().starts_with(b"%") {
                return true;
            }
        }

        false
    }

    fn to_unwrapped_str(&self) -> &str {
        self.to_str().expect("path is not a valid UTF-8 string")
    }

    // TODO: make Option?
    fn systemd_unit_type(&self) -> &str {
        self.extension()
            .expect("should have an extension")
            .to_str()
            .expect("extension is not a valid UTF-8 string")
    }
}

impl PathExt for PathBuf {
    /// splits the file name into Systemd template unit parts
    /// e.g. `"foo/template@instance.service"` would become `(Some("template"), Some("instance"))`
    fn file_name_template_parts(&self) -> (Option<&str>, Option<&str>) {
        self.as_path().file_name_template_parts()
    }

    /// Systemd Specifiers start with % with the exception of %%
    fn starts_with_systemd_specifier(&self) -> bool {
        self.as_path().starts_with_systemd_specifier()
    }

    fn to_unwrapped_str(&self) -> &str {
        self.as_path().to_unwrapped_str()
    }

    fn systemd_unit_type(&self) -> &str {
        self.as_path().systemd_unit_type()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    mod systemd_unit_type {
        use super::*;

        #[test]
        fn test_systemd_unit_type() {
            let path = PathBuf::from("some/test_unit.timer");
            assert_eq!(path.systemd_unit_type(), "timer");
        }

        #[test]
        fn test_quadlet_unit_type() {
            let path = PathBuf::from("some/test_unit.pod");
            assert_eq!(path.systemd_unit_type(), "pod");
        }

        #[test]
        fn test_multiple_extensions() {
            let path = PathBuf::from("some/test_unit.custom.build");
            assert_eq!(path.systemd_unit_type(), "build");
        }

        #[test]
        #[ignore = "until support for drop-ins is added"]
        fn test_dropin() {
            let path = PathBuf::from("some/test_unit.pod.d/dropin.conf");
            assert_eq!(path.systemd_unit_type(), "pod");
        }
    }
}
