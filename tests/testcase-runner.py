#!/usr/bin/env python3

import csv
import os
import re
import shlex
import subprocess
import sys
import tempfile
import unittest

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

def to_service(filename):
    (base, ext) = os.path.splitext(filename)
    if ext == ".image":
        base = base + "-image"
    elif ext == ".network":
        base = base + "-network"
    elif ext == ".volume":
        base = base + "-volume"
    return base + ".service"

def read_file(dir, filename):
    with open(os.path.join(dir, filename), "r") as f:
        return f.read()

def write_file(indir, filename, data):
    os.makedirs(os.path.dirname(os.path.join(indir, filename)), exist_ok=True)
    with open(os.path.join(indir, filename), "w") as f:
        f.write(data)

def get_checks_from_data(data):
    return list(
        filter(lambda line: len(line) > 0,
               map(lambda line: shlex.split(line[2:]),
                   filter(lambda line: line.startswith ("##"),
                          data.split("\n")))))

def find_check(checks, checkname):
    for check in checks:
        if check[0] == checkname:
            return check
    return None

class QuadletTestCase(unittest.TestCase):
    def __init__(self, filename):
        super().__init__()
        self._testMethodDoc = filename
        self.filename = filename
        self.servicename = to_service(os.path.basename(filename))
        self.data = read_file(testcases_dir, filename)
        self.checks = get_checks_from_data(self.data)
        self.expect_fail = find_check(self.checks, "assert-failed") is not None
        self.outdata = ""
        self.unit = {}
        self.expected_files = set()

    def lookup(self, group, key):
        return self.sections.get(group, {}).get(key, None)

    def expect_file(self, path):
        self.expected_files.add(path)
        path = os.path.dirname(path)
        while path:
            self.expected_files.add(path + "/")
            path = os.path.dirname(path)

    def listfiles(self, outdir):
        res = list()
        for root, subdirs, files in os.walk(outdir):
            prefix = os.path.relpath(root, outdir)
            if prefix != ".":
                res.append(prefix + "/")
            for f in files:
                if prefix == ".":
                    res.append(f)
                else:
                    res.append(os.path.join(prefix, f))
        return res

    def check(self, outdir):
        def assert_failed(args, testcase):
            return True # We already handled this specially in runTest() and check()

        def assert_stderr_contains(args, testcase):
            # We've combined STDOUT and STDERR when running the test
            return args[0] in testcase.stdout

        def assert_key_is(args, testcase):
            if len(args) < 3:
                return False
            group = args[0]
            key = args[1]
            values = args[2:]

            real_values = testcase.lookup(group, key)
            return real_values == values

        def assert_key_is_regex(args, testcase):
            if len(args) < 3:
                return False
            group = args[0]
            key = args[1]
            values = args[2:]

            real_values = testcase.lookup(group, key)
            if len(real_values) != len(values):
                return False

            for (needle, haystack) in zip(values, real_values):
                if re.search(needle, haystack) is None:
                    return False

            return True

        def assert_key_contains(args, testcase):
            if len(args) != 3:
                return False
            group = args[0]
            key = args[1]
            value = args[2]

            real_values = testcase.lookup(group, key)
            last_value = real_values[-1]
            return value in last_value

        def assert_podman_args(args, testcase, key, allow_regex, global_only):
            podman_args = getattr(testcase, key)
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

        def key_value_string_to_map(key_value_string, separator):
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

        def _key_val_map_equal_regex(expected_key_val_map, actual_key_val_map):
            if len(expected_key_val_map) != len(actual_key_val_map):
                return False
            for key, expected_value in expected_key_val_map.items():
                if key not in actual_key_val_map:
                    return False
                actual_value = actual_key_val_map[key]
                if re.search(expected_value, actual_value) is None:
                    return False
            return True

        def assert_podman_args_key_val(args, testcase, key, allow_regex, global_only):
            if len(args) != 3:
                return False
            opt = args[0]
            separator = args[1]
            values = args[2]
            podman_args = getattr(testcase, key)

            if global_only:
                podman_cmd_location = find_sublist(podman_args, [args[0]])
                if podman_cmd_location == -1:
                    return False

                podman_args = podman_args[:podman_cmd_location]
                args = args[1:]

            expected_key_val_map = key_value_string_to_map(values, separator)
            arg_key_location = 0
            while True:
                sub_list_location = find_sublist(podman_args[arg_key_location:], [opt])
                if sub_list_location == -1:
                    break

                arg_key_location += sub_list_location
                actual_key_val_map = key_value_string_to_map(podman_args[arg_key_location+1], separator)
                if allow_regex:
                    if _key_val_map_equal_regex(expected_key_val_map, actual_key_val_map):
                        return True
                elif expected_key_val_map == actual_key_val_map:
                    return True

                arg_key_location += 2

                if arg_key_location > len(podman_args):
                    break

            return False

        def assert_podman_final_args(args, testcase, key):
            if len(getattr(testcase, key)) < len(args):
                return False
            return match_sublist_at(getattr(testcase, key), len(getattr(testcase, key)) - len(args), args)

        def assert_podman_final_args_regex(args, testcase, key):
            if len(getattr(testcase, key)) < len(args):
                return False
            return match_sublist_regex_at(getattr(testcase, key), len(getattr(testcase, key)) - len(args), args)

        def assert_start_podman_args(*args):
            return assert_podman_args(*args, '_Service_ExecStart', False, False)

        def assert_start_podman_args_regex(*args):
            return assert_podman_args(*args, '_Service_ExecStart', True, False)

        def assert_start_podman_global_args(*args):
            return assert_podman_args(*args, '_Service_ExecStart', False, True)

        def assert_start_podman_global_args_regex(*args):
            return assert_podman_args(*args, '_Service_ExecStart', True, True)

        def assert_start_podman_args_key_val(*args):
            return assert_podman_args_key_val(*args, '_Service_ExecStart', False, False)

        def assert_start_podman_args_key_val_regex(*args):
            return assert_podman_args_key_val(*args, '_Service_ExecStart', True, False)

        def assert_start_podman_global_args_key_val(*args):
            return assert_podman_args_key_val(*args, '_Service_ExecStart', False, True)

        def assert_start_podman_global_args_key_val_regex(*args):
            return assert_podman_args_key_val(*args, '_Service_ExecStart', True, True)

        def assert_start_podman_final_args(*args):
            return assert_podman_final_args(*args, '_Service_ExecStart')

        def assert_start_podman_final_args_regex(*args):
            return assert_podman_final_args_regex(*args, '_Service_ExecStart')

        def assert_stop_podman_args(*args):
            return assert_podman_args(*args, '_Service_ExecStop', False, False)

        def assert_stop_podman_global_args(*args):
            return assert_podman_args(*args, '_Service_ExecStop', False, True)

        def assert_stop_podman_final_args(*args):
            return assert_podman_final_args(*args, '_Service_ExecStop')

        def assert_stop_podman_final_args_regex(*args):
            return assert_podman_final_args_regex(*args, '_Service_ExecStop')

        def assert_stop_podman_args_key_val(*args):
            return assert_podman_args_key_val(*args, '_Service_ExecStop', False, False)

        def assert_stop_podman_args_key_val_regex(*args):
            return assert_podman_args_key_val(*args, '_Service_ExecStop', True, False)

        def assert_stop_post_podman_args(*args):
            return assert_podman_args(*args, '_Service_ExecStopPost', False, False)

        def assert_stop_post_podman_global_args(*args):
            return assert_podman_args(*args, '_Service_ExecStopPost', False, True)

        def assert_stop_post_podman_final_args(*args):
            return assert_podman_final_args(*args, '_Service_ExecStopPost')

        def assert_stop_post_podman_final_args_regex(*args):
            return assert_podman_final_args_regex(*args, '_Service_ExecStopPost')

        def assert_stop_post_podman_args_key_val(*args):
            return assert_podman_args_key_val(*args, '_Service_ExecStopPost', False, False)

        def assert_stop_post_podman_args_key_val_regex(*args):
            return assert_podman_args_key_val(*args, '_Service_ExecStopPost', True, False)

        def assert_symlink(args, testcase):
            if len(args) != 2:
                return False
            symlink = args[0]
            expected_target = args[1]

            testcase.expect_file(symlink)

            p = os.path.join (outdir, symlink)
            if not os.path.islink(p):
                return False

            target = os.readlink(p)
            return target == expected_target


        ops = {
            "assert-failed": assert_failed,
            "assert-stderr-contains": assert_stderr_contains,
            "assert-key-is": assert_key_is,
            "assert-key-is-regex": assert_key_is_regex,
            "assert-key-contains": assert_key_contains,
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
            "assert-symlink": assert_symlink,
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
        }

        servicepath = os.path.join(outdir, self.servicename)
        if self.expect_fail:
            if os.path.isfile(servicepath):
                raise RuntimeError(self._err_msg("Unexpected success"))

        if not os.path.isfile(servicepath) and not self.expect_fail:
            raise FileNotFoundError(self._err_msg(f"Unexpected failure, can't find {servicepath}\n" + self.stdout))

        if not self.expect_fail:
            self.outdata = read_file(outdir, self.servicename)
            self.sections = parse_unitfile(canonicalize_unitfile(self.outdata))
            self._Service_ExecStart = shlex.split(self.sections.get("Service", {}).get("ExecStart", ["podman"])[0])
            self._Service_ExecStop = shlex.split(self.sections.get("Service", {}).get("ExecStop", ["podman"])[0])
            self._Service_ExecStopPost = shlex.split(self.sections.get("Service", {}).get("ExecStopPost", ["podman"])[0])
            self.expect_file(self.servicename)

        for check in self.checks:
            op = check[0]
            args = check[1:]
            invert = False
            if op[0] == '!':
                invert = True
                op = op[1:]
            if not op in ops:
                raise NameError(self._err_msg(f"unknown assertion {op}"))
            ok = ops[op](args, self)
            if invert:
                ok = not ok
            if not ok:
                raise AssertionError(self._err_msg(shlex.join(check)))

        files = self.listfiles(outdir)
        for f in self.expected_files:
            files.remove(f)
        if len(files) != 0:
            raise FileExistsError(self._err_msg(f"Unexpected files in output directory: " + str(files)))

    def runTest(self):
        res = None
        with tempfile.TemporaryDirectory(prefix="podman_test") as basedir:
            indir = os.path.join(basedir, "quadlet")
            os.mkdir(indir)
            outdir = os.path.join(basedir, "out")
            os.mkdir(outdir)
            write_file(indir, self.filename, self.data);
            cmd = [generator_bin, '--user', '--no-kmsg-log', '-v', outdir]

            env = {
                "QUADLET_UNIT_DIRS": indir
            }
            if os.getenv('PODMAN') is not None:
                env['PODMAN'] = os.getenv('PODMAN')
            res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env)

            # NOTE: STDOUT includes STDERR
            self.stdout = res.stdout.decode('utf8')

            # The generator should never fail, just log warnings
            if res.returncode != 0 and not self.expect_fail:
                raise RuntimeError(self._err_msg(f"Unexpected generator failure\n" + self.stdout))

            self.check(outdir)


    def _err_msg(self, msg):
        err_msg = msg
        if self.stdout:
            err_msg += f"\n--- STDOUT/ERR ---\n{self.stdout}"
        if self.outdata:
            err_msg += f"\n---------- contents of {self.servicename} ----------\n{self.outdata}"
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

def load_test_suite():
    if len(sys.argv) < 2:
        print("No dir arg given", file=sys.stderr)
        sys.exit(1)
    global testcases_dir
    testcases_dir = sys.argv[1]

    if len(sys.argv) < 3:
        print("No generator arg given", file=sys.stderr)
        sys.exit(1)
    global generator_bin
    generator_bin = sys.argv[2]

    test_suite = unittest.TestSuite()
    for (dirpath, _dirnames, filenames) in os.walk(testcases_dir):
        rel_dirpath = dirpath.removeprefix(testcases_dir).removeprefix('/')
        for name in filenames:
            if (name.endswith(".container") or
                name.endswith(".image") or
                name.endswith(".kube") or
                name.endswith(".network") or
                name.endswith(".pod") or
                name.endswith(".volume")) and not name.startswith("."):
                test_suite.addTest(QuadletTestCase(os.path.join(rel_dirpath, name)))

    return test_suite


if __name__ == '__main__':
    runner = unittest.TextTestRunner()
    runner.run(load_test_suite())
