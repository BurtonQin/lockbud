# lockbud
Statically detect deadlocks bugs for Rust.

This work follows up the our elaborated Rust study in [Understanding Memory and Thread Safety Practices and Issues in Real-World Rust Programs](https://songlh.github.io/paper/rust-study.pdf) in PLDI'20.
I am honored to share the co-first author with Yilun Chen and be able to collaborate with far-sighted, knowledgeable and hardworking Prof Linhai Song and Yiying Zhang.
I focus on Rust unsafe code and concurrency bugs in this paper.

Please refer to our paper for more interesting concurrency and memory bug categories in Rust.

This project is my initial efforts to improve the concurrency safety in Rust ecosystem by statically detecting two common kinds of deadlock bugs:
doublelock and locks in conflicting order (conflictlock for brevity).

A deadlock Condvar detector is implemented along with the two deadlock detectors, but it may report many FPs.
Ongoing work includes other concurrency bugs like atomicity violation and some memory bugs like use-after-free and invalid free. See branch uaf.

## Install
Currently supports rustc 1.66.0-nightly (c97d02cdb 2022-10-05)
```
$ git clone https://github.com/BurtonQin/lockbud.git
$ cd lockbud
$ rustup component add rust-src
$ rustup component add rustc-dev
$ rustup component add llvm-tools-preview
$ cargo install --path .
```

Note that you must use the same rustc nightly version as lockbud to detect your project!
You can either override your rustc version or specify rust-toolchains in your project.

## Example
Test toys
```
$ ./detect.sh toys/inter
```
It will print 15 doublelock bugs in json format, like the following one:

```
      {
        "DoubleLock": {
          "bug_kind": "DoubleLock",
          "possibility": "Possibly",
          "diagnosis": {
            "first_lock_type": "ParkingLotWrite(i32)",
            "first_lock_span": "src/main.rs:77:16: 77:32 (#0)",
            "second_lock_type": "ParkingLotRead(i32)",
            "second_lock_span": "src/main.rs:84:18: 84:33 (#0)",
            "callchains": [
              [
                [
                  "src/main.rs:79:20: 79:52 (#0)"
                ]
              ]
            ]
          },
          "explanation": "The first lock is not released when acquiring the second lock"
        }
      }
```

The output shows that there is possibly a doublelock bug. The DeadlockDiagnosis reads that the first lock is a parking_lot WriteLock acquired on src/main.rs:77 and the second lock is a parking_lot ReadLock aquired on src/main.rs:84. The first lock reaches the second lock through callsites src/main.rs:79. The explanation demonstrates the reason for doubelock.

```
$ ./detect.sh toys/conflict-inter
```
It will print one conflictlock bug

```
      {
        "ConflictLock": {
          "bug_kind": "ConflictLock",
          "possibility": "Possibly",
          "diagnosis": [
            {
              "first_lock_type": "StdRwLockRead(i32)",
              "first_lock_span": "src/main.rs:29:16: 29:40 (#0)",
              "second_lock_type": "StdMutex(i32)",
              "second_lock_span": "src/main.rs:36:10: 36:34 (#0)",
              "callchains": [
                [
                  [
                    "src/main.rs:31:20: 31:38 (#0)"
                  ]
                ]
              ]
            },
            {
              "first_lock_type": "StdMutex(i32)",
              "first_lock_span": "src/main.rs:18:16: 18:40 (#0)",
              "second_lock_type": "StdRwLockWrite(i32)",
              "second_lock_span": "src/main.rs:25:10: 25:35 (#0)",
              "callchains": [
                [
                  [
                    "src/main.rs:20:20: 20:35 (#0)"
                  ]
                ]
              ]
            }
          ],
          "explanation": "Locks mutually wait for each other to form a cycle"
        }
      }
```

The output shows that there is possibly a conflictlock bug. The DeadlockDiagnosis is similar to doublelock bugs except that there are at least two diagnosis records. All the diagnosis records form a cycle, e.g. A list of records [(first_lock, second_lock), (second_lock', first_lock')] means that it is possible that first_lock is aquired and waits for second_lock in one thread, while second_lock' is aquired and waits for first_lock' in another thread, which incurs a conflictlock bug.

`detect.sh` is mainly for development of the detector and brings more flexibility.
You can modify `detect.sh` to use release vesion of lockbud to detect large and complex projects.

For ease of use, you can also run cargo lockbud
```
$ cd toys/inter; cargo clean; cargo lockbud -k deadlock
```
Note that you need to run
```
cargo clean
```
before re-running lockbud.

You can also specify blacklist or whitelist of crate names.

The `-b` implies the list is a blacklist.

The `-l` is followed by a list of crate names seperated by commas.
```
$ cd YourProject; cargo clean; cargo lockbud -k deadlock -b -l cc,tokio_util,indicatif
```

## How it works
In Rust, a lock operation returns a lockguard. The lock will be unlocked when the lockguard is dropped.
So we can track the lifetime of lockguards to detect lock-related bugs.
For each crate (the crate to be detected and its dependencies)
1. Collect the caller-callee info to generate a callgraph.
2. Collect LockGuard info, including
   - The lockguard type and span;
   - Where it is created and where it is dropped.
3. Apply a GenKill algorithm on the callgraph to find pairs of lockguards (a, b) s.t.
   - a not dropped when b is created.
4. A pair (a, b) can doublelock if
   - the lockguard types of a & b can deadlock;
   - and a & b may point to the same lock (obtained from points-to analysis).
5. For (a, b), (c, d) in the remaining pairs
   - if b and c can deadlock then add an edge from (a, b) to (c, d) into a graph.
6. The cycle in the graph implies a conflictlock.

## Caveats
1. Currently only supports `std::sync::{Mutex, RwLock}`, `parking_lot::{Mutex, RwLock}`, `spin::{Mutex, RwLock}`
2. The callgraph is crate-specific (the callers and callees are in the same crate) and cannot track indirect call.
3. The points-to analysis is imprecise and makes heuristic assumptions for function calls and assignments.
   - A common FP comes from `cc`, where points-to analysis incorrectly assumes that two unrelated lockguards are from the same lock. Thus blacklist `cc` in `detector.sh`.

## Results
Found dozens of bugs in many repositories: openethereum, grin, winit, sonic, lighthouse, etc.
Some of the repositories are dependencies of other large projects.
We try to strike a balance between FP and FN to make the detector usable.

Bugs detected and fixed (one PR may fix multiple bugs):

1. https://github.com/openethereum/openethereum/pull/289
2. https://github.com/openethereum/parity-ethereum/pull/11764
3. https://github.com/sigp/lighthouse/pull/1241
4. https://github.com/solana-labs/solana/pull/10466
5. https://github.com/solana-labs/solana/pull/10469
6. https://github.com/wasmerio/wasmer/pull/1466
7. https://github.com/paritytech/substrate/pull/6277
8. https://github.com/mimblewimble/grin/pull/3337
9. https://github.com/mimblewimble/grin/pull/3340
10. https://github.com/sigp/lighthouse/pull/1241
11. https://github.com/rust-windowing/winit/pull/1579
12. https://github.com/solana-labs/solana/pull/26046
13. https://github.com/solana-labs/solana/pull/26047
14. https://github.com/solana-labs/solana/pull/26053
15. https://github.com/qdrant/qdrant/issues/724
16. https://github.com/apache/incubator-teaclave-sgx-sdk/pull/269

## License
The lockbud Project is licensed under BSD-3.
