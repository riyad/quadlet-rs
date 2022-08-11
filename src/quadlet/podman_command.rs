use std::collections::HashMap;

use super::ranges::IdRanges;
use crate::systemd_unit::quote_words;

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

    pub(crate) fn add_annotations(&mut self, annotations: &HashMap<String, String>) {
        self.add_keys("--annotation", annotations);
    }

    pub(crate) fn add_env(&mut self, env: &HashMap<String, String>) {
        self.add_keys("--env", env);
    }

    pub(crate) fn add_id_map(&mut self,
                              arg_prefix: &str,
                              container_id_start: u32,
                              host_id_start: u32,
                              num_ids: u32)
    {
        if num_ids != 0 {
            self.add(arg_prefix);
            self.add(format!("{container_id_start}:{host_id_start}:{num_ids}"));
        }
    }

    pub(crate) fn add_id_maps(&mut self,
                              arg_prefix: &str,
                              container_id: u32,
                              host_id: u32,
                              remap_start_id: u32,
                              available_host_ids: Option<IdRanges>)
    {
        let mut unmapped_ids: IdRanges;
        let mut mapped_ids: IdRanges;

        let mut available_host_ids = match available_host_ids {
            None => IdRanges::empty(),  // Map everything by default
            Some(v) => v,
        };

        // Map the first ids up to remap_start_id to the host equivalent
        unmapped_ids = IdRanges::new(0, remap_start_id);

        // The rest we want to map to available_host_ids. Note that this
        // overlaps unmapped_ids, because below we may remove ranges from
        // unmapped ids and we want to backfill those.
        mapped_ids = IdRanges::new(0, u32::MAX);

        // Always map specified uid to specified host_uid
        self.add_id_map(arg_prefix, container_id, host_id, 1);

        // We no longer want to map this container id as it's already mapped
        mapped_ids.remove(container_id, 1);
        unmapped_ids.remove(container_id, 1);

        // But also, we don't want to use the *host* id again, as we can only map it once
        unmapped_ids.remove(host_id, 1);
        available_host_ids.remove(host_id, 1);

        // Map unmapped ids to equivalent host range, and remove from mapped_ids to avoid double-mapping
        // FIXME: implement FromIterator
        for range in unmapped_ids.iter() {
            self.add_id_map(arg_prefix, range.start(), range.start(), range.length());
            mapped_ids.remove(range.start(), range.length());
            available_host_ids.remove(range.start(), range.length());
        }

        // Go through the rest of mapped_ids and map ids overlapping with available_host_id
        // FIXME: implement FromIterator
        for c_range in mapped_ids.iter() {
            let mut c_start = c_range.start();
            let mut c_length = c_range.length();
            while c_length > 0 {
                let h_range = available_host_ids.iter().next();
                if let Some(h_range) = h_range {
                    let h_start = h_range.start();
                    let h_length = h_range.length();

                    let next_length = h_length.min(c_length);

                    self.add_id_map(arg_prefix, c_start, h_start, next_length);
                    available_host_ids.remove(h_start, next_length);
                    c_start += next_length;
                    c_length -= next_length;
                } else {
                    break
                }
            }
        }
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
        quote_words(self.args.iter().map(|s| s.as_str()))
    }
}