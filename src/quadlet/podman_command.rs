use std::collections::HashMap;

use crate::systemd_unit::quote_words;

use super::get_podman_binary;

pub(crate) struct PodmanCommand {
    pub(crate) args: Vec<String>,
}

impl PodmanCommand {
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

    pub(crate) fn extend<I>(&mut self, args: I)
    where
        I: IntoIterator<Item = String>,
    {
        self.args.extend(args);
    }

    pub(crate) fn new() -> Self {
        let mut v = Vec::with_capacity(10);
        v.push(get_podman_binary());

        PodmanCommand { args: v }
    }

    pub(crate) fn to_escaped_string(&self) -> String {
        quote_words(self.args.iter().map(|s| s.as_str()))
    }
}
