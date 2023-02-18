mod quadlet;
mod systemd_unit;

use self::quadlet::*;
use self::quadlet::logger::*;
use self::quadlet::PathBufExt;

use self::systemd_unit::*;

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::env;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::os;
use std::path::{Path, PathBuf};
use std::process;

static SUPPORTED_EXTENSIONS: Lazy<[&OsStr; 4]> = Lazy::new(|| {
    [
        "kube",
        "container",
        "network",
        "volume",
    ].map(|ext| OsStr::new(ext))
});

const QUADLET_VERSION: &str = "0.2.0-dev";
const UNIT_DIR_ADMIN: &str  = "/etc/containers/systemd";
const UNIT_DIR_DISTRO: &str  = "/usr/share/containers/systemd";

#[derive(Debug, Default, PartialEq)]
struct Config {
    dry_run: bool,
    is_user: bool,
    no_kmsg: bool,
    output_path: PathBuf,
    verbose: bool,
    version: bool,
}


fn help() {
    println!("Usage:
quadlet-rs --version
quadlet-rs [--dry-run] [--no-kmsg-log] [--user] [-v|--verbose] OUTPUT_DIR [OUTPUT_DIR] [OUTPUT_DIR]

Options:
    --dry-run      Run in dry-run mode printing debug information
    --no-kmsg-log  Don't log to kmsg
    --user         Run as systemd user
    -v,--verbose   Print debug information");
}

fn parse_args(args: Vec<String>) -> Result<Config, String> {
    let mut cfg = Config {
        dry_run: false,
        is_user: false,
        no_kmsg: false,
        output_path: PathBuf::new(),
        verbose: false,
        version: false,
    };

    cfg.is_user = args[0].contains("user");

    if args.len() < 2 {
        return Err("Missing output directory argument".into())
    } else {
        let mut iter = args.iter();
        // skip $0
        iter.next();
        loop {
            match iter.next().map(String::as_str) {
                Some("--dry-run") => cfg.dry_run = true,
                Some("--no-kmsg-log") => cfg.no_kmsg = true,
                Some("--user") => cfg.is_user = true,
                Some("--verbose" | "-v") => cfg.verbose = true,
                Some("--version") => {
                    cfg.version = true;
                    // short circuit
                    break;
                },
                Some(path) => {
                    cfg.output_path = path.into();
                    // we only need the first path
                    break;
                },
                None => return Err("Missing output directory argument".into()),
            }
        }
    }

    Ok(cfg)
}

// This returns the directories where we read quadlet-supported unit files from
// For system generators these are in /usr/share/containers/systemd (for distro files)
// and /etc/containers/systemd (for sysadmin files).
// For user generators these live in $XDG_CONFIG_HOME/containers/systemd
fn unit_search_dirs(is_user: bool) -> Vec<PathBuf> {
    // Allow overdiding source dir, this is mainly for the CI tests
    if let Ok(unit_dirs_env) = std::env::var("QUADLET_UNIT_DIRS") {
        let unit_dirs_env: Vec<PathBuf> = unit_dirs_env
            .split(":")
            .map(|s| PathBuf::from(s))
            .collect();
        return unit_dirs_env
    }

    let mut dirs: Vec<PathBuf> = vec![];
    if is_user {
        dirs.push(dirs::config_dir().unwrap().join("containers/systemd"))
    } else {
        dirs.push(PathBuf::from(UNIT_DIR_ADMIN));
        dirs.push(PathBuf::from(UNIT_DIR_DISTRO));
    }

    dirs
}

fn load_units_from_dir(source_path: &PathBuf, units: &mut HashMap<OsString, SystemdUnit>) -> io::Result<()> {
    for entry in source_path.read_dir()? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();

        let extension = path.extension().unwrap_or(OsStr::new(""));
        if !SUPPORTED_EXTENSIONS.contains(&extension) {
            continue;
        }
        if units.contains_key(&name) {
            continue;
        }

        debug!("Loading source unit file {path:?}");

        let buf = match fs::read_to_string(&path) {
            Ok(buf) => buf,
            Err(e) => {
                log!("Error loading {path:?}, ignoring: {e}");
                continue;
           },
        };

        let unit = match SystemdUnit::load_from_str(buf.as_str()) {
            Ok(mut unit) => {
                unit.path = Some(path);
                unit
            },
            Err(e) => {
                log!("Error loading {path:?}, ignoring: {e}");
                continue;
           },
        };

        units.insert(name, unit);
    }

    Ok(())
}

fn quad_replace_extension(file: &PathBuf, new_extension: &str, extra_prefix: &str, extra_suffix: &str) -> PathBuf {
    let base_name = file.file_stem().unwrap().to_str().unwrap();

    file.with_file_name(format!("{extra_prefix}{base_name}{extra_suffix}{new_extension}"))
}

// Convert a quadlet container file (unit file with a Container group) to a systemd
// service file (unit file with Service group) based on the options in the Container group.
// The original Container group is kept around as X-Container.
fn convert_container(container: &SystemdUnit, is_user: bool) -> Result<SystemdUnit, ConversionError> {
    let mut service = SystemdUnit::new();

    service.merge_from(container);
    service.path = Some(quad_replace_extension(container.path().unwrap(), ".service", "", ""));

    if container.path().is_some() {
        service.append_entry(
            UNIT_SECTION,
            "SourcePath",
            container.path().unwrap().to_str().unwrap(),
        );
    }

    check_for_unknown_keys(&container, CONTAINER_SECTION, &*SUPPORTED_CONTAINER_KEYS)?;

    service.rename_section(CONTAINER_SECTION, X_CONTAINER_SECTION);

    // One image or rootfs must be specified for the container
    let image = container.lookup_last(CONTAINER_SECTION, "Image")
        .map_or(String::new(), |s| s.to_string());
    let rootfs = container.lookup_last(CONTAINER_SECTION, "Rootfs")
        .map_or(String::new(), |s| s.to_string());
    if image.is_empty() && rootfs.is_empty() {
        return Err(ConversionError::InvalidImageOrRootfs("no Image or Rootfs key specified".into()))
    }
    if !image.is_empty() && !rootfs.is_empty() {
        return Err(ConversionError::InvalidImageOrRootfs("the Image And Rootfs keys conflict can not be specified together".into()))
    }

    let podman_container_name = container
        .lookup_last(CONTAINER_SECTION, "ContainerName")
        .map(|v| v.to_string())
        // By default, We want to name the container by the service name
        .unwrap_or("systemd-%N".to_owned());

    // Set PODMAN_SYSTEMD_UNIT so that podman auto-update can restart the service.
    service.append_entry(
        SERVICE_SECTION,
        "Environment",
        "PODMAN_SYSTEMD_UNIT=%n",
    );

    // Only allow mixed or control-group, as nothing else works well
    let kill_mode = service.lookup_last(SERVICE_SECTION, "KillMode");
    match kill_mode {
        None | Some("mixed") | Some("control-group") => {
            // We default to mixed instead of control-group, because it lets conmon do its thing
            service.set_entry(SERVICE_SECTION, "KillMode", "mixed");
        },
        Some(kill_mode) => {
            return Err(ConversionError::InvalidKillMode(format!("invalid KillMode '{kill_mode}'")));
        }
    }

    // Read env early so we can override it below
    let environments = container
        .lookup_all_values(CONTAINER_SECTION, "Environment")
        .map(|v| v.raw().as_str())
        .collect();
    let env_args: HashMap<String, String> = quad_parse_kvs(&environments);

    // Need the containers filesystem mounted to start podman
    service.append_entry(
        UNIT_SECTION,
        "RequiresMountsFor",
        "%t/containers",
    );

    // If conmon exited uncleanly it may not have removed the container, so
    // force it, -i makes it ignore non-existing files.
    service.append_entry(
        SERVICE_SECTION,
        "ExecStop",
        "/usr/bin/podman rm -f -i --cidfile=%t/%N.cid",
    );
    // The ExecStopPost is needed when the main PID (i.e., conmon) gets killed.
    // In that case, ExecStop is not executed but *Post only.  If both are
    // fired in sequence, *Post will exit when detecting that the --cidfile
    // has already been removed by the previous `rm`..
    service.append_entry(
        SERVICE_SECTION,
        "ExecStopPost",
        "-/usr/bin/podman rm -f -i --cidfile=%t/%N.cid",
    );

    let mut podman = PodmanCommand::new_command("run");

    podman.add(format!("--name={podman_container_name}"));

    // We store the container id so we can clean it up in case of failure
    podman.add("--cidfile=%t/%N.cid");

    // And replace any previous container with the same name, not fail
    podman.add("--replace");

    // On clean shutdown, remove container
    podman.add("--rm");

    // But we still want output to the journal, so use the log driver.
    // FIXME: change to `passthrough` once we can rely on Podman v4.0.0 or newer being present
    // Podman support added in: https://github.com/containers/podman/pull/11390
    // Quadlet default changed in: https://github.com/containers/podman/pull/16237
    podman.add_slice(&["--log-driver", "journald"]);

    // We use crun as the runtime and delegated groups to it
    service.append_entry(
        SERVICE_SECTION,
        "Delegate",
        "yes",
    );
    podman.add_slice(&[ "--runtime", "crun", "--cgroups=split"]);

    let timezone = container.lookup_last(CONTAINER_SECTION, "Timezone");
    if let Some(timezone) = timezone {
        if !timezone.is_empty() {
            podman.add(format!("--tz={}", timezone));
        }
    }

    add_networks(container, CONTAINER_SECTION, &mut service, &mut podman)?;

    // Run with a pid1 init to reap zombies by default (as most apps don't do that)
    let run_init = container.lookup_last(CONTAINER_SECTION, "RunInit")
        .map(|s| parse_bool(s).unwrap_or(false));  // key found: parse or default
    if let Some(run_init) = run_init {
        podman.add_bool("--init", run_init);
    }

    let service_type = container.lookup_last(SERVICE_SECTION, "Type");
    match service_type {
        Some("oneshot") => {},
        Some("notify") | None => {
            // If we're not in oneshot mode always use some form of sd-notify, normally via conmon,
            // but we also allow passing it to the container by setting Notify=yes

            let notify = container.lookup_last(CONTAINER_SECTION, "Notify")
                .map(|s| parse_bool(s).unwrap_or(false))  // key found: parse or default
                .unwrap_or(false);  // key not found: use default
            if notify {
                podman.add("--sdnotify=container");
            } else {
                podman.add("--sdnotify=conmon");
            }
            service.set_entry(
                SERVICE_SECTION,
                "Type",
                "notify",
            );
            service.set_entry(
                SERVICE_SECTION,
                "NotifyAccess",
                "all",
            );

            // Detach from container, we don't need the podman process to hang around
            podman.add("-d");
        },
        Some(service_type) => {
            return Err(ConversionError::InvalidServiceType(format!("invalid service Type '{service_type}'")));
        },
    }

    if let None = container.lookup_last(SERVICE_SECTION, "SyslogIdentifier") {
        service.set_entry(
            SERVICE_SECTION,
            "SyslogIdentifier",
            "%N",
        );
    }

    // Default to no higher level privileges or caps
    let no_new_privileges = container.lookup_last(CONTAINER_SECTION, "NoNewPrivileges")
        .map(|s| parse_bool(s).unwrap_or(false))  // key found: parse or default
        .unwrap_or(false);  // key not found: use default
    if no_new_privileges {
        podman.add("--security-opt=no-new-privileges");
    }

    let security_label_disable = container.lookup_last(CONTAINER_SECTION, "SecurityLabelDisable")
        .map(|s| parse_bool(s).unwrap_or(false))  // key found: parse or default
        .unwrap_or(false);  // key not found: use default
    if security_label_disable {
        podman.add_slice(&["--security-opt", "label:disable"]);
    }

    let security_label_type = container.lookup_last(CONTAINER_SECTION, "SecurityLabelType").unwrap_or_default();
    if !security_label_type.is_empty() {
        podman.add("--security-opt");
        podman.add(format!("label=type:{security_label_type}"));
    }

    let security_label_file_type = container.lookup_last(CONTAINER_SECTION, "SecurityLabelFileType").unwrap_or_default();
    if !security_label_file_type.is_empty() {
        podman.add("--security-opt");
        podman.add(format!("label=filetype:{security_label_file_type}"));
    }

    let security_label_level = container.lookup_last(CONTAINER_SECTION, "SecurityLabelLevel").unwrap_or_default();
    if !security_label_level.is_empty() {
        podman.add("--security-opt");
        podman.add(format!("label=level:{security_label_level}"));
    }

    let devices: Vec<String> = container.lookup_all_values(CONTAINER_SECTION, "AddDevice")
        .flat_map(|v| SplitStrv::new(v.raw()))
        .collect();
    for device in devices {
        podman.add(format!("--device={device}"))
    }

    // Default to no higher level privileges or caps
    let seccomp_profile = container.lookup_last(CONTAINER_SECTION, "SeccompProfile");
    if let Some(seccomp_profile) = seccomp_profile {
        podman.add_slice(&["--security-opt", &format!("seccomp={seccomp_profile}")])
    }

    let drop_caps: Vec<String> = container
        .lookup_all_values(CONTAINER_SECTION, "DropCapability")
        .flat_map(|v| SplitStrv::new(v.raw()))
        .collect();
    for caps in drop_caps {
        podman.add(format!("--cap-drop={}", caps.to_ascii_lowercase()))
    }

    // But allow overrides with AddCapability
    let  add_caps: Vec<String> = container
        .lookup_all_values(CONTAINER_SECTION, "AddCapability")
        .flat_map(|v| SplitStrv::new(v.raw()))
        .collect();
    for caps in add_caps {
        podman.add(format!("--cap-add={}", caps.to_ascii_lowercase()))
    }


    let read_only = container.lookup_last(CONTAINER_SECTION, "ReadOnly")
        .map(|s| parse_bool(s).unwrap_or(false));  // key found: parse or default
    if let Some(read_only) = read_only {
        podman.add_bool("--read-only", read_only);
    }
    let read_only = read_only.unwrap_or(false);  // key not found: use default

    let volatile_tmp = container.lookup_last(CONTAINER_SECTION, "VolatileTmp")
        .map(|s| parse_bool(s).unwrap_or(false))  // key found: parse or default
        .unwrap_or(false);  // key not found: use default
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
        let uid = container.lookup_last(CONTAINER_SECTION, "User")
            .map(|s| s.parse::<u32>().unwrap_or(0))  // key found: parse or default
            .unwrap_or(0);  // key not found: use default
        let gid = container.lookup_last(CONTAINER_SECTION, "Group")
            .map(|s| s.parse::<u32>().unwrap_or(0))  // key found: parse or default
            .unwrap_or(0);  // key not found: use default

        podman.add("--user");
        if has_group {
            podman.add(format!("{uid}:{gid}"));
        } else {
            podman.add(uid.to_string());
        }
    }

    handle_user_remap(&container, CONTAINER_SECTION, &mut podman, is_user, true)?;

    let volumes: Vec<&str> = container
        .lookup_all(CONTAINER_SECTION, "Volume")
        .collect();
    for volume in volumes {
        let parts: Vec<&str> = volume.split(":").collect();

        let mut source = "";
        let dest;
        let mut options = String::new();

        if parts.len() >= 2 {
            source = parts[0];
            dest = parts[1];
        } else {
            dest = parts[0];
        }
        if parts.len() >= 3 {
            options = format!(":{}", parts[2]);
        }

        let podman_volume_name: PathBuf;

        if !source.is_empty() {
            if source.starts_with("/") {
                // Absolute path
                service.append_entry(
                    UNIT_SECTION,
                    "RequiresMountsFor",
                    source,
                );
            } else if source.ends_with(".volume") {
                // the podman volume name is systemd-$name
                podman_volume_name = quad_replace_extension(
                    &PathBuf::from(source),
                    "",
                    "systemd-",
                    "",
                );

                // the systemd unit name is $name-volume.service
                let volume_service_name = quad_replace_extension(
                    &PathBuf::from(source),
                    ".service",
                    "",
                    "-volume",
                );

                source = podman_volume_name.to_str().unwrap();

                service.append_entry(
                    UNIT_SECTION,
                    "Requires",
                    volume_service_name.to_str().unwrap(),
                );
                service.append_entry(
                    UNIT_SECTION,
                    "After",
                    volume_service_name.to_str().unwrap(),
                );
            }
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
        let exposed_port = exposed_port.trim();  // Allow whitespaces before and after

        if !quad_is_port_range(exposed_port) {
            return Err(ConversionError::InvalidPortFormat(format!("invalid port format '{exposed_port}'")));
        }

        podman.add(format!("--expose={exposed_port}"))
    }

    handle_publish_ports(container, CONTAINER_SECTION, &mut podman)?;

    podman.add_env(&env_args);

    let labels: Vec<&str> = container.lookup_all_values(CONTAINER_SECTION, "Label")
        .map(|v| v.raw().as_str())
        .collect();
    let label_args: HashMap<String, String> = quad_parse_kvs(&labels);
    podman.add_labels(&label_args);

    let annotations: Vec<&str> = container.lookup_all_values(CONTAINER_SECTION, "Annotation")
        .map(|v| v.raw().as_str())
        .collect();
    let annotation_args: HashMap<String, String> = quad_parse_kvs(&annotations);
    podman.add_annotations(&annotation_args);

    let env_files: Vec<PathBuf> = container.lookup_all_values(CONTAINER_SECTION, "EnvironmentFile")
        .flat_map(|v| SplitWord::new(v.raw()) )
        .map(|s| PathBuf::from(s).absolute_from_unit(container))
        .collect();
    for env_file in env_files {
        podman.add("--env-file");
        podman.add(env_file.to_str().expect("EnvironmentFile path is not a valid UTF-8 string"));
    }

    if let Some(env_host) = container.lookup_last(CONTAINER_SECTION, "EnvironmentHost") {
        let env_host = parse_bool(env_host).unwrap_or(false);
        podman.add_bool("--env-host", env_host);
    }

    let mut podman_args: Vec<String> = container.lookup_all_values(CONTAINER_SECTION, "PodmanArgs")
        .flat_map(|v| SplitWord::new(v.raw()) )
        .collect();
    podman.add_vec(&mut podman_args);

    if !image.is_empty() {
        podman.add(image);
    } else {
        podman.add("--rootfs");
        podman.add(rootfs);
    }

    let mut exec_args = container.lookup_last_value(CONTAINER_SECTION, "Exec")
        .map(|v| SplitWord::new(&v.raw()).collect())
        .unwrap_or(vec![]);
    podman.add_vec(&mut exec_args);

    service.append_entry_value(
        SERVICE_SECTION,
        "ExecStart",
        EntryValue::try_from_raw(podman.to_escaped_string().as_str())?,
    );

    Ok(service)
}

fn handle_user_remap(unit_file: &SystemdUnit, section: &str, podman: &mut PodmanCommand, is_user: bool, support_manual: bool) -> Result<(), ConversionError> {
    let uid_maps: Vec<String> = unit_file.
        lookup_all_values(section, "RemapUid")
        .flat_map(|v| SplitStrv::new(v.raw()))
        .collect();
    let gid_maps: Vec<String> = unit_file.
        lookup_all_values(section, "RemapGid")
        .flat_map(|v| SplitStrv::new(v.raw()))
        .collect();
    let remap_users = unit_file.lookup_last(section, "RemapUsers");
    match remap_users {
        None => {
            if !uid_maps.is_empty() {
                return Err(ConversionError::InvalidRemapUsers("RemapUid set without RemapUsers".into()));
            }
            if !gid_maps.is_empty() {
                return Err(ConversionError::InvalidRemapUsers("RemapGid set without RemapUsers".into()));
            }
        },
        Some("manual") => {
            if support_manual {
                for uid_map in uid_maps {
                    podman.add(format!("--uidmap={uid_map}"));
                }
                for gid_map in gid_maps {
                    podman.add(format!("--gidmap={gid_map}"));
                }
            } else {
                return Err(ConversionError::InvalidRemapUsers("RemapUsers=manual is not supported".into()));
            }
        },
        Some("auto") => {
            let mut auto_opts: Vec<String> = Vec::with_capacity(uid_maps.len() + gid_maps.len() + 1);
            for uid_map in uid_maps {
                auto_opts.push(format!("uidmapping={uid_map}"));
            }
            for gid_map in gid_maps {
                auto_opts.push(format!("gidmapping={gid_map}"));
            }
            let uid_size = unit_file
                .lookup_last(section, "RemapUidSize")
                .map(|s| s.parse::<u32>().unwrap_or(0))  // key found: parse or default
                .unwrap_or(0);  // key not found: use default
            if uid_size > 0 {
                auto_opts.push(format!("size={uid_size}"));
            }

            if auto_opts.is_empty() {
                podman.add("--userns=auto");
            } else {
                podman.add(format!("--userns=auto:{}", auto_opts.join(",")))
            }
        },
        Some("keep-id") => {
            if !is_user {
                return Err(ConversionError::InvalidRemapUsers("RemapUsers=keep-id is unsupported for system units".into()));
            }
            podman.add("--userns=keep-id");
        },
        Some(remap_users) => {
            return Err(ConversionError::InvalidRemapUsers(format!("unsupported RemapUsers option '{remap_users}'")));
        },
    }

    Ok(())
}

fn add_networks(quadlet_unit_file: &SystemdUnit, section: &str, service_unit_file: &mut SystemdUnit, podman: &mut PodmanCommand) -> Result<(), ConversionError> {
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
                podman_network_name = quad_replace_extension(&PathBuf::from(network_name), "", "systemd-", "");

                // the systemd unit name is $name-network.service
                let network_service_name = quad_replace_extension(&PathBuf::from(network_name), ".service", "", "-network");

                service_unit_file.append_entry(UNIT_SECTION, "Requires", network_service_name.to_str().unwrap());
                service_unit_file.append_entry(UNIT_SECTION, "After", network_service_name.to_str().unwrap());

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

fn handle_publish_ports(unit_file: &SystemdUnit, section: &str, podman: &mut PodmanCommand) -> Result<(), ConversionError> {
    let publish_ports: Vec<&str> = unit_file
        .lookup_all(section, "PublishPort")
        .collect();
    for publish_port in publish_ports {
        let publish_port = publish_port.trim();  // Allow whitespaces before and after

        //  IP address could have colons in it. For example: "[::]:8080:80/tcp, so use custom splitter
        let mut parts = quad_split_ports(publish_port);

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
            },
            2 => {
                // NOTE: order is inverted because of pop()
                container_port = parts.pop().unwrap();
                host_port = parts.pop().unwrap();
            },
            3 => {
                // NOTE: order is inverted because of pop()
                container_port = parts.pop().unwrap();
                host_port = parts.pop().unwrap();
                ip = parts.pop().unwrap();
            },
            _ => {
                return Err(ConversionError::InvalidPublishedPort(format!("invalid published port '{publish_port}'")));
            },
        }

        if ip == "0.0.0.0" {
            ip.clear();
        }

        if !host_port.is_empty() && !quad_is_port_range(host_port.as_str()) {
            return Err(ConversionError::InvalidPortFormat(format!("invalid port format '{host_port}'")));
        }

        if !container_port.is_empty() && !quad_is_port_range(container_port.as_str()) {
            return Err(ConversionError::InvalidPortFormat(format!("invalid port format '{container_port}'")));
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

fn convert_kube(kube: &SystemdUnit, is_user: bool) -> Result<SystemdUnit, ConversionError> {
    let mut service = SystemdUnit::new();
    service.merge_from(kube);
    service.path = Some(quad_replace_extension(kube.path().unwrap(), ".service", "", ""));

    if kube.path().is_some() {
        service.append_entry(
            UNIT_SECTION,
            "SourcePath",
            kube.path().unwrap().to_str().unwrap(),
        );
    }

    check_for_unknown_keys(&kube, KUBE_SECTION, &*SUPPORTED_KUBE_KEYS)?;

    // Rename old Kube group to x-Kube so that systemd ignores it
    service.rename_section(KUBE_SECTION, X_KUBE_SECTION);

    let yaml_path = kube.lookup_last(KUBE_SECTION, "Yaml").unwrap_or("");
    if yaml_path.is_empty() {
        return Err(ConversionError::YamlMissing("no Yaml key specified".into()))
    }

    let yaml_path = PathBuf::from(yaml_path).absolute_from_unit(&kube);

    // Only allow mixed or control-group, as nothing else works well
    let kill_mode = kube.lookup_last(KUBE_SECTION, "KillMode");
    match kill_mode {
        None | Some("mixed") | Some("control-group") => {
            // We default to mixed instead of control-group, because it lets conmon do its thing
            service.set_entry(SERVICE_SECTION, "KillMode", "mixed");
        },
        Some(kill_mode) => {
            return Err(ConversionError::InvalidKillMode(format!("invalid KillMode '{kill_mode}'")));
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

    handle_user_remap(&kube, KUBE_SECTION, &mut podman_start, is_user, false)?;

    add_networks(&kube, KUBE_SECTION, &mut service, &mut podman_start)?;

    let config_maps: Vec<PathBuf> = kube.
        lookup_all_values(KUBE_SECTION, "ConfigMap")
        .flat_map(|v| SplitStrv::new(v.raw()))
        .map(|s| PathBuf::from(s))
        .collect();
    for config_map in config_maps {
        let config_map_path = config_map.absolute_from_unit(&kube);
        podman_start.add("--configmap");
        podman_start.add(config_map_path.to_str().expect("ConfigMap path is not valid UTF-8 string"));
    }

    handle_publish_ports(kube, KUBE_SECTION, &mut podman_start)?;

    podman_start.add(yaml_path.to_str().expect("Yaml path is not valid UTF-8 string"));

    service.append_entry_value(
        SERVICE_SECTION,
        "ExecStart",
        EntryValue::try_from_raw(podman_start.to_escaped_string().as_str())?,
    );

    let mut podman_stop = PodmanCommand::new_command("kube");
    podman_stop.add("down");
    podman_stop.add(yaml_path.to_str().expect("Yaml path is not valid UTF-8 string"));
    service.append_entry_value(
        SERVICE_SECTION,
        "ExecStop",
        EntryValue::try_from_raw(podman_stop.to_escaped_string().as_str())?,
    );

    Ok(service)
}

// Convert a quadlet network file (unit file with a Network group) to a systemd
// service file (unit file with Service group) based on the options in the Network group.
// The original Network group is kept around as X-Network.
fn convert_network(network: &SystemdUnit) -> Result<SystemdUnit, ConversionError> {
    let mut service = SystemdUnit::new();
    service.merge_from(network);
    service.path = Some(quad_replace_extension(network.path().unwrap(), ".service", "", "-network"));

    if network.path().is_some() {
        service.append_entry(
            UNIT_SECTION,
            "SourcePath",
            network.path().unwrap().to_str().unwrap(),
        );
    }

    check_for_unknown_keys(&network, NETWORK_SECTION, &*SUPPORTED_NETWORK_KEYS)?;

    // Rename old Network group to x-Network so that systemd ignores it
    service.rename_section(NETWORK_SECTION, X_NETWORK_SECTION);

    let podman_network_name = quad_replace_extension(network.path().unwrap(),  "", "systemd-", "")
        .file_name().unwrap().to_str().unwrap().to_string();

    // Need the containers filesystem mounted to start podman
    service.append_entry(UNIT_SECTION, "RequiresMountsFor", "%t/containers");

    let mut podman = PodmanCommand::new_command("network");
    podman.add("create");
    // FIXME: add `--ignore` once we can rely on Podman v4.4.0 or newer being present
    // Podman support added in: https://github.com/containers/podman/pull/16773
    // Quadlet support added in: https://github.com/containers/podman/pull/16688
    //podman.add("--ignore");

    let disable_dns = network.lookup_last(NETWORK_SECTION, "DisableDNS")
        .map(|s| parse_bool(s).unwrap_or(false))  // key found: parse or default
        .unwrap_or(false);  // key not found: use default
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
            return Err(ConversionError::InvalidSubnet("cannot set more gateways than subnets".into()));
        }
        if ip_ranges.len() > subnets.len() {
            return Err(ConversionError::InvalidSubnet("cannot set more ranges than subnets".into()));
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
		return Err(ConversionError::InvalidSubnet("cannot set Gateway or IPRange without Subnet".into()));
    }

    let internal = network.lookup_last(NETWORK_SECTION, "Internal")
        .map(|s| parse_bool(s).unwrap_or(false))  // key found: parse or default
        .unwrap_or(false);  // key not found: use default
    if internal {
        podman.add("--internal")
    }

    let ipam_driver = network.lookup_last(NETWORK_SECTION, "IPAMDriver");
    if let Some(ipam_driver) = ipam_driver {
        podman.add(format!("--ipam-driver={ipam_driver}"));
    }

    let ipv6 = network.lookup_last(NETWORK_SECTION, "IPv6")
        .map(|s| parse_bool(s).unwrap_or(false))  // key found: parse or default
        .unwrap_or(false);  // key not found: use default
    if ipv6 {
        podman.add("--ipv6")
    }

    let network_options: Vec<&str> = network.lookup_all_values(NETWORK_SECTION, "Options")
        .map(|v| v.raw().as_str())
        .collect();
    let network_options: HashMap<String, String> = quad_parse_kvs(&network_options);
    if !network_options.is_empty() {
        podman.add_keys("--opt", &network_options);
    }

    let labels: Vec<&str> = network.lookup_all_values(NETWORK_SECTION, "Label")
        .map(|v| v.raw().as_str())
        .collect();
    let label_args: HashMap<String, String> = quad_parse_kvs(&labels);
    podman.add_labels(&label_args);

    podman.add(&podman_network_name);

    service.append_entry_value(
        SERVICE_SECTION,
        "ExecStart",
        EntryValue::try_from_raw(podman.to_escaped_string().as_str())?,
    );

    service.append_entry(SERVICE_SECTION,"Type", "oneshot");
    service.append_entry(SERVICE_SECTION,"RemainAfterExit", "yes");

    service.append_entry_value(
        SERVICE_SECTION,
        "ExecCondition",
        EntryValue::try_from_raw(format!("/usr/bin/bash -c \"! /usr/bin/podman network exists {podman_network_name}\""))?,
    );

    // The default syslog identifier is the exec basename (podman) which isn't very useful here
    service.append_entry(SERVICE_SECTION,"SyslogIdentifier", "%N");

    Ok(service)
}

// Convert a quadlet volume file (unit file with a Volume group) to a systemd
// service file (unit file with Service group) based on the options in the Volume group.
// The original Volume group is kept around as X-Volume.
fn convert_volume(volume: &SystemdUnit) -> Result<SystemdUnit, ConversionError> {
    let mut service = SystemdUnit::new();
    service.merge_from(volume);
    service.path = Some(quad_replace_extension(volume.path().unwrap(), ".service", "", "-volume"));

    if volume.path().is_some() {
        service.append_entry(
            UNIT_SECTION,
            "SourcePath",
            volume.path().unwrap().to_str().unwrap(),
        );
    }

    check_for_unknown_keys(&volume, VOLUME_SECTION, &*SUPPORTED_VOLUME_KEYS)?;

    // Rename old Volume group to x-Volume so that systemd ignores it
    service.rename_section(VOLUME_SECTION, X_VOLUME_SECTION);

    let podman_volume_name = quad_replace_extension(volume.path().unwrap(), "", "systemd-", "")
        .file_name().unwrap().to_str().unwrap().to_string();

    // Need the containers filesystem mounted to start podman
    service.append_entry(UNIT_SECTION, "RequiresMountsFor", "%t/containers");

    let labels: Vec<&str> = volume.lookup_all_values(VOLUME_SECTION, "Label")
        .map(|v| v.raw().as_str())
        .collect();
    let label_args: HashMap<String, String> = quad_parse_kvs(&labels);

    let mut podman = PodmanCommand::new_command("volume");
    podman.add("create");
    // FIXME: add `--ignore` once we can rely on Podman v4.4.0 or newer being present
    // Podman support added in: https://github.com/containers/podman/pull/16243
    // Quadlet default changed in: https://github.com/containers/podman/pull/16243
    //podman.add("--ignore")

    let mut opts: Vec<String> = Vec::with_capacity(2);
    if volume.has_key(VOLUME_SECTION, "User") {
        let uid = volume.lookup_last(VOLUME_SECTION, "User")
                .map(|s| s.parse::<u32>().unwrap_or(0))  // key found: parse or default
                .unwrap_or(0);  // key not found: use default
        opts.push(format!("uid={uid}"));
    }
    if volume.has_key(VOLUME_SECTION, "Group") {
        let gid = volume.lookup_last(VOLUME_SECTION, "Group")
                .map(|s| s.parse::<u32>().unwrap_or(0))  // key found: parse or default
                .unwrap_or(0);  // key not found: use default
        opts.push(format!("gid={gid}"));
    }

    if let Some(copy) = volume.lookup_last(VOLUME_SECTION, "Copy")
            .map(|s| parse_bool(s).unwrap_or(false)) {  // key found: parse or default
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
                return Err(ConversionError::InvalidDeviceType("key Type can't be used without Device".into()))
            }
        }
    }

    if let Some(mount_opts) = volume.lookup_last(VOLUME_SECTION, "Options") {
        if !mount_opts.is_empty() {
            if dev_valid {
                opts.push(mount_opts.into());
            } else {
                return Err(ConversionError::InvalidDeviceOptions("key Options can't be used without Device".into()))
            }
        }
    }

    if !opts.is_empty() {
        podman.add("--opt");
        podman.add(format!("o={}", opts.join(",")));
    }

    podman.add_labels(&label_args);
    podman.add(&podman_volume_name);

    service.append_entry_value(
        SERVICE_SECTION,
        "ExecStart",
        EntryValue::try_from_raw(podman.to_escaped_string().as_str())?,
    );

    service.append_entry(SERVICE_SECTION,"Type", "oneshot");
    service.append_entry(SERVICE_SECTION,"RemainAfterExit", "yes");

    service.append_entry_value(
        SERVICE_SECTION,
        "ExecCondition",
        EntryValue::try_from_raw(format!("/usr/bin/bash -c \"! /usr/bin/podman volume exists {podman_volume_name}\""))?,
    );

    // The default syslog identifier is the exec basename (podman) which isn't very useful here
    service.append_entry(SERVICE_SECTION,"SyslogIdentifier", "%N");

    Ok(service)
}

fn generate_service_file(service: &mut SystemdUnit) -> io::Result<()> {
    let out_filename = service.path().unwrap();

    debug!("Writing {out_filename:?}");

    let out_file = File::create(&out_filename)?;
    let mut writer = BufWriter::new(out_file);

    let args_0 = env::args().nth(0).unwrap();
    write!(writer, "# Automatically generated by {args_0}\n")?;

    service.write_to(&mut writer)?;

    Ok(())
}

// This parses the `Install` section of the unit file and creates the required
// symlinks to get systemd to start the newly generated file as needed.
// In a traditional setup this is done by "systemctl enable", but that doesn't
// work for auto-generated files like these.
fn enable_service_file(output_path: &Path, service: &SystemdUnit) {
    let mut symlinks: Vec<PathBuf> = Vec::new();
    let service_name = service.path().unwrap().file_name().unwrap();

    let mut alias: Vec<PathBuf> = service
        .lookup_all_values(INSTALL_SECTION, "Alias")
        .flat_map(|v| SplitStrv::new(v.raw()))
        .map(|s| PathBuf::from(s).cleaned())
        .collect();
    symlinks.append(&mut alias);

    let mut wanted_by: Vec<PathBuf> = service
        .lookup_all_values(INSTALL_SECTION, "WantedBy")
        .flat_map(|v| SplitStrv::new(v.raw()))
        .filter(|s| !s.contains('/'))  // Only allow filenames, not paths
        .map(|wanted_by_unit| {
            let mut path = PathBuf::from(format!("{wanted_by_unit}.wants/"));
            path.push(service_name);
            path
        })
        .collect();
    symlinks.append(&mut wanted_by);

    let mut required_by: Vec<PathBuf> = service
        .lookup_all_values(INSTALL_SECTION, "RequiredBy")
        .flat_map(|v| SplitStrv::new(v.raw()))
        .filter(|s| !s.contains('/'))  // Only allow filenames, not paths
        .map(|required_by_unit| {
            let mut path = PathBuf::from(format!("{required_by_unit}.requires/"));
            path.push(service_name);
            path
        })
        .collect();
    symlinks.append(&mut required_by);

    // construct relative symlink targets so that <output_path>/<symlink_rel (aka. foo/<service_name>)>
    // links to <output_path>/<service_name>
    for symlink_rel in symlinks {
        let mut target = PathBuf::new();

        // At this point the symlinks are all relative, canonicalized
        // paths, so the number of slashes corresponds to its path depth
        // i.e. number of slashes == components - 1
        for _ in 1..symlink_rel.components().count() {
            target.push("..");
        }
        target.push(service_name);

        let symlink_path = output_path.join(symlink_rel);
        let symlink_dir = symlink_path.parent().unwrap();
        if let Err(e) = fs::create_dir_all(&symlink_dir) {
            log!("Can't create dir {:?}: {e}", symlink_dir.to_str().unwrap());
            continue;
        }

        debug!("Creating symlink {symlink_path:?} -> {target:?}");
        fs::remove_file(&symlink_path).unwrap_or_default();  // overwrite existing symlinks
        if let Err(e) = os::unix::fs::symlink(target, &symlink_path) {
            log!("Failed creating symlink {:?}: {e}", symlink_path.to_str().unwrap());
            continue;
        }
    }
}

fn main() {
    let exit_code = 0;
    let args: Vec<String> = env::args().collect();

    let cfg = match parse_args(args) {
        Ok(cfg) => cfg,
        Err(msg) => {
            println!("Error: {}", msg);
            help();
            process::exit(1)
        },
    };

    if cfg.verbose || cfg.dry_run {
        logger::enable_debug();
    }
    if cfg.no_kmsg || cfg.dry_run {
        logger::disable_kmsg();
    }

    // short circuit
    if cfg.version {
        println!("quadlet-rs {}", QUADLET_VERSION);
        process::exit(0);
    }

    if !cfg.dry_run {
        debug!("Starting quadlet-rs-generator, output to: {:?}", &cfg.output_path);
    }

    let source_paths = unit_search_dirs(cfg.is_user);

    let mut units: HashMap<OsString, SystemdUnit> = HashMap::default();
    for dir in &source_paths {
        if let Err(e) = load_units_from_dir(&dir, &mut units) {
            log!("Can't read {dir:?}: {e}");
        }
    }

    if units.is_empty() {
        debug!("No files to parse from {source_paths:?}");
        process::exit(1);
    }

    if !cfg.dry_run {
        if let Err(e) = fs::create_dir_all(&cfg.output_path) {
            log!("Can't create dir {:?}: {e}", cfg.output_path.to_str().unwrap());
            process::exit(1);
        }
    }

    for (name, unit) in units {
        let name = name.into_string().unwrap();

        let service_result = if name.ends_with(".container") {
            warn_if_ambiguous_image_name(&unit);
            convert_container(&unit, cfg.is_user)
        } else if name.ends_with(".kube") {
            convert_kube(&unit, cfg.is_user)
        } else if name.ends_with(".network") {
            convert_network(&unit)
        } else if name.ends_with(".volume") {
            convert_volume(&unit)
        } else {
            log!("Unsupported file type {name:?}");
            continue;
        };
        let mut service = match service_result {
            Ok(service_unit) => service_unit,
            Err(e) => {
                log!("Error converting {name:?}, ignoring: {e}");
                continue;
            },
        };

        let mut service_output_path = cfg.output_path.clone();
        service_output_path.push(service.path().expect("should have a path").file_name().unwrap());
        service.path = Some(service_output_path);

        if cfg.dry_run {
            println!("---{:?}---", service.path().expect("should have a path"));
            io::stdout().write(service.to_string().as_bytes()).expect("should write to STDOUT");
            // NOTE: currently setting entries can fail, because of (un-)quoting errors, so we can't fail here any more
            // TODO: revisit this decision, then we could use the following code ...
            /*match service.to_string() {
                Ok(data) => {
                    println!("---{:?}---\n{data}", service.path.expect("should have a path"));
                },
                Err(e) => {
                    debug!("Error parsing {:?}\n---\n", service.path().expect("should have a path"));
                    exit_code = 1;
                }
            }*/
        } else {
            if let Err(e) = generate_service_file(&mut service) {
                log!("Error writing {:?}, ignoring: {e}", service.path().expect("should have a path"));
                continue;
            }
            enable_service_file(&cfg.output_path, &service);
        }
    }

    process::exit(exit_code);
}

#[cfg(test)]
mod tests {
    use super::*;

    mod parse_args {
        use super::*;

        #[test]
        fn fails_with_no_arguments() {
            let args: Vec<String> = vec![
                "./quadlet-rs".into(),
            ];

            assert_eq!(
                parse_args(args),
                Err("Missing output directory argument".into())
            );
        }

        #[test]
        fn short_circuits_with_version() {
            let args: Vec<String> = vec![
                "./quadlet-rs".into(),
                "--version".into(),
                "--verbose".into(),
                "./output_dir".into(),
            ];

            assert_eq!(
                parse_args(args),
                Ok(Config {
                    version: true,
                    ..Default::default()
                })
            );
        }

        #[test]
        fn parses_user_invocation_from_arg_0() {
            let args: Vec<String> = vec![
                "./quadlet-rs-user-generator".into(),
                "./output_dir".into(),
            ];

            assert_eq!(
                parse_args(args),
                Ok(Config {
                    is_user: true,
                    output_path: "./output_dir".into(),
                    ..Default::default()
                })
            );
        }

        #[test]
        fn accepts_dry_run() {
            let args: Vec<String> = vec![
                "./quadlet-rs".into(),
                "--dry-run".into(),
                "./output_dir".into(),
            ];

            assert_eq!(
                parse_args(args),
                Ok(Config {
                    dry_run: true,
                    output_path: "./output_dir".into(),
                    ..Default::default()
                })
            );
        }

        #[test]
        #[ignore = "hopefully this doesn't need to make it into a release"]
        fn accepts_borked_dry_run_for_quadlet_compat() {
            let args: Vec<String> = vec![
                "./quadlet-rs".into(),
                "-dryrun".into(),
                "./output_dir".into(),
            ];

            assert_eq!(
                parse_args(args),
                Ok(Config {
                    dry_run: true,
                    output_path: "./output_dir".into(),
                    ..Default::default()
                })
            );
        }

        #[test]
        fn accepts_no_kmsg_log() {
            let args: Vec<String> = vec![
                "./quadlet-rs".into(),
                "--no-kmsg-log".into(),
                "./output_dir".into(),
            ];

            assert_eq!(
                parse_args(args),
                Ok(Config {
                    no_kmsg: true,
                    output_path: "./output_dir".into(),
                    ..Default::default()
                })
            );
        }

        #[test]
        fn accepts_user() {
            let args: Vec<String> = vec![
                "./quadlet-rs".into(),
                "--user".into(),
                "./output_dir".into(),
            ];

            assert_eq!(
                parse_args(args),
                Ok(Config {
                    is_user: true,
                    output_path: "./output_dir".into(),
                    ..Default::default()
                })
            );
        }

        #[test]
        fn accepts_verbose() {
            let args: Vec<String> = vec![
                "./quadlet-rs".into(),
                "--verbose".into(),
                "./output_dir".into(),
            ];

            assert_eq!(
                parse_args(args),
                Ok(Config {
                    verbose: true,
                    output_path: "./output_dir".into(),
                    ..Default::default()
                })
            );
        }

        #[test]
        fn accepts_short_verbose() {
            let args: Vec<String> = vec![
                "./quadlet-rs".into(),
                "-v".into(),
                "./output_dir".into(),
            ];

            assert_eq!(
                parse_args(args),
                Ok(Config {
                    verbose: true,
                    output_path: "./output_dir".into(),
                    ..Default::default()
                })
            );
        }

        #[test]
        fn accepts_one_output_dir() {
            let args: Vec<String> = vec![
                "./quadlet-rs".into(),
                "./output_dir".into(),
            ];

            assert_eq!(
                parse_args(args),
                Ok(Config {
                    output_path: "./output_dir".into(),
                    ..Default::default()
                })
            );
        }

        #[test]
        fn requires_output_dir() {
            let args: Vec<String> = vec![
                "./quadlet-rs".into(),
                "-v".into(),
            ];

            assert_eq!(
                parse_args(args),
                Err("Missing output directory argument".into())
            );
        }

        #[test]
        fn picks_first_of_multiple_output_dirs() {
            let args: Vec<String> = vec![
                "./quadlet-rs".into(),
                "./output_dir1".into(),
                "./output_dir2".into(),
                "./output_dir3".into(),
                "./output_dir4".into(),  // systemd actually only specifies 3 output dirs
            ];

            assert_eq!(
                parse_args(args),
                Ok(Config {
                    output_path: "./output_dir1".into(),
                    ..Default::default()
                })
            );
        }
    }
}
