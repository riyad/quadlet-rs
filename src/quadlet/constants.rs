pub(crate) const DEFAULT_PODMAN_BINARY: &str = "/usr/bin/podman";

/// Directory for global Quadlet files (sysadmin owned)
pub const UNIT_DIR_ADMIN: &str = "/etc/containers/systemd";
/// Directory for global Quadlet files (distro owned)
pub const UNIT_DIR_DISTRO: &str = "/usr/share/containers/systemd";
/// Directory for temporary Quadlet files (sysadmin owned)
pub const UNIT_DIR_TEMP: &str = "/run/containers/systemd";

pub const BUILD_SECTION: &str       = "Build";
pub const CONTAINER_SECTION: &str   = "Container";
pub const IMAGE_SECTION: &str       = "Image";
pub const KUBE_SECTION: &str        = "Kube";
pub const NETWORK_SECTION: &str     = "Network";
pub const POD_SECTION: &str         = "Pod";
pub const VOLUME_SECTION: &str      = "Volume";
pub const X_BUILD_SECTION: &str     = "X-Build";
pub const X_CONTAINER_SECTION: &str = "X-Container";
pub const X_IMAGE_SECTION: &str     = "X-Image";
pub const X_KUBE_SECTION: &str      = "X-Kube";
pub const X_NETWORK_SECTION: &str   = "X-Network";
pub const X_POD_SECTION: &str       = "X-Pod";
pub const X_VOLUME_SECTION: &str    = "X-Volume";

pub const AUTO_UPDATE_LABEL: &str = "io.containers.autoupdate";

pub static SUPPORTED_EXTENSIONS: [&str; 7] =
    ["build", "container", "image", "kube", "network", "pod", "volume"];

pub static SUPPORTED_BUILD_KEYS: [&str; 24] = [
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
    "Secret",
    "ServiceName",
    "SetWorkingDirectory",
    "Target",
    "TLSVerify",
    "Variant",
    "Volume",
];

pub static SUPPORTED_CONTAINER_KEYS: [&str; 82] = [
    "AddCapability",
    "AddDevice",
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

pub static SUPPORTED_IMAGE_KEYS: [&str; 15] = [
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

pub static SUPPORTED_NETWORK_KEYS: [&str; 16] = [
    "ContainersConfModule",
    "DisableDNS",
    "DNS",
    "Driver",
    "Gateway",
    "GlobalArgs",
    "Internal",
    "IPAMDriver",
    "IPRange",
    "IPv6",
    "Label",
    "NetworkName",
    "Options",
    "PodmanArgs",
    "ServiceName",
    "Subnet",
];
pub static SUPPORTED_POD_KEYS: [&str; 18] = [
    "ContainersConfModule",
    "GIDMap",
    "GlobalArgs",
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
    "SubGIDMap",
    "SubUIDMap",
    "UIDMap",
    "UserNS",
    "Volume",
];

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
