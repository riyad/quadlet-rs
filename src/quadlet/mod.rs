mod podman_command;
mod ranges;

pub(crate) use self::podman_command::*;
pub(crate) use self::ranges::*;

use once_cell::unsync::Lazy;
use regex::Regex;
use std::fs;
use std::path::PathBuf;

pub(crate) fn is_port_range(port: &str) -> bool {
    const RE: Lazy<Regex> = Lazy::new(|| Regex::new("\\d+(-\\d+)?(/udp|/tcp)?$").unwrap());
    RE.is_match(port)
}

pub(crate) fn quad_lookup_host_subgid(user: &str) -> Option<IdRanges> {
    let file_contents = Lazy::new(|| {
        fs::read_to_string(PathBuf::from("/etc/subgid"))
            .expect("failed to read /etc/subgid")
    });

    quad_lookup_host_subid(&*file_contents, user)
}

pub(crate) fn quad_lookup_host_subuid(user: &str) -> Option<IdRanges> {
    let file_contents = Lazy::new(|| {
        fs::read_to_string(PathBuf::from("/etc/subgid"))
            .expect("failed to read /etc/subgid")
    });

    quad_lookup_host_subid(&*file_contents, user)
}

fn quad_lookup_host_subid(file_contents: &String, prefix: &str) -> Option<IdRanges>  {
    let mut ranges = IdRanges::empty();

    for line in file_contents.lines() {
        if line.starts_with(prefix) {
            let mut parts = line.splitn(3, ":");

            if let Some(name) = parts.next() {
                if name == prefix {
                    let start: u32 = parts.next().unwrap_or("").parse().unwrap_or(0);
                    let length: u32 = parts.next().unwrap_or("").parse().unwrap_or(0);

                    if start != 0 && length != 0 {
                        ranges.add(start, length);
                    }
                }
            }
        }
    }

    if !ranges.is_empty() {
        return Some(ranges)
    }

    None
}
