use once_cell::sync::Lazy;
use std::collections::HashSet;

use super::{IdRanges, quad_lookup_host_subgid, quad_lookup_host_subuid};

// BEGIN from build config
static QUADLET_FALLBACK_GID_LENGTH: u32 = 65536;
static QUADLET_FALLBACK_GID_START: u32 = 1879048192;
static QUADLET_FALLBACK_UID_LENGTH: u32 = 65536;
static QUADLET_FALLBACK_UID_START: u32 = 1879048192;
static QUADLET_USERNAME: &str = "quadlet";
// END from build config

static DEFAULT_DROP_CAPS: &[&str] = &["all"];
static DEFAULT_REMAP_GIDS: Lazy<IdRanges> = Lazy::new(|| {
    match quad_lookup_host_subgid(QUADLET_USERNAME) {
        Some(ids) => ids,
        None => IdRanges::new(QUADLET_FALLBACK_GID_START, QUADLET_FALLBACK_GID_LENGTH),
    }
});
static DEFAULT_REMAP_UIDS: Lazy<IdRanges> = Lazy::new(|| {
    match quad_lookup_host_subuid(QUADLET_USERNAME) {
        Some(ids) => ids,
        None => IdRanges::new(QUADLET_FALLBACK_UID_START, QUADLET_FALLBACK_UID_LENGTH),
    }
});

pub(crate) static SUPPORTED_CONTAINER_KEYS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    let keys = [
        "ContainerName",
        "Image",
        "Environment",
        "Exec",
        "NoNewPrivileges",
        "DropCapability",
        "AddCapability",
        "RemapUsers",
        "RemapUidStart",
        "RemapGidStart",
        "RemapUidRanges",
        "RemapGidRanges",
        "Notify",
        "SocketActivated",
        "ExposeHostPort",
        "PublishPort",
        "KeepId",
        "User",
        "Group",
        "HostUser",
        "HostGroup",
        "Volume",
        "PodmanArgs",
        "Label",
        "Annotation",
        "RunInit",
        "VolatileTmp",
        "Timezone",
    ];

    let mut set = HashSet::with_capacity(keys.len());
    for k in keys {
        set.insert(k);
    }
    set
});

pub(crate) static SUPPORTED_VOLUME_KEYS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
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