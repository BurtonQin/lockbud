#!/usr/bin/env bash

# this script's location
DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"

if [ -z "$1" ]; then
        echo "No detecting directory is provided"
	echo "Usage: ./detect.sh DIRNAME"
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
export LOCKBUD_FLAGS="-k deadlock"
time_log=time.log
time_err=time.err
pushd "$1" > /dev/null
# cargo clean
begin=$(date +%s)
CARGO_BUILD_JOBS=1 cargo build 1>${time_log} 2>${time_err}
# for wasmer
# CARGO_BUILD_JOBS=1 cargo build --manifest-path lib/cli/Cargo.toml --features cranelift,singlepass --bin wasmer 1>${time_log} 2>${time_err}
# firecracker use build instead of targets, so will be skippped. Must change code.
end=$(date +%s)
total_time=$(($end-$begin))
echo $total_time
grep '^Elapsed: ' $time_log | cut -d' ' -f2 | awk  '{s+=$1} END {print s}'
popd > /dev/null
