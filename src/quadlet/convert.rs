use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::systemd_unit::*;

use super::constants::*;
use super::podman_command::PodmanCommand;
use super::*;

fn get_base_podman_command(unit: &SystemdUnitFile, section: &str) -> PodmanCommand {
    let mut podman = PodmanCommand::new();

    podman.extend(
        unit.lookup_all(section, "ContainersConfModule")
            .iter()
            .map(|s| format!("--module={s}")),
    );

    podman.extend(unit.lookup_all_args(section, "GlobalArgs"));

    podman
}

// Convert a quadlet container file (unit file with a Container group) to a systemd
// service file (unit file with Service group) based on the options in the Container group.
// The original Container group is kept around as X-Container.
pub(crate) fn from_container_unit(
    container: &SystemdUnitFile,
    names: &ResourceNameMap,
    is_user: bool,
    pods_info_map: &mut PodsInfoMap,
) -> Result<SystemdUnitFile, ConversionError> {
    let mut service = SystemdUnitFile::new();

    service.merge_from(container);
    service.path = quad_replace_extension(container.path(), ".service", "", "");

    if !container.path().as_os_str().is_empty() {
        service.append_entry(
            UNIT_SECTION,
            "SourcePath",
            container
                .path()
                .to_str()
                .expect("EnvironmentFile path is not a valid UTF-8 string"),
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

    let image = if !image.is_empty() {
        handle_image_source(&image, &mut service, names)?.to_string()
    } else {
        image
    };

    let podman_container_name =
        if let Some(container_name) = container.lookup(CONTAINER_SECTION, "ContainerName") {
            container_name
        } else {
            // By default, We want to name the container by the service name
            if container.is_template_unit() {
                "systemd-%P_%I"
            } else {
                "systemd-%N"
            }
        };

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
            return Err(ConversionError::InvalidKillMode(kill_mode.into()));
        }
    }

    // Read env early so we can override it below
    let podman_env = container.lookup_all_key_val(CONTAINER_SECTION, "Environment");

    // Need the containers filesystem mounted to start podman
    service.append_entry(UNIT_SECTION, "RequiresMountsFor", "%t/containers");

    // If conmon exited uncleanly it may not have removed the container, so
    // force it, -i makes it ignore non-existing files.
    let mut service_stop_cmd = get_base_podman_command(container, CONTAINER_SECTION);
    service_stop_cmd.add_slice(&["rm", "-v", "-f", "-i", "--cidfile=%t/%N.cid"]);
    service.append_entry_value(
        SERVICE_SECTION,
        "ExecStop",
        EntryValue::try_from_raw(service_stop_cmd.to_escaped_string().as_str())?,
    );
    // The ExecStopPost is needed when the main PID (i.e., conmon) gets killed.
    // In that case, ExecStop is not executed but *Post only.  If both are
    // fired in sequence, *Post will exit when detecting that the --cidfile
    // has already been removed by the previous `rm`..
    service_stop_cmd.args[0] = format!("-{}", service_stop_cmd.args[0]);
    service.append_entry_value(
        SERVICE_SECTION,
        "ExecStopPost",
        EntryValue::try_from_raw(service_stop_cmd.to_escaped_string().as_str())?,
    );

    // FIXME: (COMPAT) remove once we can rely on Podman v4.4.0 or newer being present
    // Podman change in: https://github.com/containers/podman/pull/16394
    // Quadlet change in: https://github.com/containers/podman/pull/17487
    service.append_entry(SERVICE_SECTION, "ExecStopPost", "-rm -f %t/%N.cid");

    let mut podman = get_base_podman_command(container, CONTAINER_SECTION);

    podman.add("run");

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

    if let Some(timezone) = container.lookup_last(CONTAINER_SECTION, "Timezone") {
        if !timezone.is_empty() {
            podman.add(format!("--tz={}", timezone));
        }
    }

    handle_networks(
        container,
        CONTAINER_SECTION,
        &mut service,
        names,
        &mut podman,
    )?;

    // Run with a pid1 init to reap zombies by default (as most apps don't do that)
    if let Some(run_init) = container.lookup_bool(CONTAINER_SECTION, "RunInit") {
        podman.add_bool("--init", run_init);
    }

    let service_type = container.lookup_last(SERVICE_SECTION, "Type");
    match service_type {
        Some("oneshot") => {}
        Some("notify") | None => {
            // If we're not in oneshot mode always use some form of sd-notify, normally via conmon,
            // but we also allow passing it to the container by setting Notify=yes
            let notify = container.lookup(CONTAINER_SECTION, "Notify");
            match notify {
                Some(notify) if notify == "healthy" => podman.add("--sdnotify=healthy"),
                _ => {
                    let notify = container
                        .lookup_bool(CONTAINER_SECTION, "Notify")
                        .unwrap_or(false);

                    if notify {
                        podman.add("--sdnotify=container");
                    } else {
                        podman.add("--sdnotify=conmon");
                    }
                }
            }
            service.set_entry(SERVICE_SECTION, "Type", "notify");
            service.set_entry(SERVICE_SECTION, "NotifyAccess", "all");

            // Detach from container, we don't need the podman process to hang around
            podman.add("-d");
        }
        Some(service_type) => {
            return Err(ConversionError::InvalidServiceType(service_type.into()));
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

    let security_label_nested = container
        .lookup_bool(CONTAINER_SECTION, "SecurityLabelNested")
        .unwrap_or(false);
    if security_label_nested {
        podman.add_slice(&["--security-opt", "label:nested"]);
    }

    if let Some(pids_limit) = container.lookup(CONTAINER_SECTION, "PidsLimit") {
        if !pids_limit.is_empty() {
            podman.add("--pids-limit");
            podman.add(pids_limit);
        }
    }

    let security_label_type = container
        .lookup(CONTAINER_SECTION, "SecurityLabelType")
        .unwrap_or_default();
    if !security_label_type.is_empty() {
        podman.add("--security-opt");
        podman.add(format!("label=type:{security_label_type}"));
    }

    let security_label_file_type = container
        .lookup(CONTAINER_SECTION, "SecurityLabelFileType")
        .unwrap_or_default();
    if !security_label_file_type.is_empty() {
        podman.add("--security-opt");
        podman.add(format!("label=filetype:{security_label_file_type}"));
    }

    let security_label_level = container
        .lookup(CONTAINER_SECTION, "SecurityLabelLevel")
        .unwrap_or_default();
    if !security_label_level.is_empty() {
        podman.add("--security-opt");
        podman.add(format!("label=level:{security_label_level}"));
    }

    for ulimit in container.lookup_all(CONTAINER_SECTION, "Ulimit") {
        if !ulimit.is_empty() {
            podman.add("--ulimit");
            podman.add(ulimit);
        }
    }

    for mut device in container.lookup_all_strv(CONTAINER_SECTION, "AddDevice") {
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
    if let Some(seccomp_profile) = container.lookup_last(CONTAINER_SECTION, "SeccompProfile") {
        podman.add_slice(&["--security-opt", &format!("seccomp={seccomp_profile}")])
    }

    for ip_addr in container.lookup_all(CONTAINER_SECTION, "DNS") {
        podman.add(format!("--dns={ip_addr}"))
    }

    for dns_option in container.lookup_all(CONTAINER_SECTION, "DNSOption") {
        podman.add(format!("--dns-option={dns_option}"))
    }

    for dns_search in container.lookup_all(CONTAINER_SECTION, "DNSSearch") {
        podman.add(format!("--dns-search={dns_search}"))
    }

    for caps in container.lookup_all_strv(CONTAINER_SECTION, "DropCapability") {
        podman.add(format!("--cap-drop={}", caps.to_ascii_lowercase()))
    }

    // But allow overrides with AddCapability
    for caps in container.lookup_all_strv(CONTAINER_SECTION, "AddCapability") {
        podman.add(format!("--cap-add={}", caps.to_ascii_lowercase()))
    }

    if let Some(shm_size) = container.lookup(CONTAINER_SECTION, "ShmSize") {
        podman.add(format!("--shm-size={shm_size}"))
    }

    if let Some(entrypoint) = container.lookup(CONTAINER_SECTION, "Entrypoint") {
        podman.add(format!("--entrypoint={entrypoint}"))
    }

    for sysctl in container.lookup_all_strv(CONTAINER_SECTION, "Sysctl") {
        podman.add(format!("--sysctl={sysctl}"))
    }

    let read_only = container.lookup_bool(CONTAINER_SECTION, "ReadOnly");
    if let Some(read_only) = read_only {
        podman.add_bool("--read-only", read_only);
    }
    let read_only = read_only.unwrap_or(false); // key not found: use default

    if let Some(read_only_tmpfs) = container.lookup_bool(CONTAINER_SECTION, "ReadOnlyTmpfs") {
        podman.add_bool("--read-only-tmpfs", read_only_tmpfs)
    }

    let volatile_tmp = container
        .lookup_bool(CONTAINER_SECTION, "VolatileTmp")
        .unwrap_or(false);
    if volatile_tmp && !read_only {
        podman.add_slice(&["--tmpfs", "/tmp:rw,size=512M,mode=1777"]);
    }

    handle_user(container, CONTAINER_SECTION, &mut podman)?;

    if let Some(workdir) = container.lookup(CONTAINER_SECTION, "WorkingDir") {
        podman.add(format!("-w={workdir}"));
    }

    handle_user_mappings(container, CONTAINER_SECTION, &mut podman, is_user, true)?;

    for tmpfs in container.lookup_all(CONTAINER_SECTION, "Tmpfs") {
        if tmpfs.chars().filter(|c| *c == ':').count() > 1 {
            return Err(ConversionError::InvalidTmpfs(tmpfs.into()));
        }

        podman.add("--tmpfs");
        podman.add(tmpfs);
    }

    handle_volumes(
        &container,
        CONTAINER_SECTION,
        &mut service,
        names,
        &mut podman,
    )?;

    if let Some(update) = container.lookup(CONTAINER_SECTION, "AutoUpdate") {
        if !update.is_empty() {
            let mut labels: HashMap<String, String> = HashMap::new();
            labels.insert(AUTO_UPDATE_LABEL.to_string(), update.to_string());
            podman.add_labels(&labels);
        }
    }

    for exposed_port in container.lookup_all(CONTAINER_SECTION, "ExposeHostPort") {
        let exposed_port = exposed_port.trim(); // Allow whitespaces before and after

        if !is_port_range(exposed_port) {
            return Err(ConversionError::InvalidPortFormat(exposed_port.into()));
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

    for mask in container.lookup_all_args(CONTAINER_SECTION, "Mask") {
        podman.add("--security-opt");
        podman.add(format!("mask={mask}"));
    }

    for unmask in container.lookup_all_args(CONTAINER_SECTION, "Unmask") {
        podman.add("--security-opt");
        podman.add(format!("unmask={unmask}"));
    }

    let env_files: Vec<PathBuf> = container
        .lookup_all_args(CONTAINER_SECTION, "EnvironmentFile")
        .iter()
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

    for secret in container.lookup_all_args(CONTAINER_SECTION, "Secret") {
        podman.add("--secret");
        podman.add(secret);
    }

    for mount in container.lookup_all_args(CONTAINER_SECTION, "Mount") {
        let mount_str = resolve_container_mount_params(container, &mut service, mount, names)?;
        podman.add("--mount");
        podman.add(mount_str);
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

    handle_pod(
        container,
        &mut service,
        CONTAINER_SECTION,
        pods_info_map,
        &mut podman,
    )?;

    if let Some(stop_timeout) = container.lookup(CONTAINER_SECTION, "StopTimeout") {
        if !stop_timeout.is_empty() {
            podman.add("--stop-timeout");
            podman.add(stop_timeout);
        }
    }

    handle_podman_args(container, CONTAINER_SECTION, &mut podman);

    if !image.is_empty() {
        podman.add(image);
    } else {
        podman.add("--rootfs");
        podman.add(rootfs);
    }

    let exec_args = container
        .lookup_last_value(CONTAINER_SECTION, "Exec")
        .map(|v| SplitWord::new(v.raw()))
        .unwrap_or_default();
    podman.extend(exec_args);

    service.append_entry_value(
        SERVICE_SECTION,
        "ExecStart",
        EntryValue::try_from_raw(podman.to_escaped_string().as_str())?,
    );

    Ok(service)
}

pub(crate) fn from_image_unit(
    image: &SystemdUnitFile,
    names: &mut ResourceNameMap,
    _is_user: bool,
) -> Result<SystemdUnitFile, ConversionError> {
    let mut service = SystemdUnitFile::new();
    service.merge_from(image);
    service.path = quad_replace_extension(image.path(), ".service", "", "-image");

    if !image.path().as_os_str().is_empty() {
        service.append_entry(
            UNIT_SECTION,
            "SourcePath",
            image
                .path()
                .to_str()
                .expect("EnvironmentFile path is not a valid UTF-8 string"),
        );
    }

    check_for_unknown_keys(image, IMAGE_SECTION, &SUPPORTED_IMAGE_KEYS)?;

    let image_name = image
        .lookup_last(IMAGE_SECTION, "Image")
        .unwrap_or_default();
    if image_name.is_empty() {
        return Err(ConversionError::InvalidImageOrRootfs(
            "no Image key specified".into(),
        ));
    }

    // Rename old Image group to X-Image so that systemd ignores it
    service.rename_section(IMAGE_SECTION, X_IMAGE_SECTION);

    // Need the containers filesystem mounted to start podman
    service.append_entry(UNIT_SECTION, "RequiresMountsFor", "%t/containers");

    let mut podman = get_base_podman_command(image, IMAGE_SECTION);
    podman.add("image");
    podman.add("pull");

    let string_keys = [
        ("Arch", "--arch"),
        ("AuthFile", "--authfile"),
        ("CertDir", "--cert-dir"),
        ("Creds", "--creds"),
        ("DecryptionKey", "--decryption-key"),
        ("OS", "--os"),
        ("Variant", "--variant"),
    ];

    let bool_keys = [("AllTags", "--all-tags"), ("TLSVerify", "--tls-verify")];

    for (key, flag) in string_keys {
        lookup_and_add_string(image, IMAGE_SECTION, key, flag, &mut podman)
    }

    for (key, flag) in bool_keys {
        lookup_and_add_bool(image, IMAGE_SECTION, key, flag, &mut podman)
    }

    handle_podman_args(image, IMAGE_SECTION, &mut podman);

    podman.add(image_name);

    service.append_entry_value(
        SERVICE_SECTION,
        "ExecStart",
        EntryValue::try_from_raw(podman.to_escaped_string().as_str())?,
    );

    service.append_entry(SERVICE_SECTION, "Type", "oneshot");
    service.append_entry(SERVICE_SECTION, "RemainAfterExit", "yes");

    // The default syslog identifier is the exec basename (podman) which isn't very useful here
    service.append_entry(SERVICE_SECTION, "SyslogIdentifier", "%N");

    let podman_image_name = if let Some(image) = image.lookup(IMAGE_SECTION, "ImageTag") {
        if !image.is_empty() {
            image
        } else {
            image_name
        }
    } else {
        image_name
    };

    names.insert(
        service.path().as_os_str().to_os_string(),
        podman_image_name.into(),
    );

    Ok(service)
}

pub(crate) fn from_kube_unit(
    kube: &SystemdUnitFile,
    names: &ResourceNameMap,
    is_user: bool,
) -> Result<SystemdUnitFile, ConversionError> {
    let mut service = SystemdUnitFile::new();
    service.merge_from(kube);
    service.path = quad_replace_extension(kube.path(), ".service", "", "");

    if !kube.path().as_os_str().is_empty() {
        service.append_entry(
            UNIT_SECTION,
            "SourcePath",
            kube.path()
                .to_str()
                .expect("EnvironmentFile path is not a valid UTF-8 string"),
        );
    }

    check_for_unknown_keys(kube, KUBE_SECTION, &SUPPORTED_KUBE_KEYS)?;

    // Rename old Kube group to x-Kube so that systemd ignores it
    service.rename_section(KUBE_SECTION, X_KUBE_SECTION);

    let yaml_path = kube.lookup_last(KUBE_SECTION, "Yaml").unwrap_or("");
    if yaml_path.is_empty() {
        return Err(ConversionError::NoYamlKeySpecified);
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
            return Err(ConversionError::InvalidKillMode(kill_mode.into()));
        }
    }

    // Set PODMAN_SYSTEMD_UNIT so that podman auto-update can restart the service.
    service.append_entry(SERVICE_SECTION, "Environment", "PODMAN_SYSTEMD_UNIT=%n");

    // Need the containers filesystem mounted to start podman
    service.append_entry(UNIT_SECTION, "RequiresMountsFor", "%t/containers");

    // Allow users to set the Service Type to oneshot to allow resources only kube yaml
    match service.lookup(SERVICE_SECTION, "Type") {
        None => {
            service.append_entry(SERVICE_SECTION, "Type", "notify");
            service.append_entry(SERVICE_SECTION, "NotifyAccess", "all");
        }
        // could be combined with the case above
        Some(service_type) if service_type != "oneshot" => {
            service.append_entry(SERVICE_SECTION, "Type", "notify");
            service.append_entry(SERVICE_SECTION, "NotifyAccess", "all");
        }
        Some(service_type) => {
            if service_type != "notify" && service_type != "oneshot" {
                return Err(ConversionError::InvalidServiceType(service_type.into()));
            }
        }
    }

    if !kube.has_key(SERVICE_SECTION, "SyslogIdentifier") {
        service.set_entry(SERVICE_SECTION, "SyslogIdentifier", "%N");
    }

    let mut podman_start = get_base_podman_command(kube, KUBE_SECTION);
    podman_start.add("kube");
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

    handle_user_mappings(kube, KUBE_SECTION, &mut podman_start, is_user, false)?;

    handle_networks(kube, KUBE_SECTION, &mut service, names, &mut podman_start)?;

    for update in kube.lookup_all_strv(KUBE_SECTION, "AutoUpdate") {
        let annotation_suffix;
        let update_type;
        if let Some((anno_value, typ)) = update.split_once('/') {
            annotation_suffix = format!("/{}", anno_value);
            update_type = typ;
        } else {
            annotation_suffix = "".to_string();
            update_type = &update;
        }
        podman_start.add("--annotation");
        podman_start.add(format!(
            "{AUTO_UPDATE_LABEL}{annotation_suffix}={update_type}"
        ));
    }

    let config_maps: Vec<PathBuf> = kube
        .lookup_all_strv(KUBE_SECTION, "ConfigMap")
        .iter()
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

    // Use `ExecStopPost` to make sure cleanup happens even in case of
    // errors; otherwise containers, pods, etc. would be left behind.
    let mut podman_stop = get_base_podman_command(kube, KUBE_SECTION);
    podman_stop.add("kube");
    podman_stop.add("down");

    if let Some(kube_down_force) = kube.lookup_bool(KUBE_SECTION, "KubeDownForce") {
        podman_stop.add_bool("--force", kube_down_force)
    }

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

    handle_set_working_directory(kube, &mut service)?;

    Ok(service)
}

// Convert a quadlet network file (unit file with a Network group) to a systemd
// service file (unit file with Service group) based on the options in the Network group.
// The original Network group is kept around as X-Network.
// Also returns the canonical network name, either auto-generated or user-defined via the
// NetworkName key-value.
pub(crate) fn from_network_unit(
    network: &SystemdUnitFile,
    names: &mut ResourceNameMap,
) -> Result<SystemdUnitFile, ConversionError> {
    let mut service = SystemdUnitFile::new();
    service.merge_from(network);
    service.path = quad_replace_extension(network.path(), ".service", "", "-network");

    if !network.path().as_os_str().is_empty() {
        service.append_entry(
            UNIT_SECTION,
            "SourcePath",
            network
                .path()
                .to_str()
                .expect("EnvironmentFile path is not a valid UTF-8 string"),
        );
    }

    check_for_unknown_keys(network, NETWORK_SECTION, &SUPPORTED_NETWORK_KEYS)?;

    // Rename old Network group to x-Network so that systemd ignores it
    service.rename_section(NETWORK_SECTION, X_NETWORK_SECTION);

    // Derive network name from unit name (with added prefix), or use user-provided name.
    let podman_network_name = network
        .lookup(NETWORK_SECTION, "NetworkName")
        .unwrap_or_default();
    let podman_network_name = if podman_network_name.is_empty() {
        quad_replace_extension(network.path(), "", "systemd-", "")
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string()
    } else {
        podman_network_name.to_string()
    };

    // Need the containers filesystem mounted to start podman
    service.append_entry(UNIT_SECTION, "RequiresMountsFor", "%t/containers");

    let mut podman = get_base_podman_command(network, NETWORK_SECTION);
    podman.add("network");
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

    for ip_addr in network.lookup_all(NETWORK_SECTION, "DNS") {
        podman.add(format!("--dns={ip_addr}"))
    }

    let driver = network.lookup_last(NETWORK_SECTION, "Driver");
    if let Some(driver) = driver {
        if !driver.is_empty() {
            podman.add(format!("--driver={driver}"));
        }
    }

    let subnets: Vec<&str> = network.lookup_all(NETWORK_SECTION, "Subnet");
    let gateways: Vec<&str> = network.lookup_all(NETWORK_SECTION, "Gateway");
    let ip_ranges: Vec<&str> = network.lookup_all(NETWORK_SECTION, "IPRange");
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

    if let Some(ipam_driver) = network.lookup_last(NETWORK_SECTION, "IPAMDriver") {
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

    names.insert(
        service.path().as_os_str().to_os_string(),
        podman_network_name.into(),
    );

    Ok(service)
}

pub(crate) fn from_pod_unit(
    pod: &SystemdUnitFile,
    names: &mut ResourceNameMap,
    pods_info_map: &PodsInfoMap,
) -> Result<SystemdUnitFile, ConversionError> {
    let pod_info = pods_info_map.0.get(&pod.path);
    if pod_info.is_none() {
        return Err(ConversionError::InternalPodError(
            pod.path()
                .to_str()
                .expect("pod unit path is not a valid UTF-8 string")
                .to_string(),
        ));
    }
    let pod_info = pod_info.expect("should not be none");

    let mut service = SystemdUnitFile::new();
    service.merge_from(pod);
    service.path = format!("{}.service", pod_info.service_name).into();

    if !pod.path().as_os_str().is_empty() {
        service.append_entry(
            UNIT_SECTION,
            "SourcePath",
            pod.path()
                .to_str()
                .expect("EnvironmentFile path is not a valid UTF-8 string"),
        );
    }

    check_for_unknown_keys(pod, POD_SECTION, &SUPPORTED_POD_KEYS)?;

    // Derive pod name from unit name (with added prefix), or use user-provided name.
    let podman_pod_name = pod.lookup(POD_SECTION, "PodName").unwrap_or_default();
    let podman_pod_name = if podman_pod_name.is_empty() {
        quad_replace_extension(pod.path(), "", "systemd-", "")
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string()
    } else {
        podman_pod_name.to_string()
    };

    // Rename old Pod group to x-Pod so that systemd ignores it
    service.rename_section(POD_SECTION, X_POD_SECTION);

    // Need the containers filesystem mounted to start podman
    service.append_entry(UNIT_SECTION, "RequiresMountsFor", "%t/containers");

    for container_service in &pod_info.containers {
        let container_service = container_service
            .to_str()
            .expect("container service path is not a valid UTF-8 string");
        service.append_entry(UNIT_SECTION, "Wants", container_service);
        service.append_entry(UNIT_SECTION, "Before", container_service);
    }

    if pod
        .lookup_last(SERVICE_SECTION, "SyslogIdentifier")
        .is_none()
    {
        service.set_entry(SERVICE_SECTION, "SyslogIdentifier", "%N");
    }

    let mut podman_start = get_base_podman_command(pod, POD_SECTION);
    podman_start.add("pod");
    podman_start.add("start");
    podman_start.add("--pod-id-file=%t/%N.pod-id");
    service.append_entry_value(
        SERVICE_SECTION,
        "ExecStart",
        EntryValue::try_from_raw(podman_start.to_escaped_string().as_str())?,
    );

    let mut podman_stop = get_base_podman_command(pod, POD_SECTION);
    podman_stop.add("pod");
    podman_stop.add("stop");
    podman_stop.add("--pod-id-file=%t/%N.pod-id");
    podman_stop.add("--ignore");
    podman_stop.add("--time=10");
    service.append_entry_value(
        SERVICE_SECTION,
        "ExecStop",
        EntryValue::try_from_raw(podman_stop.to_escaped_string().as_str())?,
    );

    let mut podman_stop_post = get_base_podman_command(pod, POD_SECTION);
    podman_stop_post.add("pod");
    podman_stop_post.add("rm");
    podman_stop_post.add("--pod-id-file=%t/%N.pod-id");
    podman_stop_post.add("--ignore");
    podman_stop_post.add("--force");
    service.append_entry_value(
        SERVICE_SECTION,
        "ExecStopPost",
        EntryValue::try_from_raw(podman_stop_post.to_escaped_string().as_str())?,
    );

    let mut podman_start_pre = get_base_podman_command(pod, POD_SECTION);
    podman_start_pre.add("pod");
    podman_start_pre.add("create");
    podman_start_pre.add("--infra-conmon-pidfile=%t/%N.pid");
    podman_start_pre.add("--pod-id-file=%t/%N.pod-id");
    podman_start_pre.add("--exit-policy=stop");
    podman_start_pre.add("--replace");

    handle_publish_ports(pod, POD_SECTION, &mut podman_start_pre)?;

    handle_networks(pod, POD_SECTION, &mut service, names, &mut podman_start_pre)?;

    handle_volumes(pod, POD_SECTION, &mut service, names, &mut podman_start_pre)?;

    podman_start_pre.add(format!("--name={podman_pod_name}"));

    handle_podman_args(pod, POD_SECTION, &mut podman_start_pre);
    service.append_entry_value(
        SERVICE_SECTION,
        "ExecStartPre",
        EntryValue::try_from_raw(podman_start_pre.to_escaped_string().as_str())?,
    );

    service.append_entry(SERVICE_SECTION, "Environment", "PODMAN_SYSTEMD_UNIT=%n");
    service.append_entry(SERVICE_SECTION, "Type", "forking");
    service.append_entry(SERVICE_SECTION, "Restart", "on-failure");
    service.append_entry(SERVICE_SECTION, "PIDFile", "%t/%N.pid");

    Ok(service)
}

// Convert a quadlet volume file (unit file with a Volume group) to a systemd
// service file (unit file with Service group) based on the options in the
// Volume group.
// The original Volume group is kept around as X-Volume.
// Also returns the canonical volume name, either auto-generated or user-defined via the VolumeName
// key-value.
pub(crate) fn from_volume_unit(
    volume: &SystemdUnitFile,
    names: &mut ResourceNameMap,
) -> Result<SystemdUnitFile, ConversionError> {
    let mut service = SystemdUnitFile::new();
    service.merge_from(volume);
    service.path = quad_replace_extension(volume.path(), ".service", "", "-volume");

    if !volume.path().as_os_str().is_empty() {
        service.append_entry(
            UNIT_SECTION,
            "SourcePath",
            volume
                .path()
                .to_str()
                .expect("EnvironmentFile path is not a valid UTF-8 string"),
        );
    }

    check_for_unknown_keys(volume, VOLUME_SECTION, &SUPPORTED_VOLUME_KEYS)?;

    // Rename old Volume group to x-Volume so that systemd ignores it
    service.rename_section(VOLUME_SECTION, X_VOLUME_SECTION);

    // Derive volume name from unit name (with added prefix), or use user-provided name.
    let podman_volume_name = volume
        .lookup(VOLUME_SECTION, "VolumeName")
        .unwrap_or_default();
    let podman_volume_name = if podman_volume_name.is_empty() {
        quad_replace_extension(volume.path(), "", "systemd-", "")
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string()
    } else {
        podman_volume_name.to_string()
    };

    // Need the containers filesystem mounted to start podman
    service.append_entry(UNIT_SECTION, "RequiresMountsFor", "%t/containers");

    let labels = volume.lookup_all_key_val(VOLUME_SECTION, "Label");

    let mut podman = get_base_podman_command(volume, VOLUME_SECTION);
    podman.add("volume");
    podman.add("create");
    // FIXME:  (COMPAT) add `--ignore` once we can rely on Podman v4.4.0 or newer being present
    // Podman support added in: https://github.com/containers/podman/pull/16243
    // Quadlet default changed in: https://github.com/containers/podman/pull/16243
    //podman.add("--ignore")

    let driver = volume.lookup(VOLUME_SECTION, "Driver");
    if let Some(driver) = driver {
        podman.add(format!("--driver={driver}"))
    }

    if driver.unwrap_or_default() == "image" {
        let image_name = volume.lookup(VOLUME_SECTION, "Image");
        if image_name.is_none() {
            return Err(ConversionError::InvalidImageOrRootfs(
                "the key Image is mandatory when using the image driver".into(),
            ));
        }

        let image_name = image_name.expect("cannot be none");
        let image_name = handle_image_source(image_name, &mut service, &names)?;

        podman.add("--opt");
        podman.add(format!("image={image_name}"));
    } else {
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
                    return Err(ConversionError::InvalidDeviceType);
                }
            }
        }

        if let Some(mount_opts) = volume.lookup_last(VOLUME_SECTION, "Options") {
            if !mount_opts.is_empty() {
                if dev_valid {
                    opts.push(mount_opts.into());
                } else {
                    return Err(ConversionError::InvalidDeviceOptions);
                }
            }
        }

        if !opts.is_empty() {
            podman.add("--opt");
            podman.add(format!("o={}", opts.join(",")));
        }
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

    names.insert(
        service.path().as_os_str().to_os_string(),
        podman_volume_name.into(),
    );

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

fn handle_image_source<'a>(
    quadlet_image_name: &'a str,
    service_unit_file: &mut SystemdUnitFile,
    names: &'a ResourceNameMap,
) -> Result<&'a str, ConversionError> {
    if quadlet_image_name.ends_with(".image") {
        //let quadlet_image_name = OsStr::new(quadlet_image_name);

        // since there is no default name conversion, the actual image name must exist in the names map
        let image_name = names.get(&OsString::from(quadlet_image_name));
        if image_name.is_none() {
            return Err(ConversionError::ImageNotFound(quadlet_image_name.into()));
        }

        // the systemd unit name is $name-image.service
        let image_service_name = quad_replace_extension(
            Path::new(image_name.expect("cannot be none")),
            ".service",
            "",
            "-image",
        )
        .to_str()
        .expect("image service name is not a valid UTF-8 string")
        .to_string();
        service_unit_file.append_entry(UNIT_SECTION, "Requires", &image_service_name);
        service_unit_file.append_entry(UNIT_SECTION, "After", &image_service_name);

        let image_name = image_name
            .expect("cannot be none")
            .to_str()
            .expect("image name is not a valid UTF-8 string");
        return Ok(image_name);
    }

    return Ok(quadlet_image_name);
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
    names: &ResourceNameMap,
    podman: &mut PodmanCommand,
) -> Result<(), ConversionError> {
    let networks = quadlet_unit_file.lookup_all(section, "Network");
    for network in networks {
        if !network.is_empty() {
            let mut network_name = network.to_string();
            let mut options: Option<&str> = None;
            if let Some((_network_name, _options)) = network.split_once(':') {
                network_name = _network_name.to_string();
                options = Some(_options);
            }

            if network_name.ends_with(".network") {
                // the podman network name is systemd-$name if none is specified by the user.
                let podman_network_name = names
                    .get(&OsString::from(&network_name))
                    .map(|s| s.to_os_string())
                    .unwrap_or_default();
                let podman_network_name = if podman_network_name.is_empty() {
                    quad_replace_extension(&PathBuf::from(&network_name), "", "systemd-", "")
                        .as_os_str()
                        .to_os_string()
                } else {
                    podman_network_name
                };

                // the systemd unit name is $name-network.service
                let network_service_name = quad_replace_extension(
                    &PathBuf::from(&network_name),
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

                network_name = podman_network_name.to_str().unwrap().to_string();
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
    podman.extend(unit_file.lookup_all_args(section, "PodmanArgs"));
}

fn handle_pod(
    quadlet_unit: &SystemdUnit,
    service_unit_file: &mut SystemdUnitFile,
    section: &str,
    pods_info_map: &mut PodsInfoMap,
    podman: &mut PodmanCommand,
) -> Result<(), ConversionError> {
    if let Some(pod) = quadlet_unit.lookup(section, "Pod") {
        if !pod.is_empty() {
            if !pod.ends_with(".pod") {
                return Err(ConversionError::InvalidPod(pod.into()));
            }

            if let Some(pod_info) = pods_info_map.0.get_mut(&PathBuf::from(pod)) {
                podman.add("--pod-id-file");
                podman.add(format!("%t/{}.pod-id", pod_info.service_name));

                let pod_service_name = format!("{}.service", pod_info.service_name);
                service_unit_file.append_entry(UNIT_SECTION, "BindsTo", &pod_service_name);
                service_unit_file.append_entry(UNIT_SECTION, "After", &pod_service_name);

                pod_info.containers.push(service_unit_file.path.clone());
            } else {
                return Err(ConversionError::PodNotFound(pod.into()));
            }
        }
    }
    Ok(())
}

fn handle_publish_ports(
    unit_file: &SystemdUnit,
    section: &str,
    podman: &mut PodmanCommand,
) -> Result<(), ConversionError> {
    let publish_ports: Vec<&str> = unit_file.lookup_all(section, "PublishPort");
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
                return Err(ConversionError::InvalidPublishedPort(publish_port.into()));
            }
        }

        if ip == "0.0.0.0" {
            ip.clear();
        }

        if !host_port.is_empty() && !is_port_range(host_port.as_str()) {
            return Err(ConversionError::InvalidPortFormat(host_port));
        }

        if !container_port.is_empty() && !is_port_range(container_port.as_str()) {
            return Err(ConversionError::InvalidPortFormat(container_port));
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

fn handle_set_working_directory(
    kube: &SystemdUnitFile,
    service_unit_file: &mut SystemdUnitFile,
) -> Result<(), ConversionError> {
    // If WorkingDirectory is already set in the Service section do not change it
    if let Some(working_dir) = kube.lookup(SERVICE_SECTION, "WorkingDirectory") {
        if !working_dir.is_empty() {
            return Ok(());
        }
    }

    let set_working_directory;
    if let Some(set_working_dir) = kube.lookup(KUBE_SECTION, "SetWorkingDirectory") {
        if set_working_dir.is_empty() {
            return Ok(());
        }
        set_working_directory = set_working_dir;
    } else {
        return Ok(());
    }

    let relative_to_file = match set_working_directory.to_ascii_lowercase().as_str() {
        "yaml" => {
            if let Some(yaml) = kube.lookup(KUBE_SECTION, "Yaml") {
                PathBuf::from(yaml)
            } else {
                return Err(ConversionError::NoYamlKeySpecified);
            }
        }
        "unit" => kube.path().clone(),
        v => {
            return Err(ConversionError::UnsupportedValueForKey(
                "WorkingDirectory".to_string(),
                v.to_string(),
            ))
        }
    };

    let file_in_workingdir = relative_to_file.absolute_from_unit(kube);

    service_unit_file.append_entry(
        SERVICE_SECTION,
        "WorkingDirectory",
        file_in_workingdir
            .parent()
            .expect("should have a parent directory")
            .display()
            .to_string(),
    );

    Ok(())
}

fn handle_storage_source(
    quadlet_unit_file: &SystemdUnitFile,
    service_unit_file: &mut SystemdUnitFile,
    source: &str,
    names: &ResourceNameMap,
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
        // the podman volume name is systemd-$name if none has been provided by the user.
        let volume_name = names
            .get(&OsString::from(&source))
            .map(|s| PathBuf::from(s))
            .unwrap_or_default();
        let volume_name = if volume_name.as_os_str().is_empty() {
            quad_replace_extension(&PathBuf::from(&source), "", "systemd-", "")
        } else {
            volume_name
        };

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

fn handle_user(
    unit_file: &SystemdUnit,
    section: &str,
    podman: &mut PodmanCommand,
) -> Result<(), ConversionError> {
    let user = unit_file.lookup(section, "User");
    let group = unit_file.lookup(section, "Group");

    return match (user, group) {
        // if both are "empty" we return `Ok`
        (None, None) => Ok(()),
        (None, Some(group)) if !group.is_empty() => Err(ConversionError::InvalidGroup),
        (None, Some(_empty)) => Ok(()),
        (Some(user), None) if !user.is_empty() => {
            podman.add(format!("--user={user}"));
            Ok(())
        }
        (Some(_empty), None) => Ok(()),
        (Some(user), Some(group)) if !user.is_empty() && !group.is_empty() => {
            podman.add(format!("--user={user}:{group}"));
            Ok(())
        }
        (Some(_), Some(_)) => Ok(()),
    };
}

fn handle_user_mappings(
    unit_file: &SystemdUnit,
    section: &str,
    podman: &mut PodmanCommand,
    is_user: bool,
    support_manual: bool,
) -> Result<(), ConversionError> {
    let mut mappings_defined = false;

    if let Some(userns) = unit_file.lookup(section, "UserNS") {
        if !userns.is_empty() {
            podman.add("--userns");
            podman.add(userns);
            mappings_defined = true;
        }
    }

    for uid_map in unit_file.lookup_all_strv(section, "UIDMap") {
        podman.add(format!("--uidmap={uid_map}"));
        mappings_defined = true;
    }

    for gid_map in unit_file.lookup_all_strv(section, "GIDMap") {
        podman.add(format!("--gidmap={gid_map}"));
        mappings_defined = true;
    }

    if let Some(sub_uid_map) = unit_file.lookup(section, "SubUIDMap") {
        if !sub_uid_map.is_empty() {
            podman.add("--subuidname");
            podman.add(sub_uid_map);
            mappings_defined = true;
        }
    }

    if let Some(sub_gid_map) = unit_file.lookup(section, "SubGIDMap") {
        if !sub_gid_map.is_empty() {
            podman.add("--subgidname");
            podman.add(sub_gid_map);
            mappings_defined = true;
        }
    }

    if mappings_defined {
        let has_remap_uid = unit_file.lookup(section, "RemapUid").is_some();
        let has_remap_gid = unit_file.lookup(section, "RemapGid").is_some();
        let has_remap_users = unit_file.lookup_last(section, "RemapUsers").is_some();
        if has_remap_uid || has_remap_gid || has_remap_users {
            return Err(ConversionError::InvalidRemapUsers(
                "deprecated Remap keys are set along with explicit mapping keys".into(),
            ));
        }
        return Ok(());
    }

    return handle_user_remap(unit_file, section, podman, is_user, support_manual);
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

    let uid_maps: Vec<String> = unit_file.lookup_all_strv(section, "RemapUid");
    let gid_maps: Vec<String> = unit_file.lookup_all_strv(section, "RemapGid");
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

fn handle_volumes(
    quadlet_unit_file: &SystemdUnitFile,
    section: &str,
    service_unit_file: &mut SystemdUnitFile,
    names: &ResourceNameMap,
    podman: &mut PodmanCommand,
) -> Result<(), ConversionError> {
    for volume in quadlet_unit_file.lookup_all(section, "Volume") {
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
            source = handle_storage_source(quadlet_unit_file, service_unit_file, &source, names);
        }

        podman.add("-v");
        if source.is_empty() {
            podman.add(dest)
        } else {
            podman.add(format!("{source}:{dest}{options}"))
        }
    }

    Ok(())
}

// FindMountType parses the input and extracts the type of the mount type and
// the remaining non-type tokens.
fn find_mount_type(input: &str) -> Result<(String, Vec<String>), ConversionError> {
    // Split by comma, iterate over the slice and look for
    // "type=$mountType". Everything else is appended to tokens.
    let mut csv_reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(input.as_bytes());
    if csv_reader.records().count() != 1 {
        return Err(ConversionError::InvalidMountFormat(input.into()));
    }

    let mut found = false;
    let mut mount_type = String::new();
    let mut tokens = Vec::with_capacity(3);
    let mut csv_reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(input.as_bytes());
    for result in csv_reader.records() {
        let record = dbg!(result)?;
        for field in record.iter() {
            let mut kv = dbg!(field).split('=');
            if dbg!(found) || !(dbg!(kv.clone().count()) == 2 && dbg!(kv.next()) == Some("type")) {
                tokens.push(field.to_string());
                continue;
            }
            mount_type = kv.next().expect("should have type value").to_string();
            found = true;
        }
    }

    if !dbg!(found) {
        return Err(ConversionError::InvalidMountFormat(input.into()));
    }

    Ok((mount_type, tokens))
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

fn lookup_and_add_bool(
    unit_file: &SystemdUnitFile,
    section: &str,
    key: &str,
    flag: &str,
    podman: &mut PodmanCommand,
) {
    if let Some(val) = unit_file.lookup_bool(section, key) {
        podman.add_bool(flag, val);
    }
}

fn lookup_and_add_string(
    unit_file: &SystemdUnitFile,
    section: &str,
    key: &str,
    flag: &str,
    podman: &mut PodmanCommand,
) {
    if let Some(val) = unit_file.lookup(section, key) {
        if !val.is_empty() {
            podman.add(format!("{flag}={val}"));
        }
    }
}

pub(crate) fn quad_replace_extension(
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

fn resolve_container_mount_params(
    container_unit_file: &SystemdUnitFile,
    service_unit_file: &mut SystemdUnitFile,
    mount: String,
    names: &ResourceNameMap,
) -> Result<String, ConversionError> {
    let (mount_type, tokens) = find_mount_type(mount.as_str())?;

    // Source resolution is required only for these types of mounts
    if !(mount_type == "volume" || mount_type == "bind" || mount_type == "glob") {
        return Ok(mount);
    }

    let mut csv_writer = csv::Writer::from_writer(vec![]);
    csv_writer.write_field(format!("type={mount_type}"))?;
    for token in tokens.iter() {
        if token.starts_with("source=") || token.starts_with("src=") {
            if let Some((_k, v)) = token.split_once('=') {
                let resolved_source =
                    handle_storage_source(container_unit_file, service_unit_file, v, names);
                csv_writer.write_field(format!("source={resolved_source}"))?;
            } else {
                return Err(ConversionError::InvalidMountSource);
            }
        } else {
            csv_writer.write_field(token)?;
        }
    }
    csv_writer.write_record(None::<&[u8]>)?;

    return Ok(String::from_utf8(
        csv_writer
            .into_inner()
            .expect("connot convert Mount params back into CSV"),
    )
    .expect("connot convert Mount params back into CSV"));
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
