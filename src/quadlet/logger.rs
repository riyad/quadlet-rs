use std::fs::{File, OpenOptions};
use std::io::{stderr, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::process;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

// although we're using AtomicBool, all accesses have to be wrapped with
// `unsafe { }` because it is `static mut`
static mut DEBUG: AtomicBool = AtomicBool::new(false);
static mut DRY_RUN: AtomicBool = AtomicBool::new(false);
static mut NO_KMSG: AtomicBool = AtomicBool::new(false);
static KMSG_FILE: Mutex<Option<File>> = Mutex::new(None);

macro_rules! debug {
    ($($arg:tt)*) => {
        if $crate::quadlet::logger::is_debug_enabled() {
            log!($($arg)+)
        }
    }
}
pub(crate) use debug;

pub(crate) fn disable_kmsg() {
    unsafe { *NO_KMSG.get_mut() = true };
}

pub(crate) fn enable_debug() {
    unsafe { *DEBUG.get_mut() = true };
}

pub(crate) fn enable_dry_run() {
    unsafe { *DRY_RUN.get_mut() = true };
}

pub(crate) fn is_debug_enabled() -> bool {
    unsafe { *DEBUG.get_mut() }
}

pub(crate) fn is_dry_run_enabled() -> bool {
    unsafe { *DRY_RUN.get_mut() }
}

macro_rules! log {
    ($($arg:tt)*) => ({
        $crate::quadlet::logger::__log(format!($($arg)+))
    })
}
pub(crate) use log;

#[doc(hidden)]
pub(crate) fn __log(msg: String) {
    let line = format!("quadlet-rs-generator[{}]: {}", process::id(), msg);

    if !__log_to_kmsg(&line) || is_dry_run_enabled() {
        eprintln!("{line}");
        stderr().flush().unwrap();
    }
}

#[doc(hidden)]
fn __log_to_kmsg(msg: &str) -> bool {
    if unsafe { *NO_KMSG.get_mut() } {
        return false;
    }

    let mut kmsg_file = KMSG_FILE.lock().unwrap();

    if kmsg_file.is_none() {
        *kmsg_file = match OpenOptions::new().write(true).mode(0o644).open("/dev/kmsg") {
            Ok(f) => Some(f),
            Err(e) => {
                unsafe { *NO_KMSG.get_mut() = true };
                debug!("Deactivated logging to /dev/kmsg: {e}");
                return false;
            }
        };
    }

    if kmsg_file.is_some() {
        let file = kmsg_file.as_mut().unwrap();
        if file.write_all(msg.as_bytes()).is_err() {
            *kmsg_file = None;
            return false;
        }
    }

    true
}
