mod constants;
pub(crate) mod convert;
pub mod iterators;
pub(crate) mod logger;
pub(crate) mod podman_command;

use convert::quad_replace_extension;
use log::warn;
use regex_lite::Regex;

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
    #[error("internal error while processing {0} {1:?}")]
    InternalQuadletError(String, OsString),
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
pub(crate) struct UnitInfo {
    // The name of the generated systemd service unit
    pub(crate) service_name: String,
    // The name of the podman resource created by the service
    pub(crate) resource_name: String,

    // For .pod units
    // List of containers in a pod
    pub(crate) containers: Vec<PathBuf>,
}

impl UnitInfo {
    pub(crate) fn get_service_file_name(&self) -> OsString {
        PathBuf::from(format!("{}.service", self.service_name))
            .file_name()
            .expect("should have a file name")
            .to_os_string()
    }
}

#[derive(Debug, Default)]
pub(crate) struct UnitsInfoMap(HashMap<OsString, UnitInfo>);

impl UnitsInfoMap {
    pub(crate) fn from_units(units: &Vec<SystemdUnitFile>) -> UnitsInfoMap {
        let mut units_info_map = UnitsInfoMap::default();

        for unit in units {
            let mut unit_info = UnitInfo::default();

            match unit
                .path
                .extension()
                .unwrap_or(OsString::from("").as_os_str())
                .to_str()
                .expect("unit path is not a valid UTF-8 string")
            {
                "container" => {
                    unit_info.service_name = get_container_service_name(unit)
                        .to_str()
                        .expect("service name is not a valid UTF-8 string")
                        .into();
                }
                "volume" => {
                    unit_info.service_name = get_volume_service_name(unit)
                        .to_str()
                        .expect("service name is not a valid UTF-8 string")
                        .into();
                }
                "kube" => {
                    unit_info.service_name = get_kube_service_name(unit)
                        .to_str()
                        .expect("service name is not a valid UTF-8 string")
                        .into();
                }
                "network" => {
                    unit_info.service_name = get_network_service_name(unit)
                        .to_str()
                        .expect("service name is not a valid UTF-8 string")
                        .into();
                }
                "image" => {
                    unit_info.service_name = get_image_service_name(unit)
                        .to_str()
                        .expect("service name is not a valid UTF-8 string")
                        .into();
                }
                "build" => {
                    unit_info.service_name = get_build_service_name(unit)
                        .to_str()
                        .expect("service name is not a valid UTF-8 string")
                        .into();

                    // Prefill `resouce_name`s for .build files. This is significantly less complex than
                    // pre-computing all `resource_name`s for all Quadlet types (which is rather complex for a few
                    // types), but still breaks the dependency cycle between .volume and .build ([Volume] can
                    // have Image=some.build, and [Build] can have Volume=some.volume:/some-volume)
                    unit_info.resource_name = get_built_image_name(unit).unwrap_or_default();
                }
                "pod" => {
                    unit_info.service_name = get_pod_service_name(unit)
                        .to_str()
                        .expect("service name is not a valid UTF-8 string")
                        .into();
                }
                _ => {
                    warn!("Unsupported file type {:?}", unit.file_name());
                    continue;
                }
            }

            units_info_map
                .0
                .insert(unit.file_name().to_os_string(), unit_info);
        }

        units_info_map
    }
}

fn get_build_service_name(build: &SystemdUnitFile) -> PathBuf {
    get_quadlet_service_name(build, BUILD_SECTION, "-build")
}

fn get_built_image_name(build: &SystemdUnitFile) -> Option<String> {
    build
        .lookup_all(BUILD_SECTION, "ImageTag")
        .iter()
        .filter_map(|s| {
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        })
        .next()
}

fn get_container_service_name(container: &SystemdUnitFile) -> PathBuf {
    get_quadlet_service_name(container, CONTAINER_SECTION, "")
}

fn get_image_service_name(image: &SystemdUnitFile) -> PathBuf {
    get_quadlet_service_name(image, IMAGE_SECTION, "-image")
}

fn get_kube_service_name(kube: &SystemdUnitFile) -> PathBuf {
    get_quadlet_service_name(kube, KUBE_SECTION, "")
}

fn get_network_service_name(network: &SystemdUnitFile) -> PathBuf {
    get_quadlet_service_name(network, NETWORK_SECTION, "-network")
}

fn get_pod_service_name(pod: &SystemdUnitFile) -> PathBuf {
    get_quadlet_service_name(pod, POD_SECTION, "-pod")
}

fn get_quadlet_service_name(unit: &SystemdUnitFile, section: &str, name_suffix: &str) -> PathBuf {
    if let Some(service_name) = unit.lookup(section, "ServiceName") {
        return PathBuf::from(service_name);
    }

    quad_replace_extension(unit.path(), "", "", name_suffix)
}

fn get_volume_service_name(volume: &SystemdUnitFile) -> PathBuf {
    get_quadlet_service_name(volume, VOLUME_SECTION, "-volume")
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

fn is_url(maybe_url: &str) -> bool {
    // this is a shortcut to keep binary size small, we don't need a full URL parser here
    let re = Regex::new("^((https?)|(git)://)|(github\\.com/).+$").unwrap();
    re.is_match(maybe_url)
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
        if !is_unambiguous_name(&image_name) {
            let file_name = unit.file_name();
            warn!("{file_name:?} specifies the image {image_name:?} which not a fully qualified image name. This is not ideal for performance and security reasons. See the podman-pull manpage discussion of short-name-aliases.conf for details.");
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

    mod is_url {
        use super::*;

        #[test]
        fn fails_with_empty_string() {
            assert!(!is_url(""))
        }

        #[test]
        fn fails_with_basic_string() {
            assert!(!is_url("some-string"))
        }

        #[test]
        fn fails_with_relative_path() {
            assert!(!is_url("dir/file"))
        }

        #[test]
        fn fails_with_absolute_path() {
            assert!(!is_url("/dir/file"))
        }

        #[test]
        fn succeeds_with_just_domain() {
            assert!(is_url("http://foo.tld/"))
        }

        #[test]
        fn succeeds_with_domain_and_path() {
            assert!(is_url("http://foo.tld/bar/baz"))
        }

        #[test]
        fn succeeds_with_github_url() {
            assert!(is_url("https://github.com/riyad/quadlet-rs"))
        }

        #[test]
        fn succeeds_with_git_schema() {
            assert!(is_url("git://github.com/riyad/quadlet-rs"))
        }
    }
}
