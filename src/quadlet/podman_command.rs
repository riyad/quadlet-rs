
pub(crate) struct PodmanCommand {
    args: Vec<String>,
}

impl PodmanCommand {
    fn _new() -> Self {
        PodmanCommand {
            args: Vec::with_capacity(10),
        }
    }

    pub(crate) fn add<S>(&mut self, arg: S) where S: Into<String> {
        self.args.push(arg.into());
    }

    pub(crate) fn add_slice(&mut self, args: &[&str])
    {
        self.args.reserve(args.len());
        for arg in args {
            self.args.push(arg.to_string())
        }
    }


    pub(crate) fn add_vec(&mut self, args: &mut Vec<String>)
    {
        self.args.append(args);
    }

    pub(crate) fn new_command(command: &str) -> Self {
        let mut podman = Self::_new();

        podman.add("/usr/bin/podman");
        podman.add(command);

        podman
    }

    pub(crate) fn to_escaped_string(&mut self) -> String {
        shlex::join(self.args.iter().map(String::as_str))
    }
}