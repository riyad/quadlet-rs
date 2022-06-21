mod systemd_unit;

use self::systemd_unit::{SystemdUnit, SERVICE_GROUP, UNIT_GROUP};

use log::{debug, warn};
use std::collections::HashMap;
use std::env;
use std::fmt::Display;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

extern crate dirs;
extern crate env_logger;
#[macro_use]
extern crate lazy_static;

lazy_static! {
    static ref RUN_AS_USER: bool = std::env::args().nth(0).unwrap().contains("user");
    static ref UNIT_DIRS: Vec<PathBuf> = {
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
    };
}

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

        let data = match fs::read_to_string(&*name) {
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
    // warn_for_unknown_keys (container, CONTAINER_GROUP, supported_container_keys, &supported_container_keys_hash);

    // FIXME: move to top
    if let None = container.lookup_last(CONTAINER_GROUP, "Image") {
        return Err(ConversionError("No Image key specified"))
    }

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
    // TODO: g_autoptr(GHashTable) podman_env = parse_keys (environments);

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

    // TODO: continue porting

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

    let mut builder = env_logger::Builder::from_default_env();
    builder
        .target(env_logger::Target::Stdout)
        .filter_level(if cfg.verbose { log::LevelFilter::Debug } else { log::LevelFilter::Info });
    builder.init();

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
