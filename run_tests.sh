#!/bin/bash

#cargo test
cargo build --verbose && ./tests/testcase-runner.py ./tests/cases "${CARGO_TARGET_DIR:-.}/debug/quadlet-rs"

### run tests on selected cases
#cargo b
#./tests/testcase-runner.py ./tmp/workon "${CARGO_TARGET_DIR}/debug/quadlet-rs"
#QUADLET_UNIT_DIRS=./tmp/workon ~/src/quadlet/_build/src/quadlet-generator -v tmp/workon-output

### run original (C/Go) quadlet
#QUADLET_UNIT_DIRS=./tests/cases /usr/libexec/podman/quadlet -v tmp/quadlet-test-cases-output
