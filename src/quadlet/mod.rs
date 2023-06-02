mod constants;
pub(crate) mod convert;
pub(crate) mod logger;
mod path_buf_ext;
pub(crate) mod podman_command;

use self::logger::*;
use crate::systemd_unit;
use crate::systemd_unit::SystemdUnit;

pub(crate) use self::constants::*;
pub(crate) use self::path_buf_ext::*;

use std::env;
use std::fmt::Display;
use std::io;

#[derive(Debug)]
#[non_exhaustive]
pub(crate) enum ConversionError {
    InvalidDeviceOptions(String),
    InvalidDeviceType(String),
    InvalidImageOrRootfs(String),
    InvalidKillMode(String),
    InvalidPortFormat(String),
    InvalidPublishedPort(String),
    InvalidRemapUsers(String),
    InvalidServiceType(String),
    InvalidSubnet(String),
    InvalidTmpfs(String),
    Io(io::Error),
    Parsing(systemd_unit::Error),
    UnknownKey(String),
    YamlMissing(String),
}

impl Display for ConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ConversionError::InvalidDeviceOptions(msg)
            | ConversionError::InvalidDeviceType(msg)
            | ConversionError::InvalidImageOrRootfs(msg)
            | ConversionError::InvalidKillMode(msg)
            | ConversionError::InvalidPortFormat(msg)
            | ConversionError::InvalidPublishedPort(msg)
            | ConversionError::InvalidRemapUsers(msg)
            | ConversionError::InvalidServiceType(msg)
            | ConversionError::InvalidSubnet(msg)
            | ConversionError::InvalidTmpfs(msg)
            | ConversionError::UnknownKey(msg)
            | ConversionError::YamlMissing(msg) => {
                write!(f, "{msg}")
            }
            ConversionError::Io(e) => e.fmt(f),
            ConversionError::Parsing(e) => {
                write!(f, "Failed parsing unit file: {e}")
            }
        }
    }
}

impl From<io::Error> for ConversionError {
    fn from(e: io::Error) -> Self {
        ConversionError::Io(e)
    }
}

impl From<systemd_unit::Error> for ConversionError {
    fn from(e: systemd_unit::Error) -> Self {
        ConversionError::Parsing(e)
    }
}

pub(crate) fn check_for_unknown_keys(
    unit: &SystemdUnit,
    group_name: &str,
    supported_keys: &[&str],
) -> Result<(), ConversionError> {
    for (key, _) in unit.section_entries(group_name) {
        if !supported_keys.contains(&key) {
            return Err(ConversionError::UnknownKey(format!(
                "unsupported key '{key}' in group '{group_name}' in {:?}",
                unit.path()
            )));
        }
    }

    Ok(())
}

pub fn get_podman_binary() -> String {
    env::var("PODMAN").unwrap_or(DEFAULT_PODMAN_BINARY.to_owned())
}

fn is_image_id(image_name: &str) -> bool {
    // All sha25:... names are assumed by podman to be fully specified
    if image_name.starts_with("sha256:") {
        return true;
    }

    // However, podman also accepts image ids as pure hex strings,
    // but only those of length 64 are unambiguous image ids
    if image_name.len() != 64 {
        return false;
    }
    if image_name.chars().any(|c| !c.is_ascii_hexdigit()) {
        return false;
    }

    true
}

fn is_unambiguous_name(image_name: &str) -> bool {
    // Fully specified image ids are unambiguous
    if is_image_id(image_name) {
        return true;
    }

    // Otherwise we require a fully qualified name

    // What is before the first slash can be a domain or a path
    if let Some((domain, _)) = image_name.split_once('/') {
        // If its a domain (has dot or port or is "localhost") it is considered fq
        if domain.contains(['.', ':']) || domain == "localhost" {
            return true;
        }
    } else {
        // No domain or path, not fully qualified
        return false;
    }

    false
}

// warns if input is an ambiguous name, i.e. a partial image id or a short
// name (i.e. is missing a registry)
//
// Examples:
//   - short names: "image:tag", "library/fedora"
//   - fully qualified names: "quay.io/image", "localhost/image:tag",
//     "server.org:5000/lib/image", "sha256:..."
//
// We implement a simple version of this from scratch here to avoid
// a huge dependency in the generator just for a warning.
pub(crate) fn warn_if_ambiguous_image_name(container: &SystemdUnit) {
    if let Some(image_name) = container.lookup_last(CONTAINER_SECTION, "Image") {
        if !is_unambiguous_name(image_name) {
            let file_name = container.path().unwrap().file_name().unwrap();
            log!("Warning: {file_name:?} specifies the image {image_name:?} which not a fully qualified image name. This is not ideal for performance and security reasons. See the podman-pull manpage discussion of short-name-aliases.conf for details.");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod is_unambiguous_name {
        use super::*;

        #[test]
        fn with_ambiguous_names() {
            let inputs = vec![
                "fedora",
                "fedora:latest",
                "library/fedora",
                "library/fedora:latest",
                "busybox@sha256:d366a4665ab44f0648d7a00ae3fae139d55e32f9712c67accd604bb55df9d05a",
                "busybox:latest@sha256:d366a4665ab44f0648d7a00ae3fae139d55e32f9712c67accd604bb55df9d05a",
                "d366a4665ab44f0648d7a00ae3fae139d55e32f9712c67accd604bb55df9d05",
                "d366a4665ab44f0648d7a00ae3fae139d55e32f9712c67accd604bb55df9d05aa",
            ];

            for input in inputs {
                assert!(!is_unambiguous_name(input), "{input}");
            }
        }

        #[test]
        fn with_unambiguous_names() {
            let inputs = vec![
                "quay.io/fedora",
                "docker.io/fedora",
                "docker.io/library/fedora:latest",
                "localhost/fedora",
                "localhost:5000/fedora:latest",
                "example.foo.this.may.be.garbage.but.maybe.not:1234/fedora:latest",
                "docker.io/library/busybox@sha256:d366a4665ab44f0648d7a00ae3fae139d55e32f9712c67accd604bb55df9d05a",
                "docker.io/library/busybox:latest@sha256:d366a4665ab44f0648d7a00ae3fae139d55e32f9712c67accd604bb55df9d05a",
                "docker.io/fedora@sha256:d366a4665ab44f0648d7a00ae3fae139d55e32f9712c67accd604bb55df9d05a",
                "sha256:d366a4665ab44f0648d7a00ae3fae139d55e32f9712c67accd604bb55df9d05a",
                "d366a4665ab44f0648d7a00ae3fae139d55e32f9712c67accd604bb55df9d05a",
            ];

            for input in inputs {
                assert!(is_unambiguous_name(input), "{input}");
            }
        }
    }
}
