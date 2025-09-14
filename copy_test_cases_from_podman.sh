#!/bin/bash
#
# create patch file with:
#> git diff --no-index --color=always ./tests/cases.origin ./tests/cases > ./tests/cases.patch

podman_repo_root="./tmp/podman"

[[ -d ${podman_repo_root}/test/e2e/quadlet ]] || (echo "Error: no podman checkout found in ${podman_repo_root}" && exit 1)

echo "Found podman repo directory: ${podman_repo_root}"
echo "Copying test cases from podman ..."
cp -r ${podman_repo_root}/test/e2e/quadlet/* ./tests/cases.origin/
cp -r ./tests/cases.origin/* ./tests/cases/
echo "Patching test cases ..."
patch -p0 -i ./tests/cases.patch
