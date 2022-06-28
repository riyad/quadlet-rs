mod podman_command;
mod ranges;

pub(crate) use self::podman_command::*;
pub(crate) use self::ranges::*;

use once_cell::unsync::Lazy;
use regex::Regex;

pub(crate) fn is_port_range(port: &str) -> bool {
    const RE: Lazy<Regex> = Lazy::new(|| Regex::new("\\d+(-\\d+)?(/udp|/tcp)?$").unwrap());
    RE.is_match(port)
}
