extern crate dirs;
extern crate once_cell;
extern crate simplelog;

mod quadlet;
mod systemd_unit;

use self::quadlet::*;
use self::systemd_unit::*;

use log::{debug, warn};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::env;
use std::fmt::Display;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

static DEFAULT_DROP_CAPS: &[&str] = &["all"];
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

struct ConversionError<'a>(&'a str);

impl<'a> Display for ConversionError<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
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

        // FIXME: make debug!()
        println!("Loading source unit file {path:?}");

        let data = match fs::read_to_string(&path) {
            Ok(data) => data,
            Err(e) => {
                warn!("Error loading {path:?}, ignoring: {e}");
                continue;
            }
        };

        let unit = match SystemdUnit::from_string(&data) {
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
        return Err(ConversionError("No Image key specified"))
    };

    let container_name = container
        .lookup_last(CONTAINER_GROUP, "ContainerName")
        .map(|v| v.to_string())
        // By default, We want to name the container by the service name
        .unwrap_or("systemd-%N".to_owned());

    // Set PODMAN_SYSTEMD_UNIT so that podman auto-update can restart the service.
    service.add_entry(
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
    let environments = container.lookup_all(CONTAINER_GROUP, "Environment");
    let  mut env_args: Vec<String> = vec![];
    /* FIXME: port
    g_autoptr(GHashTable) podman_env = parse_keys (environments);
    */

    // Need the containers filesystem mounted to start podman
    service.add_entry(
        UNIT_GROUP,
        "RequiresMountsFor",
        "%t/containers",
    );

    // Remove any leftover cid file before starting, just to be sure.
    // We remove any actual pre-existing container by name with --replace=true.
    // But --cidfile will fail if the target exists.
    service.add_entry(
        SERVICE_GROUP,
        "ExecStartPre",
        "-rm -f %t/%N.cid",
    );

    // If the conman exited uncleanly it may not have removed the container, so force it,
    // -i makes it ignore non-existing files.
    service.add_entry(
        SERVICE_GROUP,
        "ExecStopPost",
        "-/usr/bin/podman rm -f -i --cidfile=%t/%N.cid",
    );

    // Remove the cid file, to avoid confusion as the container is no longer running.
    service.add_entry(
        SERVICE_GROUP,
        "ExecStopPost",
        "-rm -f %t/%N.cid",
    );

    let mut podman = PodmanCommand::new_command("run");

    let container_name_arg = format!("--name={container_name}");
    podman.add(container_name_arg.as_str());

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
    service.add_entry(
        SERVICE_GROUP,
        "Delegate",
        "yes",
    );
    podman.add_slice(&[ "--runtime", "/usr/bin/crun", "--cgroups=split"]);

    let timezone_arg: String;
    if let Some(timezone) = container.lookup_last(CONTAINER_GROUP, "Timezone") {
        timezone_arg = format!("--tz={}", timezone.to_string());
        podman.add(timezone_arg.as_str());
    }

    // Run with a pid1 init to reap zombies by default (as most apps don't do that)
    if let Some(_) = container.lookup_last(CONTAINER_GROUP, "RunInit") {
        podman.add("--init");
    }

    // By default we handle startup notification with conmon, but allow passing it to the container with Notify=yes
    let notify = container.lookup_last(CONTAINER_GROUP, "Notify")
        .map(|v| v.to_bool().unwrap_or(false))
        .unwrap_or(false);
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
        .map(|v| v.to_bool().unwrap_or(true))
        .unwrap_or(true);
    if no_new_privileges {
        podman.add("--security-opt=no-new-privileges");
    }

    let mut drop_caps: Vec<String> = container
        .lookup_all(CONTAINER_GROUP, "DropCapability")
        .iter()
        .map(|v| v.to_string().to_ascii_lowercase())
        .collect();
    if drop_caps.is_empty() {
        drop_caps = DEFAULT_DROP_CAPS.iter().map(|s| s.to_string()).collect();
    }
    drop_caps = drop_caps.iter().map(|caps| format!("--cap-drop={caps}")).collect();
    podman.add_vec(&drop_caps);

    // But allow overrides with AddCapability
    let add_caps: Vec<String> = container
        .lookup_all(CONTAINER_GROUP, "AddCapability")
        .iter()
        .map(|v| format!("--cap-add={}", v.to_string().to_ascii_lowercase()))
        .collect();
    podman.add_vec(&add_caps);

    // We want /tmp to be a tmpfs, like on rhel host
    let volatile_tmp = container.lookup_last(CONTAINER_GROUP, "VolatileTmp")
        .map(|v| v.to_bool().unwrap_or(true))
        .unwrap_or(true);
    if volatile_tmp {
        podman.add_slice(&["--mount", "type=tmpfs,tmpfs-size=512M,destination=/tmp"]);
    }

    let socket_activated = container.lookup_last(CONTAINER_GROUP, "SocketActivated")
        .map(|v| v.to_bool().unwrap_or(false))
        .unwrap_or(false);
    if socket_activated {
        // TODO: This will not be needed with later podman versions that support activation directly:
        // https://github.com/containers/podman/pull/11316
        podman.add("--preserve-fds=1");
        /* FIXME: port
        g_hash_table_insert (podman_env, g_strdup ("LISTEN_FDS"), g_strdup ("1"));
        */

        // TODO: This will not be 2 when catatonit forwards fds:
        //  https://github.com/openSUSE/catatonit/pull/15
        /* FIXME: port
        g_hash_table_insert (podman_env, g_strdup ("LISTEN_PID"), g_strdup ("2"));
        */
    }

    /* FIXME: port
    uid_t default_container_uid = 0;
    gid_t default_container_gid = 0;

    gboolean keep_id = quad_unit_file_lookup_boolean (container, CONTAINER_GROUP, "KeepId", FALSE);
    if (keep_id)
        {
        if (quad_is_user)
            {
            default_container_uid = getuid ();
            default_container_gid = getgid ();
            quad_podman_addv (podman, "--userns", "keep-id", NULL);
            }
        else
            {
            keep_id = FALSE;
            quad_log ("Key 'KeepId' in '%s' unsupported for system units, ignoring", quad_unit_file_get_path (container));
            }
        }

    uid_t uid = MAX (quad_unit_file_lookup_int (container, CONTAINER_GROUP, "User", default_container_uid), 0);
    gid_t gid = MAX (quad_unit_file_lookup_int (container, CONTAINER_GROUP, "Group", default_container_gid), 0);

    uid_t host_uid = quad_unit_file_lookup_uid (container,CONTAINER_GROUP, "HostUser", uid, error);
    if (host_uid == (uid_t)-1)
        return NULL;

    gid_t host_gid = quad_unit_file_lookup_gid (container,CONTAINER_GROUP, "HostGroup", gid, error);
    if (host_gid == (gid_t)-1)
        return NULL;

    if (uid != default_container_uid || gid != default_container_uid)
        {
        quad_podman_add (podman, "--user");
        if (gid == default_container_gid)
            quad_podman_addf (podman, "%lu", (long unsigned)uid);
        else
            quad_podman_addf (podman, "%lu:%lu", (long unsigned)uid, (long unsigned)gid);
        }

    gboolean remap_users = quad_unit_file_lookup_boolean (container, CONTAINER_GROUP, "RemapUsers", TRUE);

    if (quad_is_user)
        remap_users = FALSE;

    if (!remap_users)
        {
        /* No remapping of users, although we still need maps if the
            main user/group is remapped, even if most ids map one-to-one. */
        if (uid != host_uid)
            add_id_maps (podman, "--uidmap",
                        uid, host_uid, UINT32_MAX, NULL);
        if (gid != host_gid)
            add_id_maps (podman, "--gidmap",
                        gid, host_gid, UINT32_MAX, NULL);
        }
    else
        {
        g_autoptr(QuadRanges) uid_remap_ids = quad_unit_file_lookup_ranges (container, CONTAINER_GROUP, "RemapUidRanges",
                                                                            quad_lookup_host_subuid, default_remap_uids);
        g_autoptr(QuadRanges) gid_remap_ids = quad_unit_file_lookup_ranges (container, CONTAINER_GROUP, "RemapGidRanges",
                                                                            quad_lookup_host_subgid, default_remap_gids);
        guint32 remap_uid_start = MAX (quad_unit_file_lookup_int (container, CONTAINER_GROUP, "RemapUidStart", 1), 0);
        guint32 remap_gid_start = MAX (quad_unit_file_lookup_int (container, CONTAINER_GROUP, "RemapGidStart", 1), 0);

        add_id_maps (podman, "--uidmap",
                    uid, host_uid,
                    remap_uid_start, uid_remap_ids);
        add_id_maps (podman, "--gidmap",
                    gid, host_gid,
                    remap_gid_start, gid_remap_ids);
        }
    */

    let volume_args: Vec<String> = container.lookup_all(CONTAINER_GROUP, "Volume")
        .iter()
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
                service.add_entry(
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

                    service.add_entry(
                        UNIT_GROUP,
                        "Requires",
                        volume_service_name.to_str().unwrap(),
                    );
                    service.add_entry(
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
    podman.add_vec(&volume_args);

    let exposed_port_args: Vec<String> = container
        .lookup_all(CONTAINER_GROUP, "ExposeHostPort")
        .iter()
        .map(|v| {
            let exposed_port = v.to_string().trim_end().to_owned();  // Allow whitespace after

            if is_port_range(exposed_port.as_str()) {
                warn!("Ignoring invalid exposed port: '{exposed_port}'");
                return None
            }

            Some(format!("--expose={exposed_port}"))
        })
        .filter(|o| o.is_some())
        .map(|o| o.unwrap())
        .collect();
    podman.add_vec(&exposed_port_args);

    /* FIXME: port
    g_auto(GStrv) publish_ports = quad_unit_file_lookup_all (container, CONTAINER_GROUP, "PublishPort");
    for (guint i = 0; publish_ports[i] != NULL; i++)
        {
        char *publish_port = g_strstrip (publish_ports[i]); /* Allow whitespaces before and after */
        /* IP address could have colons in it. For example: "[::]:8080:80/tcp, so use custom splitter */
        g_auto(GStrv) parts = quad_split_ports (publish_port);
        const char *container_port = NULL, *ip = NULL, *host_port = NULL;

        /* format (from podman run):
        * ip:hostPort:containerPort | ip::containerPort | hostPort:containerPort | containerPort
        *
        * ip could be IPv6 with minimum of these chars "[::]"
        * containerPort can have a suffix of "/tcp" or "/udp"
        */

        switch (g_strv_length (parts))
            {
            case 1:
            container_port = parts[0];
            break;

            case 2:
            host_port = parts[0];
            container_port = parts[1];
            break;

            case 3:
            ip = parts[0];
            host_port = parts[1];
            container_port = parts[2];
            break;

            default:
            quad_log ("Ignoring invalid published port '%s'", publish_port);
            continue;
            }

        if (host_port && *host_port == 0)
            host_port = NULL;

        if (ip && (strcmp (ip, "0.0.0.0") == 0 || *ip == 0))
            ip = NULL;

        if (host_port && !is_port_range (host_port))
            {
            quad_log ("Invalid port format '%s'", host_port);
            continue;
            }

        if (container_port && !is_port_range (container_port))
            {
            quad_log ("Invalid port format '%s'", container_port);
            continue;
            }

        if (ip)
            quad_podman_addf (podman, "-p=%s:%s:%s", ip, host_port ? host_port : "", container_port);
        else if (host_port)
            quad_podman_addf (podman, "-p=%s:%s", host_port, container_port);
        else
            quad_podman_addf (podman, "-p=%s", container_port);
        }
    */

    podman.add_vec(&env_args);

    /* FIXME: port
    g_auto(GStrv) labels = quad_unit_file_lookup_all (container, CONTAINER_GROUP, "Label");
    g_autoptr(GHashTable) podman_labels = parse_keys (labels);
    quad_podman_add_labels (podman, podman_labels);
    */

    /* FIXME: port
    g_auto(GStrv) annotations = quad_unit_file_lookup_all (container, CONTAINER_GROUP, "Annotation");
    g_autoptr(GHashTable) podman_annotations = parse_keys (annotations);
    quad_podman_add_annotations (podman, podman_annotations);
    */

    let podman_args_args: Vec<&str> = container.lookup_all(CONTAINER_GROUP, "PodmanArgs")
        .iter()
        .map(|v| {
            let podman_args_s = v.to_string();
            /* FIXME: port
            // quad_split_string(
            //     podman_args_s.as_str(),
            //     WHITESPACE,
            //     make_bitflags!(QuadSplitFlags::{RELAX|UNQUOTE|CUNESCAPE}),
            // )
            */
            vec![]
        })
        .flatten()
        .collect();
    podman.add_slice(podman_args_args.as_slice());

    podman.add(image.as_str());

    let exec_key_args = container.lookup_last(CONTAINER_GROUP, "Exec")
        .map(|v| {
            let exec_key = v.to_string();
            /* FIXME: port
            // quad_split_string(
            //     exec_key.as_str(),
            //     WHITESPACE,
            //     make_bitflags!(QuadSplitFlags::{RELAX|UNQUOTE|CUNESCAPE}),
            // )
            */
            vec![]
        })
        .unwrap_or(vec![]);
    podman.add_slice(exec_key_args.as_slice());

    service.add_entry(
        SERVICE_GROUP,
        "ExecStart",
        podman.to_escaped_string().as_str(),
    );

    Ok(service)
}

fn convert_volume(unit: &SystemdUnit) -> Result<SystemdUnit, ConversionError> {
    Ok(SystemdUnit::new())
}

fn generate_service_file(output_path: &Path, service_name: &PathBuf, service: &mut SystemdUnit, orig_unit: &SystemdUnit) -> io::Result<()> {
    let orig_path = &orig_unit.path;
    let out_filename = output_path.join(service_name);

    let out_file = File::open(&out_filename)?;
    let mut writer = BufWriter::new(out_file);

    write!(writer, "# Automatically generated by quadlet-generator\n")?;

    if let Some(orig_path) = orig_path {
        service.add_entry(
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
            match convert_volume(&unit) {
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
