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

fn get_base_podman_command(unit: &SystemdUnitFile, section: &str) -> PodmanCommand {
    let mut podman = PodmanCommand::new();

    lookup_and_add_all_strings(unit, section, &[("ContainersConfModule", "--module")], &mut podman);

    podman.extend(unit.lookup_all_args(section, "GlobalArgs"));

    podman
}

pub(crate) fn from_build_unit(
    build: &SystemdUnitFile,
    units_info_map: &mut UnitsInfoMap,
    is_user: bool,
) -> Result<SystemdUnitFile, ConversionError> {
    let unit_info = units_info_map.0.get(build.file_name()).ok_or_else(|| {
        ConversionError::InternalQuadletError("build".to_string(), build.file_name().into())
    })?;

    // fail fast if resource name is not set
    if unit_info.resource_name.is_empty() {
        return Err(ConversionError::NoImageTagKeySpecified);
    }

    let mut service = SystemdUnitFile::new();

    service.merge_from(build);
    service.path = unit_info.get_service_file_name().into();

    handle_default_dependencies(&mut service, is_user);

    // Need the containers filesystem mounted to start podman
    service.add(UNIT_SECTION, "RequiresMountsFor", "%t/containers");

    if !build.path().as_os_str().is_empty() {
        service.add(
            UNIT_SECTION,
            "SourcePath",
            build
                .path()
                .to_str()
                .expect("EnvironmentFile path is not a valid UTF-8 string"),
        );
    }

    check_for_unknown_keys(build, BUILD_SECTION, &SUPPORTED_BUILD_KEYS)?;
    check_for_unknown_keys(build, QUADLET_SECTION, &SUPPORTED_QUADLET_KEYS)?;

    // Rename old Build section to X-Build so that systemd ignores it
    service.rename_section(BUILD_SECTION, X_BUILD_SECTION);

    // Rename common Quadlet section
	service.rename_section(QUADLET_SECTION, X_QUADLET_SECTION);

    let mut podman = get_base_podman_command(build, BUILD_SECTION);
    podman.add("build");

    let string_keys = [
        ("Arch", "--arch"),
        ("AuthFile", "--authfile"),
        ("Pull", "--pull"),
        ("Target", "--target"),
        ("Variant", "--variant"),
    ];
    lookup_and_add_string(build, BUILD_SECTION, &string_keys, &mut podman);

    let bool_keys = [
        ("TLSVerify", "--tls-verify"),
        ("ForceRM", "--force-rm"),
    ];
    lookup_and_add_bool(build, BUILD_SECTION, &bool_keys, &mut podman);

    let all_string_keys = [
        ("DNS", "--dns"),
        ("DNSOption", "--dns-option"),
        ("DNSSearch", "--dns-search"),
        ("GroupAdd", "--group-add"),
        ("ImageTag", "--tag"),
    ];
    lookup_and_add_all_strings(build, BUILD_SECTION, &all_string_keys, &mut podman);

    let annotations = build.lookup_all_key_val(BUILD_SECTION, "Annotation");
    podman.add_annotations(&annotations);

    let podman_env = build.lookup_all_key_val(BUILD_SECTION, "Environment");
    podman.add_env(&podman_env);

    let labels = build.lookup_all_key_val(BUILD_SECTION, "Label");
    podman.add_labels(&labels);

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
    let (working_directory, file_path) = match (working_directory.as_deref(), file_path.as_deref(), context.as_str()) {
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
    } else if !PathBuf::from(file_path).is_absolute() && !is_url(file_path) {
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

    service.add(SERVICE_SECTION, "Type", "oneshot");
    service.add(SERVICE_SECTION, "RemainAfterExit", "yes");
    // The default syslog identifier is the exec basename (podman)
    // which isn't very useful here
    service.add(SERVICE_SECTION, "SyslogIdentifier", "%N");

    return Ok(service);
}

// Convert a quadlet container file (unit file with a Container group) to a systemd
// service file (unit file with Service group) based on the options in the Container group.
// The original Container group is kept around as X-Container.
pub(crate) fn from_container_unit(
    container: &SystemdUnitFile,
    units_info_map: &mut UnitsInfoMap,
    is_user: bool,
) -> Result<SystemdUnitFile, ConversionError> {
    let unit_info = units_info_map.0.get(container.file_name()).ok_or_else(|| {
        ConversionError::InternalQuadletError("container".into(), container.file_name().into())
    })?;

    let mut service = SystemdUnitFile::new();

    service.merge_from(container);
    service.path = unit_info.get_service_file_name().into();

    handle_default_dependencies(&mut service, is_user);

    if !container.path().as_os_str().is_empty() {
        service.add(
            UNIT_SECTION,
            "SourcePath",
            container
                .path()
                .to_str()
                .expect("EnvironmentFile path is not a valid UTF-8 string"),
        );
    }

    check_for_unknown_keys(container, CONTAINER_SECTION, &SUPPORTED_CONTAINER_KEYS)?;
    check_for_unknown_keys(container, QUADLET_SECTION, &SUPPORTED_QUADLET_KEYS)?;

    // Rename old Container section to X-Container so that systemd ignores it
    service.rename_section(CONTAINER_SECTION, X_CONTAINER_SECTION);

    // Rename common Quadlet section
	service.rename_section(QUADLET_SECTION, X_QUADLET_SECTION);

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

    let podman_container_name =
        if let Some(container_name) = container.lookup(CONTAINER_SECTION, "ContainerName") {
            container_name
        } else {
            // By default, We want to name the container by the service name
            if container.is_template_unit() {
                "systemd-%p_%i"
            } else {
                "systemd-%N"
            }.to_string()
        };

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

    // Read env early so we can override it below
    let podman_env = container.lookup_all_key_val(CONTAINER_SECTION, "Environment");

    // Need the containers filesystem mounted to start podman
    service.add(UNIT_SECTION, "RequiresMountsFor", "%t/containers");

    // If conmon exited uncleanly it may not have removed the container, so
    // force it, -i makes it ignore non-existing files.
    let mut service_stop_cmd = get_base_podman_command(container, CONTAINER_SECTION);
    service_stop_cmd.add_slice(&["rm", "-v", "-f", "-i", "--cidfile=%t/%N.cid"]);
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

    let mut podman = get_base_podman_command(container, CONTAINER_SECTION);

    podman.add("run");

    podman.add("--name");
    podman.add(podman_container_name);

    // We store the container id so we can clean it up in case of failure
    podman.add("--cidfile=%t/%N.cid");

    // And replace any previous container with the same name, not fail
    podman.add("--replace");

    // On clean shutdown, remove container
    podman.add("--rm");

    handle_log_driver(container, CONTAINER_SECTION, &mut podman);
    handle_log_opt(container, CONTAINER_SECTION, &mut podman);

    // We delegate groups to the runtime
    service.add(SERVICE_SECTION, "Delegate", "yes");

    let cgroups_mode =
        container
            .lookup(CONTAINER_SECTION, "CgroupsMode")
            .map_or("split".to_string(), |cgroups_mode| {
                if cgroups_mode.is_empty() {
                    return "split".to_string();
                }

                cgroups_mode
            });
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
        ("RunInit", "--init"),
        ("EnvironmentHost", "--env-host"),
        ("ReadOnlyTmpfs", "--read-only-tmpfs"),
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

        podman.add("--expose");
        podman.add(exposed_port);
    }

    handle_publish_ports(container, CONTAINER_SECTION, &mut podman);

    podman.add_env(&podman_env);

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

    let exec_args = container
        .lookup_last_value(CONTAINER_SECTION, "Exec")
        .map(|v| SplitWord::new(v.raw()))
        .unwrap_or_default();
    podman.extend(exec_args);

    service.add_raw(
        SERVICE_SECTION,
        "ExecStart",
        podman.to_escaped_string().as_str(),
    )?;

    Ok(service)
}

pub(crate) fn from_image_unit(
    image: &SystemdUnitFile,
    units_info_map: &mut UnitsInfoMap,
    is_user: bool,
) -> Result<SystemdUnitFile, ConversionError> {
    let unit_info = units_info_map.0.get_mut(image.file_name()).ok_or_else(|| {
        ConversionError::InternalQuadletError("image".into(), image.path().into())
    })?;

    let mut service = SystemdUnitFile::new();
    service.merge_from(image);
    service.path = unit_info.get_service_file_name().into();

    handle_default_dependencies(&mut service, is_user);

    if !image.path().as_os_str().is_empty() {
        service.add(
            UNIT_SECTION,
            "SourcePath",
            image
                .path()
                .to_str()
                .expect("EnvironmentFile path is not a valid UTF-8 string"),
        );
    }

    check_for_unknown_keys(image, IMAGE_SECTION, &SUPPORTED_IMAGE_KEYS)?;
    check_for_unknown_keys(image, QUADLET_SECTION, &SUPPORTED_QUADLET_KEYS)?;

    let image_name = image
        .lookup_last(IMAGE_SECTION, "Image")
        .unwrap_or_default();
    if image_name.is_empty() {
        return Err(ConversionError::InvalidImageOrRootfs(
            "no Image key specified".into(),
        ));
    }

    // Rename old Image section to X-Image so that systemd ignores it
    service.rename_section(IMAGE_SECTION, X_IMAGE_SECTION);

    // Rename common Quadlet section
	service.rename_section(QUADLET_SECTION, X_QUADLET_SECTION);

    // Need the containers filesystem mounted to start podman
    service.add(UNIT_SECTION, "RequiresMountsFor", "%t/containers");

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
    lookup_and_add_string(image, IMAGE_SECTION, &string_keys, &mut podman);

    let bool_keys = [
        ("AllTags", "--all-tags"),
        ("TLSVerify", "--tls-verify"),
    ];
    lookup_and_add_bool(image, IMAGE_SECTION, &bool_keys, &mut podman);

    handle_podman_args(image, IMAGE_SECTION, &mut podman);

    podman.add(image_name.clone());

    service.add_raw(
        SERVICE_SECTION,
        "ExecStart",
        podman.to_escaped_string().as_str(),
    )?;

    service.add(SERVICE_SECTION, "Type", "oneshot");
    service.add(SERVICE_SECTION, "RemainAfterExit", "yes");

    // The default syslog identifier is the exec basename (podman) which isn't very useful here
    service.add(SERVICE_SECTION, "SyslogIdentifier", "%N");

    let podman_image_name = if let Some(image) = image.lookup(IMAGE_SECTION, "ImageTag") {
        if !image.is_empty() {
            image
        } else {
            image_name
        }
    } else {
        image_name
    };

    // Store the name of the created resource
    unit_info.resource_name = podman_image_name.to_string();

    Ok(service)
}

pub(crate) fn from_kube_unit(
    kube: &SystemdUnitFile,
    units_info_map: &mut UnitsInfoMap,
    is_user: bool,
) -> Result<SystemdUnitFile, ConversionError> {
    let unit_info = units_info_map
        .0
        .get(kube.file_name())
        .ok_or_else(|| ConversionError::InternalQuadletError("kube".into(), kube.path().into()))?;

    let mut service = SystemdUnitFile::new();
    service.merge_from(kube);
    service.path = unit_info.get_service_file_name().into();

    handle_default_dependencies(&mut service, is_user);

    if !kube.path().as_os_str().is_empty() {
        service.add(
            UNIT_SECTION,
            "SourcePath",
            kube.path()
                .to_str()
                .expect("EnvironmentFile path is not a valid UTF-8 string"),
        );
    }

    check_for_unknown_keys(kube, KUBE_SECTION, &SUPPORTED_KUBE_KEYS)?;
    check_for_unknown_keys(kube, QUADLET_SECTION, &SUPPORTED_QUADLET_KEYS)?;

    // Rename old Kube section to X-Kube so that systemd ignores it
    service.rename_section(KUBE_SECTION, X_KUBE_SECTION);

    // Rename common Quadlet section
	service.rename_section(QUADLET_SECTION, X_QUADLET_SECTION);

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

    // Need the containers filesystem mounted to start podman
    service.add(UNIT_SECTION, "RequiresMountsFor", "%t/containers");

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
        podman_start.add(
            config_map_path
                .to_str()
                .expect("ConfigMap path is not valid UTF-8 string"),
        );
    }

    handle_publish_ports(kube, KUBE_SECTION, &mut podman_start);

    handle_podman_args(kube, KUBE_SECTION, &mut podman_start);

    podman_start.add(
        yaml_path
            .to_str()
            .expect("Yaml path is not valid UTF-8 string"),
    );

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

    podman_stop.add(
        yaml_path
            .to_str()
            .expect("Yaml path is not valid UTF-8 string"),
    );
    service.add_raw(
        SERVICE_SECTION,
        "ExecStopPost",
        podman_stop.to_escaped_string().as_str(),
    )?;

    handle_set_working_directory(kube, &mut service, KUBE_SECTION)?;

    Ok(service)
}

// Convert a quadlet network file (unit file with a Network group) to a systemd
// service file (unit file with Service group) based on the options in the Network group.
// The original Network group is kept around as X-Network.
// Also returns the canonical network name, either auto-generated or user-defined via the
// NetworkName key-value.
pub(crate) fn from_network_unit(
    network: &SystemdUnitFile,
    units_info_map: &mut UnitsInfoMap,
    is_user: bool,
) -> Result<SystemdUnitFile, ConversionError> {
    let unit_info = units_info_map
        .0
        .get_mut(network.file_name())
        .ok_or_else(|| {
            ConversionError::InternalQuadletError("network".into(), network.path().into())
        })?;

    let mut service = SystemdUnitFile::new();
    service.merge_from(network);
    service.path = unit_info.get_service_file_name().into();

    handle_default_dependencies(&mut service, is_user);

    if !network.path().as_os_str().is_empty() {
        service.add(
            UNIT_SECTION,
            "SourcePath",
            network
                .path()
                .to_str()
                .expect("EnvironmentFile path is not a valid UTF-8 string"),
        );
    }

    check_for_unknown_keys(network, NETWORK_SECTION, &SUPPORTED_NETWORK_KEYS)?;
    check_for_unknown_keys(network, QUADLET_SECTION, &SUPPORTED_QUADLET_KEYS)?;

    // Rename old Network section to X-Network so that systemd ignores it
    service.rename_section(NETWORK_SECTION, X_NETWORK_SECTION);

    // Rename common Quadlet section
	service.rename_section(QUADLET_SECTION, X_QUADLET_SECTION);

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
    service.add(UNIT_SECTION, "RequiresMountsFor", "%t/containers");

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

    let network_options = network.lookup_all_key_val(NETWORK_SECTION, "Options");
    if !network_options.is_empty() {
        podman.add_keys("--opt", &network_options);
    }

    let labels = network.lookup_all_key_val(NETWORK_SECTION, "Label");
    podman.add_labels(&labels);

    handle_podman_args(network, NETWORK_SECTION, &mut podman);

    podman.add(&podman_network_name);

    service.add_raw(
        SERVICE_SECTION,
        "ExecStart",
        podman.to_escaped_string().as_str(),
    )?;

    service.add(SERVICE_SECTION, "Type", "oneshot");
    service.add(SERVICE_SECTION, "RemainAfterExit", "yes");
    // The default syslog identifier is the exec basename (podman) which isn't very useful here
    service.add(SERVICE_SECTION, "SyslogIdentifier", "%N");

    // Store the name of the created resource
    unit_info.resource_name = podman_network_name;

    Ok(service)
}

pub(crate) fn from_pod_unit(
    pod: &SystemdUnitFile,
    units_info_map: &mut UnitsInfoMap,
    is_user: bool,
) -> Result<SystemdUnitFile, ConversionError> {
    let unit_info = units_info_map
        .0
        .get(pod.file_name())
        .ok_or_else(|| ConversionError::InternalQuadletError("pod".into(), pod.path().into()))?;

    let mut service = SystemdUnitFile::new();
    service.merge_from(pod);
    service.path = unit_info.get_service_file_name().into();

    handle_default_dependencies(&mut service, is_user);

    if !pod.path().as_os_str().is_empty() {
        service.add(
            UNIT_SECTION,
            "SourcePath",
            pod.path()
                .to_str()
                .expect("EnvironmentFile path is not a valid UTF-8 string"),
        );
    }

    check_for_unknown_keys(pod, POD_SECTION, &SUPPORTED_POD_KEYS)?;
    check_for_unknown_keys(pod, QUADLET_SECTION, &SUPPORTED_QUADLET_KEYS)?;

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

    // Rename old Pod section to X-Pod so that systemd ignores it
    service.rename_section(POD_SECTION, X_POD_SECTION);

    // Rename common Quadlet section
	service.rename_section(QUADLET_SECTION, X_QUADLET_SECTION);

    // Need the containers filesystem mounted to start podman
    service.add(UNIT_SECTION, "RequiresMountsFor", "%t/containers");

    for container_service in &unit_info.containers {
        let container_service = container_service
            .to_str()
            .expect("container service path is not a valid UTF-8 string");
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
    podman_start.add("--pod-id-file=%t/%N.pod-id");
    service.add_raw(
        SERVICE_SECTION,
        "ExecStart",
        podman_start.to_escaped_string().as_str(),
    )?;

    let mut podman_stop = get_base_podman_command(pod, POD_SECTION);
    podman_stop.add("pod");
    podman_stop.add("stop");
    podman_stop.add("--pod-id-file=%t/%N.pod-id");
    podman_stop.add("--ignore");
    podman_stop.add("--time=10");
    service.add_raw(
        SERVICE_SECTION,
        "ExecStop",
        podman_stop.to_escaped_string().as_str(),
    )?;

    let mut podman_stop_post = get_base_podman_command(pod, POD_SECTION);
    podman_stop_post.add("pod");
    podman_stop_post.add("rm");
    podman_stop_post.add("--pod-id-file=%t/%N.pod-id");
    podman_stop_post.add("--ignore");
    podman_stop_post.add("--force");
    service.add_raw(
        SERVICE_SECTION,
        "ExecStopPost",
        podman_stop_post.to_escaped_string().as_str(),
    )?;

    let mut podman_start_pre = get_base_podman_command(pod, POD_SECTION);
    podman_start_pre.add("pod");
    podman_start_pre.add("create");
    podman_start_pre.add("--infra-conmon-pidfile=%t/%N.pid");
    podman_start_pre.add("--pod-id-file=%t/%N.pod-id");
    podman_start_pre.add("--exit-policy=stop");
    podman_start_pre.add("--replace");

    handle_user_mappings(pod, POD_SECTION, &mut podman_start_pre, true)?;

    handle_publish_ports(pod, POD_SECTION, &mut podman_start_pre);

    handle_networks(
        pod,
        POD_SECTION,
        &mut service,
        units_info_map,
        &mut podman_start_pre,
    )?;


    let string_keys = [
        ("IP", "--ip"),
        ("IP6", "--ip6"),
    ];
    // NOTE: Go Quadlet uses `lookup_and_add_all_strings()` here
    lookup_and_add_string(
        &pod,
        POD_SECTION,
        &string_keys,
        &mut podman_start_pre,
    );

    let all_string_keys = [
        ("NetworkAlias", "--network-alias"),
        ("DNS", "--dns"),
        ("DNSOption", "--dns-option"),
        ("DNSSearch", "--dns-search"),
        ("AddHost", "--add-host")
    ];
    lookup_and_add_all_strings(
        &pod,
        POD_SECTION,
        &all_string_keys,
        &mut podman_start_pre,
    );

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

    service.add(SERVICE_SECTION, "Environment", "PODMAN_SYSTEMD_UNIT=%n");
    service.add(SERVICE_SECTION, "Type", "forking");
    service.add(SERVICE_SECTION, "Restart", "on-failure");
    service.add(SERVICE_SECTION, "PIDFile", "%t/%N.pid");

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
    units_info_map: &mut UnitsInfoMap,
    is_user: bool,
) -> Result<SystemdUnitFile, ConversionError> {
    let unit_info = units_info_map
        .0
        .get_mut(volume.file_name())
        .ok_or_else(|| {
            ConversionError::InternalQuadletError("volume".into(), volume.path().into())
        })?;

    let mut service = SystemdUnitFile::new();
    service.merge_from(volume);
    service.path = unit_info.get_service_file_name().into();

    handle_default_dependencies(&mut service, is_user);

    if !volume.path().as_os_str().is_empty() {
        service.add(
            UNIT_SECTION,
            "SourcePath",
            volume
                .path()
                .to_str()
                .expect("EnvironmentFile path is not a valid UTF-8 string"),
        );
    }

    check_for_unknown_keys(volume, VOLUME_SECTION, &SUPPORTED_VOLUME_KEYS)?;
    check_for_unknown_keys(volume, QUADLET_SECTION, &SUPPORTED_QUADLET_KEYS)?;

    // Rename old Volume section to X-Volume so that systemd ignores it
    service.rename_section(VOLUME_SECTION, X_VOLUME_SECTION);

    // Rename common Quadlet section
	service.rename_section(QUADLET_SECTION, X_QUADLET_SECTION);

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
    unit_info.resource_name = podman_volume_name.clone();

    // Need the containers filesystem mounted to start podman
    service.add(UNIT_SECTION, "RequiresMountsFor", "%t/containers");

    let labels = volume.lookup_all_key_val(VOLUME_SECTION, "Label");

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

    service.add_raw(
        SERVICE_SECTION,
        "ExecStart",
        podman.to_escaped_string().as_str(),
    )?;

    service.add(SERVICE_SECTION, "Type", "oneshot");
    service.add(SERVICE_SECTION, "RemainAfterExit", "yes");
    // The default syslog identifier is the exec basename (podman) which isn't very useful here
    service.add(SERVICE_SECTION, "SyslogIdentifier", "%N");

    Ok(service)
}

fn handle_default_dependencies(service: &mut SystemdUnitFile, is_user: bool) {
    // Add a dependency on network-online.target so the image pull does not happen
    // before network is ready.
    // see https://github.com/containers/podman/issues/21873
    if service.lookup_bool(QUADLET_SECTION, "DefaultDependencies").unwrap_or(true) {
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

fn handle_log_driver(unit_file: &SystemdUnit, section: &str, podman: &mut PodmanCommand) {
    if let Some(log_driver) = unit_file.lookup_last(section, "LogDriver") {
        podman.add("--log-driver");
        podman.add(log_driver);
    }
}

fn handle_log_opt(unit_file: &SystemdUnit, section: &str, podman: &mut PodmanCommand) {
    podman.extend(
        unit_file
            .lookup_all_strv(section, "LogOpt")
            .iter()
            .flat_map(|log_opt| ["--log-opt", log_opt])
            .map(str::to_string),
    )
}

fn handle_networks(
    quadlet_unit_file: &SystemdUnit,
    section: &str,
    service_unit_file: &mut SystemdUnit,
    units_info_map: &UnitsInfoMap,
    podman: &mut PodmanCommand,
) -> Result<(), ConversionError> {
    for network in quadlet_unit_file.lookup_all(section, "Network") {
        if !network.is_empty() {
            let mut network_name = network.to_string();
            let mut options: Option<&str> = None;
            if let Some((_network_name, _options)) = network.split_once(':') {
                network_name = _network_name.to_string();
                options = Some(_options);
            }

            if network_name.ends_with(".network") {
                // the podman network name is systemd-$name if none is specified by the user.
                let network_unit_info = units_info_map
                    .0
                    .get(&OsString::from(&network_name))
                    .ok_or_else(|| {
                        ConversionError::InternalQuadletError("image".into(), network_name.into())
                    })?;

                // the systemd unit name is $name-network.service
                let network_service_name = network_unit_info.get_service_file_name();
                service_unit_file.add(
                    UNIT_SECTION,
                    "Requires",
                    network_service_name.to_str().unwrap(),
                );
                service_unit_file.add(
                    UNIT_SECTION,
                    "After",
                    network_service_name.to_str().unwrap(),
                );

                network_name = network_unit_info.resource_name.clone();
            }

            if options.is_some() {
                podman.add("--network");
                podman.add(format!("{network_name}:{}", options.unwrap()));
            } else {
                podman.add("--network");
                podman.add(network_name);
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
            podman.add("--pod-id-file");
            podman.add(format!("%t/{}.pod-id", pod_info.service_name));

            let pod_service_name = pod_info
                .get_service_file_name()
                .to_str()
                .expect("pod service name is not a valid UTF-8 string")
                .to_string();
            service_unit_file.add(UNIT_SECTION, "BindsTo", &pod_service_name);
            service_unit_file.add(UNIT_SECTION, "After", &pod_service_name);

            pod_info.containers.push(service_unit_file.path.clone());
        }
    }
    Ok(())
}

fn handle_publish_ports(unit_file: &SystemdUnit, section: &str, podman: &mut PodmanCommand) {
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

fn handle_storage_source(
    quadlet_unit_file: &SystemdUnitFile,
    service_unit_file: &mut SystemdUnitFile,
    source: &str,
    units_info_map: &UnitsInfoMap,
) -> Result<String, ConversionError> {
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
        service_unit_file.add(UNIT_SECTION, "RequiresMountsFor", &source);
    } else if source.ends_with(".volume") {
        let volume_unit_info = units_info_map
            .0
            .get(&OsString::from(&source))
            .ok_or_else(|| ConversionError::ImageNotFound(source))?;

        // the systemd unit name is $name-volume.service
        let volume_service_name = volume_unit_info.get_service_file_name();

        service_unit_file.add(
            UNIT_SECTION,
            "Requires",
            volume_service_name.to_str().unwrap(),
        );
        service_unit_file.add(
            UNIT_SECTION,
            "After",
            volume_service_name.to_str().unwrap(),
        );

        source = volume_unit_info.resource_name.clone();
    }

    Ok(source)
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
    unit_file: &SystemdUnit,
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
    unit_file: &SystemdUnit,
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
                &source,
                units_info_map,
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
    unit: &SystemdUnit,
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

fn lookup_and_add_all_strings(
    unit: &SystemdUnit,
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

fn lookup_and_add_string(
    unit: &SystemdUnit,
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
    if !(mount_type == "volume" || mount_type == "bind" || mount_type == "glob") {
        return Ok(mount);
    }

    let mut csv_writer = csv::Writer::from_writer(vec![]);
    csv_writer.write_field(format!("type={mount_type}"))?;
    for token in tokens.iter() {
        if token.starts_with("source=") || token.starts_with("src=") {
            if let Some((_k, v)) = token.split_once('=') {
                let resolved_source = handle_storage_source(
                    container_unit_file,
                    service_unit_file,
                    v,
                    units_info_map,
                )?;
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

#[cfg(test)]
mod tests {
    use super::*;
}
