pub(crate) const DEFAULT_PODMAN_BINARY: &str = "/usr/bin/podman";

pub const CONTAINER_SECTION: &str   = "Container";
pub const IMAGE_SECTION: &str       = "Image";
pub const KUBE_SECTION: &str        = "Kube";
pub const NETWORK_SECTION: &str     = "Network";
pub const VOLUME_SECTION: &str      = "Volume";
pub const X_CONTAINER_SECTION: &str = "X-Container";
pub const X_IMAGE_SECTION: &str     = "X-Image";
pub const X_KUBE_SECTION: &str      = "X-Kube";
pub const X_NETWORK_SECTION: &str   = "X-Network";
pub const X_VOLUME_SECTION: &str    = "X-Volume";

pub const AUTO_UPDATE_LABEL: &str = "io.containers.autoupdate";

pub static SUPPORTED_CONTAINER_KEYS: [&str; 66] = [
    "AddCapability",
    "AddDevice",
    "Annotation",
    "AutoUpdate",
    "ContainerName",
    "DNS",
    "DNSOption",
    "DNSSearch",
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
    "Mask",
    "Mount",
    "Network",
    "NoNewPrivileges",
    "Notify",
    "PidsLimit",
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
    "ShmSize",
    "Sysctl",
    "Timezone",
    "Tmpfs",
    "Ulimit",
    "Unmask",
    "User",
    "UserNS",
    "VolatileTmp",
    "Volume",
    "WorkingDir",
];

pub static SUPPORTED_IMAGE_KEYS: [&str; 11] = [
    "AllTags",
    "Arch",
    "AuthFile",
    "CertDir",
    "Creds",
    "DecryptionKey",
    "Image",
    "PodmanArgs",
    "OS",
    "TLSVerify",
    "Variant",
];

pub static SUPPORTED_KUBE_KEYS: [&str; 14] = [
    "AutoUpdate",
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
    "SetWorkingDirectory",
    "UserNS",
    "Yaml",
];

pub static SUPPORTED_NETWORK_KEYS: [&str; 13] = [
    "DisableDNS",
    "DNS",
    "Driver",
    "Gateway",
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


pub static SUPPORTED_SERVICE_KEYS: [&str; 1] = [
    "WorkingDirectory",
];

pub static SUPPORTED_VOLUME_KEYS: [&str; 11] = [
    "Copy",
    "Device",
    "Driver",
    "Group",
    "Image",
    "Label",
    "Options",
    "PodmanArgs",
    "Type",
    "User",
    "VolumeName",
];
