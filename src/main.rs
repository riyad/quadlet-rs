mod quadlet;
mod systemd_unit;

use self::quadlet::logger::*;
use self::quadlet::PathBufExt;
use self::quadlet::*;

use self::systemd_unit::*;

use std::cmp::Ordering;
use std::collections::HashMap;
use std::collections::HashSet;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::os;
use std::path::{Path, PathBuf};
use std::process;
use users;

static SUPPORTED_EXTENSIONS: [&str; 4] = ["container", "kube", "network", "volume"];

const QUADLET_VERSION: &str = "0.2.0-dev";
const UNIT_DIR_ADMIN:  &str = "/etc/containers/systemd";
const UNIT_DIR_DISTRO: &str = "/usr/share/containers/systemd";

#[derive(Debug, Default, PartialEq)]
pub(crate) struct CliOptions {
    dry_run: bool,
    is_user: bool,
    no_kmsg: bool,
    output_path: PathBuf,
    verbose: bool,
    version: bool,
}

fn help() {
    println!(
        "Usage:
quadlet-rs --version
quadlet-rs [--dry-run] [--no-kmsg-log] [--user] [-v|--verbose] OUTPUT_DIR [OUTPUT_DIR] [OUTPUT_DIR]

Options:
    --dry-run      Run in dry-run mode printing debug information
    --no-kmsg-log  Don't log to kmsg
    --user         Run as systemd user
    -v,--verbose   Print debug information
    --version      Print version information and exit
"
    );
}

fn parse_args(args: Vec<String>) -> Result<CliOptions, RuntimeError> {
    let mut cfg = CliOptions {
        dry_run: false,
        is_user: false,
        no_kmsg: false,
        output_path: PathBuf::new(),
        verbose: false,
        version: false,
    };

    cfg.is_user = args[0].contains("user");

    if args.len() < 2 {
        return Err(RuntimeError::CliMissingOutputDirectory(cfg));
    } else {
        let mut iter = args.iter();
        // skip $0
        iter.next();
        loop {
            match iter.next().map(String::as_str) {
                Some("-dryrun" | "--dry-run") => cfg.dry_run = true,
                Some("-no-kmsg-log" | "--no-kmsg-log") => cfg.no_kmsg = true,
                Some("-user" | "--user") => cfg.is_user = true,
                Some("-verbose" | "--verbose" | "-v") => cfg.verbose = true,
                Some("-version" | "--version") => cfg.version = true,
                Some(path) => {
                    cfg.output_path = path.into();
                    // we only need the first path
                    break;
                }
                None => return Err(RuntimeError::CliMissingOutputDirectory(cfg)),
            }
        }
    }

    Ok(cfg)
}

// This returns the directories where we read quadlet-supported unit files from
// For system generators these are in /usr/share/containers/systemd (for distro files)
// and /etc/containers/systemd (for sysadmin files).
// For user generators these can live in /etc/containers/systemd/users, /etc/containers/systemd/users/$UID, and $XDG_CONFIG_HOME/containers/systemd
fn get_unit_search_dirs(rootless: bool) -> Vec<PathBuf> {
    // Allow overdiding source dir, this is mainly for the CI tests
    if let Ok(unit_dirs_env) = std::env::var("QUADLET_UNIT_DIRS") {
        let unit_dirs_env: Vec<PathBuf> = env::split_paths(&unit_dirs_env)
            .map(PathBuf::from)
            .collect();
        return unit_dirs_env;
    }

    let mut dirs: Vec<PathBuf> = Vec::with_capacity(3);
    if rootless {
        let config_dir = dirs::config_dir().expect("could not determine config dir");
        dirs.push(config_dir.join("containers/systemd"));
        dirs.push(
            PathBuf::from(UNIT_DIR_ADMIN)
                .join("users")
                .join(users::get_current_uid().to_string()),
        );
        dirs.push(PathBuf::from(UNIT_DIR_ADMIN).join("users"));
    } else {
        dirs.push(PathBuf::from(UNIT_DIR_ADMIN));
        dirs.push(PathBuf::from(UNIT_DIR_DISTRO));
    }

    dirs
}

fn load_units_from_dir(
    source_path: &Path,
) -> (Vec<SystemdUnitFile>, Vec<RuntimeError>) {
    let mut prev_errors = Vec::new();

    let mut units = Vec::new();
    let mut seen = HashSet::new();

    let files = match source_path.read_dir() {
        Ok(entries) => entries,
        Err(e) => {
            prev_errors.push(RuntimeError::Io(format!("Can't read {source_path:?}"), e));
            return (units, prev_errors)
        },
    };

    for file in files {
        let file = match file {
            Ok(file) => file,
            Err(e) => {
                prev_errors.push(RuntimeError::Io(format!("Can't read {source_path:?}"), e));
                continue;
            },
        };

        let path = file.path();
        let name = file.file_name();

        let extension = path
            .extension()
            .unwrap_or(OsStr::new(""))
            .to_str()
            .expect("extension is not valid UTF-8 string");
        if !SUPPORTED_EXTENSIONS.contains(&extension) {
            continue;
        }
        if seen.contains(&name) {
            continue;
        }

        debug!("Loading source unit file {path:?}");

        let unit = match SystemdUnitFile::load_from_path(&path) {
            Ok(unit) => unit,
            Err(e) => {
                match e {
                    IoError::Io(e) => {
                        prev_errors.push(RuntimeError::Io(format!("Error loading {path:?}"), e));
                    },
                    IoError::Unit(e) => {
                        prev_errors.push(RuntimeError::Conversion(format!("Error loading {path:?}"), ConversionError::Parsing(e)));
                    },
                }
                continue;
            }
        };

        seen.insert(name);
        units.push(unit);
    }

    (units, prev_errors)
}

fn generate_service_file(service: &mut SystemdUnitFile) -> io::Result<()> {
    let out_filename = service.path();

    debug!("Writing {out_filename:?}");

    let out_file = File::create(out_filename)?;
    let mut writer = BufWriter::new(out_file);

    let args_0 = env::args().next().unwrap();
    writeln!(writer, "# Automatically generated by {args_0}")?;

    service.write_to(&mut writer)?;

    Ok(())
}

// This parses the `Install` section of the unit file and creates the required
// symlinks to get systemd to start the newly generated file as needed.
// In a traditional setup this is done by "systemctl enable", but that doesn't
// work for auto-generated files like these.
fn enable_service_file(output_path: &Path, service: &SystemdUnitFile) {
    let mut symlinks: Vec<PathBuf> = Vec::new();
    let service_name = service.path().file_name().expect("should have a file name");

    let mut alias: Vec<PathBuf> = service
        .lookup_all_strv(INSTALL_SECTION, "Alias")
        .map(|s| PathBuf::from(s).cleaned())
        .collect();
    symlinks.append(&mut alias);

    let mut wanted_by: Vec<PathBuf> = service
        .lookup_all_strv(INSTALL_SECTION, "WantedBy")
        .filter(|s| !s.contains('/')) // Only allow filenames, not paths
        .map(|wanted_by_unit| {
            let mut path = PathBuf::from(format!("{wanted_by_unit}.wants/"));
            path.push(service_name);
            path
        })
        .collect();
    symlinks.append(&mut wanted_by);

    let mut required_by: Vec<PathBuf> = service
        .lookup_all_strv(INSTALL_SECTION, "RequiredBy")
        .filter(|s| !s.contains('/')) // Only allow filenames, not paths
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
        if let Err(e) = fs::create_dir_all(symlink_dir) {
            log!("Can't create dir {:?}: {e}", symlink_dir.to_str().unwrap());
            continue;
        }

        debug!("Creating symlink {symlink_path:?} -> {target:?}");
        fs::remove_file(&symlink_path).unwrap_or_default(); // overwrite existing symlinks
        if let Err(e) = os::unix::fs::symlink(target, &symlink_path) {
            log!(
                "Failed creating symlink {:?}: {e}",
                symlink_path.to_str().unwrap()
            );
            continue;
        }
    }
}

fn main() {
    let errs = process();
    if !errs.is_empty() {
        for e in errs {
            log!("{e}");
        }
        process::exit(1);
    }
    process::exit(0);
}

fn process() -> Vec<RuntimeError> {
    let mut prev_errors: Vec<RuntimeError> = Vec::new();

    let args: Vec<String> = env::args().collect();

    let cfg = match parse_args(args) {
        Ok(cfg) => {
            // short circuit
            if cfg.version {
                println!("quadlet-rs {}", QUADLET_VERSION);
                return prev_errors;
            }

            if cfg.dry_run {
                logger::enable_dry_run();
            }
            if cfg.verbose || cfg.dry_run {
                logger::enable_debug();
            }
            if cfg.no_kmsg || cfg.dry_run {
                logger::disable_kmsg();
            }

            cfg
        },
        Err(RuntimeError::CliMissingOutputDirectory(cfg)) => {
            // short circuit
            if cfg.version {
                println!("quadlet-rs {}", QUADLET_VERSION);
                return prev_errors;
            }

            if cfg.dry_run {
                logger::enable_dry_run();
            }
            if cfg.verbose || cfg.dry_run {
                logger::enable_debug();
            }
            if cfg.no_kmsg || cfg.dry_run {
                logger::disable_kmsg();
            }

            if !cfg.dry_run {
                help();
                prev_errors.push(RuntimeError::CliMissingOutputDirectory(cfg));
                return prev_errors;
            }

            cfg
        },
        Err(e) => {
            help();
            prev_errors.push(e);
            return prev_errors;
        }
    };

    if !cfg.dry_run {
        debug!(
            "Starting quadlet-rs-generator, output to: {:?}",
            &cfg.output_path
        );
    }

    let source_paths = get_unit_search_dirs(cfg.is_user);

    let mut units = Vec::new();
    for dir in &source_paths {
        let (new_units, new_errors) = load_units_from_dir(dir);
        units.extend(new_units);
        prev_errors.extend(new_errors);
    }

    if units.is_empty() {
        // containers/podman/issues/17374: exit cleanly but log that we
        // had nothing to do
        debug!("No files parsed from {source_paths:?}");
        return prev_errors;
    }

    if !cfg.dry_run {
        if let Err(e) = fs::create_dir_all(&cfg.output_path) {
            prev_errors.push(RuntimeError::Io(format!("Can't create dir {:?}", cfg.output_path), e));
            return prev_errors;
        }
    }

    // Sort unit files according to potential inter-dependencies, with Volume and Network units
	// taking precedence over all others.
    units.sort_unstable_by(|a, _| {
        match a.path().extension() {
            Some(ext) if ext == "volume" || ext == "network" => Ordering::Less,
            _ => Ordering::Greater,
        }
    });

    // A map of network/volume unit file-names, against their calculated names, as needed by Podman.
	let mut resource_names = HashMap::with_capacity(units.len());

    for unit in units {
        let ext = unit.path().extension().expect("should have file extension");

        let service_result = if ext == "container" {
            warn_if_ambiguous_image_name(&unit);
            convert::from_container_unit(&unit, &resource_names, cfg.is_user)
        } else if ext == "kube" {
            convert::from_kube_unit(&unit, &resource_names, cfg.is_user)
        } else {
            // this is an ugly hack
            let named_service_result = if ext == "network" {
                convert::from_network_unit(&unit)
            } else if ext == "volume" {
                convert::from_volume_unit(&unit)
            } else {
                log!("Unsupported file type {:?}", unit.path());
                continue;
            };
            match named_service_result {
                Ok((service, name)) => {
                    resource_names.insert(service.path().as_os_str().to_os_string(), name.into());
                    Ok(service)
                },
                Err(e) => Err(e),
            }
        };
        let mut service = match service_result {
            Ok(service_unit) => service_unit,
            Err(e) => {
                prev_errors.push(RuntimeError::Conversion(format!("Converting {:?}", unit.path()), e));
                continue;
            }
        };

        let mut service_output_path = cfg.output_path.clone();
        service_output_path.push(
            service
                .path()
                .file_name()
                .unwrap(),
        );
        service.path = service_output_path;

        if cfg.dry_run {
            println!("---{:?}---", service.path());
            _ = io::stdout()
                .write(service.to_string().as_bytes())
                .expect("should write to STDOUT");
            // NOTE: currently setting entries can fail, because of (un-)quoting errors, so we can't fail here any more
            // TODO: revisit this decision, then we could use the following code ...
            /*match service.to_string() {
                Ok(data) => {
                    println!("---{:?}---\n{data}", service.path);
                },
                Err(e) => {
                    prev_errors.push(RuntimeError::Io(format!("Parsing {:?}", service.path()), e))
                    continue;
                }
            }*/
            continue;
        }

        if let Err(e) = generate_service_file(&mut service) {
            prev_errors.push(RuntimeError::Io(
                format!("Generatring service file {:?}", service.path()),
                e,
            ));
            continue; // NOTE: Go Quadlet doesn't do this, but it probably should
        }
        enable_service_file(&cfg.output_path, &service);
    }

    prev_errors
}

#[cfg(test)]
mod tests {
    use super::*;

    mod parse_args {
        use super::*;

        #[test]
        fn fails_with_no_arguments() {
            let args: Vec<String> = vec!["./quadlet-rs".into()];

            assert!(matches!(
                parse_args(args),
                Err(RuntimeError::CliMissingOutputDirectory(_))
            ));
        }

        #[test]
        fn parses_user_invocation_from_arg_0() {
            let args: Vec<String> =
                vec!["./quadlet-rs-user-generator".into(), "./output_dir".into()];

            assert_eq!(
                parse_args(args).ok().unwrap(),
                CliOptions {
                    is_user: true,
                    output_path: "./output_dir".into(),
                    ..Default::default()
                }
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
                parse_args(args).ok().unwrap(),
                CliOptions {
                    dry_run: true,
                    output_path: "./output_dir".into(),
                    ..Default::default()
                }
            );
        }

        #[test]
        fn accepts_single_dash_dry_run_for_quadlet_compat() {
            let args: Vec<String> = vec![
                "./quadlet-rs".into(),
                "-dryrun".into(),
                "./output_dir".into(),
            ];

            assert_eq!(
                parse_args(args).ok().unwrap(),
                CliOptions {
                    dry_run: true,
                    output_path: "./output_dir".into(),
                    ..Default::default()
                }
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
                parse_args(args).ok().unwrap(),
                CliOptions {
                    no_kmsg: true,
                    output_path: "./output_dir".into(),
                    ..Default::default()
                }
            );
        }

        #[test]
        fn accepts_single_dash_no_kmsg_log_for_quadlet_compat() {
            let args: Vec<String> = vec![
                "./quadlet-rs".into(),
                "-no-kmsg-log".into(),
                "./output_dir".into(),
            ];

            assert_eq!(
                parse_args(args).ok().unwrap(),
                CliOptions {
                    no_kmsg: true,
                    output_path: "./output_dir".into(),
                    ..Default::default()
                }
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
                parse_args(args).ok().unwrap(),
                CliOptions {
                    is_user: true,
                    output_path: "./output_dir".into(),
                    ..Default::default()
                }
            );
        }

        #[test]
        fn accepts_single_dash_user_for_quadlet_compat() {
            let args: Vec<String> = vec![
                "./quadlet-rs".into(),
                "-user".into(),
                "./output_dir".into(),
            ];

            assert_eq!(
                parse_args(args).ok().unwrap(),
                CliOptions {
                    is_user: true,
                    output_path: "./output_dir".into(),
                    ..Default::default()
                }
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
                parse_args(args).ok().unwrap(),
                CliOptions {
                    verbose: true,
                    output_path: "./output_dir".into(),
                    ..Default::default()
                }
            );
        }

        #[test]
        fn accepts_version() {
            let args: Vec<String> = vec![
                "./quadlet-rs".into(),
                "--version".into(),
                "./output_dir".into(),
            ];

            assert_eq!(
                parse_args(args).ok().unwrap(),
                CliOptions {
                    version: true,
                    output_path: "./output_dir".into(),
                    ..Default::default()
                }
            );
        }

        #[test]
        fn accepts_single_dash_verbose_for_quadlet_compat() {
            let args: Vec<String> = vec![
                "./quadlet-rs".into(),
                "-verbose".into(),
                "./output_dir".into(),
            ];

            assert_eq!(
                parse_args(args).ok().unwrap(),
                CliOptions {
                    verbose: true,
                    output_path: "./output_dir".into(),
                    ..Default::default()
                }
            );
        }

        #[test]
        fn accepts_short_verbose() {
            let args: Vec<String> = vec!["./quadlet-rs".into(), "-v".into(), "./output_dir".into()];

            assert_eq!(
                parse_args(args).ok().unwrap(),
                CliOptions {
                    verbose: true,
                    output_path: "./output_dir".into(),
                    ..Default::default()
                }
            );
        }

        #[test]
        fn accepts_one_output_dir() {
            let args: Vec<String> = vec!["./quadlet-rs".into(), "./output_dir".into()];

            assert_eq!(
                parse_args(args).ok().unwrap(),
                CliOptions {
                    output_path: "./output_dir".into(),
                    ..Default::default()
                }
            );
        }

        #[test]
        fn requires_output_dir() {
            let args: Vec<String> = vec!["./quadlet-rs".into(), "-v".into()];

            assert!(matches!(
                parse_args(args),
                Err(RuntimeError::CliMissingOutputDirectory(_))
            ));
        }

        #[test]
        fn picks_first_of_multiple_output_dirs() {
            let args: Vec<String> = vec![
                "./quadlet-rs".into(),
                "./output_dir1".into(),
                "./output_dir2".into(),
                "./output_dir3".into(),
                "./output_dir4".into(), // systemd actually only specifies 3 output dirs
            ];

            assert_eq!(
                parse_args(args).ok().unwrap(),
                CliOptions {
                    output_path: "./output_dir1".into(),
                    ..Default::default()
                }
            );
        }
    }

    mod unit_search_dirs {
        use super::*;

        #[test]
        fn rootful() {
            assert_eq!(
                get_unit_search_dirs(false),
                [
                    "/etc/containers/systemd",
                    "/usr/share/containers/systemd",
                ].iter().map(PathBuf::from).collect::<Vec<_>>()
            )
        }

        #[test]
        fn rootless() {
            assert_eq!(
                get_unit_search_dirs(true),
                [
                    format!(
                        "{}/containers/systemd",
                        dirs::config_dir()
                            .expect("could not determine config dir")
                            .to_str()
                            .expect("home dir ist not valid UTF-8 string")
                    ),
                    format!("/etc/containers/systemd/users/{}", users::get_current_uid()),
                    format!("/etc/containers/systemd/users"),
                ]
                .iter()
                .map(PathBuf::from)
                .collect::<Vec<_>>()
            )
        }
    }
}
