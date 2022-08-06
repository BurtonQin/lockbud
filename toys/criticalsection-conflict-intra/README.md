[2022-08-05T18:27:56Z INFO  lockbud::criticalsection] critical section analyzing crate "conflict_intra"
[2022-08-05T18:27:56Z INFO  lockbud::criticalsection] functions: 1
[2022-08-05T18:27:56Z INFO  lockbud::criticalsection] analyzing DefId(0:13 ~ conflict_intra[71eb]::main)
[2022-08-05T18:27:56Z INFO  lockbud::criticalsection] possible blocking 2 calls
[2022-08-05T18:27:56Z INFO  lockbud::criticalsection] [CallInCriticalSection { callchains: [("/home/szx5097/code/luckbud/toys/criticalsection-conflict-intra/src/main.rs", 7, 5, 7, 24)], ty: CondVarWait }, CallInCriticalSection { callchains: [("/home/szx5097/code/luckbud/toys/criticalsection-conflict-intra/src/main.rs", 7, 5, 7, 24)], ty: CondVarWait }]