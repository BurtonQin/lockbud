//! The general rustc plugin framework.
//! Inspired by <https://github.com/facebookexperimental/MIRAI/blob/9cf3067309d591894e2d0cd9b1ee6e18d0fdd26c/checker/src/main.rs>
#![feature(rustc_private)]
#![feature(box_patterns)]

extern crate rustc_driver;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_hir;
#[macro_use]
extern crate rustc_smir;
extern crate stable_mir;

mod analysis;
// mod callbacks;
// mod detector;
mod interest;
mod options;

use std::process::ExitCode;

use analysis::callgraph::{CallGraph, InstanceId};
use log::debug;
use options::Options;
use petgraph::graph::NodeIndex;
use rustc_middle::ty::TyCtxt;
use rustc_middle::mir::mono::MonoItem;
use rustc_middle::ty::Instance;
use rustc_session::config::ErrorOutputType;
use rustc_session::EarlyDiagCtxt;

use rustc_smir::rustc_internal;
use stable_mir::CompilerError;
use std::ops::ControlFlow;


fn main() -> ExitCode {
    // Initialize loggers.
    let handler = EarlyDiagCtxt::new(ErrorOutputType::default());
    if std::env::var("RUSTC_LOG").is_ok() {
        rustc_driver::init_rustc_env_logger(&handler);
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
    debug!("LOCKBUD options from environment: {options:?}");
    let mut args = std::env::args_os()
        .enumerate()
        .map(|(i, arg)| {
            arg.into_string().unwrap_or_else(|arg| {
                handler.early_fatal(format!("Argument {i} is not valid Unicode: {arg:?}"))
            })
        })
        .collect::<Vec<_>>();
    assert!(!args.is_empty());

    // Setting RUSTC_WRAPPER causes Cargo to pass 'rustc' as the first argument.
    // We're invoking the compiler programmatically, so we remove it if present.
    if args.len() > 1 && std::path::Path::new(&args[1]).file_stem() == Some("rustc".as_ref()) {
        args.remove(1);
    }

    let mut rustc_command_line_arguments = args;
    rustc_driver::install_ice_hook("ice ice ice baby", |_| ());
    // let exit_code = rustc_driver::catch_with_exit_code(|| {
    //     let print = "--print=";
    //     if rustc_command_line_arguments
    //         .iter()
    //         .any(|arg| arg.starts_with(print))
    //     {
    //         // If a --print option is given on the command line we wont get called to analyze
    //         // anything. We also don't want to the caller to know that LOCKBUD adds configuration
    //         // parameters to the command line, lest the caller be cargo and it panics because
    //         // the output from --print=cfg is not what it expects.
    //     } else {
    //         let sysroot = "--sysroot";
    //         if !rustc_command_line_arguments
    //             .iter()
    //             .any(|arg| arg.starts_with(sysroot))
    //         {
    //             // Tell compiler where to find the std library and so on.
    //             // The compiler relies on the standard rustc driver to tell it, so we have to do likewise.
    //             rustc_command_line_arguments.push(format!("{sysroot}={}", find_sysroot()));
    //         }

    //         let always_encode_mir = "always-encode-mir";
    //         if !rustc_command_line_arguments
    //             .iter()
    //             .any(|arg| arg.ends_with(always_encode_mir))
    //         {
    //             // Tell compiler to emit MIR into crate for every function with a body.
    //             rustc_command_line_arguments.push(format!("-Z{always_encode_mir}"));
    //         }
    //     }

    //     // let mut callbacks = callbacks::LockBudCallbacks::new(options);
    //     debug!("rustc_command_line_arguments {rustc_command_line_arguments:?}");
    //     let compiler =
    //         rustc_driver::RunCompiler::new(&rustc_command_line_arguments, &mut callbacks);
    //     compiler.run()
    // });
    let print = "--print=";
    if rustc_command_line_arguments
        .iter()
        .any(|arg| arg.starts_with(print))
    {
        // If a --print option is given on the command line we wont get called to analyze
        // anything. We also don't want to the caller to know that LOCKBUD adds configuration
        // parameters to the command line, lest the caller be cargo and it panics because
        // the output from --print=cfg is not what it expects.
    } else {
        let sysroot = "--sysroot";
        if !rustc_command_line_arguments
            .iter()
            .any(|arg| arg.starts_with(sysroot))
        {
            // Tell compiler where to find the std library and so on.
            // The compiler relies on the standard rustc driver to tell it, so we have to do likewise.
            rustc_command_line_arguments.push(format!("{sysroot}={}", find_sysroot()));
        }

        let always_encode_mir = "always-encode-mir";
        if !rustc_command_line_arguments
            .iter()
            .any(|arg| arg.ends_with(always_encode_mir))
        {
            // Tell compiler to emit MIR into crate for every function with a body.
            rustc_command_line_arguments.push(format!("-Z{always_encode_mir}"));
        }
    }

    // let mut callbacks = callbacks::LockBudCallbacks::new(options);
    debug!("rustc_command_line_arguments {rustc_command_line_arguments:?}");
    // println!("rustc_command_line_arguments {rustc_command_line_arguments:?}");
    
    let result = run_with_tcx!(rustc_command_line_arguments, start_demo_with_ctx);
    match result {
        Ok(_) | Err(CompilerError::Skipped | CompilerError::Interrupted(_)) => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
    // std::process::exit(exit_code);
}

fn start_demo_with_ctx(tcx: TyCtxt<'_>) -> ControlFlow<()> {
        let crate_name = stable_mir::local_crate().name;
        eprintln!("--- Analyzing crate: {crate_name}");
        // let crate_name2 = tcx.crate_name(LOCAL_CRATE).to_string();
        // eprintln!("--- Analyzing crate2: {crate_name2}");
        // let items = stable_mir::all_local_items();
        // for item in items {
        //     println!("{:?}", item);
        // }
        // let external_crates = stable_mir::external_crates();
        // for krate in external_crates {
        //     println!("{:?}", krate);
        // }
        let cgus = tcx.collect_and_partition_mono_items(()).1;
        let instances: Vec<Instance<'_>> = cgus
            .iter()
            .flat_map(|cgu| {
                cgu.items().iter().filter_map(|(mono_item, _)| {
                    if let MonoItem::Fn(instance) = mono_item {
                        Some(*instance)
                    } else {
                        None
                    }
                })
            })
            .collect();
        let instances = instances.into_iter().map(|instance| rustc_internal::stable(instance)).collect::<Vec<_>>();
        // for instance in instances {
        //     // println!("{}", instance.name());
        //     // println!("{}", instance.trimmed_name());
        //     // println!("{}", instance.mangled_name());
        //     // println!("body: {}", instance.has_body());
        //     if !instance.has_body() {
        //         println!("{}", instance.name()); 
        //     }
            
        //     // println!("{:?}", instance.def_id());
        //     // let _body = tcx.instance_mir(instance.def);
        // }}

        let mut callgraph = CallGraph::new();
        callgraph.analyze(instances.clone());
        // callgraph.dot();
        for instance in instances {
            if let Some(body) = instance.body() {
                let instance_id = NodeIndex::new(1);
                let mut lockguard_collector =
                interest::concurrency::lock::LockGuardCollector::new(instance_id, &instance, &body);
                lockguard_collector.analyze();
                if !lockguard_collector.lockguards.is_empty() {
                    println!("{}", instance.name());
                    println!("{:?}", lockguard_collector.lockguards);
                }
            }
        }
            

        // ControlFlow::Break(())
        ControlFlow::Continue(())
}


fn find_sysroot() -> String {
    let home = option_env!("RUSTUP_HOME");
    let toolchain = option_env!("RUSTUP_TOOLCHAIN");
    #[allow(clippy::option_env_unwrap)]
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
