#!/usr/bin/env bash

# this script's location
DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"

if [ -z "$1" ]; then
        echo "No detecting directory is provided"
	echo "Usage: ./detect.sh DIRNAME"
        exit 1
fi
# Build lockbud
cargo build
# For development of lockbud use debug
export RUSTC_WRAPPER=${PWD}/target/debug/lockbud
# For usage use release
# cargo build --release
# export RUSTC_WRAPPER=${PWD}/target/release/lockbud
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
export LOCKBUD_FLAGS="-k deadlock -b -l cc"

# Find all Cargo.tomls recursively under the detecting directory
# and record them in cargo_dir.txt
cargo_dir_file=$(realpath $DIR/cargo_dir.txt)
rm -f $cargo_dir_file
touch $cargo_dir_file

pushd "$1" > /dev/null
cargo clean
cargo_tomls=$(find . -name "Cargo.toml")
for cargo_toml in ${cargo_tomls[@]}
do
        echo $(dirname $cargo_toml) >> $cargo_dir_file
done

IFS=$'\n' read -d '' -r -a lines < ${cargo_dir_file}
for cargo_dir in ${lines[@]}
do
        pushd ${cargo_dir} > /dev/null
        cargo build
        popd > /dev/null
done
popd > /dev/null

rm -f $cargo_dir_file
