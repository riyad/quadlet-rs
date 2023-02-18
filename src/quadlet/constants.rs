use once_cell::sync::Lazy;
use std::collections::HashSet;

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
        "Image",
        "Label",
        "Network",
        "NoNewPrivileges",
        "Notify",
        "PodmanArgs",
        "PublishPort",
        "ReadOnly",
        "RemapGid",
        "RemapUid",
        "RemapUidSize",
        "RemapUsers",
        "Rootfs",
        "RunInit",
        "SeccompProfile",
        "SecurityLabelDisable",
        "SecurityLabelFileType",
        "SecurityLabelLevel",
        "SecurityLabelType",
        "Secret",
        "Timezone",
        "User",
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
        "Network",
        "PublishPort",
        "RemapGid",
        "RemapUid",
        "RemapUidSize",
        "RemapUsers",
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
