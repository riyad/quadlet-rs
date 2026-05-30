use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

use crate::CliOptions;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("Missing output directory argument")]
    CliMissingOutputDirectory(CliOptions),
    #[error("{0}: {1}")]
    Io(String, #[source] io::Error),
    #[error("{0}: {1}")]
    Conversion(String, #[source] ConversionError),
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConversionError {
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
    #[error(
        "incorrect mount format {0:?}: should be Mount=type=<bind|glob|tmpfs|volume>,[src=<host-dir|volume-name>,]target=<ctr-dir>[,options]"
    )]
    InvalidMountFormat(String),
    #[error("source parameter is required for mount type {0}")]
    InvalidMountSourceRequiredForMountType(String),
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
    #[error("no Artifact key specified")]
    NoArtifactKeySpecified,
    #[error("no ImageTag key specified")]
    NoImageTagKeySpecified,
    #[error("no File key specified")]
    NoFileKeySpecified,
    #[error("neither SetWorkingDirectory, nor File key specified")]
    NoSetWorkingDirectoryNorFileKeySpecified,
    #[error("source cannot be empty")]
    NoMountSourceSpecified,
    #[error("no Yaml key specified")]
    NoYamlKeySpecified,
    #[error("failed parsing unit file: {0}")]
    Parsing(#[from] systemd_unit::Error),
    #[error("Quadlet pod unit {0:?} does not exist")]
    PodNotFound(String),
    #[error("requested Quadlet source {0:?} was not found")]
    SourceNotFound(String),
    #[error("SetWorkingDirectory=yaml is only supported when a single Yaml key is provided")]
    TooManyYamlKeysSpecified,
    #[error("{0}")]
    UnknownKey(String),
    #[error("unsupported file type {0:?}")]
    UnsupportedQuadletType(PathBuf),
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
