mod constants;
pub(crate) mod convert;
pub mod iterators;
pub(crate) mod logger;
pub(crate) mod podman_command;

use self::logger::*;
use crate::systemd_unit;
use crate::systemd_unit::SystemdUnitFile;

pub(crate) use self::constants::*;
pub(crate) use self::iterators::*;

use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub(crate) enum RuntimeError {
    #[error("Missing output directory argument")]
    CliMissingOutputDirectory(crate::CliOptions),
    #[error("{0}: {1}")]
    Io(String, #[source] io::Error),
    #[error("{0}: {1}")]
    Conversion(String, #[source] ConversionError),
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum ConversionError {
    #[error("requested Quadlet image {0:?} was not found")]
    ImageNotFound(String),
    #[error("internal error while processing pod {0:?}")]
    InternalPodError(String),
    #[error("key Options can't be used without Device")]
    InvalidDeviceOptions,
    #[error("key Type can't be used without Device")]
    InvalidDeviceType,
    #[error("invalid Group set without User")]
    InvalidGroup,
    #[error("{0}")]
    InvalidImageOrRootfs(String),
    #[error("invalid KillMode {0:?}")]
    InvalidKillMode(String),
    #[error("{0}")]
    InvalidMountCsv(#[from] csv::Error),
    #[error("incorrect mount format {0:?}: should be --mount type=<bind|glob|tmpfs|volume>,[src=<host-dir|volume-name>,]target=<ctr-dir>[,options]")]
    InvalidMountFormat(String),
    #[error("source parameter does not include a value")]
    InvalidMountSource,
    #[error("pod {0:?} is not Quadlet based")]
    InvalidPod(String),
    #[error("invalid port format {0:?}")]
    InvalidPortFormat(String),
    #[error("invalid published port {0:?}")]
    InvalidPublishedPort(String),
    #[error("relative path in File key requires SetWorkingDirectory key to be set")]
    InvalidRelativeFile,
    #[error("{0}")]
    InvalidRemapUsers(String),
    #[error("invalid service Type {0:?}")]
    InvalidServiceType(String),
    #[error("SetWorkingDirectory={0:?} is only supported in .{1} files")]
    InvalidSetWorkingDirectory(String, String),
    #[error("{0}")]
    InvalidSubnet(String),
    #[error("invalid tmpfs format {0:?}")]
    InvalidTmpfs(String),
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("no ImageTag key specified")]
    NoImageTagKeySpecified,
    #[error("no File key specified")]
    NoFileKeySpecified,
    #[error("neither SetWorkingDirectory, nor File key specified")]
    NoSetWorkingDirectoryNorFileKeySpecified,
    #[error("no Yaml key specified")]
    NoYamlKeySpecified,
    #[error("failed parsing unit file: {0}")]
    Parsing(#[from] systemd_unit::Error),
    #[error("Quadlet pod unit {0:?} does not exist")]
    PodNotFound(String),
    #[error("{0}")]
    UnknownKey(String),
    #[error("unsupported value for {0:?}: {1:?}")]
    UnsupportedValueForKey(String, String),
}

impl From<systemd_unit::IoError> for ConversionError {
    fn from(e: systemd_unit::IoError) -> Self {
        match e {
            systemd_unit::IoError::Io(e) => ConversionError::Io(e),
            systemd_unit::IoError::Unit(e) => ConversionError::Parsing(e),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct PodInfo {
    pub(crate) service_name: String,
    pub(crate) containers: Vec<PathBuf>,
}

#[derive(Debug, Default)]
pub(crate) struct PodsInfoMap(HashMap<PathBuf, PodInfo>);

impl PodsInfoMap {
    pub(crate) fn from_units(units: &Vec<SystemdUnitFile>) -> PodsInfoMap {
        let mut pods_info_map = PodsInfoMap::default();

        for unit in units {
            if let Some(ext) = unit.path.extension() {
                if ext != "pod" {
                    continue;
                }
            }

            let service_name = PodsInfoMap::_get_pod_service_name(unit);
            pods_info_map.0.insert(
                unit.path.clone(),
                PodInfo {
                    service_name: service_name
                        .to_str()
                        .expect("pod service name is not a valid UTF-8 string")
                        .to_string(),
                    containers: Default::default(),
                },
            );
        }

        pods_info_map
    }

    fn _get_pod_service_name(pod: &SystemdUnitFile) -> PathBuf {
        if let Some(service_name) = pod.lookup(POD_SECTION, "ServiceName") {
            service_name.into()
        } else {
            convert::quad_replace_extension(&pod.path, "", "", "-pod")
        }
    }
}

pub(crate) type ResourceNameMap = HashMap<OsString, OsString>;

pub(crate) fn check_for_unknown_keys(
    unit: &SystemdUnitFile,
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

fn get_built_image_name(built_unit: &SystemdUnitFile) -> Option<&str> {
    if let Some(built_image_name) = built_unit.lookup(BUILD_SECTION, "Image") {
        return Some(built_image_name);
    }

    None
}

pub fn get_podman_binary() -> String {
    env::var("PODMAN").unwrap_or(DEFAULT_PODMAN_BINARY.to_owned())
}

fn prefill_built_image_names(units: &Vec<SystemdUnitFile>, resource_names: &mut ResourceNameMap) {
    for unit in units {
        if !unit
            .file_name()
            .to_str()
            .unwrap_or_default()
            .ends_with(".build")
        {
            continue;
        }

        let image_name = get_built_image_name(unit);
        // imageName := quadlet.GetBuiltImageName(unit)
        // if len(imageName) > 0 {
        // 	resourceNames[unit.Filename] = imageName
        // }
    }
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

fn is_url(maybe_url: &str) -> bool {
    // FIXME: in its simplest form `^((https?)|(git)://)|(github\.com/).+$` would be enough
    unimplemented!()
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
pub(crate) fn warn_if_ambiguous_image_name(unit: &SystemdUnitFile, section: &str) {
    if let Some(image_name) = unit.lookup_last(section, "Image") {
        let unit_path_extension = unit.path().extension().unwrap_or_default();
        if unit_path_extension == "build" || unit_path_extension == "image" {
            return;
        }
        if !is_unambiguous_name(image_name) {
            let file_name = unit.file_name();
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
