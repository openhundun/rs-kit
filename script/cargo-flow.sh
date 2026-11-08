#!/usr/bin/env bash
set -euo pipefail

readonly ARG_JOB="${1:-}"
readonly ARG_STEP="${2:-}"
readonly GIT_REPOSITORY_DIR="${GIT_REPOSITORY_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
readonly APP_NAME="cargo-flow"

function help() {
    echo "USAGE: ${0} <job__check> [step__fmt|step__clippy|step__build|step__smoke]"
}

function job__check() {
    function step__fmt() {
        cd "${GIT_REPOSITORY_DIR}"
        cargo fmt --all -- --check
    }

    function step__clippy() {
        cd "${GIT_REPOSITORY_DIR}"
        cargo clippy --workspace --all-targets -- -D warnings
    }

    function step__build() {
        cd "${GIT_REPOSITORY_DIR}"
        cargo build --workspace --release
    }

    function step__smoke() {
        cd "${GIT_REPOSITORY_DIR}"
        local tmp_dir="${GIT_REPOSITORY_DIR}/target/smoke"
        rm -rf "${tmp_dir}"
        mkdir -p "${tmp_dir}"

        local manifest_path="${tmp_dir}/Cargo.toml"
        cat > "${manifest_path}" << 'EOF'
[package]
name = "smoke"
version = "0.1.0"
edition.workspace = true

[dependencies]
alpha.workspace = true # relocated comment
beta = "1.0.0"
gamma = "0.0.4"
delta = "=1.2.3"

[workspace.dependencies]
epsilon = "2.3.0"
EOF

        cat > "${tmp_dir}/expected.toml" << 'EOF'
[package]
name = "smoke"
version = "0.1.0"
edition = { workspace = true }

[dependencies]
# relocated comment
alpha = { workspace = true, features = [] }
beta = { version = "1", features = [] }
gamma = { version = "0.0.4", features = [] }
delta = { version = "=1.2.3", features = [] }

[workspace.dependencies]
epsilon = "2.3"
EOF

        cargo run --quiet --release -p "${APP_NAME}" -- "${manifest_path}" --features --normalize-versions
        diff -u "${tmp_dir}/expected.toml" "${manifest_path}"
        cp "${manifest_path}" "${tmp_dir}/converted.toml"
        local second_output="$(cargo run --quiet --release -p "${APP_NAME}" -- "${manifest_path}" --features --normalize-versions)"
        cmp "${tmp_dir}/converted.toml" "${manifest_path}"
        if [[ -n "${second_output}" ]]; then
            echo "[check][smoke]: FAIL - second run is not silent" >&2
            return 1
        fi
        rm -rf "${tmp_dir}"
        echo "[check][smoke]: PASS"
    }

    function all() {
        step__fmt
        step__clippy
        step__build
        step__smoke
    }

    case "${ARG_STEP}" in
    step__fmt) step__fmt ;;
    step__clippy) step__clippy ;;
    step__build) step__build ;;
    step__smoke) step__smoke ;;
    *) all ;;
    esac
}

case "${ARG_JOB}" in
job__check) job__check ;;
*) help ;;
esac
