#![feature(rustc_private)]
#![feature(box_patterns)]

extern crate rustc_driver;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_session;

mod config;
mod conflict_lock_checker;
mod double_lock_checker;

mod analysis;
mod callbacks;
mod options;
mod interest;
mod detector;

use options::Options;

use config::*;
use conflict_lock_checker::ConflictLockChecker;
use double_lock_checker::DoubleLockChecker;
use log::{debug, info};
use rustc_driver::Compilation;
use rustc_interface::{interface, Queries};
use rustc_session::config::ErrorOutputType;
use rustc_session::early_error;

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
    args.push("-Z".to_owned());
    args.push("always-encode-mir".to_owned());
    args.push("-Z".to_owned());
    args.push("mir-opt-level=0".to_owned());

    // Invoke compiler, and handle return code.
    let exit_code = rustc_driver::catch_with_exit_code(move || {
        rustc_driver::RunCompiler::new(&args, callbacks).run()
    });
    std::process::exit(exit_code)
}

fn main() {
    // Initialize loggers.
    if std::env::var("RUSTC_LOG").is_ok() {
        rustc_driver::init_rustc_env_logger();
    }
    if std::env::var("LOCKBUD_LOG").is_ok() {
        let e = env_logger::Env::new()
            .filter("LOCKBUD_LOG")
            .write_style("LOCKBUD_LOG_STYLE");
        env_logger::init_from_env(e);
    }
    // Get any options specified via the LOCKBUD_FLAGS environment variable
    let options = Options::parse_from_str(&std::env::var("LOCKBUD_FLAGS").unwrap_or_default())
        .unwrap_or_default();
    info!("LOCKBUD options from environment: {:?}", options);
    let mut args = std::env::args_os()
        .enumerate()
        .map(|(i, arg)| {
            arg.into_string().unwrap_or_else(|arg| {
                early_error(
                    ErrorOutputType::default(),
                    &format!("Argument {} is not valid Unicode: {:?}", i, arg),
                )
            })
        })
        .collect::<Vec<_>>();
    assert!(!args.is_empty());

    // Setting RUSTC_WRAPPER causes Cargo to pass 'rustc' as the first argument.
    // We're invoking the compiler programmatically, so we remove it if present.
    if args.len() > 1 && std::path::Path::new(&args[1]).file_stem() == Some("rustc".as_ref()) {
        args.remove(1);
    }

    let mut rustc_command_line_arguments: Vec<String> = args[1..].into();
    rustc_driver::install_ice_hook();
    let result = rustc_driver::catch_fatal_errors(|| {
        // Add back the binary name
        rustc_command_line_arguments.insert(0, args[0].clone());

        let print: String = "--print=".into();
        if rustc_command_line_arguments
            .iter()
            .any(|arg| arg.starts_with(&print))
        {
            // If a --print option is given on the command line we wont get called to analyze
            // anything. We also don't want to the caller to know that LOCKBUD adds configuration
            // parameters to the command line, lest the caller be cargo and it panics because
            // the output from --print=cfg is not what it expects.
        } else {
            let sysroot: String = "--sysroot".into();
            if !rustc_command_line_arguments
                .iter()
                .any(|arg| arg.starts_with(&sysroot))
            {
                // Tell compiler where to find the std library and so on.
                // The compiler relies on the standard rustc driver to tell it, so we have to do likewise.
                rustc_command_line_arguments.push(sysroot);
                rustc_command_line_arguments.push(find_sysroot());
            }

            let always_encode_mir: String = "always-encode-mir".into();
            if !rustc_command_line_arguments
                .iter()
                .any(|arg| arg.ends_with(&always_encode_mir))
            {
                // Tell compiler to emit MIR into crate for every function with a body.
                rustc_command_line_arguments.push("-Z".into());
                rustc_command_line_arguments.push(always_encode_mir);
            }

            // Replace ``mir-opt-level=?'' with ``mir-opt-level=0'' if exists
            // to preserve StorageLive and StorageDead in MIR.
            let mir_opt_level_eq: String = "mir-opt-level=".into();
            let mut contained = false;
            rustc_command_line_arguments.iter_mut().for_each(|arg| {
                let len = arg.len() - 1;
                if len >= mir_opt_level_eq.len()
                    && arg[..len].ends_with(&mir_opt_level_eq)
                    && &arg[len..] != "0"
                {
                    arg.replace_range(len.., "0");
                    contained = true;
                }
            });
            // If ``mir-opt-level=?'' not exists, add it to arguments.
            if !contained {
                rustc_command_line_arguments.push("-Z".into());
                rustc_command_line_arguments.push("mir-opt-level=0".into());
            }
        }

        let mut callbacks = callbacks::LockBudCallbacks::new(options);
        debug!(
            "rustc_command_line_arguments {:?}",
            rustc_command_line_arguments
        );
        let compiler =
            rustc_driver::RunCompiler::new(&rustc_command_line_arguments, &mut callbacks);
        compiler.run()
    })
    .and_then(|result| result);
    let exit_code = match result {
        Ok(_) => rustc_driver::EXIT_SUCCESS,
        Err(_) => rustc_driver::EXIT_FAILURE,
    };
    std::process::exit(exit_code);
}

fn find_sysroot() -> String {
    let home = option_env!("RUSTUP_HOME");
    let toolchain = option_env!("RUSTUP_TOOLCHAIN");
    match (home, toolchain) {
        (Some(home), Some(toolchain)) => format!("{}/toolchains/{}", home, toolchain),
        _ => option_env!("RUST_SYSROOT")
            .expect(
                "Could not find sysroot. Specify the RUST_SYSROOT environment variable, \
                 or use rustup to set the compiler to use for LOCKBUD",
            )
            .to_owned(),
    }
}
