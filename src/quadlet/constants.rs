use once_cell::sync::Lazy;
use std::collections::HashSet;
use std::env;

pub(crate) const DEFAULT_PODMAN_BINARY: &str = "/usr/bin/podman";
pub(crate) static PODMAN_BINARY: Lazy<String> = Lazy::new(|| match env::var("PODMAN") {
    Ok(p) => p,
    Err(_) => DEFAULT_PODMAN_BINARY.to_owned(),
});

pub const CONTAINER_SECTION: &str   = "Container";
pub const KUBE_SECTION: &str        = "Kube";
pub const NETWORK_SECTION: &str     = "Network";
pub const VOLUME_SECTION: &str      = "Volume";
pub const X_CONTAINER_SECTION: &str = "X-Container";
pub const X_KUBE_SECTION: &str      = "X-Kube";
pub const X_NETWORK_SECTION: &str   = "X-Network";
pub const X_VOLUME_SECTION: &str    = "X-Volume";

pub static SUPPORTED_CONTAINER_KEYS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    let keys = [
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
        "SecurityLabelType",
        "Secret",
        "Timezone",
        "Tmpfs",
        "User",
        "UserNS",
        "VolatileTmp",
        "Volume",
    ];

    let mut set = HashSet::with_capacity(keys.len());
    for k in keys {
        set.insert(k);
    }
    set
});

pub static SUPPORTED_KUBE_KEYS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    let keys = [
        "ConfigMap",
        "LogDriver",
        "Network",
        "PublishPort",
        "RemapGid",     // deprecated, use UserNS instead
        "RemapUid",     // deprecated, use UserNS instead
        "RemapUidSize", // deprecated, use UserNS instead
        "RemapUsers",   // deprecated, use UserNS instead
        "UserNS",
        "Yaml",
    ];

    let mut set = HashSet::with_capacity(keys.len());
    for k in keys {
        set.insert(k);
    }
    set
});

pub static SUPPORTED_NETWORK_KEYS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    let keys = [
        "DisableDNS",
        "Driver",
        "Gateway",
        "Internal",
        "IPAMDriver",
        "IPRange",
        "IPv6",
        "Label",
        "Options",
        "Subnet",
    ];

    let mut set = HashSet::with_capacity(keys.len());
    for k in keys {
        set.insert(k);
    }
    set
});

pub static SUPPORTED_VOLUME_KEYS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    let keys = [
        "Copy",
        "Device",
        "Group",
        "Label",
        "Options",
        "Type",
        "User",
    ];

    let mut set = HashSet::with_capacity(keys.len());
    for k in keys {
        set.insert(k);
    }
    set
});
