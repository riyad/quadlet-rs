use std::env;
use std::fs::{File, OpenOptions};
use std::io::{stderr, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::process;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use log::{debug, Level, Metadata, Record};

pub(crate) struct KmsgLogger {
    pub(crate) debug_enabled: bool,
    pub(crate) dry_run: bool,
    kmsg_file: Mutex<Option<File>>,
    pub(crate) kmsg_enabled: AtomicBool,
}

impl KmsgLogger {
    // Honor Systemd telling us to use DEBUG log level
    pub(crate) fn from_systemd_env() -> Self {
        let mut kmsg_logger = Self {
            debug_enabled: false,
            dry_run: false,
            kmsg_file: Mutex::new(None),
            kmsg_enabled: AtomicBool::new(true),
        };

        // see https://www.freedesktop.org/software/systemd/man/devel/systemd.generator.html#Examples
        if let Some(systed_log_level) = env::var("SYSTEMD_LOG_LEVEL").ok() {
            if systed_log_level == "debug" {
                kmsg_logger.debug_enabled = true;
            }
        }
        // see https://mastodon.social/@pid_eins/113548790734704600
        if let Some(debug_invocation_env) = env::var("DEBUG_INVOCATION").ok() {
            if !debug_invocation_env.is_empty() {
                kmsg_logger.debug_enabled = true;
            }
        }

        kmsg_logger
    }

    pub(crate) fn init(self) -> Result<(), log::SetLoggerError> {
        let max_log_level = if self.debug_enabled {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Info
        };

        log::set_boxed_logger(Box::new(self)).map(|()| log::set_max_level(max_log_level))
    }

    fn new() -> Self {
        Self {
            debug_enabled: false,
            dry_run: false,
            kmsg_file: Mutex::new(None),
            kmsg_enabled: AtomicBool::new(true),
        }
    }

    fn log(&self, record: &Record) {
        // every now and then I thinks, wouldn't it by nice if we could use `format_args!()` here ...
        // ... yes, it would ... but we can't: https://github.com/rust-lang/rust/issues/92698
        let msg = format!(
            "quadlet-rs-generator[{}]: {} - {}\n",
            process::id(),
            record.level(),
            record.args()
        );

        if !self.log_to_kmsg(&msg) || self.dry_run {
            stderr()
                .write_all(msg.as_bytes())
                .expect("couldn't write to STDERR");
        }
    }

    fn log_to_kmsg(&self, msg: &str) -> bool {
        if !self.kmsg_enabled.load(std::sync::atomic::Ordering::SeqCst) {
            return false;
        }

        let mut kmsg_file = self.kmsg_file.lock().expect("cannot lock file for logging");

        if kmsg_file.is_none() {
            *kmsg_file = match OpenOptions::new().write(true).mode(0o644).open("/dev/kmsg") {
                Ok(f) => Some(f),
                Err(e) => {
                    self.kmsg_enabled
                        .store(false, std::sync::atomic::Ordering::SeqCst);
                    debug!("Deactivated logging to /dev/kmsg: {e}");
                    return false;
                }
            };
        }

        if let Some(file) = kmsg_file.as_mut() {
            if file.write_all(msg.as_bytes()).is_err() {
                *kmsg_file = None;
                return false;
            }
        }

        true
    }
}

impl log::Log for KmsgLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level()
            <= if self.debug_enabled {
                Level::Debug
            } else {
                Level::Info
            }
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            self.log(record)
        }
    }

    fn flush(&self) {
        // no need to flush here, because we use write_all()
    }
}
