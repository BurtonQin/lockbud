#!/usr/bin/env bash

# this script's location
DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"


if [ -z "$1" ]; then
        echo "No detecting directory is provided"
        exit 1
fi
cargo build
export RUSTC_WRAPPER=${PWD}/target/debug/rust-lock-bug-detector
export RUST_BACKTRACE=full
# export RUSTC_LOG=info
export LOCKBUD_LOG=debug
export LOCKBUD_FLAGS="-k doublelock -d 4 -n 10000"

cargo_dir_file=$(realpath $DIR/cargo_dir.txt)
rm -f $cargo_dir_file
touch $cargo_dir_file

pushd "$1" > /dev/null
cargo clean
cargo_tomls=$(find . -name "Cargo.toml")
for cargo_toml in ${cargo_tomls[@]}
do
#	echo $cargo_toml
        echo $(dirname $cargo_toml) >> $cargo_dir_file
done

IFS=$'\n' read -d '' -r -a lines < ${cargo_dir_file}
for cargo_dir in ${lines[@]}
do
        echo ${cargo_dir}
        pushd ${cargo_dir} > /dev/null
        cargo build
        popd > /dev/null
done
popd > /dev/null

#pushd "$1" > /dev/null
#cargo clean
#cargo check
#popd > /dev/null
