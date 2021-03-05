#![feature(rustc_private)]
#![feature(box_patterns)]

extern crate rustc_driver;
extern crate rustc_interface;
extern crate rustc_session;

mod config;
mod conflict_lock_checker;
mod double_lock_checker;

use config::*;
use conflict_lock_checker::ConflictLockChecker;
use double_lock_checker::DoubleLockChecker;
use rustc_driver::Compilation;
use rustc_interface::{interface, Queries};
use rustc_session::early_error;
use rustc_session::{config::ErrorOutputType, CtfeBacktrace};

struct DetectorCallbacks;

impl rustc_driver::Callbacks for DetectorCallbacks {
    fn after_analysis<'tcx>(
        &mut self,
        compiler: &interface::Compiler,
        queries: &'tcx Queries<'tcx>,
    ) -> Compilation {
        compiler.session().abort_if_errors();
        queries.global_ctxt().unwrap().peek_mut().enter(|tcx| {
            let lock_config = LockDetectorConfig::from_env().unwrap();
            match lock_config.lock_detector_type {
                LockDetectorType::DoubleLockDetector => match lock_config.crate_name_lists {
                    CrateNameLists::Black(crate_name_black_lists) => {
                        let mut double_lock_checker =
                            DoubleLockChecker::new(false, crate_name_black_lists);
                        double_lock_checker.check(tcx);
                    }
                    CrateNameLists::White(crate_name_white_lists) => {
                        let mut double_lock_checker =
                            DoubleLockChecker::new(true, crate_name_white_lists);
                        double_lock_checker.check(tcx);
                    }
                },
                LockDetectorType::ConflictLockDetector => match lock_config.crate_name_lists {
                    CrateNameLists::Black(crate_name_black_lists) => {
                        let mut conflict_lock_checker =
                            ConflictLockChecker::new(false, crate_name_black_lists);
                        conflict_lock_checker.check(tcx);
                    }
                    CrateNameLists::White(crate_name_white_lists) => {
                        let mut conflict_lock_checker =
                            ConflictLockChecker::new(true, crate_name_white_lists);
                        conflict_lock_checker.check(tcx);
                    }
                },
            }
        });
        Compilation::Continue
    }
}

fn compile_time_sysroot() -> Option<String> {
    if option_env!("RUST_STAGE").is_some() {
        return None;
    }
    let home = option_env!("RUSTUP_HOME").or(option_env!("MULTIRUST_HOME"));
    let toolchain = option_env!("RUSTUP_TOOLCHAIN").or(option_env!("MULTIRUST_TOOLCHAIN"));
    Some(match (home, toolchain) {
        (Some(home), Some(toolchain)) => format!("{}/toolchains/{}", home, toolchain),
        _ => option_env!("RUST_SYSROOT")
            .expect("To build rust-lock-bug-detector without rustup, set the `RUST_SYSROOT` env var at build time")
            .to_owned(),
    })
}

/// Execute a compiler with the given CLI arguments and callbacks.
fn run_compiler(mut args: Vec<String>, callbacks: &mut (dyn rustc_driver::Callbacks + Send)) -> ! {
    // Make sure we use the right default sysroot. The default sysroot is wrong,
    // because `get_or_default_sysroot` in `librustc_session` bases that on `current_exe`.
    //
    // Make sure we always call `compile_time_sysroot` as that also does some sanity-checks
    // of the environment we were built in.
    // FIXME: Ideally we'd turn a bad build env into a compile-time error via CTFE or so.
    if let Some(sysroot) = compile_time_sysroot() {
        let sysroot_flag = "--sysroot";
        if !args.iter().any(|e| e == sysroot_flag) {
            // We need to overwrite the default that librustc_session would compute.
            args.push(sysroot_flag.to_owned());
            args.push(sysroot);
        }
    }

    // Invoke compiler, and handle return code.
    let exit_code = rustc_driver::catch_with_exit_code(move || {
        rustc_driver::RunCompiler::new(&args, callbacks).run()
    });
    std::process::exit(exit_code)
}

fn main() {
    rustc_driver::init_rustc_env_logger();
    // We cannot use `rustc_driver::main` as we need to adjust the CLI arguments.
    let args = std::env::args_os()
        .enumerate()
        .map(|(i, arg)| {
            arg.into_string().unwrap_or_else(|arg| {
                early_error(
                    ErrorOutputType::default(),
                    &format!("argument {} is not valid Unicode: {:?}", i, arg),
                )
            })
        })
        .collect::<Vec<_>>();
    run_compiler(args, &mut DetectorCallbacks {})
}

