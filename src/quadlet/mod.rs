mod constants;
pub(crate) mod convert;
pub mod iterators;
pub(crate) mod logger;
pub(crate) mod podman_command;

use convert::quad_replace_extension;
use log::debug;
use log::warn;
use regex_lite::Regex;

use crate::systemd_unit;
use crate::systemd_unit::PathExt;
use crate::systemd_unit::SystemdUnitFile;

pub(crate) use self::constants::*;
pub(crate) use self::iterators::*;

use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub(crate) enum RuntimeError {
    #[error("Missing output directory argument")]
    CliMissingOutputDirectory(crate::CliOptions),
    #[error("{0}: {1}")]
    Io(String, #[source] io::Error),
    #[error("{0}: {1}")]
    Conversion(String, #[source] ConversionError),
    #[error("unsupported file type {0:?}")]
    UnsupportedQuadletType(PathBuf),
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum ConversionError {
    #[error("requested Quadlet image {0:?} was not found")]
    ImageNotFound(String),
    #[error("internal error while processing {0} {1:?}")]
    InternalQuadletError(String, OsString),
    #[error("unable to translate dependency for {0:?}")]
    InvalidUnitDependency(String),
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
    #[error("extra options are not supported when joining another container's network")]
    InvalidNetworkOptions,
    #[error("pod {0:?} is not Quadlet based")]
    InvalidPod(String),
    #[error("invalid port format {0:?}")]
    InvalidPortFormat(String),
    #[error("relative path in File key requires SetWorkingDirectory key to be set")]
    InvalidRelativeFile,
    #[error("{0}")]
    InvalidRemapUsers(String),
    #[error("cannot get the resource name of {0}")]
    InvalidResourceNameIn(String),
    #[error("invalid service Type {0:?}")]
    InvalidServiceType(String),
    #[error("SetWorkingDirectory={0:?} is only supported in .{1} files")]
    InvalidSetWorkingDirectory(String, String),
    #[error("{0}")]
    InvalidSubnet(String),
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0} and {1} are mutually exclusive, but both are set")]
    MutuallyExclusiveKeys(String, String),
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
    #[error("requested Quadlet source {0:?} was not found")]
    SourceNotFound(String),
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

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub(crate) enum QuadletType {
    Build,
    Container,
    Image,
    Kube,
    Network,
    Pod,
    Volume,
}

impl QuadletType {
    pub(crate) fn from_path(path: &Path) -> Result<QuadletType, RuntimeError> {
        match path
            .extension()
            .map(|e| e.to_str().unwrap_or_default())
            .unwrap_or_default()
        {
            "build" => Ok(QuadletType::Build),
            "container" => Ok(QuadletType::Container),
            "image" => Ok(QuadletType::Image),
            "kube" => Ok(QuadletType::Kube),
            "network" => Ok(QuadletType::Network),
            "pod" => Ok(QuadletType::Pod),
            "volume" => Ok(QuadletType::Volume),
            _ => Err(RuntimeError::UnsupportedQuadletType(path.to_path_buf())),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct QuadletSourceUnitFile {
    pub(crate) unit_file: SystemdUnitFile,
    pub(crate) quadlet_type: QuadletType,

    // The name of the generated systemd service unit
    pub(crate) service_name: String,
    // The name of the podman resource created by the service
    pub(crate) resource_name: String,

    // For .pod units
    // List of containers to start with the pod
    pub(crate) containers_to_start: Vec<PathBuf>,
}

impl QuadletSourceUnitFile {
    pub(crate) fn from_unit_file(
        unit_file: SystemdUnitFile,
    ) -> Result<QuadletSourceUnitFile, RuntimeError> {
        let quadlet_type = QuadletType::from_path(unit_file.path())?;
        let service_name = match quadlet_type {
            QuadletType::Container => get_container_service_name(&unit_file)
                .to_unwrapped_str()
                .to_owned(),
            QuadletType::Volume => get_volume_service_name(&unit_file)
                .to_unwrapped_str()
                .to_owned(),
            QuadletType::Kube => get_kube_service_name(&unit_file)
                .to_unwrapped_str()
                .to_owned(),
            QuadletType::Network => get_network_service_name(&unit_file)
                .to_unwrapped_str()
                .to_owned(),
            QuadletType::Image => get_image_service_name(&unit_file)
                .to_unwrapped_str()
                .to_owned(),
            QuadletType::Build => get_build_service_name(&unit_file)
                .to_unwrapped_str()
                .to_owned(),
            QuadletType::Pod => get_pod_service_name(&unit_file)
                .to_unwrapped_str()
                .to_owned(),
        };
        let resource_name = match quadlet_type {
            QuadletType::Build => {
                // Prefill `resouce_name`s for .build files. This is significantly less complex than
                // pre-computing all `resource_name`s for all Quadlet types (which is rather complex for a few
                // types), but still breaks the dependency cycle between .volume and .build ([Volume] can
                // have Image=some.build, and [Build] can have Volume=some.volume:/some-volume)
                get_built_image_name(&unit_file).unwrap_or_default()
            }
            QuadletType::Container => {
                // Prefill resouceNames for .container files. This solves network reusing.
                get_container_resource_name(&unit_file)
            }
            _ => String::default(),
        };

        Ok(QuadletSourceUnitFile {
            unit_file,
            service_name,
            resource_name,
            quadlet_type,
            containers_to_start: Vec::default(),
        })
    }

    pub(crate) fn get_service_file_name(&self) -> OsString {
        PathBuf::from(format!("{}.service", self.service_name))
            .file_name()
            .expect("should have a file name")
            .to_os_string()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct QuadletServiceUnitFile<'q> {
    pub(crate) quadlet: &'q QuadletSourceUnitFile,

    // The generated systemd service unit
    pub(crate) service_file: SystemdUnitFile,
}

impl QuadletServiceUnitFile<'_> {
    pub(crate) fn generate_service_file(&self) -> io::Result<()> {
        let out_filename = self.service_file.path();

        debug!("Writing {out_filename:?}");

        let out_file = File::create(out_filename)?;
        let mut writer = BufWriter::new(out_file);

        let args_0 = env::args().next().unwrap();
        writeln!(writer, "# Automatically generated by {args_0}")?;

        self.service_file.write_to(&mut writer)?;

        Ok(())
    }
}

#[derive(Debug, Default)]
pub(crate) struct UnitsInfoMap(HashMap<OsString, QuadletSourceUnitFile>);

impl UnitsInfoMap {
    pub(crate) fn from_quadlet_units(quadlet_units: Vec<QuadletSourceUnitFile>) -> UnitsInfoMap {
        let mut units_info_map = UnitsInfoMap::default();

        for quadlet in quadlet_units {
            units_info_map
                .0
                .insert(quadlet.unit_file.file_name().to_os_string(), quadlet);
        }

        units_info_map
    }

    pub(crate) fn get_source_unit_info(
        &self,
        quadlet: &SystemdUnitFile,
    ) -> Option<&QuadletSourceUnitFile> {
        self.0.get(quadlet.file_name())
    }

    pub(crate) fn get_mut_source_unit_info(
        &mut self,
        quadlet: &SystemdUnitFile,
    ) -> Option<&mut QuadletSourceUnitFile> {
        self.0.get_mut(quadlet.file_name())
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

// Get the unresolved container name that may contain '%'.
fn get_container_name(container: &SystemdUnitFile) -> String {
    if let Some(container_name) = container.lookup(CONTAINER_SECTION, "ContainerName") {
        container_name
    } else {
        // By default, We want to name the container by the service name
        if container.is_template_unit() {
            "systemd-%p_%i"
        } else {
            "systemd-%N"
        }
        .to_string()
    }
}

// Get the resolved container name that contains no '%'.
// Returns an empty string if not resolvable.
fn get_container_resource_name(container: &SystemdUnitFile) -> String {
    let container_name = get_container_name(container);

    // XXX: only %N is handled.
    // it is difficult to properly implement specifiers handling without consulting systemd.
    let resource_name = container_name.replace(
        "%N",
        get_container_service_name(container).to_unwrapped_str(),
    );

    if !resource_name.contains("%") {
        resource_name
    } else {
        String::default()
    }
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

    quad_replace_extension(
        Path::new(unit.path().file_name().unwrap()),
        "",
        "",
        name_suffix,
    )
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

    mod get_quadlet_service_name {
        use super::*;

        #[test]
        fn looks_up_service_name_from_unit() {
            let mut unit_file = SystemdUnitFile::new();
            let section = "Foo";
            unit_file.add(section, "ServiceName", "test-name");

            assert_eq!(
                get_quadlet_service_name(&unit_file, section, "-test"),
                PathBuf::from("test-name")
            )
        }

        #[test]
        fn use_only_file_name_for_service_name() {
            let path = PathBuf::from("/foo/bar/baz.buf");
            let mut unit_file = SystemdUnitFile::new();
            unit_file.path = path;

            assert_eq!(
                get_quadlet_service_name(&unit_file, "", "-test"),
                PathBuf::from("baz-test")
            )
        }
    }

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
