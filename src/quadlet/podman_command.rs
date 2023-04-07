use std::collections::HashMap;

use crate::systemd_unit::quote_words;

use super::PODMAN_BINARY;

pub(crate) struct PodmanCommand {
    args: Vec<String>,
}

impl PodmanCommand {
    fn _new() -> Self {
        PodmanCommand {
            args: Vec::with_capacity(10),
        }
    }

    pub(crate) fn add<S>(&mut self, arg: S)
    where
        S: Into<String>,
    {
        self.args.push(arg.into());
    }

    pub(crate) fn add_annotations(&mut self, annotations: &HashMap<String, String>) {
        self.add_keys("--annotation", annotations);
    }

    pub(crate) fn add_bool<S>(&mut self, arg: S, val: bool)
    where
        S: Into<String>,
    {
        if val {
            self.add(arg);
        } else {
            self.add(format!("{}=false", arg.into()));
        }
    }

    pub(crate) fn add_env(&mut self, env: &HashMap<String, String>) {
        self.add_keys("--env", env);
    }

    pub(crate) fn add_keys(&mut self, prefix: &str, env: &HashMap<String, String>) {
        for (key, value) in env {
            self.add(prefix);
            self.add(format!("{key}={value}"));
        }
    }

    pub(crate) fn add_labels(&mut self, labels: &HashMap<String, String>) {
        self.add_keys("--label", labels);
    }

    pub(crate) fn add_slice(&mut self, args: &[&str]) {
        self.args.reserve(args.len());
        for arg in args {
            self.args.push(arg.to_string())
        }
    }

    pub(crate) fn add_vec(&mut self, args: &mut Vec<String>) {
        self.args.append(args);
    }

    pub(crate) fn new_command(command: &str) -> Self {
        let mut podman = Self::_new();

        podman.add(&*PODMAN_BINARY);
        podman.add(command);

        podman
    }

    pub(crate) fn to_escaped_string(&self) -> String {
        quote_words(self.args.iter().map(|s| s.as_str()))
    }
}
