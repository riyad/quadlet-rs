mod systemd_unit;

use std::env;

const QUADLET_VERSION: &str = "0.1.0";

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
        is_user: args[0] == "user",
        output_path: None,
        verbose: false,
        version: false,
    };

    for arg in &args[1..args.len()-1] {
        match &arg[..] {
            "--verbose" => cfg.verbose =true,
            "-v" => cfg.verbose =true,
            "--version" => cfg.version =true,
            _ => return Err(ArgError(format!("Unknown argument {}", arg))),
        }
    }

    cfg.output_path = Some(args.last().unwrap().into());

    Ok(cfg)
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

    // TODO: find quadlet unit files
    // TODO: parse quadlet unit files
    // TODO: generate service units
}
