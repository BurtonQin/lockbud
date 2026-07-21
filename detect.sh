#!/usr/bin/env bash

# this script's location
DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"

recursive=false
while [ $# -gt 0 ]; do
        case "$1" in
                -r|--recursive)
                        recursive=true
                        shift
                        ;;
                *)
                        detecting_dir="$1"
                        shift
                        ;;
        esac
done

if [ -z "${detecting_dir:-}" ]; then
        echo "No detecting directory is provided"
        echo "Usage: ./detect.sh [-r|--recursive] DIRNAME"
        exit 1
fi
# Build lockbud
# cargo build
# For development of lockbud use debug
# export RUSTC_WRAPPER=${PWD}/target/debug/lockbud
# For usage use release
cargo build --release
export RUSTC_WRAPPER=${PWD}/target/release/lockbud
export RUST_BACKTRACE=full
export LOCKBUD_LOG=info
# To only detect inter,intra
#export LOCKBUD_FLAGS="--detector-kind deadlock --crate-name-list inter,intra"
# or shorter
#export LOCKBUD_FLAGS="-k deadlock -l inter,intra"
# To skip detecting inter or intra
#export LOCKBUD_FLAGS="--detector-kind deadlock --blacklist-mode --crate-name-list inter,intra"
# or shorter
#export LOCKBUD_FLAGS="-k deadlock -b -l inter,intra"
#export LOCKBUD_FLAGS="-k deadlock -b -l cc"
#export LOCKBUD_FLAGS="-k atomicity_violation"
#export LOCKBUD_FLAGS="-k memory"
#export LOCKBUD_FLAGS="-k panic"

pushd "${detecting_dir}" > /dev/null
if [ "${recursive}" = true ]; then
        export LOCKBUD_FLAGS=${LOCKBUD_FLAGS:-"-k all"}
else
        crate_name=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json, sys; data = json.load(sys.stdin); print(data["packages"][0]["name"])')
        export LOCKBUD_FLAGS=${LOCKBUD_FLAGS:-"-k all -l ${crate_name}"}
fi
cargo clean
cargo build
popd > /dev/null
