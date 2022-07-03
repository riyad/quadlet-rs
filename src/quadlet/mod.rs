mod constants;
mod podman_command;
mod ranges;

use crate::systemd_unit::SplitWord;

pub(crate) use self::constants::*;
pub(crate) use self::podman_command::*;
pub(crate) use self::ranges::*;

use log::warn;
use once_cell::unsync::Lazy;
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

pub(crate) fn is_port_range(port: &str) -> bool {
    const RE: Lazy<Regex> = Lazy::new(|| Regex::new("\\d+(-\\d+)?(/udp|/tcp)?$").unwrap());
    RE.is_match(port)
}

/// parse `key=value` pairs from given list
pub(crate) fn parse_keys<'a>(key_vals: &'a Vec<&str>) -> HashMap<String, String> {
    let mut res = HashMap::new();

    for key_val in key_vals {
        for assign_s in SplitWord::new(key_val) {
            if assign_s.contains("=") {
                let mut splits = assign_s.splitn(2, "=");
                let k = splits.next().unwrap();
                let v = splits.next().unwrap();
                res.insert(k.to_string(), v.to_string());
            } else {
                warn!("Invalid key=value assignment '{assign_s}'");
            }
        }
    }

    res
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

/// Parses arguments to podman-run's `--publish` option.
/// see also the documentation for the `PublishPort` field.
///
/// NOTE: the last part will also include the protocol if specified
pub(crate) fn quad_split_ports(ports: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();

    let mut next_part = String::new();
    let mut chars = ports.chars();
    while let Some(c) = chars.next() {
        let c = c;
        match c {
            '[' => { // IPv6 contain ':' characters, hence they are enclosed with '[...]'
                // so we consume all characters until ']' (including ':') for this part
                next_part.push(c);
                while let Some(c) = chars.next() {
                    next_part.push(c);
                    match c {
                        ']' => break,
                        _ => (),
                    }
                }
            },
            ':' => { // assume all ':' characters are boundaries that start a new part
                parts.push(next_part);
                next_part = String::new();
                continue;
            },
            _ => {
                next_part.push(c);
            }
        }
    }
    // don't forget the last part
    parts.push(next_part);

    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    mod quad_split_ports {
        use super::*;

        #[test]
        fn with_only_port() {
            let input = "123";

            assert_eq!(
                quad_split_ports(input),
                vec!["123"],
            );
        }

        #[test]
        fn with_ipv4_and_port() {
            let input = "1.2.3.4:567";

            assert_eq!(
                quad_split_ports(input),
                vec!["1.2.3.4", "567"],
            );
        }

        #[test]
        fn with_ipv6_and_port() {
            let input = "[::]:567";

            assert_eq!(
                quad_split_ports(input),
                vec!["[::]", "567"],
            );
        }

        #[test]
        fn with_host_and_container_ports() {
            let input = "123:567";

            assert_eq!(
                quad_split_ports(input),
                vec!["123", "567"],
            );
        }

        #[test]
        fn with_ipv4_host_and_container_ports() {
            let input = "0.0.0.0:123:567";

            assert_eq!(
                quad_split_ports(input),
                vec!["0.0.0.0", "123", "567"],
            );
        }

        #[test]
        fn with_ipv6_empty_host_container_port_and_protocol() {
            let input = "[1:2:3:4::]::567/tcp";

            assert_eq!(
                quad_split_ports(input),
                vec!["[1:2:3:4::]", "", "567/tcp"],
            );
        }
    }
}