mod systemd_unit;

use std::{env, path::{Path, PathBuf}, collections::HashMap, io::{ErrorKind, self}, fs, ffi::OsString};

use crate::systemd_unit::SystemdUnit;

const QUADLET_VERSION: &str = "0.1.0";
const QUADLET_ADMIN_UNIT_SEARCH_PATH: &str  = "/etc/containers/systemd";
const QUADLET_DISTRO_UNIT_SEARCH_PATH: &str  = "/usr/share/containers/systemd";
const QUADLET_USER_UNIT_SEARCH_PATH: &str  = "~/.local/containers/systemd";

struct ArgError(String);

#[derive(Debug)]
struct Config {
    is_user: bool,
    output_path: Option<String>,
    verbose: bool,
    version: bool,
}

fn help() {
    println!("Usage:
quadlet [-v|-verbose] [--version] OUTPUTDIR");
}


fn parse_args(args: Vec<String>) -> Result<Config, ArgError> {
    if args.len() < 2 {
        return Err(ArgError("Missing output directory argument".into()));
    }

    let mut cfg = Config {
        is_user: args[0].contains("user"),
        output_path: None,
        verbose: false,
        version: false,
    };

    for arg in &args[1..args.len()-1] {
        match &arg[..] {
            "--verbose" => cfg.verbose = true,
            "-v" => cfg.verbose = true,
            "--version" => cfg.version = true,
            _ => return Err(ArgError(format!("Unknown argument {arg}"))),
        }
    }

    cfg.output_path = Some(args.last().unwrap().into());

    Ok(cfg)
}

fn get_user_config_dir() -> PathBuf {
    // FIXME: get user's proper XDG_CONFIG_PATH
    PathBuf::from("~/.config")
}

fn quad_get_unit_dirs<'a>(user: bool) -> Vec<PathBuf> {
    let mut unit_dirs: Vec<PathBuf> = vec![];  // TODO: make lazy static

    if let Ok(unit_dirs_env) = std::env::var("QUADLET_UNIT_DIRS") {
        let mut segments: Vec<PathBuf> = unit_dirs_env.split(":").map(|s| PathBuf::from(s)).collect();
        unit_dirs.append(segments.as_mut());
    } else {
        if user {
            unit_dirs.push(get_user_config_dir().join("containers/systemd"))
        } else {
            unit_dirs.push(PathBuf::from(QUADLET_ADMIN_UNIT_SEARCH_PATH));
            unit_dirs.push(PathBuf::from(QUADLET_DISTRO_UNIT_SEARCH_PATH));
        }
    }

    unit_dirs
}

fn load_units_from_dir(source_path: &PathBuf, units: &mut HashMap<String, SystemdUnit>) -> io::Result<()> {
    for entry in source_path.read_dir().expect("failed to read source path") {
        let entry = entry?;
        let name = entry.file_name().to_str().unwrap();

        if !name.ends_with(".container") && !name.ends_with(".volume") {
            continue;
        }

        if units.contains_key(name) {
            continue;
        }

        let path = entry.path();

        // FIXME: make debug!()
        println!("Loading source unit file {path:?}");

        let data = match fs::read_to_string(&name) {
            Ok(data) => data,
            Err(e) => {
                println!("Error loading {path:?}, ignoring: {e}");
                continue;
            }
        };

        let unit = match SystemdUnit::from_string(&data) {
            Ok(unit) => unit,
            Err(e) => {
                println!("Error loading {path:?}, ignoring: {e}");
                continue;
           },
        };

        units.insert(name.to_owned(), unit);
    }

    Ok(())
}

fn quad_replace_extension(file: &Path, new_extension: &str, extra_prefix: &str, extra_suffix: &str) -> PathBuf {
    let parent = file.parent().unwrap();
    let base_name = file.file_stem().unwrap().to_str().unwrap();

    parent.join(format!("{extra_prefix}{base_name}{extra_suffix}{new_extension}"))
}

fn main() {
    let args: Vec<String> = env::args().collect();

    let cfg = match parse_args(args) {
        Ok(cfg) => cfg,
        Err(ArgError(msg)) => {
            eprintln!("Error: {}", msg);
            help();
            std::process::exit(1)
        },
    };

    if cfg.version {
        println!("quadlet {}", QUADLET_VERSION);
        std::process::exit(0);
    }

    dbg!("Starting quadlet-generator, output to: {}", &cfg.output_path);
    dbg!(&cfg);

    let unit_search_dirs = quad_get_unit_dirs(cfg.is_user);

    let mut units: HashMap<String, SystemdUnit> = HashMap::default();
    for source_path in unit_search_dirs {
        load_units_from_dir(&source_path, &mut units).expect("failed to load unit files");
    }

    for (name, unit) in units {
        let extra_suffix = "";

        if name.ends_with(".container") {
            // TODO: let service =  match convert_container(unit);
            // TODO: print!("Error converting '{name:?}', ignoring: {e}")
        } else if name.ends_with(".volume") {
            // TODO: let service = match convert_volume(unit)
            // TODO: print!("Error converting '{name:?}', ignoring: {e}")
            let extra_suffix = "-volume";
        } else {
            // FIXME: make debug!()
            println!("Unsupported type '{name:?}'");
            continue;
        }

        let service_name = quad_replace_extension(&PathBuf::from(name), ".service", "", extra_suffix);

        // TODO: generate_service_file(output_path, service_name, service, unit);
        // TODO: enable_service_file(output_path, service_name, service);
    }
}
