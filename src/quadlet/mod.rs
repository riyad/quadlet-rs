mod constants;
pub(crate) mod logger;
mod path_buf_ext;
mod podman_command;

use self::logger::*;
use crate::systemd_unit;
use crate::systemd_unit::{SplitWord, SystemdUnit};

pub(crate) use self::constants::*;
pub(crate) use self::path_buf_ext::*;
pub(crate) use self::podman_command::*;

use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::Display;
use std::io;

#[derive(Debug)]
#[non_exhaustive]
pub(crate) enum ConversionError {
    InvalidDeviceOptions(String),
    InvalidDeviceType(String),
    InvalidImageOrRootfs(String),
    InvalidKillMode(String),
    InvalidPortFormat(String),
    InvalidPublishedPort(String),
    InvalidRemapUsers(String),
    InvalidServiceType(String),
    InvalidSubnet(String),
    Io(io::Error),
    Parsing(systemd_unit::Error),
    UnknownKey(String),
    YamlMissing(String),
}

impl Display for ConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ConversionError::InvalidDeviceOptions(msg)
            | ConversionError::InvalidDeviceType(msg)
            | ConversionError::InvalidImageOrRootfs(msg)
            | ConversionError::InvalidKillMode(msg)
            | ConversionError::InvalidPortFormat(msg)
            | ConversionError::InvalidPublishedPort(msg)
            | ConversionError::InvalidRemapUsers(msg)
            | ConversionError::InvalidServiceType(msg)
            | ConversionError::InvalidSubnet(msg)
            | ConversionError::UnknownKey(msg)
            | ConversionError::YamlMissing(msg) => {
                write!(f, "{msg}")
            }
            ConversionError::Io(e) => e.fmt(f),
            ConversionError::Parsing(e) => {
                write!(f, "Failed parsing unit file: {e}")
            }
        }
    }
}

impl From<io::Error> for ConversionError {
    fn from(e: io::Error) -> Self {
        ConversionError::Io(e)
    }
}

impl From<systemd_unit::Error> for ConversionError {
    fn from(e: systemd_unit::Error) -> Self {
        ConversionError::Parsing(e)
    }
}

pub(crate) fn quad_is_port_range(port: &str) -> bool {
    // NOTE: We chose to implement a parser ouselves, because pulling in the regex crate just for this
    // increases the binary size by at least 0.5M. :/
    // But if we were to use the regex crate, all this function does is this:
    // const RE: Lazy<Regex> = Lazy::new(|| Regex::new("\\d+(-\\d+)?(/udp|/tcp)?$").unwrap());
    // return RE.is_match(port)

    if port.is_empty() {
        return false;
    }

    let mut chars = port.chars();
    let mut cur: Option<char>;
    let mut digits; // count how many digits we've read

    // necessary "\\d+" part
    digits = 0;
    loop {
        cur = chars.next();
        match cur {
            Some(c) if c.is_ascii_digit() => digits += 1,
            // start of next part
            Some('-' | '/') => break,
            // illegal character
            Some(_) => return false,
            // string has ended, just make sure we've seen at least one digit
            None => return digits > 0,
        }
    }

    // parse optional "(-\\d+)?" part
    if cur.unwrap() == '-' {
        digits = 0;
        loop {
            cur = chars.next();
            match cur {
                Some(c) if c.is_ascii_digit() => digits += 1,
                // start of next part
                Some('/') => break,
                // illegal character
                Some(_) => return false,
                // string has ended, just make sure we've seen at least one digit
                None => return digits > 0,
            }
        }
    }

    // parse optional "(/udp|/tcp)?" part
    let mut tcp = 0; // count how many characters we've read
    let mut udp = 0; // count how many characters we've read
    loop {
        cur = chars.next();
        match cur {
            // parse "tcp"
            Some('t') if tcp == 0 && udp == 0 => tcp += 1,
            Some('c') if tcp == 1 => tcp += 1,
            Some('p') if tcp == 2 => break,
            // parse "udp"
            Some('u') if udp == 0 && tcp == 0 => udp += 1,
            Some('d') if udp == 1 => udp += 1,
            Some('p') if udp == 2 => break,
            // illegal character
            Some(_) => return false,
            // string has ended, just after '/' or in the middle of "tcp" or "udp"
            None => return false,
        }
    }

    // make sure we're at the end of the string
    return chars.next().is_none();
}

/// parse `key=value` pairs from given list
pub(crate) fn quad_parse_kvs<'a>(all_key_vals: &'a Vec<&str>) -> HashMap<String, String> {
    let mut res = HashMap::new();

    for key_vals in all_key_vals {
        for assigns in SplitWord::new(key_vals) {
            if let Some((key, value)) = assigns.split_once("=") {
                res.insert(key.to_string(), value.to_string());
            }
        }
    }

    res
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
            '[' => {
                // IPv6 contain ':' characters, hence they are enclosed with '[...]'
                // so we consume all characters until ']' (including ':') for this part
                next_part.push(c);
                while let Some(c) = chars.next() {
                    next_part.push(c);
                    match c {
                        ']' => break,
                        _ => (),
                    }
                }
            }
            ':' => {
                // assume all ':' characters are boundaries that start a new part
                parts.push(next_part);
                next_part = String::new();
                continue;
            }
            _ => {
                next_part.push(c);
            }
        }
    }
    // don't forget the last part
    parts.push(next_part);

    parts
}

pub(crate) fn check_for_unknown_keys(
    unit: &SystemdUnit,
    group_name: &str,
    supported_keys: &HashSet<&'static str>,
) -> Result<(), ConversionError> {
    for (key, _) in unit.section_entries(group_name) {
        if !supported_keys.contains(key) {
            return Err(ConversionError::UnknownKey(format!(
                "unsupported key '{key}' in group '{group_name}' in {:?}",
                unit.path()
            )));
        }
    }

    Ok(())
}

fn is_image_id(image_name: &str) -> bool {
    // All sha25:... names are assumed by podman to be fully specified
    if image_name.starts_with("sha256:") {
        return true;
    }

    // However, podman also accepts image ids as pure hex strings,
    // but only those of length 64 are unambiguous image ids
    if image_name.len() != 64 {
        return false;
    }
    if image_name.chars().any(|c| !c.is_ascii_hexdigit()) {
        return false;
    }

    return true;
}

fn is_unambiguous_name(image_name: &str) -> bool {
    // Fully specified image ids are unambiguous
    if is_image_id(image_name) {
        return true;
    }

    // Otherwise we require a fully qualified name

    // What is before the first slash can be a domain or a path
    if let Some((domain, _)) = image_name.split_once("/") {
        // If its a domain (has dot or port or is "localhost") it is considered fq
        if domain.contains(['.', ':']) || domain == "localhost" {
            return true;
        }
    } else {
        // No domain or path, not fully qualified
        return false;
    }

    return false;
}

// warns if input is an ambiguous name, i.e. a partial image id or a short
// name (i.e. is missing a registry)
//
// Examples:
//   - short names: "image:tag", "library/fedora"
//   - fully qualified names: "quay.io/image", "localhost/image:tag",
//     "server.org:5000/lib/image", "sha256:..."
//
// We implement a simple version of this from scratch here to avoid
// a huge dependency in the generator just for a warning.
pub(crate) fn warn_if_ambiguous_image_name(container: &SystemdUnit) {
    if let Some(image_name) = container.lookup_last(CONTAINER_SECTION, "Image") {
        if !is_unambiguous_name(image_name) {
            let file_name = container.path().unwrap().file_name().unwrap();
            log!("Warning: {file_name:?} specifies the image {image_name:?} which not a fully qualified image name. This is not ideal for performance and security reasons. See the podman-pull manpage discussion of short-name-aliases.conf for details.");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod quad_split_ports {
        use super::*;

        #[test]
        fn with_empty() {
            let input = "";

            assert_eq!(quad_split_ports(input), vec![""],);
        }

        #[test]
        fn with_only_port() {
            let input = "123";

            assert_eq!(quad_split_ports(input), vec!["123"],);
        }

        #[test]
        fn with_ipv4_and_port() {
            let input = "1.2.3.4:567";

            assert_eq!(quad_split_ports(input), vec!["1.2.3.4", "567"],);
        }

        #[test]
        fn with_ipv6_and_port() {
            let input = "[::]:567";

            assert_eq!(quad_split_ports(input), vec!["[::]", "567"],);
        }

        #[test]
        fn with_host_and_container_ports() {
            let input = "123:567";

            assert_eq!(quad_split_ports(input), vec!["123", "567"],);
        }

        #[test]
        fn with_ipv4_host_and_container_ports() {
            let input = "0.0.0.0:123:567";

            assert_eq!(quad_split_ports(input), vec!["0.0.0.0", "123", "567"],);
        }

        #[test]
        fn with_ipv6_empty_host_container_port_and_protocol() {
            let input = "[1:2:3:4::]::567/tcp";

            assert_eq!(quad_split_ports(input), vec!["[1:2:3:4::]", "", "567/tcp"],);
        }
    }

    mod is_unambiguous_name {
        use super::*;

        #[test]
        fn with_ambiguous_names() {
            let inputs = vec![
                "fedora",
                "fedora:latest",
                "library/fedora",
                "library/fedora:latest",
                "busybox@sha256:d366a4665ab44f0648d7a00ae3fae139d55e32f9712c67accd604bb55df9d05a",
                "busybox:latest@sha256:d366a4665ab44f0648d7a00ae3fae139d55e32f9712c67accd604bb55df9d05a",
                "d366a4665ab44f0648d7a00ae3fae139d55e32f9712c67accd604bb55df9d05",
                "d366a4665ab44f0648d7a00ae3fae139d55e32f9712c67accd604bb55df9d05aa",
            ];

            for input in inputs {
                assert!(!is_unambiguous_name(input), "{input}");
            }
        }

        #[test]
        fn with_unambiguous_names() {
            let inputs = vec![
                "quay.io/fedora",
                "docker.io/fedora",
                "docker.io/library/fedora:latest",
                "localhost/fedora",
                "localhost:5000/fedora:latest",
                "example.foo.this.may.be.garbage.but.maybe.not:1234/fedora:latest",
                "docker.io/library/busybox@sha256:d366a4665ab44f0648d7a00ae3fae139d55e32f9712c67accd604bb55df9d05a",
                "docker.io/library/busybox:latest@sha256:d366a4665ab44f0648d7a00ae3fae139d55e32f9712c67accd604bb55df9d05a",
                "docker.io/fedora@sha256:d366a4665ab44f0648d7a00ae3fae139d55e32f9712c67accd604bb55df9d05a",
                "sha256:d366a4665ab44f0648d7a00ae3fae139d55e32f9712c67accd604bb55df9d05a",
                "d366a4665ab44f0648d7a00ae3fae139d55e32f9712c67accd604bb55df9d05a",
            ];

            for input in inputs {
                assert!(is_unambiguous_name(input), "{input}");
            }
        }
    }
}
