#!/usr/bin/env bash

if [ -z "$1" ]; then
	echo "No detecting directory is provided"
	exit 1
fi
cargo build --release
export RUSTC=${PWD}/target/release/rust-lock-bug-detector
export RUST_BACKTRACE=full
#export RUST_LOCK_DETECTOR_TYPE=DoubleLockDetector
export RUST_LOCK_DETECTOR_TYPE=ConflictLockDetector
export RUST_LOCK_DETECTOR_BLACK_LISTS="cc"
#export RUST_LOCK_DETECTOR_WHITE_LISTS="inter,intra,static_ref"
cd "$1"
cargo clean
cargo check
