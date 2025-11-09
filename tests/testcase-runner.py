#!/usr/bin/env python3

import csv
import itertools
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

def match_sublist_at(full_list, pos, sublist):
    if len(sublist) > len(full_list) - pos:
        return False

    for i in range(len(sublist)):
        if sublist[i] != full_list[pos+i]:
            return False
    return True

def match_sublist_regex_at(full_list, pos, sublist):
    if len(sublist) > len(full_list) - pos:
        return False

    for i in range(len(sublist)):
        if re.search(sublist[i], full_list[pos+i]) is None:
            return False
    return True

def find_sublist(full_list, sublist):
    if len(sublist) > len(full_list):
        return -1
    if len(sublist) == 0:
        return -1
    for i in range(len(full_list) - len(sublist) + 1):
        if match_sublist_at(full_list, i, sublist):
            return i
    return -1

def find_sublist_regex(full_list, sublist):
    if len(sublist) > len(full_list):
        return -1
    if len(sublist) == 0:
        return -1
    for i in range(len(full_list) - len(sublist) + 1):
        if match_sublist_regex_at(full_list, i, sublist):
            return i
    return -1

def to_servicefile_name(file_path: Path):
    base = Path(file_path.name).stem
    ext = Path(file_path.name).suffix
    sections = parse_unitfile(file_path.read_text())
    if ext == ".build":
        base = f"{base}-build"
        base = sections.get('Build', {}).get('ServiceName', [base])[-1]
    elif ext == ".container":
        base = base
        base = sections.get('Container', {}).get('ServiceName', [base])[-1]
    elif ext == ".image":
        base = f"{base}-image"
        base = sections.get('Image', {}).get('ServiceName', [base])[-1]
    elif ext == ".network":
        base = f"{base}-network"
        base = sections.get('Network', {}).get('ServiceName', [base])[-1]
    elif ext == ".pod":
        base = f"{base}-pod"
        base = sections.get('Pod', {}).get('ServiceName', [base])[-1]
    elif ext == ".volume":
        base = f"{base}-volume"
        base = sections.get('Volume', {}).get('ServiceName', [base])[-1]
    return f"{base}.service"

def get_generic_template_file(filename: Path):
    base = filename.stem
    ext = filename.suffix
    parts = base.split('@', 2)
    if len(parts) == 2 and len(parts[1]) > 0:
        return f"{parts[0]}@{ext}"
    return None

def find_check(checks, checkname):
    for check in checks:
        if check[0] == checkname:
            return check
    return None

class QuadletTestCase(unittest.TestCase):
    def __init__(self, filename: Path, run_rootless: bool):
        super().__init__()
        self._testMethodDoc = str(filename)
        self.filename = Path(filename)
        self.servicename = to_servicefile_name(testcases_dir.joinpath(filename))
        self.data = testcases_dir.joinpath(filename).read_text()
        self.unit = {}
        self.rootless = run_rootless

    def write_testfile_to(self, indir: Path):
        # Write the tested file to the quadlet dir
        indir.joinpath(self.filename).parent.mkdir(parents=True, exist_ok=True)
        indir.joinpath(self.filename).write_text(self.data)

        # Also copy any extra snippets
        snippetdirs = [f"{self.filename}.d"]
        generic_file_name = get_generic_template_file(self.filename)
        if generic_file_name is not None:
            snippetdirs.append(f"{generic_file_name}.d")

        for snippetdir in snippetdirs:
            dot_dir = testcases_dir.joinpath(snippetdir)
            if dot_dir.is_dir():
                dot_dir_dest = indir.joinpath(snippetdir)
                shutil.copytree(dot_dir, dot_dir_dest)

        # Also copy quadlet dependencies
        for dependency_file_name in self.get_dependency_data():
            dep_file_src = testcases_dir.joinpath(dependency_file_name)
            dep_file_dst = indir.joinpath(dependency_file_name)
            shutil.copyfile(dep_file_src, dep_file_dst)

    def get_dependency_data(self):
        return list(itertools.chain.from_iterable(
            filter(lambda line: len(line) > 0,
                map(lambda line: shlex.split(line.removeprefix("## depends-on ")),
                    filter(lambda line: line.startswith("## depends-on "), self.data.split("\n"))))))

    def runTest(self):
        res = None
        with tempfile.TemporaryDirectory(prefix="podman-e2e-") as basedir:
            basedir = Path(basedir)
            # match directory structure from podman-quadlet
            basedir = basedir.joinpath("subtest-0")
            basedir.mkdir()
            indir = basedir.joinpath("quadlet")
            indir.mkdir()
            outdir = basedir.joinpath("out")
            outdir.mkdir()
            self.write_testfile_to(indir);
            cmd = [generator_bin]
            if self.rootless:
                cmd.append('--user')
            cmd.extend(['--no-kmsg-log', '-v', outdir])

            env = {
                "QUADLET_UNIT_DIRS": indir
            }
            if os.getenv('PODMAN') is not None:
                env['PODMAN'] = os.getenv('PODMAN')
            res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env)

            Outcome(self, res, outdir).check()

class Outcome:
    def __init__(self, testcase: QuadletTestCase, process: subprocess.CompletedProcess, outdir: Path):
        self.testcase = testcase
        self.outdir = outdir
        self.checks = self.get_checks_from_data()
        self.expect_fail = find_check(self.checks, "assert-failed") is not None
        self.outdata = ""
        self.expected_files = set()

        # NOTE: STDOUT includes STDERR
        self.stdout = process.stdout.decode('utf8')

        # The generator should never fail, just log warnings
        if process.returncode != 0 and not self.expect_fail:
            raise RuntimeError(self._err_msg(f"Unexpected generator failure\n" + self.stdout))

        for dependency_file in self.testcase.get_dependency_data():
            self.add_expected_file(Path(to_servicefile_name(testcases_dir.joinpath(dependency_file))))

    def get_checks_from_data(self):
            return list(
                filter(lambda line: len(line) > 0,
                      map(lambda line: shlex.split(line.removeprefix("##")),
                          filter(lambda line: line.startswith("## assert-"),
                                  self.testcase.data.split("\n")))))

    def list_outdir_files(self):
        res = list()
        for root, subdirs, files in self.outdir.walk():
            prefix = root.relative_to(self.outdir)
            if prefix != Path("."):
                res.append(f"{prefix}/")
            for f in files:
                if prefix == Path("."):
                    res.append(str(f))
                else:
                    res.append(str(prefix.joinpath(f)))
        return res

    def lookup(self, group, key):
        return self.sections.get(group, {}).get(key, None)

    def add_expected_file(self, path: Path):
        self.expected_files.add(str(path))
        for path in path.parents:
            if path != Path('.'):
              self.expected_files.add(f"{path}/")

    def assert_failed(self, args):
        return True # We already handled this specially in runTest() and check()

    def assert_stderr_contains(self, args):
        # We've combined STDOUT and STDERR when running the test
        return args[0] in self.stdout

    def assert_has_key(self, args):
        if len(args) != 3:
            return False
        group = args[0]
        key = args[1]
        value = args[2]

        real_values = self.lookup(group, key)
        return value in real_values

    def assert_key_is(self, args):
        if len(args) < 3:
            return False
        group = args[0]
        key = args[1]
        values = args[2:]

        real_values = self.lookup(group, key)
        return real_values == values

    def assert_key_is_empty(self, args):
        if len(args) < 2:
            return False
        group = args[0]
        key = args[1]

        real_values = self.lookup(group, key)
        return real_values is None or len(real_values) == 0

    def assert_key_is_regex(self, args):
        if len(args) < 3:
            return False
        group = args[0]
        key = args[1]
        values = args[2:]

        real_values = self.lookup(group, key)
        if len(real_values) != len(values):
            return False

        for (needle, haystack) in zip(values, real_values):
            if re.search(needle, haystack) is None:
                return False

        return True

    def assert_last_key_is_regex(self, args):
        if len(args) != 3:
            return False
        group = args[0]
        key = args[1]
        value = args[2]

        real_values = self.lookup(group, key)
        last_value = real_values[-1]

        if re.search(value, last_value) is None:
            return False

        return True

    def assert_last_key_contains(self, args):
        if len(args) != 3:
            return False
        group = args[0]
        key = args[1]
        value = args[2]

        real_values = self.lookup(group, key)
        last_value = real_values[-1]
        return value in last_value

    def assert_podman_args(self, args, key, allow_regex, global_only):
        podman_args = getattr(self, key)
        if global_only:
            podman_cmd_location = find_sublist(podman_args, [args[0]])
            if podman_cmd_location == 1:
                return False
            podman_args = podman_args[:podman_cmd_location]
            args = args[1:]

        location = -1
        if allow_regex:
            location = find_sublist_regex(podman_args, args)
        else:
            location = find_sublist(podman_args, args)

        return location != -1

    def key_value_string_to_map(self, key_value_string, separator):
        key_val_map = dict()
        csv_reader = csv.reader(key_value_string, delimiter=separator)
        key_var_list = list(csv_reader)
        for param in key_var_list[0]:
            val = ""
            kv = param.split('=', maxsplit=2)
            if len(kv) == 2:
                val = kv[1]
            key_val_map[kv[0]] = val

        return key_val_map

    def _key_val_map_equal_regex(self, expected_key_val_map, actual_key_val_map):
        if len(expected_key_val_map) != len(actual_key_val_map):
            return False
        for key, expected_value in expected_key_val_map.items():
            if key not in actual_key_val_map:
                return False
            actual_value = actual_key_val_map[key]
            if re.search(expected_value, actual_value) is None:
                return False
        return True

    def assert_podman_args_key_val(self, args, key, allow_regex, global_only):
        if len(args) != 3:
            return False
        opt = args[0]
        separator = args[1]
        values = args[2]
        podman_args = getattr(self, key)

        if global_only:
            podman_cmd_location = find_sublist(podman_args, [args[0]])
            if podman_cmd_location == -1:
                return False

            podman_args = podman_args[:podman_cmd_location]
            args = args[1:]

        expected_key_val_map = self.key_value_string_to_map(values, separator)
        arg_key_location = 0
        while True:
            sub_list_location = find_sublist(podman_args[arg_key_location:], [opt])
            if sub_list_location == -1:
                break

            arg_key_location += sub_list_location
            actual_key_val_map = self.key_value_string_to_map(podman_args[arg_key_location+1], separator)
            if allow_regex:
                if self._key_val_map_equal_regex(expected_key_val_map, actual_key_val_map):
                    return True
            elif expected_key_val_map == actual_key_val_map:
                return True

            arg_key_location += 2

            if arg_key_location > len(podman_args):
                break

        return False

    def assert_podman_final_args(self, args, key):
        if len(getattr(self, key)) < len(args):
            return False
        return match_sublist_at(getattr(self, key), len(getattr(self, key)) - len(args), args)

    def assert_podman_final_args_regex(self, args, key):
        if len(getattr(self, key)) < len(args):
            return False
        return match_sublist_regex_at(getattr(self, key), len(getattr(self, key)) - len(args), args)

    def assert_reload_podman_args(self, *args):
        return self.assert_podman_args(*args, '_Service_ExecReload', False, False)

    def assert_reload_podman_global_args(self, *args):
        return self.assert_podman_args(*args, '_Service_ExecReload', False, True)

    def assert_reload_podman_final_args(self, *args):
        return self.assert_podman_final_args(*args, '_Service_ExecReload')

    def assert_reload_podman_final_args_regex(self, *args):
        return self.assert_podman_final_args_regex(*args, '_Service_ExecReload')

    def assert_reload_podman_args_key_val(self, *args):
        return self.assert_podman_args_key_val(*args, '_Service_ExecReload', False, False)

    def assert_reload_podman_args_key_val_regex(self, *args):
        return self.assert_podman_args_key_val(*args, '_Service_ExecReload', True, False)

    def assert_start_podman_args(self, *args):
        return self.assert_podman_args(*args, '_Service_ExecStart', False, False)

    def assert_start_podman_args_regex(self, *args):
        return self.assert_podman_args(*args, '_Service_ExecStart', True, False)

    def assert_start_podman_global_args(self, *args):
        return self.assert_podman_args(*args, '_Service_ExecStart', False, True)

    def assert_start_podman_global_args_regex(self, *args):
        return self.assert_podman_args(*args, '_Service_ExecStart', True, True)

    def assert_start_podman_args_key_val(self, *args):
        return self.assert_podman_args_key_val(*args, '_Service_ExecStart', False, False)

    def assert_start_podman_args_key_val_regex(self, *args):
        return self.assert_podman_args_key_val(*args, '_Service_ExecStart', True, False)

    def assert_start_podman_global_args_key_val(self, *args):
        return self.assert_podman_args_key_val(*args, '_Service_ExecStart', False, True)

    def assert_start_podman_global_args_key_val_regex(self, *args):
        return self.assert_podman_args_key_val(*args, '_Service_ExecStart', True, True)

    def assert_start_podman_final_args(self, *args):
        return self.assert_podman_final_args(*args, '_Service_ExecStart')

    def assert_start_podman_final_args_regex(self, *args):
        return self.assert_podman_final_args_regex(*args, '_Service_ExecStart')

    def assert_start_pre_podman_args(self, *args):
        return self.assert_podman_args(*args, '_Service_ExecStartPre', False, False)

    def assert_start_pre_podman_args_regex(self, *args):
        return self.assert_podman_args(*args, '_Service_ExecStartPre', True, False)

    def assert_start_pre_podman_global_args(self, *args):
        return self.assert_podman_args(*args, '_Service_ExecStartPre', False, True)

    def assert_start_pre_podman_global_args_regex(self, *args):
        return self.assert_podman_args(*args, '_Service_ExecStartPre', True, True)

    def assert_start_pre_podman_args_key_val(self, *args):
        return self.assert_podman_args_key_val(*args, '_Service_ExecStartPre', False, False)

    def assert_start_pre_podman_args_key_val_regex(self, *args):
        return self.assert_podman_args_key_val(*args, '_Service_ExecStartPre', True, False)

    def assert_start_pre_podman_global_args_key_val(self, *args):
        return self.assert_podman_args_key_val(*args, '_Service_ExecStartPre', False, True)

    def assert_start_pre_podman_global_args_key_val_regex(self, *args):
        return self.assert_podman_args_key_val(*args, '_Service_ExecStartPre', True, True)

    def assert_start_pre_podman_final_args(self, *args):
        return self.assert_podman_final_args(*args, '_Service_ExecStartPre')

    def assert_start_pre_podman_final_args_regex(self, *args):
        return self.assert_podman_final_args_regex(*args, '_Service_ExecStartPre')

    def assert_stop_podman_args(self, *args):
        return self.assert_podman_args(*args, '_Service_ExecStop', False, False)

    def assert_stop_podman_global_args(self, *args):
        return self.assert_podman_args(*args, '_Service_ExecStop', False, True)

    def assert_stop_podman_final_args(self, *args):
        return self.assert_podman_final_args(*args, '_Service_ExecStop')

    def assert_stop_podman_final_args_regex(self, *args):
        return self.assert_podman_final_args_regex(*args, '_Service_ExecStop')

    def assert_stop_podman_args_key_val(self, *args):
        return self.assert_podman_args_key_val(*args, '_Service_ExecStop', False, False)

    def assert_stop_podman_args_key_val_regex(self, *args):
        return self.assert_podman_args_key_val(*args, '_Service_ExecStop', True, False)

    def assert_stop_post_podman_args(self, *args):
        return self.assert_podman_args(*args, '_Service_ExecStopPost', False, False)

    def assert_stop_post_podman_global_args(self, *args):
        return self.assert_podman_args(*args, '_Service_ExecStopPost', False, True)

    def assert_stop_post_podman_final_args(self, *args):
        return self.assert_podman_final_args(*args, '_Service_ExecStopPost')

    def assert_stop_post_podman_final_args_regex(self, *args):
        return self.assert_podman_final_args_regex(*args, '_Service_ExecStopPost')

    def assert_stop_post_podman_args_key_val(self, *args):
        return self.assert_podman_args_key_val(*args, '_Service_ExecStopPost', False, False)

    def assert_stop_post_podman_args_key_val_regex(self, *args):
        return self.assert_podman_args_key_val(*args, '_Service_ExecStopPost', True, False)

    def assert_symlink(self, args):
        if len(args) != 2:
            return False
        symlink = Path(args[0])
        expected_target = Path(args[1])

        self.add_expected_file(symlink)

        p = self.outdir.joinpath(symlink)
        if not p.is_symlink():
            return False

        target = p.readlink()
        return target == expected_target

    ops = {
        "assert-failed": assert_failed,
        "assert-stderr-contains": assert_stderr_contains,
        "assert-has-key": assert_has_key,
        "assert-key-is": assert_key_is,
        "assert-key-is-empty": assert_key_is_empty,
        "assert-key-is-regex": assert_key_is_regex,
        "assert-last-key-contains": assert_last_key_contains,
        "assert-last-key-is-regex": assert_last_key_is_regex,
        "assert-podman-args": assert_start_podman_args,
        "assert-podman-args-regex": assert_start_podman_args_regex,
        "assert-podman-args-key-val": assert_start_podman_args_key_val,
        "assert-podman-args-key-val-regex": assert_start_podman_args_key_val_regex,
        "assert-podman-global-args": assert_start_podman_global_args,
        "assert-podman-global-args-regex": assert_start_podman_global_args_regex,
        "assert-podman-global-args-key-val": assert_start_podman_global_args_key_val,
        "assert-podman-global-args-key-val-regex": assert_start_podman_global_args_key_val_regex,
        "assert-podman-final-args": assert_start_podman_final_args,
        "assert-podman-final-args-regex": assert_start_podman_final_args_regex,
        "assert-podman-pre-args": assert_start_pre_podman_args,
        "assert-podman-pre-args-regex": assert_start_pre_podman_args_regex,
        "assert-podman-pre-args-key-val": assert_start_pre_podman_args_key_val,
        "assert-podman-pre-args-key-val-regex": assert_start_pre_podman_args_key_val_regex,
        "assert-podman-pre-global-args": assert_start_pre_podman_global_args,
        "assert-podman-pre-global-args-regex": assert_start_pre_podman_global_args_regex,
        "assert-podman-pre-global-args-key-val": assert_start_pre_podman_global_args_key_val,
        "assert-podman-pre-global-args-key-val-regex": assert_start_pre_podman_global_args_key_val_regex,
        "assert-podman-pre-final-args": assert_start_pre_podman_final_args,
        "assert-podman-pre-final-args-regex": assert_start_pre_podman_final_args_regex,
        "assert-podman-reload-args": assert_reload_podman_args,
        "assert-podman-reload-global-args": assert_reload_podman_global_args,
        "assert-podman-reload-final-args": assert_reload_podman_final_args,
        "assert-podman-reload-final-args-regex": assert_reload_podman_final_args_regex,
        "assert-podman-reload-args-key-val": assert_reload_podman_args_key_val,
        "assert-podman-reload-args-key-val-regex": assert_reload_podman_args_key_val_regex,
        "assert-podman-stop-args": assert_stop_podman_args,
        "assert-podman-stop-global-args": assert_stop_podman_global_args,
        "assert-podman-stop-final-args": assert_stop_podman_final_args,
        "assert-podman-stop-final-args-regex": assert_stop_podman_final_args_regex,
        "assert-podman-stop-args-key-val": assert_stop_podman_args_key_val,
        "assert-podman-stop-args-key-val-regex": assert_stop_podman_args_key_val_regex,
        "assert-podman-stop-post-args": assert_stop_post_podman_args,
        "assert-podman-stop-post-global-args": assert_stop_post_podman_global_args,
        "assert-podman-stop-post-final-args": assert_stop_post_podman_final_args,
        "assert-podman-stop-post-final-args-regex": assert_stop_post_podman_final_args_regex,
        "assert-podman-stop-post-args-key-val": assert_stop_post_podman_args_key_val,
        "assert-podman-stop-post-args-key-val-regex": assert_stop_post_podman_args_key_val_regex,
        "assert-symlink": assert_symlink,
    }

    def check(self):
        outdir = self.outdir
        servicepath = outdir.joinpath(self.testcase.servicename)
        if self.expect_fail:
            if servicepath.is_file():
                raise RuntimeError(self._err_msg(f"Unexpected success, found {servicepath}"))

        if not servicepath.is_file():
          # maybe it has another name ...
          try:
              # look for any .service file
              servicepath = next(outdir.glob("*.service"))
              self.testcase.servicename = servicepath.name
              # but make sure there's only one'
              assert len(list(outdir.glob("*.service"))) == 1
          except StopIteration:
              # no .service files found at all
              if not self.expect_fail:
                  raise FileNotFoundError(self._err_msg(f"Unexpected failure, can't find {servicepath}\n" + self.stdout))

        if not self.expect_fail:
            self.outdata = outdir.joinpath(self.testcase.servicename).read_text()
            self.sections = parse_unitfile(canonicalize_unitfile(self.outdata))
            self._Service_ExecReload = shlex.split(self.sections.get("Service", {}).get("ExecReload", ["podman"])[0])
            self._Service_ExecStart = shlex.split(self.sections.get("Service", {}).get("ExecStart", ["podman"])[0])
            self._Service_ExecStartPre = shlex.split(self.sections.get("Service", {}).get("ExecStartPre", ["podman"])[0])
            self._Service_ExecStop = shlex.split(self.sections.get("Service", {}).get("ExecStop", ["podman"])[0])
            self._Service_ExecStopPost = shlex.split(self.sections.get("Service", {}).get("ExecStopPost", ["podman"])[0])
            self.add_expected_file(Path(self.testcase.servicename))

        for check in self.checks:
            op = check[0]
            args = check[1:]
            invert = False
            if op[0] == '!':
                invert = True
                op = op[1:]
            if not op in self.ops:
                raise NameError(self._err_msg(f"unknown assertion {op}"))
            ok = self.ops[op](self, args)
            if invert:
                ok = not ok
            if not ok:
                raise AssertionError(self._err_msg(shlex.join(check)))

        files = self.list_outdir_files()
        for f in self.expected_files:
            if f not in files:
                raise FileExistsError(self._err_msg(f"Expected file not found in output directory: {f}"))
            files.remove(f)
        if len(files) != 0:
            raise FileExistsError(self._err_msg(f"Unexpected files in output directory: {str(files)}"))

    def _err_msg(self, msg):
        err_msg = msg
        if self.stdout:
            err_msg += f"\n--- STDOUT/ERR ---\n{self.stdout}"
        if self.outdata:
            err_msg += f"\n---------- contents of {self.testcase.servicename} ----------\n{self.outdata}"
        return err_msg

# Removes comments and merges lines
def canonicalize_unitfile(data):
    r = ""
    for line in data.split("\n"):
        if line.startswith("#") or line.startswith(";"):
            continue
        if line.endswith("\\"):
            r += line[:-1] + " "
        else:
            r += line + "\n"
    return r

# This is kinda lame, but should handle all the tests
def parse_unitfile(data):
    sections = { }
    section = "none"
    for line in data.split("\n"):
        if line.startswith("["):
            section = line[1:line.index("]")]
        parts = line.split("=", 1)
        if len(parts) == 2:
            key = parts[0].strip()
            val = parts[1].strip()
            if not section in sections:
                sections[section] = {}
            if not key in sections[section]:
                sections[section][key] = []
            sections[section][key].append(val)
    return sections

def load_test_suite(run_rootless: bool):
    if len(sys.argv) < 2:
        print("No dir arg given", file=sys.stderr)
        sys.exit(1)
    global testcases_dir
    testcases_dir = Path(sys.argv[1])

    if len(sys.argv) < 3:
        print("No generator arg given", file=sys.stderr)
        sys.exit(1)
    global generator_bin
    generator_bin = Path(sys.argv[2])

    test_suite = unittest.TestSuite()
    for (dirpath, _dirnames, filenames) in testcases_dir.walk():
        rel_dirpath = dirpath.relative_to(testcases_dir)
        for name in filenames:
            if (name.endswith(".build") or
                name.endswith(".container") or
                name.endswith(".image") or
                name.endswith(".kube") or
                name.endswith(".network") or
                name.endswith(".pod") or
                name.endswith(".volume")) and not name.startswith("."):
                test_suite.addTest(QuadletTestCase(rel_dirpath.joinpath(name), run_rootless))

    return test_suite


if __name__ == '__main__':
    runner = unittest.TextTestRunner()
    print(f"\n--- rootful test suite ---")
    runner.run(load_test_suite(False))
    print(f"\n--- rootless test suite ---")
    runner.run(load_test_suite(True))
