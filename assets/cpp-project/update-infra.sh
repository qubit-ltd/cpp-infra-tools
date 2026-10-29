#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail

project_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
bootstrap="$project_root/.infra/bootstrap.py"

if [[ -f "$bootstrap" && ! -L "$bootstrap" ]]; then
    python_cmd=${CPP_INFRA_PYTHON:-}
    if [[ -z "$python_cmd" ]]; then
        if command -v python3 >/dev/null 2>&1; then
            python_cmd=python3
        elif command -v python >/dev/null 2>&1; then
            python_cmd=python
        else
            echo "error: Python 3.10 or newer is required to update cpp-infra" >&2
            exit 2
        fi
    fi
    if ! command -v "$python_cmd" >/dev/null 2>&1; then
        echo "error: configured Python executable not found: $python_cmd" >&2
        exit 2
    fi
    exec "$python_cmd" "$bootstrap" "$@"
fi

if [[ $# -ne 0 ]]; then
    echo "error: cpp-infra is not initialized; run ./update-infra.sh without options first" >&2
    exit 2
fi

for required_command in git cargo; do
    if ! command -v "$required_command" >/dev/null 2>&1; then
        echo "error: $required_command is required to initialize cpp-infra" >&2
        exit 2
    fi
done

tmp_root=${TMPDIR:-/tmp}
work=$(mktemp -d "${tmp_root%/}/cpp-infra-bootstrap.XXXXXX")
marker="$work/.cpp-infra-bootstrap-temp"
printf '%s\n' 'cpp-infra-bootstrap-temp-v1' > "$marker"
cleanup() {
    if [[ -d "$work" && ! -L "$work" && -f "$marker" && ! -L "$marker" ]] \
        && IFS= read -r marker_contents < "$marker" \
        && [[ "$marker_contents" == 'cpp-infra-bootstrap-temp-v1' ]]; then
        rm -rf -- "$work"
    else
        echo "warning: temporary bootstrap directory was not removed: $work" >&2
    fi
}
trap cleanup EXIT

repository=https://github.com/qubit-ltd/cpp-infra-tools.git
if ! git clone --quiet --depth 1 --single-branch --branch main "$repository" "$work/source"; then
    echo "error: unable to fetch cpp-infra-tools main for first-time initialization" >&2
    exit 1
fi

(cd -- "$work/source" && cargo run --locked -- --project "$project_root" init)
