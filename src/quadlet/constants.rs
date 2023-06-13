pub(crate) const DEFAULT_PODMAN_BINARY: &str = "/usr/bin/podman";

pub const CONTAINER_SECTION: &str   = "Container";
pub const KUBE_SECTION: &str        = "Kube";
pub const NETWORK_SECTION: &str     = "Network";
pub const VOLUME_SECTION: &str      = "Volume";
pub const X_CONTAINER_SECTION: &str = "X-Container";
pub const X_KUBE_SECTION: &str      = "X-Kube";
pub const X_NETWORK_SECTION: &str   = "X-Network";
pub const X_VOLUME_SECTION: &str    = "X-Volume";

pub static SUPPORTED_CONTAINER_KEYS: [&str; 57] = [
    "AddCapability",
    "AddDevice",
    "Annotation",
    "ContainerName",
    "DropCapability",
    "Environment",
    "EnvironmentFile",
    "EnvironmentHost",
    "Exec",
    "ExposeHostPort",
    "Group",
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
    "Mount",
    "Network",
    "NoNewPrivileges",
    "Notify",
    "PodmanArgs",
    "PublishPort",
    "Pull",
    "ReadOnly",
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
    "Sysctl",
    "Timezone",
    "Tmpfs",
    "User",
    "UserNS",
    "VolatileTmp",
    "Volume",
    "WorkingDir",
];

pub static SUPPORTED_KUBE_KEYS: [&str; 12] = [
    "ConfigMap",
    "ExitCodePropagation",
    "LogDriver",
    "Network",
    "PodmanArgs",
    "PublishPort",
    "RemapGid",     // deprecated, use UserNS instead
    "RemapUid",     // deprecated, use UserNS instead
    "RemapUidSize", // deprecated, use UserNS instead
    "RemapUsers",   // deprecated, use UserNS instead
    "UserNS",
    "Yaml",
];

pub static SUPPORTED_NETWORK_KEYS: [&str; 11] = [
    "DisableDNS",
    "Driver",
    "Gateway",
    "Internal",
    "IPAMDriver",
    "IPRange",
    "IPv6",
    "Label",
    "Options",
    "PodmanArgs",
    "Subnet",
];

pub static SUPPORTED_VOLUME_KEYS: [&str; 8] = [
    "Copy",
    "Device",
    "Group",
    "Label",
    "Options",
    "PodmanArgs",
    "Type",
    "User",
];
