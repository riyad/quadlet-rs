pub(crate) const DEFAULT_PODMAN_BINARY: &str = "/usr/bin/podman";

/// Directory for global Quadlet files (sysadmin owned)
pub const UNIT_DIR_ADMIN: &str = "/etc/containers/systemd";
/// Directory for global Quadlet files (distro owned)
pub const UNIT_DIR_DISTRO: &str = "/usr/share/containers/systemd";
/// Directory for temporary Quadlet files (sysadmin owned)
pub const UNIT_DIR_TEMP: &str = "/run/containers/systemd";
pub const SYSTEM_USER_DIR_LEVEL: usize = 5;

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

pub static SUPPORTED_BUILD_KEYS: [&str; 23] = [
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
    "SetWorkingDirectory",
    "Target",
    "TLSVerify",
    "Variant",
    "Volume",
];

pub static SUPPORTED_CONTAINER_KEYS: [&str; 80] = [
    "AddCapability",
    "AddDevice",
    "Annotation",
    "AutoUpdate",
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

pub static SUPPORTED_IMAGE_KEYS: [&str; 14] = [
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
    "TLSVerify",
    "Variant",
];

pub static SUPPORTED_KUBE_KEYS: [&str; 18] = [
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
    "SetWorkingDirectory",
    "UserNS",
    "Yaml",
];

pub static SUPPORTED_NETWORK_KEYS: [&str; 15] = [
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
    "Subnet",
];
pub static SUPPORTED_POD_KEYS: [&str; 9] = [
    "ContainersConfModule",
    "GlobalArgs",
    "Network",
    "NetworkAlias",
    "PodmanArgs",
    "PodName",
    "PublishPort",
    "ServiceName",
    "Volume",
];

pub static SUPPORTED_SERVICE_KEYS: [&str; 1] = ["WorkingDirectory"];

pub static SUPPORTED_VOLUME_KEYS: [&str; 13] = [
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
    "Type",
    "User",
    "VolumeName",
];
