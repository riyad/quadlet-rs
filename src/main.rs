mod quadlet;
mod systemd_unit;

use self::quadlet::*;
use self::systemd_unit::*;

use log::{debug, info, warn};
use nix::unistd::{Gid, Uid};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::env;
use std::fmt::Display;
use std::fs;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::os;
use std::path::{Path, PathBuf};

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

#[derive(Debug, Default, PartialEq)]
struct Config {
    output_path: PathBuf,
    verbose: bool,
    version: bool,
}

#[derive(Debug)]
#[non_exhaustive]
enum ConversionError {
    ImageMissing(String),
    InvalidKillMode(String),
    InvalidPortFormat(String),
    InvalidPublishedPort(String),
    InvalidServiceType(String),
    Parsing(Error),
}

impl Display for ConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ConversionError::ImageMissing(msg) |
            ConversionError::InvalidKillMode(msg) |
            ConversionError::InvalidPortFormat(msg) |
            ConversionError::InvalidPublishedPort(msg) |
            ConversionError::InvalidServiceType(msg) => {
                write!(f, "{msg}")
            },
            ConversionError::Parsing(e) => {
                write!(f, "Failed parsing unit file: {e}")
            },
        }
    }
}

impl From<Error> for ConversionError {
    fn from(e: Error) -> Self {
        ConversionError::Parsing(e)
    }
}

fn help() {
    println!("Usage:
quadlet-rs --version
quadlet-rs [-v|-verbose] OUTPUT_DIR [OUTPUT_DIR] [OUTPUT_DIR]");
}

fn parse_args(args: Vec<String>) -> Result<Config, String> {
    let mut cfg = Config {
        output_path: PathBuf::new(),
        verbose: false,
        version: false,
    };

    if args.len() < 2 {
        return Err("missing output dir".into())
    } else {
        let mut iter = args.iter();
        // skip $0
        iter.next();
        loop {
            match iter.next().map(String::as_str) {
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
                None => return Err("missing output dir".into()),
            }
        }
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

        let buf = match fs::read_to_string(&path) {
            Ok(buf) => buf,
            Err(e) => {
                warn!("Error loading {path:?}, ignoring: {e}");
                continue;
           },
        };

        let unit = match SystemdUnit::load_from_str(buf.as_str()) {
            Ok(mut unit) => {
                unit.path = Some(path);
                unit
            },
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
    let base_name = file.file_stem().unwrap().to_str().unwrap();

    file.with_file_name(format!("{extra_prefix}{base_name}{extra_suffix}{new_extension}"))
}

// Convert a quadlet container file (unit file with a Container group) to a systemd
// service file (unit file with Service group) based on the options in the Container group.
// The original Container group is kept around as X-Container.
fn convert_container(container: &SystemdUnit) -> Result<SystemdUnit, ConversionError> {
    let mut service = SystemdUnit::new();

    service.merge_from(container);

    service.rename_section(CONTAINER_SECTION, X_CONTAINER_SECTION);

    // FIXME: move to top
    warn_for_unknown_keys(&container, CONTAINER_SECTION, &*SUPPORTED_CONTAINER_KEYS);

    // FIXME: move to top
    let image = if let Some(image) = container.lookup_last(CONTAINER_SECTION, "Image") {
        image.to_string()
    } else {
        return Err(ConversionError::ImageMissing("no Image key specified".into()))
    };

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
    let mut env_args: HashMap<String, String> = quad_parse_kvs(&environments);

    // Need the containers filesystem mounted to start podman
    service.append_entry(
        UNIT_SECTION,
        "RequiresMountsFor",
        "%t/containers",
    );

    // If the conman exited uncleanly it may not have removed the container, so force it,
    // -i makes it ignore non-existing files.
    service.append_entry(
        SERVICE_SECTION,
        "ExecStopPost",
        "-/usr/bin/podman rm -f -i --cidfile=%t/%N.cid",
    );

    // Remove the cid file, to avoid confusion as the container is no longer running.
    service.append_entry(
        SERVICE_SECTION,
        "ExecStopPost",
        "-rm -f %t/%N.cid",
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
    podman.add_slice(&["--log-driver", "passthrough"]);

    // We use crun as the runtime and delegated groups to it
    service.append_entry(
        SERVICE_SECTION,
        "Delegate",
        "yes",
    );
    podman.add_slice(&[ "--runtime", "/usr/bin/crun", "--cgroups=split"]);

    let timezone = container.lookup_last(CONTAINER_SECTION, "Timezone");
    if let Some(timezone) = timezone {
        if !timezone.is_empty() {
            podman.add(format!("--tz={}", timezone));
        }
    }

    // Run with a pid1 init to reap zombies by default (as most apps don't do that)
    let run_init = container.lookup_last(CONTAINER_SECTION, "RunInit")
        .map(|s| parse_bool(s).unwrap_or(false))  // key found: parse or default
        .unwrap_or(false);  // key not found: use default
    if run_init {
        podman.add("--init");
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

    // let mut remap_users = container
    //     .lookup_last(CONTAINER_SECTION, "RemapUsers")
    //     .map(|s| parse_bool(s).unwrap_or(false))  // key found: parse or default
    //     .unwrap_or(false);

    // if *RUN_AS_USER {
    //     remap_users = false;
    // }

    // if !remap_users {
    //     // No remapping of users, although we still need maps if the
    //     // main user/group is remapped, even if most ids map one-to-one.
    //     if uid != host_uid {
    //         podman.add_id_maps(
    //             "--uidmap",
    //             uid.as_raw(),
    //             host_uid.as_raw(),
    //             u32::MAX,
    //             None,
    //         )
    //     }
    //     if gid != host_gid {
    //         podman.add_id_maps(
    //             "--gidmap",
    //             gid.as_raw(),
    //             host_gid.as_raw(),
    //             u32::MAX,
    //             None,
    //         );
    //     }
    // } else {
    //     let uid_remap_ids = container.lookup_last(CONTAINER_SECTION, "RemapUidRanges")
    //         .map(|s| parse_ranges(s, quad_lookup_host_subuid))
    //         .unwrap_or(DEFAULT_REMAP_UIDS.clone());
    //     let gid_remap_ids = container.lookup_last(CONTAINER_SECTION, "RemapGidRanges")
    //         .map(|s| parse_ranges(s, quad_lookup_host_subgid))
    //         .unwrap_or(DEFAULT_REMAP_GIDS.clone());

    //     let remap_uid_start = Uid::from_raw(
    //         0.max(
    //             container.lookup_last(CONTAINER_SECTION, "RemapUidStart")
    //                 .map(|s| s.parse::<u32>().unwrap_or(1))  // key found: parse or default
    //                 .unwrap_or(1)  // key not found: use default
    //         )
    //     );
    //     let remap_gid_start = Gid::from_raw(
    //         0.max(
    //             container.lookup_last(CONTAINER_SECTION, "RemapGidStart")
    //                 .map(|s| s.parse::<u32>().unwrap_or(1))  // key found: parse or default
    //                 .unwrap_or(1)  // key not found: use default
    //         )
    //     );

    //     podman.add_id_maps(
    //         "--uidmap",
    //         uid.as_raw(),
    //         host_uid.as_raw(),
    //         remap_uid_start.as_raw(),
    //         Some(uid_remap_ids),
    //     );
    //     podman.add_id_maps(
    //         "--gidmap",
    //         gid.as_raw(),
    //         host_gid.as_raw(),
    //         remap_gid_start.as_raw(),
    //         Some(gid_remap_ids),
    //     );
    // }

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

        if source.is_empty() {
            podman.add(format!("-v={dest}"))
        } else {
            podman.add(format!("-v={source}:{dest}{options}"))
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

    let publish_ports: Vec<&str> = container
        .lookup_all(CONTAINER_SECTION, "PublishPort")
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

        if !ip.is_empty() && !host_port.is_empty() {
            podman.add(format!("-p={ip}:{host_port}:{container_port}"));
        } else if !ip.is_empty() {
            podman.add(format!("-p={ip}::{container_port}"));
        } else if !host_port.is_empty() {
            podman.add(format!("-p={host_port}:{container_port}"));
        } else {
            podman.add(format!("-p={container_port}"));
        }
    }

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

    let mut podman_args: Vec<String> = container.lookup_all_values(CONTAINER_SECTION, "PodmanArgs")
        .flat_map(|v| SplitWord::new(v.raw()) )
        .collect();
    podman.add_vec(&mut podman_args);

    podman.add(image);

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

// Convert a quadlet volume file (unit file with a Volume group) to a systemd
// service file (unit file with Service group) based on the options in the Volume group.
// The original Volume group is kept around as X-Volume.
fn convert_volume(volume: &SystemdUnit, volume_name: &str) -> Result<SystemdUnit, ConversionError> {
    let mut service = SystemdUnit::new();
    service.merge_from(volume);
    service.path = Some(quad_replace_extension(&PathBuf::from(volume_name), ".service", "", "-volume"));

    warn_for_unknown_keys(&volume, VOLUME_SECTION, &*SUPPORTED_VOLUME_KEYS);

    // Rename old Volume group to x-Volume so that systemd ignores it
    service.rename_section(VOLUME_SECTION, X_VOLUME_SECTION);


    let podman_volume_name = quad_replace_extension(&PathBuf::from(volume_name), "", "systemd-", "");
    let podman_volume_name = podman_volume_name.to_str().unwrap();

    // Need the containers filesystem mounted to start podman
    service.append_entry(UNIT_SECTION, "RequiresMountsFor", "%t/containers");

    let exec_cond_arg = format!("/usr/bin/bash -c \"! /usr/bin/podman volume exists {podman_volume_name}\"",);

    let labels: Vec<&str> = volume.lookup_all_values(VOLUME_SECTION, "Label")
        .map(|v| v.raw().as_str())
        .collect();
    let label_args: HashMap<String, String> = quad_parse_kvs(&labels);

    let mut podman = PodmanCommand::new_command("volume");
    podman.add_slice(&["create", "--ignore"]);

    let mut opts_arg = String::from("o=");
    if volume.has_key(VOLUME_SECTION, "User") {
        let uid = volume.lookup_last(VOLUME_SECTION, "User")
                .map(|s| s.parse::<u32>().unwrap_or(0))  // key found: parse or default
                .unwrap_or(0);  // key not found: use default
        if opts_arg.len() > 2 {
            opts_arg.push(',');
        }
        opts_arg.push_str(format!("uid={uid}").as_str());
    }
    if volume.has_key(VOLUME_SECTION, "Group") {
        let gid = volume.lookup_last(VOLUME_SECTION, "Group")
                .map(|s| s.parse::<u32>().unwrap_or(0))  // key found: parse or default
                .unwrap_or(0);  // key not found: use default
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
    podman.add(podman_volume_name);

    service.append_entry_value(
        SERVICE_SECTION,
        "ExecStart",
        EntryValue::try_from_raw(podman.to_escaped_string().as_str())?,
    );

    service.append_entry(SERVICE_SECTION,"Type", "oneshot");
    service.append_entry(SERVICE_SECTION,"RemainAfterExit", "yes");

    // The default syslog identifier is the exec basename (podman) which isn't very useful here
    service.append_entry(SERVICE_SECTION,"SyslogIdentifier", "%N");

    Ok(service)
}

fn generate_service_file(output_path: &Path, service_name: &PathBuf, service: &mut SystemdUnit, orig_unit: &SystemdUnit) -> io::Result<()> {
    let orig_path = &orig_unit.path();
    let out_filename = output_path.join(service_name);

    let out_file = File::create(&out_filename)?;
    let mut writer = BufWriter::new(out_file);

    write!(writer, "# Automatically generated by quadlet-rs-generator\n")?;

    if let Some(orig_path) = orig_path {
        service.append_entry(
            UNIT_SECTION,
            "SourcePath",
            orig_path.to_str().unwrap(),
        );
    }

    debug!("writing {out_filename:?}");

    service.write_to(&mut writer)?;

    Ok(())
}

/// This function normalizes relative the paths by dropping multiple slashes,
/// removing "." elements and making ".." drop the parent element as long
/// as there is not (otherwise the .. is just removed). Symlinks are not
/// handled in any way.
/// TODO: we could use std::path::absolute() here, but it's nightly-only ATM
/// see https://doc.rust-lang.org/std/path/fn.absolute.html
fn canonicalize_relative_path(path: PathBuf) -> PathBuf {
    assert!(path.is_relative());

    // normalized path could be shorter, but never longer
    let mut normalized = PathBuf::with_capacity(path.as_os_str().len());

    for element in path.components() {
        if element.as_os_str().is_empty() || element.as_os_str() == "." {
            continue;
        } else if element.as_os_str() == ".." {
            if normalized.components().count() > 0 {
                normalized.pop();
            }
        } else {
            normalized.push(element);
        }
    }

    normalized
}

fn enable_service_file(output_path: &Path, service_name: &PathBuf, service: &SystemdUnit) -> io::Result<()> {
    let mut symlinks: Vec<PathBuf> = Vec::new();

    let mut alias: Vec<PathBuf> = service
        .lookup_all_values(INSTALL_SECTION, "Alias")
        .flat_map(|v| SplitStrv::new(v.raw()))
        .map(|s| canonicalize_relative_path(PathBuf::from(s)))
        .collect();
    symlinks.append(&mut alias);

    let mut wanted_by: Vec<PathBuf> = service
        .lookup_all_values(INSTALL_SECTION, "WantedBy")
        .flat_map(|v| SplitStrv::new(v.raw()))
        .filter(|s| !s.contains('/'))
        .map(|s| {
            let wanted_by_unit = s;
            PathBuf::from(format!("{wanted_by_unit}.wants/{}", service_name.to_str().unwrap()))
        })
        .collect();
    symlinks.append(&mut wanted_by);

    let mut required_by: Vec<PathBuf> = service
        .lookup_all_values(INSTALL_SECTION, "RequiredBy")
        .flat_map(|v| SplitStrv::new(v.raw()))
        .filter(|s| !s.contains('/'))
        .map(|s| {
            let required_by_unit = s;
            PathBuf::from(format!("{required_by_unit}.requires/{}", service_name.to_str().unwrap()))
        })
        .collect();
    symlinks.append(&mut required_by);

    for symlink_rel in symlinks {
        let mut target = PathBuf::new();

        // At this point the symlinks are all relative, canonicalized
        // paths, so number of slashes is the depth
        // number of slashes == components - 1
        for _ in 1..symlink_rel.components().count() {
            target.push("..");
        }
        target.push(service_name);

        let symlink_path = output_path.join(symlink_rel);
        let symlink_dir = symlink_path.parent().unwrap();
        fs::create_dir_all(&symlink_dir)?;

        info!("Creating symlink {symlink_path:?} -> {target:?}");
        os::unix::fs::symlink(target, symlink_path)?
    }

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

    // short circuit
    if cfg.version {
        println!("quadlet-rs {}", QUADLET_VERSION);
        std::process::exit(0);
    }

    debug!("Starting quadlet-rs-generator, output to: {:?}", &cfg.output_path);

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
            match convert_volume(&unit, name.as_str()) {
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

mod tests {
    mod parse_args {
        use super::super::{Config, parse_args};

        #[test]
        fn fails_with_no_arguments() {
            let args: Vec<String> = vec![
                "./quadlet-rs".into(),
            ];

            assert_eq!(
                parse_args(args),
                Err("missing output dir".into())
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
                Err("missing output dir".into())
            );
        }

        #[test]
        fn picks_first_of_multiple_output_dirs() {
            let args: Vec<String> = vec![
                "./quadlet-rs".into(),
                "./output_dir1".into(),
                "./output_dir2".into(),
                "./output_dir3".into(),
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
