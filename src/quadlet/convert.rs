use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::systemd_unit::*;

use super::constants::*;
use super::podman_command::PodmanCommand;
use super::*;

// Convert a quadlet container file (unit file with a Container group) to a systemd
// service file (unit file with Service group) based on the options in the Container group.
// The original Container group is kept around as X-Container.
pub(crate) fn from_container_unit(
    container: &SystemdUnit,
    is_user: bool,
) -> Result<SystemdUnit, ConversionError> {
    let mut service = SystemdUnit::new();

    service.merge_from(container);
    service.path = Some(quad_replace_extension(
        container.path().unwrap(),
        ".service",
        "",
        "",
    ));

    if container.path().is_some() {
        service.append_entry(
            UNIT_SECTION,
            "SourcePath",
            container.path().unwrap().to_str().unwrap(),
        );
    }

    check_for_unknown_keys(container, CONTAINER_SECTION, &SUPPORTED_CONTAINER_KEYS)?;

    service.rename_section(CONTAINER_SECTION, X_CONTAINER_SECTION);

    // One image or rootfs must be specified for the container
    let image = container
        .lookup_last(CONTAINER_SECTION, "Image")
        .map_or(String::new(), |s| s.to_string());
    let rootfs = container
        .lookup_last(CONTAINER_SECTION, "Rootfs")
        .map_or(String::new(), |s| s.to_string());
    if image.is_empty() && rootfs.is_empty() {
        return Err(ConversionError::InvalidImageOrRootfs(
            "no Image or Rootfs key specified".into(),
        ));
    }
    if !image.is_empty() && !rootfs.is_empty() {
        return Err(ConversionError::InvalidImageOrRootfs(
            "the Image And Rootfs keys conflict can not be specified together".into(),
        ));
    }

    let podman_container_name = container
        .lookup_last(CONTAINER_SECTION, "ContainerName")
        .map(|v| v.to_string())
        // By default, We want to name the container by the service name
        .unwrap_or("systemd-%N".to_owned());

    // Set PODMAN_SYSTEMD_UNIT so that podman auto-update can restart the service.
    service.append_entry(SERVICE_SECTION, "Environment", "PODMAN_SYSTEMD_UNIT=%n");

    // Only allow mixed or control-group, as nothing else works well
    let kill_mode = service.lookup_last(SERVICE_SECTION, "KillMode");
    match kill_mode {
        None | Some("mixed") | Some("control-group") => {
            // We default to mixed instead of control-group, because it lets conmon do its thing
            service.set_entry(SERVICE_SECTION, "KillMode", "mixed");
        }
        Some(kill_mode) => {
            return Err(ConversionError::InvalidKillMode(format!(
                "invalid KillMode '{kill_mode}'"
            )));
        }
    }

    // Read env early so we can override it below
    let podman_env = container.lookup_all_key_val(CONTAINER_SECTION, "Environment");

    // Need the containers filesystem mounted to start podman
    service.append_entry(UNIT_SECTION, "RequiresMountsFor", "%t/containers");

    // If conmon exited uncleanly it may not have removed the container, so
    // force it, -i makes it ignore non-existing files.
    service.append_entry(
        SERVICE_SECTION,
        "ExecStop",
        format!("{} rm -f -i --cidfile=%t/%N.cid", get_podman_binary()),
    );
    // The ExecStopPost is needed when the main PID (i.e., conmon) gets killed.
    // In that case, ExecStop is not executed but *Post only.  If both are
    // fired in sequence, *Post will exit when detecting that the --cidfile
    // has already been removed by the previous `rm`..
    service.append_entry(
        SERVICE_SECTION,
        "ExecStopPost",
        format!("-{} rm -f -i --cidfile=%t/%N.cid", get_podman_binary()),
    );

    // FIXME: (COMPAT) remove once we can rely on Podman v4.4.0 or newer being present
    // Podman change in: https://github.com/containers/podman/pull/16394
    // Quadlet change in: https://github.com/containers/podman/pull/17487
    service.append_entry(SERVICE_SECTION, "ExecStopPost", "-rm -f %t/%N.cid");

    let mut podman = PodmanCommand::new_command("run");

    podman.add(format!("--name={podman_container_name}"));

    // We store the container id so we can clean it up in case of failure
    podman.add("--cidfile=%t/%N.cid");

    // And replace any previous container with the same name, not fail
    podman.add("--replace");

    // On clean shutdown, remove container
    podman.add("--rm");

    handle_log_driver(container, CONTAINER_SECTION, &mut podman);

    // We delegate groups to the runtime
    service.append_entry(SERVICE_SECTION, "Delegate", "yes");
    podman.add_slice(&["--cgroups=split"]);

    let timezone = container.lookup_last(CONTAINER_SECTION, "Timezone");
    if let Some(timezone) = timezone {
        if !timezone.is_empty() {
            podman.add(format!("--tz={}", timezone));
        }
    }

    handle_networks(container, CONTAINER_SECTION, &mut service, &mut podman)?;

    // Run with a pid1 init to reap zombies by default (as most apps don't do that)
    let run_init = container.lookup_bool(CONTAINER_SECTION, "RunInit");
    if let Some(run_init) = run_init {
        podman.add_bool("--init", run_init);
    }

    let service_type = container.lookup_last(SERVICE_SECTION, "Type");
    match service_type {
        Some("oneshot") => {}
        Some("notify") | None => {
            // If we're not in oneshot mode always use some form of sd-notify, normally via conmon,
            // but we also allow passing it to the container by setting Notify=yes

            let notify = container
                .lookup_bool(CONTAINER_SECTION, "Notify")
                .unwrap_or(false);
            if notify {
                podman.add("--sdnotify=container");
            } else {
                podman.add("--sdnotify=conmon");
            }
            service.set_entry(SERVICE_SECTION, "Type", "notify");
            service.set_entry(SERVICE_SECTION, "NotifyAccess", "all");

            // Detach from container, we don't need the podman process to hang around
            podman.add("-d");
        }
        Some(service_type) => {
            return Err(ConversionError::InvalidServiceType(format!(
                "invalid service Type '{service_type}'"
            )));
        }
    }

    if container
        .lookup_last(SERVICE_SECTION, "SyslogIdentifier")
        .is_none()
    {
        service.set_entry(SERVICE_SECTION, "SyslogIdentifier", "%N");
    }

    // Default to no higher level privileges or caps
    let no_new_privileges = container
        .lookup_bool(CONTAINER_SECTION, "NoNewPrivileges")
        .unwrap_or(false);
    if no_new_privileges {
        podman.add("--security-opt=no-new-privileges");
    }

    let security_label_disable = container
        .lookup_bool(CONTAINER_SECTION, "SecurityLabelDisable")
        .unwrap_or(false);
    if security_label_disable {
        podman.add_slice(&["--security-opt", "label:disable"]);
    }

    let security_label_type = container
        .lookup_last(CONTAINER_SECTION, "SecurityLabelType")
        .unwrap_or_default();
    if !security_label_type.is_empty() {
        podman.add("--security-opt");
        podman.add(format!("label=type:{security_label_type}"));
    }

    let security_label_file_type = container
        .lookup_last(CONTAINER_SECTION, "SecurityLabelFileType")
        .unwrap_or_default();
    if !security_label_file_type.is_empty() {
        podman.add("--security-opt");
        podman.add(format!("label=filetype:{security_label_file_type}"));
    }

    let security_label_level = container
        .lookup_last(CONTAINER_SECTION, "SecurityLabelLevel")
        .unwrap_or_default();
    if !security_label_level.is_empty() {
        podman.add("--security-opt");
        podman.add(format!("label=level:{security_label_level}"));
    }

    let devices: Vec<String> = container
        .lookup_all_strv(CONTAINER_SECTION, "AddDevice")
        .collect();
    for mut device in devices {
        if device.starts_with('-') {
            // ignore device if it doesn't exist
            device = device.strip_prefix('-').unwrap().into();
            let path = match device.split_once(':') {
                Some((path, _)) => path,
                None => &device,
            };
            if !PathBuf::from(path).exists() {
                continue;
            }
        }
        podman.add(format!("--device={device}"))
    }

    // Default to no higher level privileges or caps
    let seccomp_profile = container.lookup_last(CONTAINER_SECTION, "SeccompProfile");
    if let Some(seccomp_profile) = seccomp_profile {
        podman.add_slice(&["--security-opt", &format!("seccomp={seccomp_profile}")])
    }

    let drop_caps: Vec<String> = container
        .lookup_all_strv(CONTAINER_SECTION, "DropCapability")
        .collect();
    for caps in drop_caps {
        podman.add(format!("--cap-drop={}", caps.to_ascii_lowercase()))
    }

    // But allow overrides with AddCapability
    let add_caps: Vec<String> = container
        .lookup_all_strv(CONTAINER_SECTION, "AddCapability")
        .collect();
    for caps in add_caps {
        podman.add(format!("--cap-add={}", caps.to_ascii_lowercase()))
    }

    let read_only = container.lookup_bool(CONTAINER_SECTION, "ReadOnly");
    if let Some(read_only) = read_only {
        podman.add_bool("--read-only", read_only);
    }
    let read_only = read_only.unwrap_or(false); // key not found: use default

    let volatile_tmp = container
        .lookup_bool(CONTAINER_SECTION, "VolatileTmp")
        .unwrap_or(false);
    if volatile_tmp {
        // Read only mode already has a tmpfs by default
        if !read_only {
            podman.add_slice(&["--tmpfs", "/tmp:rw,size=512M,mode=1777"]);
        }
    } else if read_only {
        // !volatile_tmp, disable the default tmpfs from --read-only
        podman.add("--read-only-tmpfs=false")
    }

    let has_user = container.has_key(CONTAINER_SECTION, "User");
    let has_group = container.has_key(CONTAINER_SECTION, "Group");
    if has_user || has_group {
        let uid = container
            .lookup_last(CONTAINER_SECTION, "User")
            .map(|s| s.parse::<u32>().unwrap_or(0)) // key found: parse or default
            .unwrap_or(0); // key not found: use default
        let gid = container
            .lookup_last(CONTAINER_SECTION, "Group")
            .map(|s| s.parse::<u32>().unwrap_or(0)) // key found: parse or default
            .unwrap_or(0); // key not found: use default

        podman.add("--user");
        if has_group {
            podman.add(format!("{uid}:{gid}"));
        } else {
            podman.add(uid.to_string());
        }
    }

    handle_user_remap(container, CONTAINER_SECTION, &mut podman, is_user, true)?;

    handle_user_ns(container, CONTAINER_SECTION, &mut podman);

    for tmpfs in container.lookup_all(CONTAINER_SECTION, "Tmpfs") {
        if tmpfs.chars().filter(|c| *c == ':').count() > 1 {
            return Err(ConversionError::InvalidTmpfs(format!(
                "invalid tmpfs format {tmpfs:?}"
            )));
        }

        podman.add("--tmpfs");
        podman.add(tmpfs);
    }

    let volumes: Vec<&str> = container.lookup_all(CONTAINER_SECTION, "Volume").collect();
    for volume in volumes {
        let parts: Vec<&str> = volume.split(':').collect();

        let mut source = String::new();
        let dest;
        let mut options = String::new();

        if parts.len() >= 2 {
            source = parts[0].to_string();
            dest = parts[1];
        } else {
            dest = parts[0];
        }
        if parts.len() >= 3 {
            options = format!(":{}", parts[2]);
        }

        if !source.is_empty() {
            source = handle_storage_source(container, &mut service, &source);
        }

        podman.add("-v");
        if source.is_empty() {
            podman.add(dest)
        } else {
            podman.add(format!("{source}:{dest}{options}"))
        }
    }

    let exposed_ports = container.lookup_all(CONTAINER_SECTION, "ExposeHostPort");
    for exposed_port in exposed_ports {
        let exposed_port = exposed_port.trim(); // Allow whitespaces before and after

        if !is_port_range(exposed_port) {
            return Err(ConversionError::InvalidPortFormat(format!(
                "invalid port format '{exposed_port}'"
            )));
        }

        podman.add(format!("--expose={exposed_port}"))
    }

    handle_publish_ports(container, CONTAINER_SECTION, &mut podman)?;

    podman.add_env(&podman_env);

    if let Some(ip) = container.lookup_last(CONTAINER_SECTION, "IP") {
        if !ip.is_empty() {
            podman.add("--ip");
            podman.add(ip);
        }
    }

    if let Some(ip6) = container.lookup_last(CONTAINER_SECTION, "IP6") {
        if !ip6.is_empty() {
            podman.add("--ip6");
            podman.add(ip6);
        }
    }

    let labels = container.lookup_all_key_val(CONTAINER_SECTION, "Label");
    podman.add_labels(&labels);

    let annotations = container.lookup_all_key_val(CONTAINER_SECTION, "Annotation");
    podman.add_annotations(&annotations);

    let env_files: Vec<PathBuf> = container
        .lookup_all_args(CONTAINER_SECTION, "EnvironmentFile")
        .map(|s| PathBuf::from(s).absolute_from_unit(container))
        .collect();
    for env_file in env_files {
        podman.add("--env-file");
        podman.add(
            env_file
                .to_str()
                .expect("EnvironmentFile path is not a valid UTF-8 string"),
        );
    }

    if let Some(env_host) = container.lookup_bool(CONTAINER_SECTION, "EnvironmentHost") {
        podman.add_bool("--env-host", env_host);
    }

    let secrets = container.lookup_all_args(CONTAINER_SECTION, "Secret");
    for secret in secrets {
        podman.add("--secret");
        podman.add(secret);
    }

    let mounts = container.lookup_all_args(CONTAINER_SECTION, "Mount");
    for mount in mounts {
        let params: Vec<&str> = mount.split(',').collect();
        let mut params_map: HashMap<&str, String> = HashMap::with_capacity(params.len());
        for param in &params {
            let kv: Vec<&str> = param.split('=').collect();
            params_map.insert(kv[0], kv[1].to_string());
        }
        if let Some(param_type) = params_map.get("type") {
            if param_type == "volume" || param_type == "bind" {
                if let Some(param_source) = params_map.get("source") {
                    params_map.insert(
                        "source",
                        handle_storage_source(container, &mut service, param_source),
                    );
                } else if let Some(param_source) = params_map.get("src") {
                    params_map.insert(
                        "src",
                        handle_storage_source(container, &mut service, param_source),
                    );
                }
            }
        }
        let mut params_array = Vec::with_capacity(params.len());
        params_array.push(format!("type={}", params_map["type"]));
        for (k, v) in params_map {
            if k == "type" {
                continue;
            }
            params_array.push(format!("{k}={v}"));
        }
        podman.add("--mount");
        podman.add(params_array.join(","));
    }

    handle_health(container, CONTAINER_SECTION, &mut podman);

    if let Some(hostname) = container.lookup(CONTAINER_SECTION, "HostName") {
        podman.add("--hostname");
        podman.add(hostname);
    }

    if let Some(pull) = container.lookup(CONTAINER_SECTION, "Pull") {
        if !pull.is_empty() {
            podman.add("--pull");
            podman.add(pull);
        }
    }

    handle_podman_args(container, CONTAINER_SECTION, &mut podman);

    if !image.is_empty() {
        podman.add(image);
    } else {
        podman.add("--rootfs");
        podman.add(rootfs);
    }

    let mut exec_args = container
        .lookup_last_value(CONTAINER_SECTION, "Exec")
        .map(|v| SplitWord::new(v.raw()).collect())
        .unwrap_or(vec![]);
    podman.add_vec(&mut exec_args);

    service.append_entry_value(
        SERVICE_SECTION,
        "ExecStart",
        EntryValue::try_from_raw(podman.to_escaped_string().as_str())?,
    );

    Ok(service)
}

pub(crate) fn from_kube_unit(
    kube: &SystemdUnit,
    is_user: bool,
) -> Result<SystemdUnit, ConversionError> {
    let mut service = SystemdUnit::new();
    service.merge_from(kube);
    service.path = Some(quad_replace_extension(
        kube.path().unwrap(),
        ".service",
        "",
        "",
    ));

    if kube.path().is_some() {
        service.append_entry(
            UNIT_SECTION,
            "SourcePath",
            kube.path().unwrap().to_str().unwrap(),
        );
    }

    check_for_unknown_keys(kube, KUBE_SECTION, &SUPPORTED_KUBE_KEYS)?;

    // Rename old Kube group to x-Kube so that systemd ignores it
    service.rename_section(KUBE_SECTION, X_KUBE_SECTION);

    let yaml_path = kube.lookup_last(KUBE_SECTION, "Yaml").unwrap_or("");
    if yaml_path.is_empty() {
        return Err(ConversionError::YamlMissing("no Yaml key specified".into()));
    }

    let yaml_path = PathBuf::from(yaml_path).absolute_from_unit(kube);

    // Only allow mixed or control-group, as nothing else works well
    let kill_mode = kube.lookup_last(KUBE_SECTION, "KillMode");
    match kill_mode {
        None | Some("mixed") | Some("control-group") => {
            // We default to mixed instead of control-group, because it lets conmon do its thing
            service.set_entry(SERVICE_SECTION, "KillMode", "mixed");
        }
        Some(kill_mode) => {
            return Err(ConversionError::InvalidKillMode(format!(
                "invalid KillMode '{kill_mode}'"
            )));
        }
    }

    // Set PODMAN_SYSTEMD_UNIT so that podman auto-update can restart the service.
    service.append_entry(SERVICE_SECTION, "Environment", "PODMAN_SYSTEMD_UNIT=%n");

    // Need the containers filesystem mounted to start podman
    service.append_entry(UNIT_SECTION, "RequiresMountsFor", "%t/containers");

    service.append_entry(SERVICE_SECTION, "Type", "notify");
    service.append_entry(SERVICE_SECTION, "NotifyAccess", "all");

    if !kube.has_key(SERVICE_SECTION, "SyslogIdentifier") {
        service.set_entry(SERVICE_SECTION, "SyslogIdentifier", "%N");
    }

    let mut podman_start = PodmanCommand::new_command("kube");
    podman_start.add("play");

    podman_start.add_slice(&[
        // Replace any previous container with the same name, not fail
        "--replace",
        // Use a service container
        "--service-container=true",
    ]);

    if let Some(ecp) = kube.lookup(KUBE_SECTION, "ExitCodePropagation") {
        if !ecp.is_empty() {
            podman_start.add(format!("--service-exit-code-propagation={ecp}"));
        }
    }

    handle_log_driver(kube, KUBE_SECTION, &mut podman_start);

    handle_user_remap(kube, KUBE_SECTION, &mut podman_start, is_user, false)?;

    handle_user_ns(kube, KUBE_SECTION, &mut podman_start);

    handle_networks(kube, KUBE_SECTION, &mut service, &mut podman_start)?;

    let config_maps: Vec<PathBuf> = kube
        .lookup_all_strv(KUBE_SECTION, "ConfigMap")
        .map(PathBuf::from)
        .collect();
    for config_map in config_maps {
        let config_map_path = config_map.absolute_from_unit(kube);
        podman_start.add("--configmap");
        podman_start.add(
            config_map_path
                .to_str()
                .expect("ConfigMap path is not valid UTF-8 string"),
        );
    }

    handle_publish_ports(kube, KUBE_SECTION, &mut podman_start)?;

    handle_podman_args(kube, KUBE_SECTION, &mut podman_start);

    podman_start.add(
        yaml_path
            .to_str()
            .expect("Yaml path is not valid UTF-8 string"),
    );

    service.append_entry_value(
        SERVICE_SECTION,
        "ExecStart",
        EntryValue::try_from_raw(podman_start.to_escaped_string().as_str())?,
    );

    let mut podman_stop = PodmanCommand::new_command("kube");
    podman_stop.add("down");
    podman_stop.add(
        yaml_path
            .to_str()
            .expect("Yaml path is not valid UTF-8 string"),
    );
    service.append_entry_value(
        SERVICE_SECTION,
        "ExecStopPost",
        EntryValue::try_from_raw(podman_stop.to_escaped_string().as_str())?,
    );

    Ok(service)
}

// Convert a quadlet network file (unit file with a Network group) to a systemd
// service file (unit file with Service group) based on the options in the Network group.
// The original Network group is kept around as X-Network.
pub(crate) fn from_network_unit(network: &SystemdUnit) -> Result<SystemdUnit, ConversionError> {
    let mut service = SystemdUnit::new();
    service.merge_from(network);
    service.path = Some(quad_replace_extension(
        network.path().unwrap(),
        ".service",
        "",
        "-network",
    ));

    if network.path().is_some() {
        service.append_entry(
            UNIT_SECTION,
            "SourcePath",
            network.path().unwrap().to_str().unwrap(),
        );
    }

    check_for_unknown_keys(network, NETWORK_SECTION, &SUPPORTED_NETWORK_KEYS)?;

    // Rename old Network group to x-Network so that systemd ignores it
    service.rename_section(NETWORK_SECTION, X_NETWORK_SECTION);

    let podman_network_name = quad_replace_extension(network.path().unwrap(), "", "systemd-", "")
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    // Need the containers filesystem mounted to start podman
    service.append_entry(UNIT_SECTION, "RequiresMountsFor", "%t/containers");

    let mut podman = PodmanCommand::new_command("network");
    podman.add("create");
    // FIXME:  (COMPAT) add `--ignore` once we can rely on Podman v4.4.0 or newer being present
    // Podman support added in: https://github.com/containers/podman/pull/16773
    // Quadlet support added in: https://github.com/containers/podman/pull/16688
    //podman.add("--ignore");

    let disable_dns = network
        .lookup_bool(NETWORK_SECTION, "DisableDNS")
        .unwrap_or(false);
    if disable_dns {
        podman.add("--disable-dns")
    }

    let driver = network.lookup_last(NETWORK_SECTION, "Driver");
    if let Some(driver) = driver {
        if !driver.is_empty() {
            podman.add(format!("--driver={driver}"));
        }
    }

    let subnets: Vec<&str> = network.lookup_all(NETWORK_SECTION, "Subnet").collect();
    let gateways: Vec<&str> = network.lookup_all(NETWORK_SECTION, "Gateway").collect();
    let ip_ranges: Vec<&str> = network.lookup_all(NETWORK_SECTION, "IPRange").collect();
    if !subnets.is_empty() {
        if gateways.len() > subnets.len() {
            return Err(ConversionError::InvalidSubnet(
                "cannot set more gateways than subnets".into(),
            ));
        }
        if ip_ranges.len() > subnets.len() {
            return Err(ConversionError::InvalidSubnet(
                "cannot set more ranges than subnets".into(),
            ));
        }
        for (i, subnet) in subnets.iter().enumerate() {
            podman.add(format!("--subnet={subnet}"));
            if i < gateways.len() {
                podman.add(format!("--gateway={}", gateways[i]));
            }
            if i < ip_ranges.len() {
                podman.add(format!("--ip-range={}", ip_ranges[i]));
            }
        }
    } else if !gateways.is_empty() || !ip_ranges.is_empty() {
        return Err(ConversionError::InvalidSubnet(
            "cannot set Gateway or IPRange without Subnet".into(),
        ));
    }

    let internal = network
        .lookup_bool(NETWORK_SECTION, "Internal")
        .unwrap_or(false);
    if internal {
        podman.add("--internal")
    }

    let ipam_driver = network.lookup_last(NETWORK_SECTION, "IPAMDriver");
    if let Some(ipam_driver) = ipam_driver {
        podman.add(format!("--ipam-driver={ipam_driver}"));
    }

    let ipv6 = network
        .lookup_bool(NETWORK_SECTION, "IPv6")
        .unwrap_or(false);
    if ipv6 {
        podman.add("--ipv6")
    }

    let network_options = network.lookup_all_key_val(NETWORK_SECTION, "Options");
    if !network_options.is_empty() {
        podman.add_keys("--opt", &network_options);
    }

    let labels = network.lookup_all_key_val(NETWORK_SECTION, "Label");
    podman.add_labels(&labels);

    handle_podman_args(network, NETWORK_SECTION, &mut podman);

    podman.add(&podman_network_name);

    service.append_entry_value(
        SERVICE_SECTION,
        "ExecStart",
        EntryValue::try_from_raw(podman.to_escaped_string().as_str())?,
    );

    service.append_entry(SERVICE_SECTION, "Type", "oneshot");
    service.append_entry(SERVICE_SECTION, "RemainAfterExit", "yes");

    service.append_entry_value(
        SERVICE_SECTION,
        "ExecCondition",
        EntryValue::try_from_raw(format!(
            "/usr/bin/bash -c \"! {} network exists {podman_network_name}\"",
            get_podman_binary()
        ))?,
    );

    // The default syslog identifier is the exec basename (podman) which isn't very useful here
    service.append_entry(SERVICE_SECTION, "SyslogIdentifier", "%N");

    Ok(service)
}

pub(crate) fn from_volume_unit(volume: &SystemdUnit) -> Result<SystemdUnit, ConversionError> {
    let mut service = SystemdUnit::new();
    service.merge_from(volume);
    service.path = Some(quad_replace_extension(
        volume.path().unwrap(),
        ".service",
        "",
        "-volume",
    ));

    if volume.path().is_some() {
        service.append_entry(
            UNIT_SECTION,
            "SourcePath",
            volume.path().unwrap().to_str().unwrap(),
        );
    }

    check_for_unknown_keys(volume, VOLUME_SECTION, &SUPPORTED_VOLUME_KEYS)?;

    // Rename old Volume group to x-Volume so that systemd ignores it
    service.rename_section(VOLUME_SECTION, X_VOLUME_SECTION);

    let podman_volume_name = quad_replace_extension(volume.path().unwrap(), "", "systemd-", "")
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    // Need the containers filesystem mounted to start podman
    service.append_entry(UNIT_SECTION, "RequiresMountsFor", "%t/containers");

    let labels = volume.lookup_all_key_val(VOLUME_SECTION, "Label");

    let mut podman = PodmanCommand::new_command("volume");
    podman.add("create");
    // FIXME:  (COMPAT) add `--ignore` once we can rely on Podman v4.4.0 or newer being present
    // Podman support added in: https://github.com/containers/podman/pull/16243
    // Quadlet default changed in: https://github.com/containers/podman/pull/16243
    //podman.add("--ignore")

    let mut opts: Vec<String> = Vec::with_capacity(2);
    if volume.has_key(VOLUME_SECTION, "User") {
        let uid = volume
            .lookup_last(VOLUME_SECTION, "User")
            .map(|s| s.parse::<u32>().unwrap_or(0)) // key found: parse or default
            .unwrap_or(0); // key not found: use default
        opts.push(format!("uid={uid}"));
    }
    if volume.has_key(VOLUME_SECTION, "Group") {
        let gid = volume
            .lookup_last(VOLUME_SECTION, "Group")
            .map(|s| s.parse::<u32>().unwrap_or(0)) // key found: parse or default
            .unwrap_or(0); // key not found: use default
        opts.push(format!("gid={gid}"));
    }

    if let Some(copy) = volume.lookup_bool(VOLUME_SECTION, "Copy") {
        if copy {
            podman.add_slice(&["--opt", "copy"]);
        } else {
            podman.add_slice(&["--opt", "nocopy"]);
        }
    }

    let mut dev_valid = false;

    if let Some(dev) = volume.lookup_last(VOLUME_SECTION, "Device") {
        if !dev.is_empty() {
            podman.add("--opt");
            podman.add(format!("device={dev}"));
            dev_valid = true;
        }
    }

    if let Some(dev_type) = volume.lookup_last(VOLUME_SECTION, "Type") {
        if !dev_type.is_empty() {
            if dev_valid {
                podman.add("--opt");
                podman.add(format!("type={dev_type}"));
            } else {
                return Err(ConversionError::InvalidDeviceType(
                    "key Type can't be used without Device".into(),
                ));
            }
        }
    }

    if let Some(mount_opts) = volume.lookup_last(VOLUME_SECTION, "Options") {
        if !mount_opts.is_empty() {
            if dev_valid {
                opts.push(mount_opts.into());
            } else {
                return Err(ConversionError::InvalidDeviceOptions(
                    "key Options can't be used without Device".into(),
                ));
            }
        }
    }

    if !opts.is_empty() {
        podman.add("--opt");
        podman.add(format!("o={}", opts.join(",")));
    }

    podman.add_labels(&labels);

    handle_podman_args(volume, VOLUME_SECTION, &mut podman);

    podman.add(&podman_volume_name);

    service.append_entry_value(
        SERVICE_SECTION,
        "ExecStart",
        EntryValue::try_from_raw(podman.to_escaped_string().as_str())?,
    );

    service.append_entry(SERVICE_SECTION, "Type", "oneshot");
    service.append_entry(SERVICE_SECTION, "RemainAfterExit", "yes");

    service.append_entry_value(
        SERVICE_SECTION,
        "ExecCondition",
        EntryValue::try_from_raw(format!(
            "/usr/bin/bash -c \"! {} volume exists {podman_volume_name}\"",
            get_podman_binary()
        ))?,
    );

    // The default syslog identifier is the exec basename (podman) which isn't very useful here
    service.append_entry(SERVICE_SECTION, "SyslogIdentifier", "%N");

    Ok(service)
}

fn handle_health(unit_file: &SystemdUnit, section: &str, podman: &mut PodmanCommand) {
    let key_arg_map: [[&str; 2]; 11] = [
        ["HealthCmd", "cmd"],
        ["HealthInterval", "interval"],
        ["HealthOnFailure", "on-failure"],
        ["HealthRetries", "retries"],
        ["HealthStartPeriod", "start-period"],
        ["HealthTimeout", "timeout"],
        ["HealthStartupCmd", "startup-cmd"],
        ["HealthStartupInterval", "startup-interval"],
        ["HealthStartupRetries", "startup-retries"],
        ["HealthStartupSuccess", "startup-success"],
        ["HealthStartupTimeout", "startup-timeout"],
    ];

    for key_arg in key_arg_map {
        if let Some(val) = unit_file.lookup(section, key_arg[0]) {
            if !val.is_empty() {
                podman.add(format!("--health-{}", key_arg[1]));
                podman.add(val);
            }
        }
    }
}

fn handle_log_driver(unit_file: &SystemdUnit, section: &str, podman: &mut PodmanCommand) {
    if let Some(log_driver) = unit_file.lookup_last(section, "LogDriver") {
        podman.add_slice(&["--log-driver", log_driver]);
    }
}

fn handle_networks(
    quadlet_unit_file: &SystemdUnit,
    section: &str,
    service_unit_file: &mut SystemdUnit,
    podman: &mut PodmanCommand,
) -> Result<(), ConversionError> {
    let networks = quadlet_unit_file.lookup_all(section, "Network");
    for network in networks {
        if !network.is_empty() {
            let mut network_name = network;
            let mut options: Option<&str> = None;
            if let Some((_network_name, _options)) = network.split_once(':') {
                network_name = _network_name;
                options = Some(_options);
            }

            let podman_network_name;
            if network_name.ends_with(".network") {
                // the podman network name is systemd-$name
                podman_network_name =
                    quad_replace_extension(&PathBuf::from(network_name), "", "systemd-", "");

                // the systemd unit name is $name-network.service
                let network_service_name = quad_replace_extension(
                    &PathBuf::from(network_name),
                    ".service",
                    "",
                    "-network",
                );

                service_unit_file.append_entry(
                    UNIT_SECTION,
                    "Requires",
                    network_service_name.to_str().unwrap(),
                );
                service_unit_file.append_entry(
                    UNIT_SECTION,
                    "After",
                    network_service_name.to_str().unwrap(),
                );

                network_name = podman_network_name.to_str().unwrap();
            }

            if options.is_some() {
                podman.add(format!("--network={network_name}:{}", options.unwrap()));
            } else {
                podman.add(format!("--network={network_name}"));
            }
        }
    }

    Ok(())
}

fn handle_podman_args(unit_file: &SystemdUnit, section: &str, podman: &mut PodmanCommand) {
    let mut podman_args: Vec<String> = unit_file
        .lookup_all_args(section, "PodmanArgs")
        .collect();

    if !podman_args.is_empty() {
        podman.add_vec(&mut podman_args);
    }
}

fn handle_publish_ports(
    unit_file: &SystemdUnit,
    section: &str,
    podman: &mut PodmanCommand,
) -> Result<(), ConversionError> {
    let publish_ports: Vec<&str> = unit_file.lookup_all(section, "PublishPort").collect();
    for publish_port in publish_ports {
        let publish_port = publish_port.trim(); // Allow whitespaces before and after

        //  IP address could have colons in it. For example: "[::]:8080:80/tcp, so use custom splitter
        let mut parts = split_ports(publish_port);

        // format (from podman run):
        // ip:hostPort:containerPort | ip::containerPort | hostPort:containerPort | containerPort
        //
        // ip could be IPv6 with minimum of these chars "[::]"
        // containerPort can have a suffix of "/tcp" or "/udp"
        let container_port;
        let mut ip = String::new();
        let mut host_port = String::new();
        match parts.len() {
            1 => {
                container_port = parts.pop().unwrap();
            }
            2 => {
                // NOTE: order is inverted because of pop()
                container_port = parts.pop().unwrap();
                host_port = parts.pop().unwrap();
            }
            3 => {
                // NOTE: order is inverted because of pop()
                container_port = parts.pop().unwrap();
                host_port = parts.pop().unwrap();
                ip = parts.pop().unwrap();
            }
            _ => {
                return Err(ConversionError::InvalidPublishedPort(format!(
                    "invalid published port '{publish_port}'"
                )));
            }
        }

        if ip == "0.0.0.0" {
            ip.clear();
        }

        if !host_port.is_empty() && !is_port_range(host_port.as_str()) {
            return Err(ConversionError::InvalidPortFormat(format!(
                "invalid port format '{host_port}'"
            )));
        }

        if !container_port.is_empty() && !is_port_range(container_port.as_str()) {
            return Err(ConversionError::InvalidPortFormat(format!(
                "invalid port format '{container_port}'"
            )));
        }

        podman.add("--publish");
        if !ip.is_empty() && !host_port.is_empty() {
            podman.add(format!("{ip}:{host_port}:{container_port}"));
        } else if !ip.is_empty() {
            podman.add(format!("{ip}::{container_port}"));
        } else if !host_port.is_empty() {
            podman.add(format!("{host_port}:{container_port}"));
        } else {
            podman.add(container_port);
        }
    }

    Ok(())
}

fn handle_storage_source(
    quadlet_unit_file: &SystemdUnit,
    service_unit_file: &mut SystemdUnit,
    source: &str,
) -> String {
    let mut source = source.to_owned();

    if source.starts_with('.') {
        source = PathBuf::from(source)
            .absolute_from_unit(quadlet_unit_file)
            .to_str()
            .expect("source ist not valid UTF-8 string")
            .to_string();
    }

    if source.starts_with('/') {
        // Absolute path
        service_unit_file.append_entry(UNIT_SECTION, "RequiresMountsFor", &source);
    } else if source.ends_with(".volume") {
        // the podman volume name is systemd-$name
        let volume_name = quad_replace_extension(&PathBuf::from(&source), "", "systemd-", "");

        // the systemd unit name is $name-volume.service
        let volume_service_name =
            quad_replace_extension(&PathBuf::from(&source), ".service", "", "-volume");

        source = volume_name
            .to_str()
            .expect("volume name ist not valid UTF-8 string")
            .to_string();

        service_unit_file.append_entry(
            UNIT_SECTION,
            "Requires",
            volume_service_name.to_str().unwrap(),
        );
        service_unit_file.append_entry(
            UNIT_SECTION,
            "After",
            volume_service_name.to_str().unwrap(),
        );
    }

    source
}

fn handle_user_ns(unit_file: &SystemdUnit, section: &str, podman: &mut PodmanCommand) {
    if let Some(userns) = unit_file.lookup(section, "UserNS") {
        if !userns.is_empty() {
            podman.add("--userns");
            podman.add(userns);
        }
    }
}

fn handle_user_remap(
    unit_file: &SystemdUnit,
    section: &str,
    podman: &mut PodmanCommand,
    is_user: bool,
    support_manual: bool,
) -> Result<(), ConversionError> {
    // ignore Remap keys if UserNS is set
    if unit_file.lookup(section, "UserNS").is_some() {
        return Ok(());
    }

    let uid_maps: Vec<String> = unit_file.lookup_all_strv(section, "RemapUid").collect();
    let gid_maps: Vec<String> = unit_file.lookup_all_strv(section, "RemapGid").collect();
    let remap_users = unit_file.lookup_last(section, "RemapUsers");
    match remap_users {
        None => {
            if !uid_maps.is_empty() {
                return Err(ConversionError::InvalidRemapUsers(
                    "RemapUid set without RemapUsers".into(),
                ));
            }
            if !gid_maps.is_empty() {
                return Err(ConversionError::InvalidRemapUsers(
                    "RemapGid set without RemapUsers".into(),
                ));
            }
        }
        Some("manual") => {
            if support_manual {
                for uid_map in uid_maps {
                    podman.add(format!("--uidmap={uid_map}"));
                }
                for gid_map in gid_maps {
                    podman.add(format!("--gidmap={gid_map}"));
                }
            } else {
                return Err(ConversionError::InvalidRemapUsers(
                    "RemapUsers=manual is not supported".into(),
                ));
            }
        }
        Some("auto") => {
            let mut auto_opts: Vec<String> =
                Vec::with_capacity(uid_maps.len() + gid_maps.len() + 1);
            for uid_map in uid_maps {
                auto_opts.push(format!("uidmapping={uid_map}"));
            }
            for gid_map in gid_maps {
                auto_opts.push(format!("gidmapping={gid_map}"));
            }
            let uid_size = unit_file
                .lookup_last(section, "RemapUidSize")
                .map(|s| s.parse::<u32>().unwrap_or(0)) // key found: parse or default
                .unwrap_or(0); // key not found: use default
            if uid_size > 0 {
                auto_opts.push(format!("size={uid_size}"));
            }

            if auto_opts.is_empty() {
                podman.add("--userns=auto");
            } else {
                podman.add(format!("--userns=auto:{}", auto_opts.join(",")))
            }
        }
        Some("keep-id") => {
            if !is_user {
                return Err(ConversionError::InvalidRemapUsers(
                    "RemapUsers=keep-id is unsupported for system units".into(),
                ));
            }

            let mut keepid_opts: Vec<String> = Vec::new();
            if !uid_maps.is_empty() {
                if uid_maps.len() > 1 {
                    return Err(ConversionError::InvalidRemapUsers(
                        "RemapUsers=keep-id supports only a single value for UID mapping".into(),
                    ));
                }
                keepid_opts.push(format!("uid={}", uid_maps[0]));
            }
            if !gid_maps.is_empty() {
                if gid_maps.len() > 1 {
                    return Err(ConversionError::InvalidRemapUsers(
                        "RemapUsers=keep-id supports only a single value for GID mapping".into(),
                    ));
                }
                keepid_opts.push(format!("gid={}", gid_maps[0]));
            }

            if keepid_opts.is_empty() {
                podman.add("--userns=keep-id");
            } else {
                podman.add(format!("--userns=keep-id:{}", keepid_opts.join(",")));
            }
        }
        Some(remap_users) => {
            return Err(ConversionError::InvalidRemapUsers(format!(
                "unsupported RemapUsers option '{remap_users}'"
            )));
        }
    }

    Ok(())
}

fn is_port_range(port: &str) -> bool {
    // NOTE: We chose to implement a parser ouselves, because pulling in the regex crate just for this
    // increases the binary size by at least 0.5M. :/
    // But if we were to use the regex crate, all this function does is this:
    // const RE: Lazy<Regex> = Lazy::new(|| Regex::new("\\d+(-\\d+)?(/udp|/tcp)?$").unwrap());
    // return RE.is_match(port)

    if port.is_empty() {
        return false;
    }

    let mut chars = port.chars();
    let mut cur: Option<char>;
    let mut digits; // count how many digits we've read

    // necessary "\\d+" part
    digits = 0;
    loop {
        cur = chars.next();
        match cur {
            Some(c) if c.is_ascii_digit() => digits += 1,
            // start of next part
            Some('-' | '/') => break,
            // illegal character
            Some(_) => return false,
            // string has ended, just make sure we've seen at least one digit
            None => return digits > 0,
        }
    }

    // parse optional "(-\\d+)?" part
    if cur.unwrap() == '-' {
        digits = 0;
        loop {
            cur = chars.next();
            match cur {
                Some(c) if c.is_ascii_digit() => digits += 1,
                // start of next part
                Some('/') => break,
                // illegal character
                Some(_) => return false,
                // string has ended, just make sure we've seen at least one digit
                None => return digits > 0,
            }
        }
    }

    // parse optional "(/udp|/tcp)?" part
    let mut tcp = 0; // count how many characters we've read
    let mut udp = 0; // count how many characters we've read
    loop {
        cur = chars.next();
        match cur {
            // parse "tcp"
            Some('t') if tcp == 0 && udp == 0 => tcp += 1,
            Some('c') if tcp == 1 => tcp += 1,
            Some('p') if tcp == 2 => break,
            // parse "udp"
            Some('u') if udp == 0 && tcp == 0 => udp += 1,
            Some('d') if udp == 1 => udp += 1,
            Some('p') if udp == 2 => break,
            // illegal character
            Some(_) => return false,
            // string has ended, just after '/' or in the middle of "tcp" or "udp"
            None => return false,
        }
    }

    // make sure we're at the end of the string
    chars.next().is_none()
}

fn quad_replace_extension(
    file: &Path,
    new_extension: &str,
    extra_prefix: &str,
    extra_suffix: &str,
) -> PathBuf {
    let base_name = file.file_stem().unwrap().to_str().unwrap();

    file.with_file_name(format!(
        "{extra_prefix}{base_name}{extra_suffix}{new_extension}"
    ))
}

/// Parses arguments to podman-run's `--publish` option.
/// see also the documentation for the `PublishPort` field.
///
/// NOTE: the last part will also include the protocol if specified
fn split_ports(ports: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();

    let mut next_part = String::new();
    let mut chars = ports.chars();
    while let Some(c) = chars.next() {
        let c = c;
        match c {
            '[' => {
                // IPv6 contain ':' characters, hence they are enclosed with '[...]'
                // so we consume all characters until ']' (including ':') for this part
                next_part.push(c);
                while let Some(c) = chars.next() {
                    next_part.push(c);
                    if c == ']' {
                        break;
                    }
                }
            }
            ':' => {
                // assume all ':' characters are boundaries that start a new part
                parts.push(next_part);
                next_part = String::new();
                continue;
            }
            _ => {
                next_part.push(c);
            }
        }
    }
    // don't forget the last part
    parts.push(next_part);

    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    mod split_ports {
        use super::*;

        #[test]
        fn with_empty() {
            let input = "";

            assert_eq!(split_ports(input), vec![""],);
        }

        #[test]
        fn with_only_port() {
            let input = "123";

            assert_eq!(split_ports(input), vec!["123"],);
        }

        #[test]
        fn with_ipv4_and_port() {
            let input = "1.2.3.4:567";

            assert_eq!(split_ports(input), vec!["1.2.3.4", "567"],);
        }

        #[test]
        fn with_ipv6_and_port() {
            let input = "[::]:567";

            assert_eq!(split_ports(input), vec!["[::]", "567"],);
        }

        #[test]
        fn with_host_and_container_ports() {
            let input = "123:567";

            assert_eq!(split_ports(input), vec!["123", "567"],);
        }

        #[test]
        fn with_ipv4_host_and_container_ports() {
            let input = "0.0.0.0:123:567";

            assert_eq!(split_ports(input), vec!["0.0.0.0", "123", "567"],);
        }

        #[test]
        fn with_ipv6_empty_host_container_port_and_protocol() {
            let input = "[1:2:3:4::]::567/tcp";

            assert_eq!(split_ports(input), vec!["[1:2:3:4::]", "", "567/tcp"],);
        }
    }
}
