# rust-lock-bug-detector
Statically detect double-lock &amp; conflicting-lock bugs on MIR.

This work follows up the our elaborated Rust study in [Understanding Memory and Thread Safety Practices and Issues in Real-World Rust Programs](https://songlh.github.io/paper/rust-study.pdf) in PLDI'20.
I am honored to share the co-first author with Yilun Chen and be able to collaborate with far-sighted, knowledgeable and hardworking Prof Linhai Song and Yiying Zhang.
I focus on Rust unsafe code and concurrency bugs in this paper.
This project is my initial efforts to improve the concurrency safety in Rust ecosystem by statically detecting two common kinds of concurrency bugs:
double lock and locks in conflicting order.

## Install
Currently supports rustc version: 1.45.0-nightly (fede83ccf 2020-07-06)
```
$ git clone https://github.com/BurtonQin/rust-lock-bug-detector.git
$ cd rust-lock-bug-detector
$ rustup component add rust-src
$ rustup component add rustc-dev
$ cargo install --path .
$ export LD_LIBRARY_PATH=$HOME/.rustup/toolchains/nightly-2020-05-09-x86_64-unknown-linux-gnu/lib:$LD_LIBRARY_PATH
```

## Example
Test examples
```
$ ./run.sh examples/inter
```

Run with cargo subcommands
```
$ cd examples/inter; cargo clean; cargo lock-bug-detect double-lock
$ cd examples/conflict-inter; cargo clean; cargo lock-bug-detect conflict-lock
```
You need to run
```
cargo clean
```
before re-detecting.

## How it works
In Rust, a lock operation returns a lockguard. The lock will be unlocked when the lockguard is dropped.
So we can track the lifetime of lockguards to detect lock-related bugs.
For each crate (the crate to be checked and its dependencies)
1. Collect LockGuard info, including
   - Where its lifetime begins and where it is dropped.
   - Use an (immature) automata to track its src (where the lockguard is created) to check if two lockguards come from the same lock heuristically.
2. Collect the caller-callee relationship to generate the callgraph.
3. Apply a GenKill algorithm to detect the lock-related bugs.

## Caveats
1. Currently only supports `std::sync::{Mutex, RwLock}`, `parking_lot::{Mutex, RwLock}`, `spin::{Mutex, RwLock}`
2. The automata to track lockguard src location is still immature and uses many heuristic assumptions.
3. The callgraph is crate-specific (the callers and callees are in the same crate) and cannot track indirect call.
4. In the GenKill algorithm, the current iteration times for one function is limited to 10000 and the call-chain depth is 4 for speed.

## Results
Found dozens of bugs in many repositories: openethereum, grin, winit, sonic, lighthouse, etc.
Some of the repositories are dependencies of other large projects.
I only find one FP is in crate `cc` because the automata mistakenly assumes two unrelated lockguards are from the same src.
