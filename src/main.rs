mod quadlet;
mod systemd_unit;

use self::quadlet::*;
use self::systemd_unit::*;

use log::{debug, warn};
use nix::unistd::{Gid, Uid};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::env;
use std::fmt::Display;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

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
static RUN_AS_USER: Lazy<bool> = Lazy::new(|| {
    env::args().nth(0).unwrap().contains("user")
});
static UNIT_DIRS: Lazy<Vec<PathBuf>> = Lazy::new(|| {
    let mut unit_dirs: Vec<PathBuf> = vec![];

    if let Ok(unit_dirs_env) = std::env::var("QUADLET_UNIT_DIRS") {
        let mut pathes_from_env: Vec<PathBuf> = unit_dirs_env
            .split(":")
            .map(|s| PathBuf::from(s))
            .collect();
        unit_dirs.append(pathes_from_env.as_mut());
    } else {
        if *RUN_AS_USER {
            unit_dirs.push(dirs::config_dir().unwrap().join("containers/systemd"))
        } else {
            unit_dirs.push(PathBuf::from(QUADLET_ADMIN_UNIT_SEARCH_PATH));
            unit_dirs.push(PathBuf::from(QUADLET_DISTRO_UNIT_SEARCH_PATH));
        }
    }

    unit_dirs
});

const QUADLET_VERSION: &str = "0.1.0";
const QUADLET_ADMIN_UNIT_SEARCH_PATH: &str  = "/etc/containers/systemd";
const QUADLET_DISTRO_UNIT_SEARCH_PATH: &str  = "/usr/share/containers/systemd";

const CONTAINER_GROUP: &str = "Container";
const X_CONTAINER_GROUP: &str = "X-Container";
const VOLUME_GROUP: &str = "Volume";
const X_VOLUME_GROUP: &str = "X-Volume";

#[derive(Debug)]
struct Config {
    output_path: PathBuf,
    verbose: bool,
    version: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
enum ConversionError<'a> {
    ImageMissing(&'a str),
    Parsing(ParseError),
}

impl<'a> Display for ConversionError<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self)
    }
}

fn help() {
    println!("Usage:
quadlet --version
quadlet [-v|-verbose] OUTPUTDIR");
}


fn parse_args(args: Vec<String>) -> Result<Config, String> {
    let mut cfg = Config {
        output_path: PathBuf::new(),
        verbose: false,
        version: false,
    };

    if args.len() < 2 {
        return Err("Too few arguments".into())
    } else if args.len() == 2 {
        if args[1] == "--version" {
            cfg.version = true
        } else {
            cfg.output_path = args.last().unwrap().into()
        }
    } else {
        for arg in &args[1..args.len()-1] {
            match &arg[..] {
                "--verbose" => cfg.verbose = true,
                "-v" => cfg.verbose = true,
                "--version" => cfg.version = true,
                _ => return Err(format!("Unknown argument: {arg}")),
            }
        }

        cfg.output_path = args.last().unwrap().into();
    }


    Ok(cfg)
}

fn load_units_from_dir(source_path: &PathBuf, units: &mut HashMap<String, SystemdUnit>) -> io::Result<()> {
    for entry in source_path.read_dir()? {
        let entry = entry?;
        let name = entry.file_name();

        if !name.to_string_lossy().ends_with(".container") && !name.to_string_lossy().ends_with(".volume") {
            continue;
        }

        if units.contains_key(name.to_string_lossy().as_ref()) {
            continue;
        }

        let path = entry.path();

        debug!("Loading source unit file {path:?}");

        let unit = match SystemdUnit::load_from_file(&path) {
            Ok(unit) => unit,
            Err(e) => {
                warn!("Error loading {path:?}, ignoring: {e}");
                continue;
           },
        };

        units.insert(name.to_string_lossy().to_string(), unit);
    }

    Ok(())
}

fn quad_replace_extension(file: &PathBuf, new_extension: &str, extra_prefix: &str, extra_suffix: &str) -> PathBuf {
    let parent = file.parent().unwrap();
    let base_name = file.file_stem().unwrap().to_str().unwrap();

    parent.join(format!("{extra_prefix}{base_name}{extra_suffix}{new_extension}"))
}

fn convert_container(container: &SystemdUnit) -> Result<SystemdUnit, ConversionError> {
    let mut service = SystemdUnit::new();

    service.merge_from(container);

    service.rename_section(CONTAINER_GROUP, X_CONTAINER_GROUP);

    // FIXME: move to top
    /* FIXME: port
    warn_for_unknown_keys (container, CONTAINER_GROUP, supported_container_keys, &supported_container_keys_hash);
    */

    // FIXME: move to top
    let image = if let Some(image) =container.lookup_last(CONTAINER_GROUP, "Image") {
        image.to_string()
    } else {
        return Err(ConversionError::ImageMissing("No Image key specified"))
    };

    let container_name = container
        .lookup_last(CONTAINER_GROUP, "ContainerName")
        .map(|v| v.to_string())
        // By default, We want to name the container by the service name
        .unwrap_or("systemd-%N".to_owned());

    // Set PODMAN_SYSTEMD_UNIT so that podman auto-update can restart the service.
    service.append_entry(
        SERVICE_GROUP,
        "Environment".into(),
        "PODMAN_SYSTEMD_UNIT=%n".into(),
    );

    // Only allow mixed or control-group, as nothing else works well
    let kill_mode = service.lookup_last(SERVICE_GROUP, "KillMode");
    if kill_mode.is_none() || !["mixed", "control-group"].contains(&kill_mode.unwrap().to_string().as_str()) {
        if kill_mode.is_some() {
            warn!("Invalid KillMode {:?}, ignoring", kill_mode.unwrap());
        }

        // We default to mixed instead of control-group, because it lets conmon do its thing
        service.set_entry(SERVICE_GROUP, "KillMode", "mixed");
    }

    // Read env early so we can override it below
    let environments = container
        .lookup_all(CONTAINER_GROUP, "Environment")
        .collect();
    let mut env_args: HashMap<String, String> = parse_keys(&environments);

    // Need the containers filesystem mounted to start podman
    service.append_entry(
        UNIT_GROUP,
        "RequiresMountsFor",
        "%t/containers",
    );

    // Remove any leftover cid file before starting, just to be sure.
    // We remove any actual pre-existing container by name with --replace=true.
    // But --cidfile will fail if the target exists.
    service.append_entry(
        SERVICE_GROUP,
        "ExecStartPre",
        "-rm -f %t/%N.cid",
    );

    // If the conman exited uncleanly it may not have removed the container, so force it,
    // -i makes it ignore non-existing files.
    service.append_entry(
        SERVICE_GROUP,
        "ExecStopPost",
        "-/usr/bin/podman rm -f -i --cidfile=%t/%N.cid",
    );

    // Remove the cid file, to avoid confusion as the container is no longer running.
    service.append_entry(
        SERVICE_GROUP,
        "ExecStopPost",
        "-rm -f %t/%N.cid",
    );

    let mut podman = PodmanCommand::new_command("run");

    podman.add(format!("--name={container_name}"));

    // We store the container id so we can clean it up in case of failure
    podman.add("--cidfile=%t/%N.cid");

    // And replace any previous container with the same name, not fail
    podman.add("--replace");

    // On clean shutdown, remove container
    podman.add("--rm");

    // Detach from container, we don't need the podman process to hang around
    podman.add("-d");

    // But we still want output to the journal, so use the log driver.
    // TODO: Once available we want to use the passthrough log-driver instead.
    podman.add_slice(&["--log-driver", "journald"]);

    // Never try to pull the image during service start
    podman.add("--pull=never");

    // We use crun as the runtime and delegated groups to it
    service.append_entry(
        SERVICE_GROUP,
        "Delegate",
        "yes",
    );
    podman.add_slice(&[ "--runtime", "/usr/bin/crun", "--cgroups=split"]);

    if let Some(timezone) = container.lookup_last(CONTAINER_GROUP, "Timezone") {
        if !timezone.is_empty() {
            podman.add(format!("--tz={}", timezone));
        }
    }

    // Run with a pid1 init to reap zombies by default (as most apps don't do that)
    let run_init = container.lookup_last(CONTAINER_GROUP, "RunInit")
        .map(|s| parse_bool(s).unwrap_or(true))  // key found: parse or default
        .unwrap_or(true);  // key not found: use default
    if run_init {
        podman.add("--init");
    }

    // By default we handle startup notification with conmon, but allow passing it to the container with Notify=yes
    let notify = container.lookup_last(CONTAINER_GROUP, "Notify")
        .map(|s| parse_bool(s).unwrap_or(false))  // key found: parse or default
        .unwrap_or(false);  // key not found: use default
    if notify {
        podman.add("--sdnotify=container");
    } else {
        podman.add("--sdnotify=conmon");
    }
    service.set_entry(
        SERVICE_GROUP,
        "Type",
        "notify",
    );
    service.set_entry(
        SERVICE_GROUP,
        "NotifyAccess",
        "all",
    );

    if let None = container.lookup_last(SERVICE_GROUP, "SyslogIdentifier") {
        service.set_entry(
            SERVICE_GROUP,
            "SyslogIdentifier",
            "%N",
        );
    }

    // Default to no higher level privileges or caps
    let no_new_privileges = container.lookup_last(CONTAINER_GROUP, "NoNewPrivileges")
        .map(|s| parse_bool(s).unwrap_or(true))  // key found: parse or default
        .unwrap_or(true);  // key not found: use default
    if no_new_privileges {
        podman.add("--security-opt=no-new-privileges");
    }

    let mut drop_caps: Vec<String> = container
        .lookup_all(CONTAINER_GROUP, "DropCapability")
        .map(|s| s.to_ascii_lowercase())
        .collect();
    if drop_caps.is_empty() {
        drop_caps = DEFAULT_DROP_CAPS.iter().map(|s| s.to_string()).collect();
    }
    drop_caps = drop_caps.iter()
        .filter(|s| !s.is_empty())  // explicitly filter empty values
        .map(|caps| format!("--cap-drop={caps}"))
        .collect();
    podman.add_vec(&mut drop_caps);

    // But allow overrides with AddCapability
    let mut  add_caps: Vec<String> = container
        .lookup_all(CONTAINER_GROUP, "AddCapability")
        .map(|v| format!("--cap-add={}", v.to_string().to_ascii_lowercase()))
        .collect();
    podman.add_vec(&mut add_caps);

    // We want /tmp to be a tmpfs, like on rhel host
    let volatile_tmp = container.lookup_last(CONTAINER_GROUP, "VolatileTmp")
        .map(|s| parse_bool(s).unwrap_or(true))  // key found: parse or default
        .unwrap_or(true);  // key not found: use default
    if volatile_tmp {
        podman.add_slice(&["--mount", "type=tmpfs,tmpfs-size=512M,destination=/tmp"]);
    }

    let socket_activated = container.lookup_last(CONTAINER_GROUP, "SocketActivated")
        .map(|s| parse_bool(s).unwrap_or(false))  // key found: parse or default
        .unwrap_or(false);  // key not found: use default
    if socket_activated {
        // TODO: This will not be needed with later podman versions that support activation directly:
        // https://github.com/containers/podman/pull/11316
        podman.add("--preserve-fds=1");
        env_args.insert("LISTEN_FDS".into(), "1".into());

        // TODO: This will not be 2 when catatonit forwards fds:
        //  https://github.com/openSUSE/catatonit/pull/15
        env_args.insert("LISTEN_PID".into(), "2".into());
    }

    let mut default_container_uid = Uid::from_raw(0);
    let mut default_container_gid = Gid::from_raw(0);

    let mut keep_id = container
        .lookup_last(CONTAINER_GROUP, "KeepId")
        .map(|s| parse_bool(s).unwrap_or(false))  // key found: parse or default
        .unwrap_or(false);  // key not found: use default
    if keep_id {
        if *RUN_AS_USER {
            default_container_uid = Uid::current();
            default_container_gid = Gid::current();
            podman.add_slice(&[ "--userns", "keep-id"]);
        } else {
            keep_id = false;
            warn!("Key 'KeepId' in {:?} unsupported for system units, ignoring", container.path());
        }
    }
    let uid = Uid::from_raw(
        0.max(
            container.lookup_last(CONTAINER_GROUP, "User")
                .map(|s| s.parse::<u32>().unwrap_or(0))  // key found: parse or default
                .unwrap_or(0)  // key not found: use default
        )
    );
    let gid = Gid::from_raw(
        0.max(
            container.lookup_last(CONTAINER_GROUP, "Group")
                .map(|s| s.parse::<u32>().unwrap_or(0))  // key found: parse or default
                .unwrap_or(0)  // key not found: use default
        )
    );

    let host_uid = container.lookup_last(CONTAINER_GROUP, "HostUser")
        .map(|s| parse_uid(s))
        .unwrap_or(Ok(uid))  // key not found: use default
        .map_err(|e| ConversionError::Parsing(e))?;  // key found, but parsing caused error: propagate error


    let host_gid = container.lookup_last(CONTAINER_GROUP, "HostGroup")
        .map(|s| parse_gid(s))
        .unwrap_or(Ok(gid))  // key not found: use default
        .map_err(|e| ConversionError::Parsing(e))?;  // key found, but parsing caused error: propagate error

    if uid != default_container_uid || gid != default_container_gid {
        podman.add("--user");
        if gid == default_container_gid {
            podman.add(uid.to_string())
        } else {
            podman.add(format!("{uid}:{gid}"))
        }
    }

    let mut remap_users = container
        .lookup_last(CONTAINER_GROUP, "RemapUsers")
        .map(|s| parse_bool(s).unwrap_or(true))  // key found: parse or default
        .unwrap_or(true);

    if *RUN_AS_USER {
        remap_users = false;
    }

    if !remap_users {
        // No remapping of users, although we still need maps if the
        // main user/group is remapped, even if most ids map one-to-one.
        if uid != host_uid {
            podman.add_id_maps(
                "--uidmap",
                uid.as_raw(),
                host_uid.as_raw(),
                u32::MAX,
                None,
            )
        }
        if gid != host_gid {
            podman.add_id_maps(
                "--gidmap",
                gid.as_raw(),
                host_gid.as_raw(),
                u32::MAX,
                None,
            );
        }
    } else {
        let uid_remap_ids = container.lookup_last(CONTAINER_GROUP, "RemapUidRanges")
            .map(|s| parse_ranges(s, quad_lookup_host_subuid))
            .unwrap_or(DEFAULT_REMAP_UIDS.clone());
        let gid_remap_ids = container.lookup_last(CONTAINER_GROUP, "RemapGidRanges")
            .map(|s| parse_ranges(s, quad_lookup_host_subgid))
            .unwrap_or(DEFAULT_REMAP_GIDS.clone());

        let remap_uid_start = Uid::from_raw(
            0.max(
                container.lookup_last(CONTAINER_GROUP, "RemapUidStart")
                    .map(|s| s.parse::<u32>().unwrap_or(1))  // key found: parse or default
                    .unwrap_or(1)  // key not found: use default
            )
        );
        let remap_gid_start = Gid::from_raw(
            0.max(
                container.lookup_last(CONTAINER_GROUP, "RemapGidStart")
                    .map(|s| s.parse::<u32>().unwrap_or(1))  // key found: parse or default
                    .unwrap_or(1)  // key not found: use default
            )
        );

        podman.add_id_maps(
            "--uidmap",
            uid.as_raw(),
            host_uid.as_raw(),
            remap_uid_start.as_raw(),
            Some(uid_remap_ids),
        );
        podman.add_id_maps(
            "--gidmap",
            gid.as_raw(),
            host_gid.as_raw(),
            remap_gid_start.as_raw(),
            Some(gid_remap_ids),
        );
    }

    let mut volume_args: Vec<String> = container.lookup_all(CONTAINER_GROUP, "Volume")
        .map(|v| {
            let volume = v.to_string();
            let parts: Vec<&str> = volume.split(":").collect();
            if parts.len() < 2 {
                warn!("Ignoring invalid volume '{volume}'");
                return None
            }
            let mut source = parts[0];
            let dest = parts[1];
            let options = if parts.len() >= 3 {
                parts[2]
            } else {
                ""
            };
            let volume_name: PathBuf;

            if source.starts_with("/") {
                // Absolute path
                service.append_entry(
                    UNIT_GROUP,
                    "RequiresMountsFor",
                    source,
                );
            } else {
                // unit name (with .volume suffix) or named podman volume

                if source.ends_with(".volume") {
                    // the podman volume name is systemd-$name
                    volume_name = quad_replace_extension(
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

                    source = volume_name.to_str().unwrap();

                    service.append_entry(
                        UNIT_GROUP,
                        "Requires",
                        volume_service_name.to_str().unwrap(),
                    );
                    service.append_entry(
                        UNIT_GROUP,
                        "After",
                        volume_service_name.to_str().unwrap(),
                    );
                }
            }

            if options.is_empty() {
                Some(format!("-v={source}:{dest}"))
            } else {
                Some(format!("-v={source}:{dest}:{options}"))
            }
        })
        .filter(|o| o.is_some())
        .map(|o| o.unwrap())
        .collect();
    podman.add_vec(&mut volume_args);

    let mut exposed_port_args: Vec<String> = container
        .lookup_all(CONTAINER_GROUP, "ExposeHostPort")
        .map(|v| {
            let exposed_port = v.to_string().trim_end().to_owned();  // Allow whitespace after

            if !is_port_range(exposed_port.as_str()) {
                warn!("Ignoring invalid exposed port: '{exposed_port}'");
                return None
            }

            Some(format!("--expose={exposed_port}"))
        })
        .filter(|o| o.is_some())
        .map(|o| o.unwrap())
        .collect();
    podman.add_vec(&mut exposed_port_args);

    let publish_ports: Vec<&str> = container
        .lookup_all(CONTAINER_GROUP, "PublishPort")
        .collect();
    for publish_port in publish_ports {
        let publish_port = publish_port.trim(); // Allow whitespaces before and after
        //  IP address could have colons in it. For example: "[::]:8080:80/tcp, so use custom splitter
        let mut parts = quad_split_ports(publish_port);

        // format (from podman run):
        // ip:hostPort:containerPort | ip::containerPort | hostPort:containerPort | containerPort
        //
        // ip could be IPv6 with minimum of these chars "[::]"
        // containerPort can have a suffix of "/tcp" or "/udp"
        let mut container_port = String::new();
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
                warn!("Ignoring invalid published port '{publish_port}'");
                continue;
            },
        }

        if ip == "0.0.0.0" {
            ip.clear();
        }

        if !host_port.is_empty() && !is_port_range(host_port.as_str()) {
            warn!("Invalid port format '{host_port}'");
            continue;
        }

        if !container_port.is_empty() && !is_port_range(container_port.as_str()) {
            warn!("Invalid port format '{container_port}'");
            continue;
        }

        if !ip.is_empty() {
            podman.add(format!("-p={ip}:{host_port}:{container_port}"));
        } else if !host_port.is_empty() {
            podman.add(format!("-p={host_port}:{container_port}"));
        } else {
            podman.add(format!("-p={container_port}"));
        }
    }

    podman.add_env(&env_args);

    let labels: Vec<&str> = container.lookup_all(CONTAINER_GROUP, "Label")
        .collect();
    let label_args: HashMap<String, String> = parse_keys(&labels);
    podman.add_labels(&label_args);

    let annotations: Vec<&str> = container.lookup_all(CONTAINER_GROUP, "Annotation")
        .collect();
    let annotation_args: HashMap<String, String> = parse_keys(&annotations);
    podman.add_annotations(&annotation_args);

    let mut podman_args_args: Vec<String> = container.lookup_all(CONTAINER_GROUP, "PodmanArgs")
        .flat_map(|v| SplitWord::new(v) )
        .collect();
    podman.add_vec(&mut podman_args_args);

    podman.add(image);

    let mut exec_args = container.lookup_last(CONTAINER_GROUP, "Exec")
        .map(|v| SplitWord::new(v).collect())
        .unwrap_or(vec![]);
    podman.add_vec(&mut exec_args);

    service.append_entry(
        SERVICE_GROUP,
        "ExecStart",
        podman.to_escaped_string().as_str(),
    );

    Ok(service)
}

fn convert_volume<'a>(volume: &SystemdUnit, name: &String) -> Result<SystemdUnit, ConversionError<'a>> {
    let mut service = SystemdUnit::new();

    service.merge_from(volume);
    let volume_name = quad_replace_extension(&PathBuf::from(name), "", "systemd-", "");

    /* FIXME: port
    warn_for_unknown_keys (container, VOLUME_GROUP, supported_volume_keys, &supported_volume_keys_hash);
     */

    // Rename old Volume group to x-Volume so that systemd ignores it
    service.rename_section(CONTAINER_GROUP, X_CONTAINER_GROUP);

    // Need the containers filesystem mounted to start podman
    service.append_entry(UNIT_GROUP, "RequiresMountsFor", "%t/containers");

    let exec_cond_arg = format!("/usr/bin/bash -c \"! /usr/bin/podman volume exists {}\"", volume_name.to_str().unwrap());

    let labels: Vec<&str> = volume.lookup_all(VOLUME_GROUP, "Label")
        .collect();
    let label_args: HashMap<String, String> = parse_keys(&labels);

    let mut podman = PodmanCommand::new_command("volume");
    podman.add("create");

    let mut opts_arg = String::from("o=");
    if volume.has_key(VOLUME_GROUP, "User") {
        let uid = 0.max(
            volume.lookup_last(VOLUME_GROUP, "User")
                .map(|s| s.parse::<u32>().unwrap_or(0))  // key found: parse or default
                .unwrap_or(0)  // key not found: use default
        );
        if opts_arg.len() > 2 {
            opts_arg.push(',');
        }
        opts_arg.push_str(format!("uid={uid}").as_str());
    }
    if volume.has_key(VOLUME_GROUP, "Group") {
        let gid = 0.max(
            volume.lookup_last(VOLUME_GROUP, "Group")
                .map(|s| s.parse::<u32>().unwrap_or(0))  // key found: parse or default
                .unwrap_or(0)  // key not found: use default
        );
        if opts_arg.len() > 2 {
            opts_arg.push(',');
        }
        opts_arg.push_str(format!("gid={gid}").as_str());
    }
    if opts_arg.len() > 2 {
        podman.add("--opt");
        podman.add(opts_arg);
    }

    podman.add_labels(&label_args);
    podman.add(volume_name.to_string_lossy());

    service.append_entry(SERVICE_GROUP,"Type", "oneshot");
    service.append_entry(SERVICE_GROUP,"RemainAfterExit", "yes");
    service.append_entry(SERVICE_GROUP,"ExecCondition", &exec_cond_arg);
    service.append_entry(
        SERVICE_GROUP,
        "ExecStart",
        podman.to_escaped_string().as_str(),
    );
    service.append_entry(SERVICE_GROUP,"SyslogIdentifier", "%N");

    Ok(service)
}

fn generate_service_file(output_path: &Path, service_name: &PathBuf, service: &mut SystemdUnit, orig_unit: &SystemdUnit) -> io::Result<()> {
    let orig_path = &orig_unit.path();
    let out_filename = output_path.join(service_name);

    let out_file = File::create(&out_filename)?;
    let mut writer = BufWriter::new(out_file);

    write!(writer, "# Automatically generated by quadlet-generator\n")?;

    if let Some(orig_path) = orig_path {
        service.append_entry(
            UNIT_GROUP,
            "SourcePath".into(),
            orig_path.to_str().unwrap().into(),
        );
    }

    debug!("writing {out_filename:?}");

    service.write_to(&mut writer)?;

    Ok(())
}

fn enable_service_file(output_path: &Path, service_name: &PathBuf, service: &SystemdUnit) -> io::Result<()> {
    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().collect();

    let cfg = match parse_args(args) {
        Ok(cfg) => cfg,
        Err(msg) => {
            println!("Error: {}", msg);
            help();
            std::process::exit(1)
        },
    };

    let _ = simplelog::SimpleLogger::init(
        if cfg.verbose { log::LevelFilter::Debug } else { log::LevelFilter::Info },
        simplelog::Config::default(),
    );

    if cfg.version {
        println!("quadlet {}", QUADLET_VERSION);
        std::process::exit(0);
    }

    debug!("Starting quadlet-generator, output to: {:?}", &cfg.output_path);

    let unit_search_dirs = &*UNIT_DIRS;

    let mut units: HashMap<String, SystemdUnit> = HashMap::default();
    for source_path in unit_search_dirs {
        if let Err(e) = load_units_from_dir(&source_path, &mut units) {
            warn!("Can't read {source_path:?}: {e}");
        }
    }

    for (name, unit) in units {
        let mut extra_suffix = "";

        let mut service = if name.ends_with(".container") {
            match convert_container(&unit) {
                Ok(service_unit) => service_unit,
                Err(e) => {
                    warn!("Error converting {name:?}, ignoring: {e}");
                    continue;
                },
            }
        } else if name.ends_with(".volume") {
            extra_suffix = "-volume";
            match convert_volume(&unit, &name) {
                Ok(service_unit) => service_unit,
                Err(e) => {
                    warn!("Error converting {name:?}, ignoring: {e}");
                    continue;
                },
            }
        } else {
            debug!("Unsupported type {name:?}");
            continue;
        };

        let service_name = quad_replace_extension(
            &PathBuf::from(name),
            ".service",
            "",
            extra_suffix,
        );

        match generate_service_file(&cfg.output_path, &service_name, &mut service, &unit){
            Ok(_) => {},
            Err(e) => {
                warn!("Error writing {service_name:?}, ignoring: {e}")
            },
        };
        match enable_service_file(&cfg.output_path, &service_name, &service) {
            Ok(_) => {},
            Err(e) => {
                warn!("Failed to enable generated unit for {service_name:?}, ignoring: {e}")
            },
        }
    }
}
