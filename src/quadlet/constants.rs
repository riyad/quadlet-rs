pub(crate) static DEFAULT_PODMAN_BINARY: &str = "/usr/bin/podman";

/// Directory for global Quadlet files (sysadmin owned)
pub static UNIT_DIR_ADMIN: &str = "/etc/containers/systemd";
/// Directory for global Quadlet files (distro owned)
pub static UNIT_DIR_DISTRO: &str = "/usr/share/containers/systemd";
/// Directory for temporary Quadlet files (sysadmin owned)
pub static UNIT_DIR_TEMP: &str = "/run/containers/systemd";

pub static BUILD_SECTION: &str      = "Build";
pub static CONTAINER_SECTION: &str  = "Container";
pub static IMAGE_SECTION: &str      = "Image";
pub static KUBE_SECTION: &str       = "Kube";
pub static NETWORK_SECTION: &str    = "Network";
pub static POD_SECTION: &str        = "Pod";
pub static QUADLET_SECTION: &str    = "Quadlet";
pub static VOLUME_SECTION: &str     = "Volume";
pub static X_BUILD_SECTION: &str    = "X-Build";
pub static X_CONTAINER_SECTION: &str = "X-Container";
pub static X_IMAGE_SECTION: &str    = "X-Image";
pub static X_KUBE_SECTION: &str     = "X-Kube";
pub static X_NETWORK_SECTION: &str  = "X-Network";
pub static X_POD_SECTION: &str      = "X-Pod";
pub static X_QUADLET_SECTION: &str  = "X-Quadlet";
pub static X_VOLUME_SECTION: &str   = "X-Volume";

pub static AUTO_UPDATE_LABEL: &str = "io.containers.autoupdate";

pub static SUPPORTED_EXTENSIONS: [&str; 7] = [
    "build",
    "container",
    "image",
    "kube",
    "network",
    "pod",
    "volume",
];

pub static SUPPORTED_BUILD_KEYS: [&str; 26] = [
    "Annotation",
    "Arch",
    "AuthFile",
    "ContainersConfModule",
    "DNS",
    "DNSOption",
    "DNSSearch",
    "Environment",
    "File",
    "ForceRM",
    "GlobalArgs",
    "GroupAdd",
    "ImageTag",
    "Label",
    "Network",
    "PodmanArgs",
    "Pull",
    "Retry",
    "RetryDelay",
    "Secret",
    "ServiceName",
    "SetWorkingDirectory",
    "Target",
    "TLSVerify",
    "Variant",
    "Volume",
];

pub static SUPPORTED_CONTAINER_KEYS: [&str; 89] = [
    "AddCapability",
    "AddDevice",
    "AddHost",
    "Annotation",
    "AutoUpdate",
    "CgroupsMode",
    "ContainerName",
    "ContainersConfModule",
    "DNS",
    "DNSOption",
    "DNSSearch",
    "DropCapability",
    "Entrypoint",
    "Environment",
    "EnvironmentFile",
    "EnvironmentHost",
    "Exec",
    "ExposeHostPort",
    "GIDMap",
    "GlobalArgs",
    "Group",
    "GroupAdd",
    "HealthCmd",
    "HealthInterval",
    "HealthOnFailure",
    "HealthRetries",
    "HealthStartPeriod",
    "HealthStartupCmd",
    "HealthStartupInterval",
    "HealthStartupRetries",
    "HealthStartupSuccess",
    "HealthStartupTimeout",
    "HealthTimeout",
    "HostName",
    "Image",
    "IP",
    "IP6",
    "Label",
    "LogDriver",
    "LogOpt",
    "Mask",
    "Memory",
    "Mount",
    "Network",
    "NetworkAlias",
    "NoNewPrivileges",
    "Notify",
    "PidsLimit",
    "PodmanArgs",
    "Pod",
    "PublishPort",
    "Pull",
    "ReloadCmd",
    "ReloadSignal",
    "Retry",
    "RetryDelay",
    "ReadOnly",
    "ReadOnlyTmpfs",
    "RemapGid",     // deprecated, use UserNS instead
    "RemapUid",     // deprecated, use UserNS instead
    "RemapUidSize", // deprecated, use UserNS instead
    "RemapUsers",   // deprecated, use UserNS instead
    "Rootfs",
    "RunInit",
    "SeccompProfile",
    "SecurityLabelDisable",
    "SecurityLabelFileType",
    "SecurityLabelLevel",
    "SecurityLabelNested",
    "SecurityLabelType",
    "Secret",
    "ServiceName",
    "ShmSize",
    "StartWithPod",
    "StopSignal",
    "StopTimeout",
    "SubGIDMap",
    "SubUIDMap",
    "Sysctl",
    "Timezone",
    "Tmpfs",
    "UIDMap",
    "Ulimit",
    "Unmask",
    "User",
    "UserNS",
    "VolatileTmp", // deprecated, use ReadOnlyTmpfs instead
    "Volume",
    "WorkingDir",
];

pub static SUPPORTED_IMAGE_KEYS: [&str; 17] = [
    "AllTags",
    "Arch",
    "AuthFile",
    "CertDir",
    "ContainersConfModule",
    "Creds",
    "DecryptionKey",
    "GlobalArgs",
    "Image",
    "ImageTag",
    "PodmanArgs",
    "Retry",
    "RetryDelay",
    "OS",
    "ServiceName",
    "TLSVerify",
    "Variant",
];

pub static SUPPORTED_KUBE_KEYS: [&str; 19] = [
    "AutoUpdate",
    "ConfigMap",
    "ContainersConfModule",
    "ExitCodePropagation",
    "GlobalArgs",
    "KubeDownForce",
    "LogDriver",
    "LogOpt",
    "Network",
    "PodmanArgs",
    "PublishPort",
    "RemapGid",     // deprecated, use UserNS instead
    "RemapUid",     // deprecated, use UserNS instead
    "RemapUidSize", // deprecated, use UserNS instead
    "RemapUsers",   // deprecated, use UserNS instead
    "ServiceName",
    "SetWorkingDirectory",
    "UserNS",
    "Yaml",
];

pub static SUPPORTED_NETWORK_KEYS: [&str; 18] = [
    "ContainersConfModule",
    "DisableDNS",
    "DNS",
    "Driver",
    "Gateway",
    "GlobalArgs",
    "InterfaceName",
    "Internal",
    "IPAMDriver",
    "IPRange",
    "IPv6",
    "Label",
    "NetworkName",
    "NetworkDeleteOnStop",
    "Options",
    "PodmanArgs",
    "ServiceName",
    "Subnet",
];

pub static SUPPORTED_POD_KEYS: [&str; 27] = [
    "AddHost",
    "ContainersConfModule",
    "DNS",
    "DNSOption",
    "DNSSearch",
    "GIDMap",
    "GlobalArgs",
    "HostName",
    "IP",
    "IP6",
    "Label",
    "Network",
    "NetworkAlias",
    "PodmanArgs",
    "PodName",
    "PublishPort",
    "RemapGid",     // deprecated, use UserNS instead
    "RemapUid",     // deprecated, use UserNS instead
    "RemapUidSize", // deprecated, use UserNS instead
    "RemapUsers",   // deprecated, use UserNS instead
    "ServiceName",
    "ShmSize",
    "SubGIDMap",
    "SubUIDMap",
    "UIDMap",
    "UserNS",
    "Volume",
];

pub static SUPPORTED_QUADLET_KEYS: [&str; 1] = ["DefaultDependencies"];

pub static SUPPORTED_SERVICE_KEYS: [&str; 1] = ["WorkingDirectory"];

pub static SUPPORTED_VOLUME_KEYS: [&str; 14] = [
    "ContainersConfModule",
    "Copy",
    "Device",
    "Driver",
    "GlobalArgs",
    "Group",
    "Image",
    "Label",
    "Options",
    "PodmanArgs",
    "ServiceName",
    "Type",
    "User",
    "VolumeName",
];

pub static UNIT_DEPENDENCY_KEYS: [&str; 15] = [
    "After",
    "Before",
    "BindsTo",
    "Conflicts",
    "OnFailure",
    "OnSuccess",
    "PartOf",
    "PropagatesReloadTo",
    "PropagatesStopTo",
    "ReloadPropagatedFrom",
    "Requires",
    "Requisite",
    "StopPropagatedFrom",
    "Upholds",
    "Wants",
];
