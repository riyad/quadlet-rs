mod quadlet;
mod systemd_unit;

use log::{debug, error};

use self::quadlet::logger::*;
use self::quadlet::*;

use self::systemd_unit::*;

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::env;

use std::ffi::OsString;
use std::fs;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process;

const QUADLET_VERSION: &str = "0.2.0-dev";

#[derive(Debug, Default, PartialEq)]
pub(crate) struct CliOptions {
    dry_run: bool,
    is_user: bool,
    no_kmsg: bool,
    output_path: PathBuf,
    verbose: bool,
    version: bool,
}

impl CliOptions {
    fn from_systemd_env() -> Self {
        Self {
            dry_run: false,
            is_user: env::var("SYSTEMD_SCOPE").map_or(false, |scope| scope == "user"),
            no_kmsg: false,
            output_path: PathBuf::new(),
            verbose: env::var("SYSTEMD_LOG_LEVEL").map_or(false, |log_level| log_level == "debug"),
            version: false,
        }
    }
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
    let mut cfg = CliOptions::from_systemd_env();

    // NOTE: overrides value set by SYSTEMD_SCOPE env var
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

fn validate_args(mut kmsg_logger: KmsgLogger) -> Result<CliOptions, RuntimeError> {
    let args = env::args().collect();

    let cfg = match parse_args(args) {
        Ok(cfg) => {
            // short circuit
            if cfg.version {
                println!("quadlet-rs {}", QUADLET_VERSION);
                process::exit(0);
            }

            if cfg.dry_run {
                kmsg_logger.dry_run = true;
            }
            if cfg.verbose || cfg.dry_run {
                kmsg_logger.debug_enabled = true;
            }
            if cfg.no_kmsg || cfg.dry_run {
                kmsg_logger.kmsg_enabled = false.into();
            }

            cfg
        }
        Err(RuntimeError::CliMissingOutputDirectory(cfg)) => {
            // short circuit
            if cfg.version {
                println!("quadlet-rs {}", QUADLET_VERSION);
                process::exit(0)
            }

            if cfg.dry_run {
                kmsg_logger.dry_run = true;
            }
            if cfg.verbose || cfg.dry_run {
                kmsg_logger.debug_enabled = true;
            }
            if cfg.no_kmsg || cfg.dry_run {
                kmsg_logger.kmsg_enabled = false.into();
            }

            // FIXME: DRY the code around
            if !cfg.dry_run {
                return Err(RuntimeError::CliMissingOutputDirectory(cfg));
            }

            cfg
        }
        Err(e) => return Err(e),
    };

    kmsg_logger.init().expect("could not initialize logger");

    if !cfg.dry_run {
        debug!(
            "Starting quadlet-rs-generator, output to: {:?}",
            &cfg.output_path
        );
    }

    Ok(cfg)
}

fn load_units_from_dir(
    source_path: &Path,
    seen: &mut HashSet<OsString>,
) -> Vec<Result<SystemdUnitFile, RuntimeError>> {
    let mut results = Vec::new();

    let files = match iterators::UnitFiles::new(source_path) {
        Ok(entries) => entries,
        Err(e) => {
            results.push(Err(e));
            return results;
        }
    };

    for file in files {
        let file = match file {
            Ok(file) => file,
            Err(e) => {
                results.push(Err(e));
                continue;
            }
        };

        let path = file.path();
        let name = file.file_name();

        if seen.contains(&name) {
            continue;
        }

        debug!("Loading source unit file {path:?}");

        let unit = match SystemdUnitFile::load_from_path(&path) {
            Ok(unit) => unit,
            Err(e) => {
                match e {
                    IoError::Io(e) => {
                        results.push(Err(RuntimeError::Io(format!("Error loading {path:?}"), e)));
                    }
                    IoError::Unit(e) => {
                        results.push(Err(RuntimeError::Conversion(
                            format!("Error loading {path:?}"),
                            ConversionError::Parsing(e),
                        )));
                    }
                }
                continue;
            }
        };

        seen.insert(name);
        results.push(Ok(unit));
    }

    results
}

fn main() {
    let kmsg_logger = KmsgLogger::from_systemd_env();

    let cfg = match validate_args(kmsg_logger) {
        Ok(cfg) => cfg,
        Err(e) => {
            help();
            error!("{e}");
            process::exit(1);
        }
    };

    let errs = process(cfg);
    if !errs.is_empty() {
        for e in errs {
            error!("{e}");
        }
        process::exit(1);
    }
    process::exit(0);
}

fn process(cfg: CliOptions) -> Vec<RuntimeError> {
    let mut prev_errors: Vec<RuntimeError> = Vec::new();

    let mut seen = HashSet::new();

    // This returns the directories where we read quadlet-supported unit files from
    // For system generators these are in /usr/share/containers/systemd (for distro files)
    // and /etc/containers/systemd (for sysadmin files).
    // For user generators these can live in /etc/containers/systemd/users, /etc/containers/systemd/users/$UID, and $XDG_CONFIG_HOME/containers/systemd
    let source_paths = UnitSearchDirs::from_env_or_system()
        .rootless(cfg.is_user)
        .build();

    let mut units: Vec<QuadletSourceUnitFile> = source_paths
        .iter()
        .flat_map(|dir| load_units_from_dir(dir.as_path(), &mut seen))
        .map(|result| match result {
            Ok(u) => match QuadletSourceUnitFile::from_unit_file(u) {
                Ok(u) => Ok(u),
                Err(e) => Err(RuntimeError::Conversion(
                    format!("initializing Quadlet source unit"),
                    e.into(),
                )),
            },
            Err(e) => Err(e),
        })
        .filter_map(|result| match result {
            Ok(u) => Some(u),
            Err(e) => {
                prev_errors.push(e);
                None
            }
        })
        .collect();

    if units.is_empty() {
        // containers/podman/issues/17374: exit cleanly but log that we
        // had nothing to do
        debug!("No files parsed from {:?}", source_paths.dirs());
        return prev_errors;
    }

    for quadlet in units.iter_mut() {
        let _ = quadlet
            .unit_file
            .load_dropins_from(source_paths.dirs().iter().map(|d| d.as_path()))
            .map_err(|e| {
                prev_errors.push(RuntimeError::Conversion(
                    format!("failed loading drop-ins for {quadlet:?}"),
                    e.into(),
                ))
            });
    }

    if !cfg.dry_run {
        if let Err(e) = fs::create_dir_all(&cfg.output_path) {
            prev_errors.push(RuntimeError::Io(
                format!("Can't create dir {:?}", cfg.output_path),
                e,
            ));
            return prev_errors;
        }
    }

    // Key: Extension
    // Value: Processing order for resource naming dependencies
    let sorting_priority: HashMap<QuadletType, usize> = HashMap::from([
        (QuadletType::Container, 4),
        (QuadletType::Build, 3),
        (QuadletType::Image, 1),
        (QuadletType::Kube, 4),
        (QuadletType::Network, 2),
        (QuadletType::Pod, 5),
        (QuadletType::Volume, 2),
    ]);

    // Sort unit files according to potential inter-dependencies, with Image, Volume and Network
    // units taking precedence over all others.
    // resulting order: .image < (.network | .volume) < .build < (.container | .kube) < .pod
    units.sort_unstable_by(|a, b| {
        let a_typ = match QuadletType::from_path(a.unit_file.path()) {
            Ok(typ) => sorting_priority.get(&typ).unwrap_or(&usize::MAX),
            Err(_) => &usize::MAX,
        };
        let b_typ = match QuadletType::from_path(b.unit_file.path()) {
            Ok(typ) => sorting_priority.get(&typ).unwrap_or(&usize::MAX),
            Err(_) => &usize::MAX,
        };

        a_typ.partial_cmp(b_typ).unwrap_or(Ordering::Less)
    });

    // Generate the PodsInfoMap to allow containers to link to their pods and add themselves to the pod's containers list
    let mut units_info_map = UnitsInfoMap::from_quadlet_units(units.clone());

    for quadlet_source in units {
        let quadlet_result = convert(&quadlet_source, &mut units_info_map, cfg.is_user);

        let mut quadlet_service = match quadlet_result {
            Ok(service_unit) => service_unit,
            Err(e) => {
                prev_errors.push(RuntimeError::Conversion(
                    format!("Converting {:?}", quadlet_source.unit_file.path()),
                    e,
                ));
                continue;
            }
        };

        let mut service_output_path = cfg.output_path.clone();
        service_output_path.push(quadlet_service.service_file.file_name());
        quadlet_service.service_file.path = service_output_path;

        if cfg.dry_run {
            println!("---{:?}---", quadlet_service.service_file.path());
            _ = io::stdout()
                .write(quadlet_service.service_file.to_string().as_bytes())
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

        if let Err(e) = quadlet_service.generate_service_file() {
            prev_errors.push(RuntimeError::Io(
                format!(
                    "Generatring service file {:?}",
                    quadlet_service.service_file.path()
                ),
                e,
            ));
            continue; // NOTE: Go Quadlet doesn't do this, but it probably should
        }
        quadlet_service
            .service_file
            .enable_service_file(&cfg.output_path);
    }

    prev_errors
}

fn convert<'q>(
    quadlet: &'q QuadletSourceUnitFile,
    units_info_map: &mut UnitsInfoMap,
    is_user: bool,
) -> Result<QuadletServiceUnitFile<'q>, ConversionError> {
    match quadlet.quadlet_type {
        QuadletType::Artifact => convert::from_artifact_unit(quadlet, units_info_map, is_user),
        QuadletType::Build => convert::from_build_unit(quadlet, units_info_map, is_user),
        QuadletType::Container => {
            warn_if_ambiguous_image_name(&quadlet.unit_file, CONTAINER_SECTION);
            convert::from_container_unit(quadlet, units_info_map, is_user)
        }
        QuadletType::Image => {
            warn_if_ambiguous_image_name(&quadlet.unit_file, IMAGE_SECTION);
            convert::from_image_unit(quadlet, units_info_map, is_user)
        }
        QuadletType::Kube => convert::from_kube_unit(quadlet, units_info_map, is_user),
        QuadletType::Network => convert::from_network_unit(quadlet, units_info_map, is_user),
        QuadletType::Pod => convert::from_pod_unit(quadlet, units_info_map, is_user),
        QuadletType::Volume => {
            warn_if_ambiguous_image_name(&quadlet.unit_file, VOLUME_SECTION);
            convert::from_volume_unit(quadlet, units_info_map, is_user)
        } // _ => {
          //     warn!("Unsupported file type {:?}", unit.path());
          //     continue;
          // }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod cli_options {
        use super::*;

        mod from_systemd_env {
            use super::*;

            #[test]
            #[serial_test::serial]
            fn defaults_to_system_scope() {
                // remember global state
                let _systemd_scope = env::var("SYSTEMD_SCOPE");

                env::remove_var("SYSTEMD_SCOPE");

                assert_eq!(CliOptions::from_systemd_env().is_user, false);

                // restore global setate
                match _systemd_scope {
                    Ok(val) => env::set_var("SYSTEMD_SCOPE", val),
                    Err(_) => env::remove_var("SYSTEMD_SCOPE"),
                }
            }

            #[test]
            #[serial_test::serial]
            fn recognizes_system_scope_from_env() {
                // remember global state
                let _systemd_scope = env::var("SYSTEMD_SCOPE");

                env::set_var("SYSTEMD_SCOPE", "system");

                assert_eq!(CliOptions::from_systemd_env().is_user, false);

                // restore global setate
                match _systemd_scope {
                    Ok(val) => env::set_var("SYSTEMD_SCOPE", val),
                    Err(_) => env::remove_var("SYSTEMD_SCOPE"),
                }
            }

            #[test]
            #[serial_test::serial]
            fn recognizes_user_scope_from_env() {
                // remember global state
                let _systemd_scope = env::var("SYSTEMD_SCOPE");

                env::set_var("SYSTEMD_SCOPE", "user");

                assert_eq!(CliOptions::from_systemd_env().is_user, true);

                // restore global setate
                match _systemd_scope {
                    Ok(val) => env::set_var("SYSTEMD_SCOPE", val),
                    Err(_) => env::remove_var("SYSTEMD_SCOPE"),
                }
            }

            #[test]
            #[serial_test::serial]
            fn defaults_to_non_verbose_log_level() {
                // remember global state
                let _systemd_log_level = env::var("SYSTEMD_LOG_LEVEL");

                env::set_var("SYSTEMD_LOG_LEVEL", "foo");

                assert_eq!(CliOptions::from_systemd_env().verbose, false);

                // restore global setate
                match _systemd_log_level {
                    Ok(val) => env::set_var("SYSTEMD_LOG_LEVEL", val),
                    Err(_) => env::remove_var("SYSTEMD_LOG_LEVEL"),
                }
            }

            #[test]
            #[serial_test::serial]
            fn recognizes_debug_log_level_from_env() {
                // remember global state
                let _systemd_log_level = env::var("SYSTEMD_LOG_LEVEL");

                env::set_var("SYSTEMD_LOG_LEVEL", "debug");

                assert_eq!(CliOptions::from_systemd_env().verbose, true);

                // restore global setate
                match _systemd_log_level {
                    Ok(val) => env::set_var("SYSTEMD_LOG_LEVEL", val),
                    Err(_) => env::remove_var("SYSTEMD_LOG_LEVEL"),
                }
            }
        }
    }

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
        fn recognizes_user_scope_from_arg_0_filename() {
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
        fn recognizes_user_scope_from_arg_0_path() {
            let args: Vec<String> = vec![
                "./user-generators/quadlet-rs-generator".into(),
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
            let args: Vec<String> =
                vec!["./quadlet-rs".into(), "-user".into(), "./output_dir".into()];

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
}
