#!/usr/bin/python3

import sys
import os
import re
import tempfile
import subprocess
import shlex
import unittest

def match_sublist_at(full_list, pos, sublist):
    if len(sublist) > len(full_list) - pos:
        return False

    for i in range(0, len(sublist)):
        if sublist[i] != full_list[pos+i]:
            return False
    return True

def find_sublist(full_list, sublist):
    if len(sublist) > len(full_list):
        return -1
    if len(sublist) == 0:
        return -1;
    for i in range(0, len(full_list) - len(sublist) + 1):
        if match_sublist_at(full_list, i, sublist):
            return i;
    return -1

def to_service(filename):
    (base, ext) = os.path.splitext(filename)
    if ext == ".network":
        base = base + "-network"
    elif ext == ".volume":
        base = base + "-volume"
    return base + ".service"

def read_file(dir, filename):
    data=""
    with open(os.path.join(dir, filename), "r") as f:
        data = f.read()
    return data

def write_file(indir, filename, data):
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
        self.servicename = to_service(filename)
        self.data = read_file(testcases_dir, filename)
        self.checks = get_checks_from_data(self.data)
        self.expect_fail = False
        if find_check(self.checks, "assert-failed"):
            self.expect_fail = True
        self.outdata = ""
        self.unit = {}
        self.podman_args = []
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
            return True # We already handled this specially after running

        def assert_stderr_contains(args, testcase):
            return args[0] in testcase.stdout

        def assert_podman_args(args, testcase):
            return find_sublist(testcase.podman_args, args) != -1

        def assert_podman_final_args(args, testcase):
            return match_sublist_at(testcase.podman_args, len(testcase.podman_args) - len(args), args)

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
            last_value = real_values[len(real_values)-1]
            return value in last_value

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
            "assert-podman-args": assert_podman_args,
            "assert-podman-final-args": assert_podman_final_args,
            "assert-symlink": assert_symlink,
        }

        servicepath = os.path.join(outdir, self.servicename)
        if self.expect_fail:
            if os.path.isfile(servicepath):
                raise RuntimeError(self._err_msg("Unexpected success"))
            return # Successfully failed checks done

        if not os.path.isfile(servicepath):
            raise FileNotFoundError(self._err_msg(f"Unexpected failure, can't find {servicepath}\n" + self.stdout))

        self.outdata = read_file(outdir, self.servicename)
        self.sections = parse_unitfile(canonicalize_unitfile(self.outdata))
        self.podman_args = shlex.split(self.sections.get("Service", {}).get("ExecStart", ["podman"])[0])
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
        outdata = {}
        with tempfile.TemporaryDirectory(prefix="quadlet-test-") as basedir:
            indir = os.path.join(basedir, "in")
            os.mkdir(indir)
            outdir = os.path.join(basedir, "out")
            os.mkdir(outdir)
            write_file (indir, self.filename, self.data);
            cmd = [generator_bin, '--no-kmsg-log', '-v', outdir]
            if use_valgrind:
                cmd = ["valgrind", "--error-exitcode=1", "--leak-check=full", "--show-possibly-lost=no", "--errors-for-leak-kinds=definite"] + cmd

            res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env = {
                "QUADLET_UNIT_DIRS": indir
            })

            self.stdout = res.stdout.decode('utf8')
            # The generator should never fail, just log warnings
            if res.returncode != 0:
                raise RuntimeError(self._err_msg(f"Unexpected generator failure\n" + self.stdout))

            self.check(outdir)


    def _err_msg(self, msg):
        err_msg = msg
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

    global use_valgrind
    use_valgrind = False
    if len(sys.argv) >= 4 and sys.argv[3] == '--valgrind':
        use_valgrind = True

    test_suite = unittest.TestSuite()
    for de in os.scandir(testcases_dir):
        name = de.name
        if (name.endswith(".container") or
            name.endswith(".network") or
            name.endswith(".volume")) and not name.startswith("."):
            test_suite.addTest(QuadletTestCase(name))

    return test_suite


if __name__ == '__main__':
    runner = unittest.TextTestRunner()
    runner.run(load_test_suite())
