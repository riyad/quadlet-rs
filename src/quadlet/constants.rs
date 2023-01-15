use once_cell::sync::Lazy;
use std::collections::HashSet;

pub const CONTAINER_SECTION: &str = "Container";
pub const X_CONTAINER_SECTION: &str = "X-Container";
pub const KUBE_SECTION: &str = "Kube";
pub const X_KUBE_SECTION: &str = "X-Kube";
pub const NETWORK_SECTION: &str = "Network";
pub const X_NETWORK_SECTION: &str = "X-Network";
pub const VOLUME_SECTION: &str = "Volume";
pub const X_VOLUME_SECTION: &str = "X-Volume";

pub static SUPPORTED_CONTAINER_KEYS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    let keys = [
        "ContainerName",
        "Image",
        "KillMode",
        "Environment",
        "EnvironmentFile",
        "EnvironmentHost",
        "Exec",
        "NoNewPrivileges",
        "DropCapability",
        "AddCapability",
        "ReadOnly",
        "RemapUsers",
        "RemapUid",
        "RemapGid",
        "RemapUidSize",
        "Notify",
        "ExposeHostPort",
        "PublishPort",
        "User",
        "Group",
        "Volume",
        "PodmanArgs",
        "Label",
        "Annotation",
        "RunInit",
        "VolatileTmp",
        "Timezone",
        "SeccompProfile",
        "AddDevice",
        "Network",
    ];

    let mut set = HashSet::with_capacity(keys.len());
    for k in keys {
        set.insert(k);
    }
    set
});

pub static SUPPORTED_KUBE_KEYS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    let keys = [
        "Yaml",
        "KillMode",
        "RemapUsers",
        "RemapUid",
        "RemapGid",
        "RemapUidSize",
        "Network",
        "ConfigMap",
        "PublishPort",
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
        "IPRange",
        "IPAMDriver",
        "IPv6",
        "Options",
        "Subnet",
        "Label",
    ];

    let mut set = HashSet::with_capacity(keys.len());
    for k in keys {
        set.insert(k);
    }
    set
});

pub static SUPPORTED_VOLUME_KEYS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    let keys = [
        "User",
        "Group",
        "Label",
    ];

    let mut set = HashSet::with_capacity(keys.len());
    for k in keys {
        set.insert(k);
    }
    set
});
