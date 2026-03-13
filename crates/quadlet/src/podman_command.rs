use std::collections::HashMap;

use systemd_unit::quote_words;

use super::get_podman_binary;

pub struct PodmanCommand {
    pub args: Vec<String>,
}

impl PodmanCommand {
    pub fn add<S>(&mut self, arg: S)
    where
        S: Into<String>,
    {
        self.args.push(arg.into());
    }

    pub fn add_bool<S>(&mut self, arg: S, val: bool)
    where
        S: Into<String>,
    {
        if val {
            self.add(arg);
        } else {
            self.add(format!("{}=false", arg.into()));
        }
    }

    pub fn add_keys(&mut self, prefix: &str, env: &HashMap<String, Option<String>>) {
        for (key, value) in env {
            self.add(prefix);
            if let Some(value) = value {
                self.add(format!("{key}={value}"));
            } else {
                self.add(key);
            }
        }
    }

    pub fn add_slice(&mut self, args: &[&str]) {
        self.args.reserve(args.len());
        for arg in args {
            self.args.push(arg.to_string())
        }
    }

    pub fn extend<I>(&mut self, args: I)
    where
        I: IntoIterator<Item = String>,
    {
        self.args.extend(args);
    }

    pub fn new() -> Self {
        let mut v = Vec::with_capacity(10);
        v.push(get_podman_binary());

        PodmanCommand { args: v }
    }

    pub fn to_escaped_string(&self) -> String {
        quote_words(self.args.iter().map(|s| s.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod podman_command {
        use super::*;

        mod to_escaped_string {
            use super::*;
            use systemd_unit::EntryValue;

            #[test]
            fn with_word_splitting_and_setting_constructed_command() {
                let input = "/some/path \"an arg\" \"a;b\\nc\\td\'e\" a;b\\nc\\td \'a\"b\'";
                let exec_start = EntryValue::from_raw(input);

                let mut command = PodmanCommand::new();
                command.add("test");
                command.extend(exec_start.split_words().into_iter());

                let new_exec_start = command.to_escaped_string();
                assert_eq!(
                    new_exec_start,
                    "/usr/bin/podman test /some/path \"an arg\" \"a;b\\nc\\td\'e\" \"a;b\\nc\\td\" \"a\\\"b\""
                );
            }
        }
    }
}
