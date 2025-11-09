use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::systemd_unit::*;

use super::constants::*;
use super::podman_command::PodmanCommand;
use super::*;

fn check_for_unknown_keys(
    unit: &SystemdUnitFile,
    group_name: &str,
    supported_keys: &[&str],
) -> Result<(), ConversionError> {
    for (key, _) in unit.section_entries(group_name) {
        if !supported_keys.contains(&key) {
            return Err(ConversionError::UnknownKey(format!(
                "unsupported key '{key}' in group '{group_name}' in {:?}",
                unit.path()
            )));
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
        let record = result?;
        for field in record.iter() {
            let mut kv = field.split('=');
            if found || !(kv.clone().count() == 2 && kv.next() == Some("type")) {
                tokens.push(field.to_string());
                continue;
            }
            mount_type = kv.next().expect("should have type value").to_string();
            found = true;
        }
    }

    if !found {
        mount_type = "volume".to_string();
    }

    Ok((mount_type, tokens))
}

pub(crate) fn from_build_unit<'q>(
    build_source: &'q QuadletSourceUnitFile,
    units_info_map: &mut UnitsInfoMap,
    is_user: bool,
) -> Result<QuadletServiceUnitFile<'q>, ConversionError> {
    let mut quadlet_service = init_service_unit_file(
        build_source,
        BUILD_SECTION,
        X_BUILD_SECTION,
        &SUPPORTED_BUILD_KEYS,
        units_info_map,
        is_user,
    )?;
    let build = &quadlet_service.quadlet.unit_file;
    let mut service = quadlet_service.service_file;

    // fail fast if resource name is not set
    if build_source.resource_name.is_empty() {
        return Err(ConversionError::NoImageTagKeySpecified);
    }

    let mut podman = get_base_podman_command(build, BUILD_SECTION);
    podman.add("build");

    // The `--pull` flag has to be handled separately and the `=` sign must be present
    // see https://github.com/containers/podman/issues/24599
    if let Some(pull) = build.lookup(BUILD_SECTION, "Pull") {
        if !pull.is_empty() {
            podman.add(format!("--pull={pull}"));
        }
    }

    let string_keys = [
        ("Arch", "--arch"),
        ("AuthFile", "--authfile"),
        ("Target", "--target"),
        ("Variant", "--variant"),
        ("Retry", "--retry"),
        ("RetryDelay", "--retry-delay"),
    ];
    lookup_and_add_string(build, BUILD_SECTION, &string_keys, &mut podman);

    let bool_keys = [("TLSVerify", "--tls-verify"), ("ForceRM", "--force-rm")];
    lookup_and_add_bool(build, BUILD_SECTION, &bool_keys, &mut podman);

    let all_string_keys = [
        ("DNS", "--dns"),
        ("DNSOption", "--dns-option"),
        ("DNSSearch", "--dns-search"),
        ("GroupAdd", "--group-add"),
        ("ImageTag", "--tag"),
    ];
    lookup_and_add_all_strings(build, BUILD_SECTION, &all_string_keys, &mut podman);

    let all_key_val_keys = [
        ("Annotation", "--annotation"),
        ("Environment", "--env"),
        ("Label", "--label"),
    ];
    lookup_and_add_all_key_vals(build, BUILD_SECTION, &all_key_val_keys, &mut podman);

    handle_networks(
        build,
        BUILD_SECTION,
        &mut service,
        units_info_map,
        &mut podman,
    )?;

    podman.extend(
        build
            .lookup_all_args(BUILD_SECTION, "Secret")
            .iter()
            .flat_map(|secret| ["--secret", secret])
            .map(str::to_string),
    );

    handle_volumes(
        build,
        BUILD_SECTION,
        &mut service,
        units_info_map,
        &mut podman,
    )?;

    // In order to build an image locally, we need either a File key pointing directly at a
    // Containerfile, or we need a context or WorkingDirectory containing all required files.
    // SetWorkingDirectory= can also be a path, a URL to either a Containerfile, a Git repo, or
    // an archive.
    let context = handle_set_working_directory(build, &mut service, BUILD_SECTION)?;

    let working_directory = service.lookup(SERVICE_SECTION, "WorkingDirectory");
    let file_path = build.lookup(BUILD_SECTION, "File");
    let (working_directory, file_path) = match (
        working_directory.as_deref(),
        file_path.as_deref(),
        context.as_str(),
    ) {
        (None, None, "") => return Err(ConversionError::NoSetWorkingDirectoryNorFileKeySpecified),
        (None, None, _) => ("", ""),
        (Some(""), None, "") => {
            return Err(ConversionError::NoSetWorkingDirectoryNorFileKeySpecified)
        }
        (Some(wd), None, _) => (wd, ""),
        (None, Some(""), "") => {
            return Err(ConversionError::NoSetWorkingDirectoryNorFileKeySpecified)
        }
        (None, Some(fp), _) => ("", fp),
        (Some(wd), Some(fp), _) => (wd, fp),
    };

    if !file_path.is_empty() {
        podman.add("--file");
        podman.add(file_path);
    }

    handle_podman_args(build, BUILD_SECTION, &mut podman);

    // Context or WorkingDirectory has to be last argument
    if !context.is_empty() {
        podman.add(context);
    } else if !PathBuf::from(file_path).starts_with_systemd_specifier()
        && !PathBuf::from(file_path).is_absolute()
        && !is_url(file_path)
    {
        // Special handling for relative filePaths
        if working_directory.is_empty() {
            return Err(ConversionError::InvalidRelativeFile);
        }
        podman.add(working_directory);
    }

    service.add_raw(
        SERVICE_SECTION,
        "ExecStart",
        podman.to_escaped_string().as_str(),
    )?;

    handle_one_shot_service_section(&mut service, false);

    quadlet_service.service_file = service;
    Ok(quadlet_service)
}

// Convert a quadlet container file (unit file with a Container group) to a systemd
// service file (unit file with Service group) based on the options in the Container group.
// The original Container group is kept around as X-Container.
pub(crate) fn from_container_unit<'q>(
    container_source: &'q QuadletSourceUnitFile,
    units_info_map: &mut UnitsInfoMap,
    is_user: bool,
) -> Result<QuadletServiceUnitFile<'q>, ConversionError> {
    let mut quadlet_service = init_service_unit_file(
        container_source,
        CONTAINER_SECTION,
        X_CONTAINER_SECTION,
        &SUPPORTED_CONTAINER_KEYS,
        units_info_map,
        is_user,
    )?;
    let container = &quadlet_service.quadlet.unit_file;
    let mut service = quadlet_service.service_file;

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
        handle_image_source(&image, &mut service, units_info_map)?.to_string()
    } else {
        image
    };

    let podman_container_name = get_container_name(container);

    // Set PODMAN_SYSTEMD_UNIT so that podman auto-update can restart the service.
    service.add(SERVICE_SECTION, "Environment", "PODMAN_SYSTEMD_UNIT=%n");

    // Only allow mixed or control-group, as nothing else works well
    let kill_mode = service.lookup_last(SERVICE_SECTION, "KillMode");
    match kill_mode.as_deref() {
        None | Some("mixed") | Some("control-group") => {
            // We default to mixed instead of control-group, because it lets conmon do its thing
            service.set(SERVICE_SECTION, "KillMode", "mixed");
        }
        Some(kill_mode) => {
            return Err(ConversionError::InvalidKillMode(kill_mode.into()));
        }
    }

    // If conmon exited uncleanly it may not have removed the container, so
    // force it, -i makes it ignore non-existing files.
    let mut service_stop_cmd = get_base_podman_command(container, CONTAINER_SECTION);
    service_stop_cmd.add_slice(&["rm", "-v", "-f", "-i", &podman_container_name]);
    service.add_raw(
        SERVICE_SECTION,
        "ExecStop",
        service_stop_cmd.to_escaped_string().as_str(),
    )?;
    // The ExecStopPost is needed when the main PID (i.e., conmon) gets killed.
    // In that case, ExecStop is not executed but *Post only.  If both are
    // fired in sequence, *Post will exit when detecting that the --cidfile
    // has already been removed by the previous `rm`..
    service_stop_cmd.args[0] = format!("-{}", service_stop_cmd.args[0]);
    service.add_raw(
        SERVICE_SECTION,
        "ExecStopPost",
        service_stop_cmd.to_escaped_string().as_str(),
    )?;

    handle_exec_reload(
        container,
        &mut service,
        CONTAINER_SECTION,
        &podman_container_name,
    )?;

    let mut podman = get_base_podman_command(container, CONTAINER_SECTION);

    podman.add("run");

    podman.add("--name");
    podman.add(&podman_container_name);

    // And replace any previous container with the same name, not fail
    podman.add("--replace");

    // On clean shutdown, remove container
    podman.add("--rm");

    handle_log_driver(container, CONTAINER_SECTION, &mut podman);
    handle_log_opt(container, CONTAINER_SECTION, &mut podman);

    // We delegate groups to the runtime
    service.add(SERVICE_SECTION, "Delegate", "yes");

    let cgroups_mode = container.lookup(CONTAINER_SECTION, "CgroupsMode").map_or(
        "split".to_string(),
        |cgroups_mode| {
            if cgroups_mode.is_empty() {
                return "split".to_string();
            }

            cgroups_mode
        },
    );
    podman.add("--cgroups");
    podman.add(cgroups_mode);

    let string_keys = [
        ("Timezone", "--tz"),
        ("PidsLimit", "--pids-limit"),
        ("ShmSize", "--shm-size"),
        ("Entrypoint", "--entrypoint"),
        ("WorkingDir", "--workdir"),
        ("IP", "--ip"),
        ("IP6", "--ip6"),
        ("HostName", "--hostname"),
        ("StopSignal", "--stop-signal"),
        ("StopTimeout", "--stop-timeout"),
        ("Pull", "--pull"),
        ("Memory", "--memory"),
        ("Retry", "--retry"),
        ("RetryDelay", "--retry-delay"),
    ];
    lookup_and_add_string(container, CONTAINER_SECTION, &string_keys, &mut podman);

    let all_string_keys = [
        ("NetworkAlias", "--network-alias"),
        ("Ulimit", "--ulimit"),
        ("DNS", "--dns"),
        ("DNSOption", "--dns-option"),
        ("DNSSearch", "--dns-search"),
        ("GroupAdd", "--group-add"),
        ("AddHost", "--add-host"),
        ("Tmpfs", "--tmpfs"),
    ];
    lookup_and_add_all_strings(container, CONTAINER_SECTION, &all_string_keys, &mut podman);

    let bool_keys = [
        ("EnvironmentHost", "--env-host"),
        ("HttpProxy", "--http-proxy"),
        ("ReadOnlyTmpfs", "--read-only-tmpfs"),
        ("RunInit", "--init"),
    ];
    lookup_and_add_bool(container, CONTAINER_SECTION, &bool_keys, &mut podman);

    handle_networks(
        container,
        CONTAINER_SECTION,
        &mut service,
        units_info_map,
        &mut podman,
    )?;

    let service_type = container.lookup_last(SERVICE_SECTION, "Type");
    match service_type.as_deref() {
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
            service.set(SERVICE_SECTION, "Type", "notify");
            service.set(SERVICE_SECTION, "NotifyAccess", "all");

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
        service.set(SERVICE_SECTION, "SyslogIdentifier", "%N");
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
        podman.add_slice(&["--security-opt", "label=disable"]);
    }

    let security_label_nested = container
        .lookup_bool(CONTAINER_SECTION, "SecurityLabelNested")
        .unwrap_or(false);
    if security_label_nested {
        podman.add_slice(&["--security-opt", "label=nested"]);
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
        podman.add("--device");
        podman.add(device);
    }

    // Default to no higher level privileges or caps
    if let Some(seccomp_profile) = container.lookup_last(CONTAINER_SECTION, "SeccompProfile") {
        podman.add_slice(&["--security-opt", &format!("seccomp={seccomp_profile}")])
    }

    for caps in container.lookup_all_strv(CONTAINER_SECTION, "DropCapability") {
        podman.add("--cap-drop");
        podman.add(caps.to_ascii_lowercase());
    }

    // But allow overrides with AddCapability
    for caps in container.lookup_all_strv(CONTAINER_SECTION, "AddCapability") {
        podman.add("--cap-add");
        podman.add(caps.to_ascii_lowercase());
    }

    for sysctl in container.lookup_all_strv(CONTAINER_SECTION, "Sysctl") {
        podman.add("--sysctl");
        podman.add(sysctl);
    }

    let read_only = container.lookup_bool(CONTAINER_SECTION, "ReadOnly");
    if let Some(read_only) = read_only {
        podman.add_bool("--read-only", read_only);
    }
    let read_only = read_only.unwrap_or(false); // key not found: use default

    let volatile_tmp = container
        .lookup_bool(CONTAINER_SECTION, "VolatileTmp")
        .unwrap_or(false);
    if volatile_tmp && !read_only {
        podman.add_slice(&["--tmpfs", "/tmp:rw,size=512M,mode=1777"]);
    }

    handle_user(container, CONTAINER_SECTION, &mut podman)?;

    handle_user_mappings(container, CONTAINER_SECTION, &mut podman, true)?;

    handle_volumes(
        &container,
        CONTAINER_SECTION,
        &mut service,
        units_info_map,
        &mut podman,
    )?;

    if let Some(update) = container.lookup(CONTAINER_SECTION, "AutoUpdate") {
        if !update.is_empty() {
            let mut labels = HashMap::new();
            labels.insert(AUTO_UPDATE_LABEL.to_string(), Some(update.to_string()));
            podman.add_keys("--label", &labels);
        }
    }

    for exposed_port in container.lookup_all(CONTAINER_SECTION, "ExposeHostPort") {
        let exposed_port = exposed_port.trim(); // Allow whitespaces before and after

        if !is_port_range(exposed_port) {
            return Err(ConversionError::InvalidPortFormat(exposed_port.into()));
        }

        podman.add("--expose");
        podman.add(exposed_port);
    }

    handle_publish_ports(container, CONTAINER_SECTION, &mut podman);

    let all_key_val_keys = [
        ("Annotation", "--annotation"),
        ("Environment", "--env"),
        ("Label", "--label"),
    ];
    lookup_and_add_all_key_vals(container, CONTAINER_SECTION, &all_key_val_keys, &mut podman);

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
        podman.add(env_file.to_unwrapped_str());
    }

    podman.extend(
        container
            .lookup_all_args(CONTAINER_SECTION, "Secret")
            .iter()
            .flat_map(|secret| ["--secret", secret])
            .map(str::to_string),
    );

    for mount in container.lookup_all_args(CONTAINER_SECTION, "Mount") {
        let mount_str =
            resolve_container_mount_params(container, &mut service, mount, units_info_map)?;
        podman.add("--mount");
        podman.add(mount_str);
    }

    handle_health(container, CONTAINER_SECTION, &mut podman);

    handle_pod(
        container,
        &mut service,
        CONTAINER_SECTION,
        units_info_map,
        &mut podman,
    )?;

    handle_podman_args(container, CONTAINER_SECTION, &mut podman);

    if !image.is_empty() {
        podman.add(image);
    } else {
        podman.add("--rootfs");
        podman.add(rootfs);
    }

    let exec_args = container.lookup_all_args(CONTAINER_SECTION, "Exec");
    podman.extend(exec_args);

    service.add_raw(
        SERVICE_SECTION,
        "ExecStart",
        podman.to_escaped_string().as_str(),
    )?;

    quadlet_service.service_file = service;
    Ok(quadlet_service)
}

pub(crate) fn from_image_unit<'q>(
    image_source: &'q QuadletSourceUnitFile,
    units_info_map: &mut UnitsInfoMap,
    is_user: bool,
) -> Result<QuadletServiceUnitFile<'q>, ConversionError> {
    let mut quadlet_service = init_service_unit_file(
        image_source,
        IMAGE_SECTION,
        X_IMAGE_SECTION,
        &SUPPORTED_IMAGE_KEYS,
        units_info_map,
        is_user,
    )?;
    let image = &quadlet_service.quadlet.unit_file;
    let mut service = quadlet_service.service_file;

    let image_name = image
        .lookup_last(IMAGE_SECTION, "Image")
        .unwrap_or_default();
    if image_name.is_empty() {
        return Err(ConversionError::InvalidImageOrRootfs(
            "no Image key specified".into(),
        ));
    }

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
        ("Policy", "--policy"),
        ("Variant", "--variant"),
        ("Retry", "--retry"),
        ("RetryDelay", "--retry-delay"),
    ];
    lookup_and_add_string(image, IMAGE_SECTION, &string_keys, &mut podman);

    let bool_keys = [("AllTags", "--all-tags"), ("TLSVerify", "--tls-verify")];
    lookup_and_add_bool(image, IMAGE_SECTION, &bool_keys, &mut podman);

    handle_podman_args(image, IMAGE_SECTION, &mut podman);

    podman.add(image_name.clone());

    service.add_raw(
        SERVICE_SECTION,
        "ExecStart",
        podman.to_escaped_string().as_str(),
    )?;

    handle_one_shot_service_section(&mut service, true);

    let podman_image_name = if let Some(image) = image.lookup(IMAGE_SECTION, "ImageTag") {
        if !image.is_empty() {
            image
        } else {
            image_name
        }
    } else {
        image_name
    };

    if let Some(unit_info) = units_info_map.get_mut_source_unit_info(image) {
        // Store the name of the created resource
        unit_info.resource_name = podman_image_name.to_string();
    };

    quadlet_service.service_file = service;
    Ok(quadlet_service)
}

pub(crate) fn from_kube_unit<'q>(
    kube_source: &'q QuadletSourceUnitFile,
    units_info_map: &mut UnitsInfoMap,
    is_user: bool,
) -> Result<QuadletServiceUnitFile<'q>, ConversionError> {
    let mut quadlet_service = init_service_unit_file(
        kube_source,
        KUBE_SECTION,
        X_KUBE_SECTION,
        &SUPPORTED_KUBE_KEYS,
        units_info_map,
        is_user,
    )?;
    let kube = &quadlet_service.quadlet.unit_file;
    let mut service = quadlet_service.service_file;

    let yaml_path = kube.lookup_last(KUBE_SECTION, "Yaml").unwrap_or_default();
    if yaml_path.is_empty() {
        return Err(ConversionError::NoYamlKeySpecified);
    }

    let yaml_path = PathBuf::from(yaml_path).absolute_from_unit(kube);

    // Only allow mixed or control-group, as nothing else works well
    let kill_mode = kube.lookup_last(KUBE_SECTION, "KillMode");
    match kill_mode.as_deref() {
        None | Some("mixed") | Some("control-group") => {
            // We default to mixed instead of control-group, because it lets conmon do its thing
            service.set(SERVICE_SECTION, "KillMode", "mixed");
        }
        Some(kill_mode) => {
            return Err(ConversionError::InvalidKillMode(kill_mode.into()));
        }
    }

    // Set PODMAN_SYSTEMD_UNIT so that podman auto-update can restart the service.
    service.add(SERVICE_SECTION, "Environment", "PODMAN_SYSTEMD_UNIT=%n");

    // Allow users to set the Service Type to oneshot to allow resources only kube yaml
    match service.lookup(SERVICE_SECTION, "Type") {
        None => {
            service.add(SERVICE_SECTION, "Type", "notify");
            service.add(SERVICE_SECTION, "NotifyAccess", "all");
        }
        // could be combined with the case above
        Some(service_type) if service_type != "oneshot" => {
            service.add(SERVICE_SECTION, "Type", "notify");
            service.add(SERVICE_SECTION, "NotifyAccess", "all");
        }
        Some(service_type) => {
            if service_type != "notify" && service_type != "oneshot" {
                return Err(ConversionError::InvalidServiceType(service_type.into()));
            }
        }
    }

    if !kube.has_key(SERVICE_SECTION, "SyslogIdentifier") {
        service.set(SERVICE_SECTION, "SyslogIdentifier", "%N");
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
    handle_log_opt(kube, KUBE_SECTION, &mut podman_start);

    handle_user_mappings(kube, KUBE_SECTION, &mut podman_start, false)?;

    handle_networks(
        kube,
        KUBE_SECTION,
        &mut service,
        units_info_map,
        &mut podman_start,
    )?;

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
        podman_start.add(config_map_path.to_unwrapped_str());
    }

    handle_publish_ports(kube, KUBE_SECTION, &mut podman_start);

    handle_podman_args(kube, KUBE_SECTION, &mut podman_start);

    podman_start.add(yaml_path.to_unwrapped_str());

    service.add_raw(
        SERVICE_SECTION,
        "ExecStart",
        podman_start.to_escaped_string().as_str(),
    )?;

    // Use `ExecStopPost` to make sure cleanup happens even in case of
    // errors; otherwise containers, pods, etc. would be left behind.
    let mut podman_stop = get_base_podman_command(kube, KUBE_SECTION);
    podman_stop.add("kube");
    podman_stop.add("down");

    if let Some(kube_down_force) = kube.lookup_bool(KUBE_SECTION, "KubeDownForce") {
        podman_stop.add_bool("--force", kube_down_force)
    }

    podman_stop.add(yaml_path.to_unwrapped_str());
    service.add_raw(
        SERVICE_SECTION,
        "ExecStopPost",
        podman_stop.to_escaped_string().as_str(),
    )?;

    handle_set_working_directory(kube, &mut service, KUBE_SECTION)?;

    quadlet_service.service_file = service;
    Ok(quadlet_service)
}

// Convert a quadlet network file (unit file with a Network group) to a systemd
// service file (unit file with Service group) based on the options in the Network group.
// The original Network group is kept around as X-Network.
// Also returns the canonical network name, either auto-generated or user-defined via the
// NetworkName key-value.
pub(crate) fn from_network_unit<'q>(
    network_source: &'q QuadletSourceUnitFile,
    units_info_map: &mut UnitsInfoMap,
    is_user: bool,
) -> Result<QuadletServiceUnitFile<'q>, ConversionError> {
    let mut quadlet_service = init_service_unit_file(
        network_source,
        NETWORK_SECTION,
        X_NETWORK_SECTION,
        &SUPPORTED_NETWORK_KEYS,
        units_info_map,
        is_user,
    )?;
    let network = &quadlet_service.quadlet.unit_file;
    let mut service = quadlet_service.service_file;

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

    if network
        .lookup_bool(NETWORK_SECTION, "NetworkDeleteOnStop")
        .unwrap_or(false)
    {
        let mut podman_stop_post = get_base_podman_command(network, NETWORK_SECTION);
        podman_stop_post.add_slice(&["network", "rm", &podman_network_name]);
        service.add_raw(
            SERVICE_SECTION,
            "ExecStopPost",
            podman_stop_post.to_escaped_string().as_str(),
        )?
    }

    let mut podman = get_base_podman_command(network, NETWORK_SECTION);
    podman.add("network");
    podman.add("create");
    podman.add("--ignore");

    let bool_keys = [
        ("DisableDNS", "--disable-dns"),
        ("Internal", "--internal"),
        ("IPv6", "--ipv6"),
    ];
    lookup_and_add_bool(network, NETWORK_SECTION, &bool_keys, &mut podman);

    let string_keys = [
        ("Driver", "--driver"),
        ("InterfaceName", "--interface-name"),
        ("IPAMDriver", "--ipam-driver"),
    ];
    lookup_and_add_string(network, NETWORK_SECTION, &string_keys, &mut podman);

    lookup_and_add_all_strings(network, NETWORK_SECTION, &[("DNS", "--dns")], &mut podman);

    let subnets = network.lookup_all(NETWORK_SECTION, "Subnet");
    let gateways = network.lookup_all(NETWORK_SECTION, "Gateway");
    let ip_ranges = network.lookup_all(NETWORK_SECTION, "IPRange");
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
            podman.add("--subnet");
            podman.add(subnet);
            if i < gateways.len() {
                podman.add("--gateway");
                podman.add(gateways[i].as_str());
            }
            if i < ip_ranges.len() {
                podman.add("--ip-range");
                podman.add(ip_ranges[i].as_str());
            }
        }
    } else if !gateways.is_empty() || !ip_ranges.is_empty() {
        return Err(ConversionError::InvalidSubnet(
            "cannot set Gateway or IPRange without Subnet".into(),
        ));
    }

    let all_key_val_keys = [("Label", "--label"), ("Options", "--opt")];
    lookup_and_add_all_key_vals(network, NETWORK_SECTION, &all_key_val_keys, &mut podman);

    handle_podman_args(network, NETWORK_SECTION, &mut podman);

    podman.add(&podman_network_name);

    service.add_raw(
        SERVICE_SECTION,
        "ExecStart",
        podman.to_escaped_string().as_str(),
    )?;

    handle_one_shot_service_section(&mut service, true);

    if let Some(unit_info) = units_info_map.get_mut_source_unit_info(network) {
        // Store the name of the created resource
        unit_info.resource_name = podman_network_name;
    }

    quadlet_service.service_file = service;
    Ok(quadlet_service)
}

pub(crate) fn from_pod_unit<'q>(
    pod_source: &'q QuadletSourceUnitFile,
    units_info_map: &mut UnitsInfoMap,
    is_user: bool,
) -> Result<QuadletServiceUnitFile<'q>, ConversionError> {
    let mut quadlet_service = init_service_unit_file(
        pod_source,
        POD_SECTION,
        X_POD_SECTION,
        &SUPPORTED_POD_KEYS,
        units_info_map,
        is_user,
    )?;
    let pod = &quadlet_service.quadlet.unit_file;
    let mut service = quadlet_service.service_file;

    let podman_pod_name = &pod_source.resource_name;

    let unit_info = units_info_map
        .get_source_unit_info(pod)
        .ok_or_else(|| ConversionError::InternalQuadletError("pod".into(), pod.path().into()))?;
    // FIXME: why can't this be `&quadlet_service.quadlet.containers_to_start`
    for container_service in &unit_info.containers_to_start {
        let container_service = container_service.to_unwrapped_str();
        service.add(UNIT_SECTION, "Wants", container_service);
        service.add(UNIT_SECTION, "Before", container_service);
    }

    if pod
        .lookup_last(SERVICE_SECTION, "SyslogIdentifier")
        .is_none()
    {
        service.set(SERVICE_SECTION, "SyslogIdentifier", "%N");
    }

    let mut podman_start = get_base_podman_command(pod, POD_SECTION);
    podman_start.add("pod");
    podman_start.add("start");
    podman_start.add(podman_pod_name);
    service.add_raw(
        SERVICE_SECTION,
        "ExecStart",
        podman_start.to_escaped_string().as_str(),
    )?;

    let mut podman_stop = get_base_podman_command(pod, POD_SECTION);
    podman_stop.add("pod");
    podman_stop.add("stop");
    podman_stop.add("--ignore");

    let mut stop_timeout = String::from("10");
    if let Some(timeout) = pod_source.unit_file.lookup(POD_SECTION, "StopTimeout") {
        stop_timeout = timeout;
    }
    podman_stop.add(format!("--time={stop_timeout}"));

    podman_stop.add(podman_pod_name);
    service.add_raw(
        SERVICE_SECTION,
        "ExecStop",
        podman_stop.to_escaped_string().as_str(),
    )?;

    let mut podman_stop_post = get_base_podman_command(pod, POD_SECTION);
    podman_stop_post.add("pod");
    podman_stop_post.add("rm");
    podman_stop_post.add("--ignore");
    podman_stop_post.add("--force");
    podman_stop_post.add(podman_pod_name);
    service.add_raw(
        SERVICE_SECTION,
        "ExecStopPost",
        podman_stop_post.to_escaped_string().as_str(),
    )?;

    let mut podman_start_pre = get_base_podman_command(pod, POD_SECTION);
    podman_start_pre.add("pod");
    podman_start_pre.add("create");
    podman_start_pre.add("--infra-conmon-pidfile=%t/%N.pid");
    podman_start_pre.add("--replace");

    if let Some(exit_policy) = pod.lookup(POD_SECTION, "ExitPolicy") {
        podman_start_pre.add(format!("--exit-policy={exit_policy}"));
    } else {
        podman_start_pre.add("--exit-policy=stop");
    }

    handle_user_mappings(pod, POD_SECTION, &mut podman_start_pre, true)?;

    handle_publish_ports(pod, POD_SECTION, &mut podman_start_pre);

    let all_key_val_keys = [("Label", "--label")];
    lookup_and_add_all_key_vals(pod, POD_SECTION, &all_key_val_keys, &mut podman_start_pre);

    handle_networks(
        pod,
        POD_SECTION,
        &mut service,
        units_info_map,
        &mut podman_start_pre,
    )?;

    let string_keys = [("IP", "--ip"), ("IP6", "--ip6"), ("ShmSize", "--shm-size")];
    lookup_and_add_string(&pod, POD_SECTION, &string_keys, &mut podman_start_pre);

    let all_string_keys = [
        ("NetworkAlias", "--network-alias"),
        ("DNS", "--dns"),
        ("DNSOption", "--dns-option"),
        ("DNSSearch", "--dns-search"),
        ("AddHost", "--add-host"),
        ("HostName", "--hostname"),
    ];
    lookup_and_add_all_strings(&pod, POD_SECTION, &all_string_keys, &mut podman_start_pre);

    handle_volumes(
        pod,
        POD_SECTION,
        &mut service,
        units_info_map,
        &mut podman_start_pre,
    )?;

    podman_start_pre.add("--infra-name");
    podman_start_pre.add(format!("{podman_pod_name}-infra"));
    podman_start_pre.add("--name");
    podman_start_pre.add(podman_pod_name);

    handle_podman_args(pod, POD_SECTION, &mut podman_start_pre);
    service.add_raw(
        SERVICE_SECTION,
        "ExecStartPre",
        podman_start_pre.to_escaped_string().as_str(),
    )?;

    // Set PODMAN_SYSTEMD_UNIT so that podman auto-update can restart the service.
    service.add(SERVICE_SECTION, "Environment", "PODMAN_SYSTEMD_UNIT=%n");

    service.add(SERVICE_SECTION, "Type", "forking");
    service.add(SERVICE_SECTION, "Restart", "on-failure");
    service.add(SERVICE_SECTION, "PIDFile", "%t/%N.pid");

    if let Some(unit_info) = units_info_map.get_mut_source_unit_info(pod) {
        // Store the name of the created resource
        unit_info.resource_name = podman_pod_name.to_string();
    };

    quadlet_service.service_file = service;
    Ok(quadlet_service)
}

// Convert a quadlet volume file (unit file with a Volume group) to a systemd
// service file (unit file with Service group) based on the options in the
// Volume group.
// The original Volume group is kept around as X-Volume.
// Also returns the canonical volume name, either auto-generated or user-defined via the VolumeName
// key-value.
pub(crate) fn from_volume_unit<'q>(
    volume_source: &'q QuadletSourceUnitFile,
    units_info_map: &mut UnitsInfoMap,
    is_user: bool,
) -> Result<QuadletServiceUnitFile<'q>, ConversionError> {
    let mut quadlet_service = init_service_unit_file(
        volume_source,
        VOLUME_SECTION,
        X_VOLUME_SECTION,
        &SUPPORTED_VOLUME_KEYS,
        units_info_map,
        is_user,
    )?;
    let volume = &quadlet_service.quadlet.unit_file;
    let mut service = quadlet_service.service_file;

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
    // Store the name of the created resource
    units_info_map
        .get_mut_source_unit_info(volume)
        .map(|unit_info| unit_info.resource_name = podman_volume_name.clone());

    let mut podman = get_base_podman_command(volume, VOLUME_SECTION);
    podman.add("volume");
    podman.add("create");
    podman.add("--ignore");

    let driver = volume.lookup(VOLUME_SECTION, "Driver");
    if let Some(driver) = driver.as_deref() {
        podman.add("--driver");
        podman.add(driver);
    }

    if driver.unwrap_or_default() == "image" {
        let image_name = volume.lookup(VOLUME_SECTION, "Image").ok_or_else(|| {
            ConversionError::InvalidImageOrRootfs(
                "the key Image is mandatory when using the image driver".into(),
            )
        })?;

        let image_name = handle_image_source(image_name.as_str(), &mut service, &units_info_map)?;

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

        let device = volume.lookup(VOLUME_SECTION, "Device").unwrap_or_default();
        if !device.is_empty() {
            podman.add("--opt");
            podman.add(format!("device={device}"));
        }
        let device_valid = !device.is_empty();

        if let Some(dev_type) = volume.lookup(VOLUME_SECTION, "Type") {
            if !dev_type.is_empty() {
                if device_valid {
                    podman.add("--opt");
                    podman.add(format!("type={dev_type}"));
                    if dev_type == "bind" {
                        service.add(UNIT_SECTION, "RequiresMountsFor", &device);
                    }
                } else {
                    return Err(ConversionError::InvalidDeviceType);
                }
            }
        }

        if let Some(mount_opts) = volume.lookup(VOLUME_SECTION, "Options") {
            if !mount_opts.is_empty() {
                if device_valid {
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

    let all_key_val_keys = [("Label", "--label")];
    lookup_and_add_all_key_vals(volume, VOLUME_SECTION, &all_key_val_keys, &mut podman);

    handle_podman_args(volume, VOLUME_SECTION, &mut podman);

    podman.add(&podman_volume_name);

    service.add_raw(
        SERVICE_SECTION,
        "ExecStart",
        podman.to_escaped_string().as_str(),
    )?;

    handle_one_shot_service_section(&mut service, true);

    quadlet_service.service_file = service;
    Ok(quadlet_service)
}

fn get_base_podman_command(unit: &SystemdUnitFile, section: &str) -> PodmanCommand {
    let mut podman = PodmanCommand::new();

    lookup_and_add_all_strings(
        unit,
        section,
        &[("ContainersConfModule", "--module")],
        &mut podman,
    );

    podman.extend(unit.lookup_all_args(section, "GlobalArgs"));

    podman
}

fn handle_default_dependencies(service: &mut SystemdUnitFile, is_user: bool) {
    // Add a dependency on network-online.target so the image pull does not happen
    // before network is ready.
    // see https://github.com/containers/podman/issues/21873
    if service
        .lookup_bool(QUADLET_SECTION, "DefaultDependencies")
        .unwrap_or(true)
    {
        let mut network_unit = "network-online.target";
        // network-online.target only exists as root and user session cannot wait for it.
        // Given this pasta will fail to start or use the wrong interface if the network
        // is not fully set up. We need to work around that.
        // see https://github.com/containers/podman/issues/22197
        if is_user {
            network_unit = "network-online.target";
        }
        service.prepend(UNIT_SECTION, "After", network_unit);
        service.prepend(UNIT_SECTION, "Wants", network_unit);
    }
}

// this function handles the ExecReload key
fn handle_exec_reload(
    quadlet: &SystemdUnitFile,
    service: &mut SystemdUnitFile,
    quadlet_section: &str,
    container_name: &str,
) -> Result<(), ConversionError> {
    let reload_cmd: Vec<String> = quadlet
        .lookup_last_value(quadlet_section, "ReloadCmd")
        .unwrap_or(&EntryValue::default())
        .split_words()
        .collect();
    let reload_signal = quadlet
        .lookup(quadlet_section, "ReloadSignal")
        .unwrap_or_default();

    if !reload_cmd.is_empty() && !reload_signal.is_empty() {
        return Err(ConversionError::MutuallyExclusiveKeys(
            "ReloadCmd".into(),
            "ReloadSignal".into(),
        ));
    }

    // bail if both keys are empty
    if reload_cmd.is_empty() && reload_signal.is_empty() {
        return Ok(());
    }

    let mut podman_reload = get_base_podman_command(quadlet, quadlet_section);
    if !reload_cmd.is_empty() {
        podman_reload.add_slice(&["exec", &container_name]);
        podman_reload.extend(reload_cmd);
    } else {
        podman_reload.add_slice(&["kill", "--signal", &reload_signal, &container_name]);
    }
    service.add_raw(
        SERVICE_SECTION,
        "ExecReload",
        podman_reload.to_escaped_string().as_str(),
    )?;

    Ok(())
}

fn handle_health(unit_file: &SystemdUnitData, section: &str, podman: &mut PodmanCommand) {
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
    units_info_map: &'a UnitsInfoMap,
) -> Result<&'a str, ConversionError> {
    for extension in ["build", "image"] {
        if quadlet_image_name.ends_with(&format!(".{extension}")) {
            // since there is no default name conversion, the actual image name must exist in the names map
            let unit_info = units_info_map
                .0
                .get(&OsString::from(quadlet_image_name))
                .ok_or_else(|| ConversionError::ImageNotFound(quadlet_image_name.into()))?;

            // the systemd unit name is $name-$suffix.service
            let image_service_name = unit_info
                .get_service_file_name()
                .to_str()
                .expect("image service name is not a valid UTF-8 string")
                .to_string();
            service_unit_file.add(UNIT_SECTION, "Requires", &image_service_name);
            service_unit_file.add(UNIT_SECTION, "After", &image_service_name);

            let image_name = unit_info.resource_name.as_str();
            return Ok(image_name);
        }
    }

    return Ok(quadlet_image_name);
}

fn handle_log_driver(unit_file: &SystemdUnitData, section: &str, podman: &mut PodmanCommand) {
    if let Some(log_driver) = unit_file.lookup_last(section, "LogDriver") {
        podman.add("--log-driver");
        podman.add(log_driver);
    }
}

fn handle_log_opt(unit_file: &SystemdUnitData, section: &str, podman: &mut PodmanCommand) {
    podman.extend(
        unit_file
            .lookup_all_strv(section, "LogOpt")
            .iter()
            .flat_map(|log_opt| ["--log-opt", log_opt])
            .map(str::to_string),
    )
}

fn handle_networks(
    quadlet_unit_file: &SystemdUnitData,
    section: &str,
    service_unit_file: &mut SystemdUnitData,
    units_info_map: &UnitsInfoMap,
    podman: &mut PodmanCommand,
) -> Result<(), ConversionError> {
    for network in quadlet_unit_file.lookup_all(section, "Network") {
        if !network.is_empty() {
            let mut quadlet_network_name = network.as_str();
            let mut options: Option<&str> = None;
            if let Some((_network_name, _options)) = network.split_once(':') {
                quadlet_network_name = _network_name;
                options = Some(_options);
            }

            let is_network_unit = quadlet_network_name.ends_with(".network");
            let is_container_unit = quadlet_network_name.ends_with(".container");

            if is_network_unit || is_container_unit {
                let unit_info = units_info_map
                    .0
                    .get(&OsString::from(&quadlet_network_name))
                    .ok_or_else(|| {
                        ConversionError::InternalQuadletError(
                            "unit".into(),
                            quadlet_network_name.into(),
                        )
                    })?;

                // XXX: this is usually because a '@' in service name
                if unit_info.resource_name.is_empty() {
                    return Err(ConversionError::InvalidResourceNameIn(
                        quadlet_network_name.into(),
                    ));
                }

                // the systemd unit name is $name-network.service
                let service_file_name = unit_info.get_service_file_name();
                service_unit_file.add(
                    UNIT_SECTION,
                    "Requires",
                    service_file_name.to_str().unwrap(),
                );
                service_unit_file.add(UNIT_SECTION, "After", service_file_name.to_str().unwrap());

                quadlet_network_name = unit_info.resource_name.as_str();
            }

            podman.add("--network");
            if let Some(options) = options {
                if is_container_unit {
                    return Err(ConversionError::InvalidNetworkOptions);
                }
                podman.add(format!("{quadlet_network_name}:{options}"));
            } else {
                if is_container_unit {
                    podman.add(format!("container:{quadlet_network_name}"));
                } else {
                    podman.add(format!("{quadlet_network_name}"));
                }
            }
        }
    }

    Ok(())
}

fn handle_one_shot_service_section(service: &mut SystemdUnitFile, remain_after_exit: bool) {
    // The default syslog identifier is the exec basename (podman) which isn't very useful here
    if service
        .lookup(SERVICE_SECTION, "SyslogIdentifier")
        .is_none()
    {
        service.set(SERVICE_SECTION, "SyslogIdentifier", "%N")
    }
    if service.lookup(SERVICE_SECTION, "Type").is_none() {
        service.set(SERVICE_SECTION, "Type", "oneshot")
    }
    if remain_after_exit {
        if service.lookup(SERVICE_SECTION, "RemainAfterExit").is_none() {
            service.set(SERVICE_SECTION, "RemainAfterExit", "yes")
        }
    }
}

fn handle_podman_args(unit_file: &SystemdUnitData, section: &str, podman: &mut PodmanCommand) {
    podman.extend(unit_file.lookup_all_args(section, "PodmanArgs"));
}

fn handle_pod(
    quadlet_unit: &SystemdUnitData,
    service_unit_file: &mut SystemdUnitFile,
    section: &str,
    units_info_map: &mut UnitsInfoMap,
    podman: &mut PodmanCommand,
) -> Result<(), ConversionError> {
    if let Some(pod) = quadlet_unit.lookup(section, "Pod") {
        if !pod.is_empty() {
            if !pod.ends_with(".pod") {
                return Err(ConversionError::InvalidPod(pod));
            }

            let pod_info = units_info_map
                .0
                .get_mut(&OsString::from(&pod))
                .ok_or_else(|| ConversionError::PodNotFound(pod))?;
            podman.add("--pod");
            podman.add(&pod_info.resource_name);

            let pod_service_name = pod_info
                .get_service_file_name()
                .to_str()
                .expect("pod service name is not a valid UTF-8 string")
                .to_string();
            service_unit_file.add(UNIT_SECTION, "BindsTo", &pod_service_name);
            service_unit_file.add(UNIT_SECTION, "After", &pod_service_name);

            // If we want to start the container with the pod, we add it to this list.
            // This creates corresponding Wants=/Before= statements in the pod service.
            if quadlet_unit
                .lookup_bool(section, "StartWithPod")
                .unwrap_or(true)
            {
                pod_info
                    .containers_to_start
                    .push(service_unit_file.path.clone());
            }
        }
    }
    Ok(())
}

fn handle_publish_ports(unit_file: &SystemdUnitData, section: &str, podman: &mut PodmanCommand) {
    lookup_and_add_all_strings(unit_file, section, &[("PublishPort", "--publish")], podman);
}

fn handle_set_working_directory(
    quadlet_unit_file: &SystemdUnitFile,
    service_unit_file: &mut SystemdUnitFile,
    quadlet_section: &str,
) -> Result<String, ConversionError> {
    let set_working_directory = if let Some(set_working_dir) =
        quadlet_unit_file.lookup(quadlet_section, "SetWorkingDirectory")
    {
        if set_working_dir.is_empty() {
            return Ok(String::default());
        }
        set_working_dir
    } else {
        return Ok(String::default());
    };

    let mut context = "";
    let relative_to_file;
    match set_working_directory.to_ascii_lowercase().as_str() {
        "yaml" => {
            if quadlet_section != KUBE_SECTION {
                return Err(ConversionError::InvalidSetWorkingDirectory(
                    set_working_directory.to_string(),
                    "kube".to_string(),
                ));
            }

            if let Some(yaml) = quadlet_unit_file.lookup(quadlet_section, "Yaml") {
                relative_to_file = PathBuf::from(yaml)
            } else {
                return Err(ConversionError::NoYamlKeySpecified);
            }
        }
        "file" => {
            if quadlet_section != BUILD_SECTION {
                return Err(ConversionError::InvalidSetWorkingDirectory(
                    set_working_directory.to_string(),
                    "build".to_string(),
                ));
            }

            if let Some(file) = quadlet_unit_file.lookup(quadlet_section, "File") {
                relative_to_file = PathBuf::from(file)
            } else {
                return Err(ConversionError::NoFileKeySpecified);
            }
        }
        "unit" => relative_to_file = quadlet_unit_file.path().clone(),
        _ => {
            // Path / URL handling is for .build files only
            if quadlet_section != BUILD_SECTION {
                return Err(ConversionError::UnsupportedValueForKey(
                    "SetWorkingDirectory".to_string(),
                    set_working_directory.to_string(),
                ));
            }

            // Any value other than the above cases will be returned as context
            context = &set_working_directory;

            // If we have a relative path, set the WorkingDirectory to that of the quadlet_unit_file
            if !PathBuf::from(context).is_absolute() {
                relative_to_file = quadlet_unit_file.path().clone();
            } else {
                relative_to_file = PathBuf::default()
            }
        }
    };

    if !relative_to_file.as_os_str().is_empty() && !is_url(context) {
        // If WorkingDirectory is already set in the Service section do not change it
        if let Some(working_dir) = quadlet_unit_file.lookup(SERVICE_SECTION, "WorkingDirectory") {
            if !working_dir.is_empty() {
                return Ok(String::default());
            }
        }

        let file_in_workingdir = relative_to_file.absolute_from_unit(quadlet_unit_file);

        service_unit_file.add(
            SERVICE_SECTION,
            "WorkingDirectory",
            file_in_workingdir
                .parent()
                .expect("should have a parent directory")
                .display()
                .to_string()
                .as_str(),
        );
    }

    Ok(context.to_string())
}

fn handle_unit_dependencies(
    service_unit_file: &mut SystemdUnitFile,
    units_info_map: &UnitsInfoMap,
) -> Result<(), ConversionError> {
    for unit_dependency_key in UNIT_DEPENDENCY_KEYS {
        let deps = service_unit_file.lookup_all_strv(UNIT_SECTION, unit_dependency_key);
        if deps.len() == 0 {
            continue;
        }
        let mut translated_deps = Vec::with_capacity(deps.len());
        let mut translated = false;
        for dep in deps {
            let dep_path = PathBuf::from(&dep);
            let translated_dep = if SUPPORTED_EXTENSIONS.contains(&dep_path.systemd_unit_type()) {
                let unit_info = units_info_map
                    .0
                    .get(dep_path.as_os_str())
                    .ok_or(ConversionError::InvalidUnitDependency(dep))?;
                translated = true;
                PathBuf::from(unit_info.get_service_file_name())
                    .to_unwrapped_str()
                    .to_string()
            } else {
                dep
            };
            translated_deps.push(translated_dep);
        }
        if !translated {
            continue;
        }
        service_unit_file.remove_entries(UNIT_SECTION, unit_dependency_key);
        service_unit_file.add(
            UNIT_SECTION,
            unit_dependency_key,
            translated_deps.join(" ").as_str(),
        );
    }

    Ok(())
}

fn handle_storage_source(
    quadlet_unit_file: &SystemdUnitFile,
    service_unit_file: &mut SystemdUnitFile,
    source: Option<&str>,
    units_info_map: &UnitsInfoMap,
    check_image: bool,
) -> Result<String, ConversionError> {
    let mut source = match source {
        Some(source) => source.to_owned(),
        None => return Err(ConversionError::InvalidMountSource),
    };

    if source.starts_with('.') {
        source = PathBuf::from(source)
            .absolute_from_unit(quadlet_unit_file)
            .to_unwrapped_str()
            .to_string();
    }

    if source.starts_with('/') {
        // Absolute path
        service_unit_file.add(UNIT_SECTION, "RequiresMountsFor", &source);
    } else if source.ends_with(".volume") || (check_image && source.ends_with(".image")) {
        let source_unit_info = units_info_map
            .0
            .get(&OsString::from(&source))
            .ok_or_else(|| ConversionError::SourceNotFound(source))?;

        // the systemd unit name is $name-volume.service
        let volume_service_name = source_unit_info.get_service_file_name();

        service_unit_file.add(
            UNIT_SECTION,
            "Requires",
            volume_service_name.to_str().unwrap(),
        );
        service_unit_file.add(UNIT_SECTION, "After", volume_service_name.to_str().unwrap());

        source = source_unit_info.resource_name.clone();
    }

    Ok(source)
}

fn handle_user(
    unit_file: &SystemdUnitData,
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
            podman.add("--user");
            podman.add(user);
            Ok(())
        }
        (Some(_empty), None) => Ok(()),
        (Some(user), Some(group)) if !user.is_empty() && !group.is_empty() => {
            podman.add("--user");
            podman.add(format!("{user}:{group}"));
            Ok(())
        }
        (Some(_), Some(_)) => Ok(()),
    };
}

fn handle_user_mappings(
    unit_file: &SystemdUnitData,
    section: &str,
    podman: &mut PodmanCommand,
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
        podman.add("--uidmap");
        podman.add(uid_map);
        mappings_defined = true;
    }

    for gid_map in unit_file.lookup_all_strv(section, "GIDMap") {
        podman.add("--gidmap");
        podman.add(gid_map);
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

    return handle_user_remap(unit_file, section, podman, support_manual);
}

fn handle_user_remap(
    unit_file: &SystemdUnitData,
    section: &str,
    podman: &mut PodmanCommand,
    support_manual: bool,
) -> Result<(), ConversionError> {
    // ignore Remap keys if UserNS is set
    if unit_file.lookup(section, "UserNS").is_some() {
        return Ok(());
    }

    let uid_maps: Vec<String> = unit_file.lookup_all_strv(section, "RemapUid");
    let gid_maps: Vec<String> = unit_file.lookup_all_strv(section, "RemapGid");
    let remap_users = unit_file.lookup_last(section, "RemapUsers");
    match remap_users.as_deref() {
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
                    podman.add("--uidmap");
                    podman.add(uid_map);
                }
                for gid_map in gid_maps {
                    podman.add("--gidmap");
                    podman.add(gid_map);
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
                podman.add_slice(&["--userns", "auto"]);
            } else {
                podman.add("--userns");
                podman.add(format!("auto:{}", auto_opts.join(",")));
            }
        }
        Some("keep-id") => {
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
                podman.add_slice(&["--userns", "keep-id"]);
            } else {
                podman.add("--userns");
                podman.add(format!("keep-id:{}", keepid_opts.join(",")));
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
    units_info_map: &UnitsInfoMap,
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
            source = handle_storage_source(
                quadlet_unit_file,
                service_unit_file,
                Some(&source),
                units_info_map,
                false,
            )?;
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

fn init_service_unit_file<'q>(
    quadlet: &'q QuadletSourceUnitFile,
    section: &str,
    x_section: &str,
    supported_keys: &[&str],
    units_info_map: &UnitsInfoMap,
    is_user: bool,
) -> Result<QuadletServiceUnitFile<'q>, ConversionError> {
    let quadlet_file = &quadlet.unit_file;
    check_for_unknown_keys(quadlet_file, section, &supported_keys)?;
    check_for_unknown_keys(quadlet_file, QUADLET_SECTION, &SUPPORTED_QUADLET_KEYS)?;

    warn_if_unsupported_service_keys(quadlet_file);

    let mut service_file = SystemdUnitFile::new();
    service_file.merge_from(quadlet_file);

    let unit_info = units_info_map
        .0
        .get(quadlet_file.file_name())
        .ok_or_else(|| {
            ConversionError::InternalQuadletError(
                quadlet_file.unit_type().into(),
                quadlet_file.file_name().into(),
            )
        })?;

    service_file.path = unit_info.get_service_file_name().into();

    handle_unit_dependencies(&mut service_file, units_info_map)?;

    handle_default_dependencies(&mut service_file, is_user);

    if !quadlet_file.path().as_os_str().is_empty() {
        service_file.add(
            UNIT_SECTION,
            "SourcePath",
            quadlet_file.path().to_unwrapped_str(),
        );
    }

    // Need the containers filesystem mounted to start podman
    service_file.add(UNIT_SECTION, "RequiresMountsFor", "%t/containers");

    // Rename old Container section to X-Container so that systemd ignores it
    service_file.rename_section(section, x_section);

    // Rename common Quadlet section
    service_file.rename_section(QUADLET_SECTION, X_QUADLET_SECTION);

    Ok(QuadletServiceUnitFile {
        service_file,
        quadlet,
    })
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

fn lookup_and_add_all_key_vals(
    unit: &SystemdUnitData,
    section: &str,
    keys: &[(&str, &str)],
    podman: &mut PodmanCommand,
) {
    for (key, flag) in keys {
        let key_vals = unit.lookup_all_key_val(section, *key);
        podman.add_keys(flag, &key_vals);
    }
}

fn lookup_and_add_all_strings(
    unit: &SystemdUnitData,
    section: &str,
    keys: &[(&str, &str)],
    podman: &mut PodmanCommand,
) {
    // NOTE: Rust doesn't seem to like the doubly nested `flat_map()` variant I tried.
    // e.g. `keys.iter().flat_map(<for loop part>)`
    // it clmplains that:
    // > returns a value referencing data owned by the current function
    // but all the `&str`s should get "owned" in the end by `to_string()` and passed on to `podman`
    for (key, flag) in keys {
        podman.extend(
            unit.lookup_all(section, *key)
                .iter()
                .flat_map(|val| [*flag, val])
                .map(str::to_string),
        );
    }
}

fn lookup_and_add_bool(
    unit: &SystemdUnitData,
    section: &str,
    keys: &[(&str, &str)],
    podman: &mut PodmanCommand,
) {
    for (key, flag) in keys {
        if let Some(val) = unit.lookup_bool(section, *key) {
            podman.add_bool(*flag, val);
        }
    }
}

fn lookup_and_add_string(
    unit: &SystemdUnitData,
    section: &str,
    keys: &[(&str, &str)],
    podman: &mut PodmanCommand,
) {
    for (key, flag) in keys {
        if let Some(val) = unit.lookup(section, *key) {
            if !val.is_empty() {
                podman.add(*flag);
                podman.add(val);
            }
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
    units_info_map: &mut UnitsInfoMap,
) -> Result<String, ConversionError> {
    let (mount_type, tokens) = find_mount_type(mount.as_str())?;

    // Source resolution is required only for these types of mounts
    if !(mount_type == "volume"
        || mount_type == "bind"
        || mount_type == "glob"
        || mount_type == "image")
    {
        return Ok(mount);
    }

    let mut csv_writer = csv::Writer::from_writer(vec![]);
    csv_writer.write_field(format!("type={mount_type}"))?;

    let mut original_source = None;
    for token in tokens.iter() {
        if token.starts_with("source=") || token.starts_with("src=") {
            if let Some((_k, v)) = token.split_once('=') {
                original_source = Some(v);
            } else {
                return Err(ConversionError::InvalidMountSource);
            }
        } else {
            // we're only interested in the mount source
            // everything else is piped through as is
            csv_writer.write_field(token)?;
        }
    }

    let resolved_source = handle_storage_source(
        container_unit_file,
        service_unit_file,
        original_source,
        units_info_map,
        true,
    )?;
    csv_writer.write_field(format!("source={resolved_source}"))?;

    csv_writer.write_record(None::<&[u8]>)?;

    return Ok(String::from_utf8(
        csv_writer
            .into_inner()
            .expect("connot convert Mount params back into CSV"),
    )
    .expect("connot convert Mount params back into CSV"));
}

// Warns if the unit has any properties defined in the Service group that are known to cause issues.
// We want to warn instead of erroring to avoid breaking any existing users' units,
// or to allow users to use these properties if they know what they are doing.
// We implement this here instead of in quadlet.initServiceUnitFile to avoid
// having to refactor a large amount of code in the generator just for a warning.
fn warn_if_unsupported_service_keys(quadlet_file: &SystemdUnitFile) {
    for key in UNSUPPORTED_SERVICE_KEYS {
        if quadlet_file.lookup(SERVICE_SECTION, key).is_some() {
            warn!("using key {key} in the Service group is not supported - use at your own risk")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
}
