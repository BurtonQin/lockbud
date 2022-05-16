extern crate rustc_driver;
extern crate rustc_hir;

use std::path::PathBuf;

use crate::detector::lock::LockGuardInstanceGraph;
use crate::options::{CrateNameList, Options};
use log::info;
use rustc_driver::Compilation;
use rustc_hir::def_id::LOCAL_CRATE;
use rustc_interface::{interface};
use rustc_middle::mir::mono::MonoItem;
use rustc_middle::ty::{Instance, ParamEnv, TyCtxt};

use crate::analysis::callgraph::CallGraph;

use crate::detector::lock::DeadLockDetector;

pub struct LockBudCallbacks {
    options: Options,
    file_name: String,
    output_directory: PathBuf,
    test_run: bool,
}

impl LockBudCallbacks {
    pub fn new(options: Options) -> Self {
        Self {
            options,
            file_name: String::new(),
            output_directory: PathBuf::default(),
            test_run: false,
        }
    }
}

impl rustc_driver::Callbacks for LockBudCallbacks {
    fn config(&mut self, config: &mut rustc_interface::interface::Config) {
        self.file_name = config.input.source_name().prefer_remapped().to_string();
        info!("Processing input file: {}", self.file_name);
        if config.opts.test {
            info!("in test only mode");
            // self.options.test_only = true;
        }
        match &config.output_dir {
            None => {
                self.output_directory = std::env::temp_dir();
                self.output_directory.pop();
            }
            Some(path_buf) => self.output_directory.push(path_buf.as_path()),
        }
    }
    fn after_analysis<'tcx>(
        &mut self,
        compiler: &rustc_interface::interface::Compiler,
        queries: &'tcx rustc_interface::Queries<'tcx>,
    ) -> rustc_driver::Compilation {
        compiler.session().abort_if_errors();
        if self
            .output_directory
            .to_str()
            .expect("valid string")
            .contains("/build/")
        {
            // No need to analyze a build script, but do generate code.
            return Compilation::Continue;
        }
        queries
            .global_ctxt()
            .unwrap()
            .peek_mut()
            .enter(|tcx| self.analyze_with_lockbud(compiler, tcx));
        if self.test_run {
            // We avoid code gen for test cases because LLVM is not used in a thread safe manner.
            Compilation::Stop
        } else {
            // Although MIRAI is only a checker, cargo still needs code generation to work.
            Compilation::Continue
        }
    }
}

impl LockBudCallbacks {
    fn analyze_with_lockbud<'tcx>(&mut self, _compiler: &interface::Compiler, tcx: TyCtxt<'tcx>) {
        // Skip crates by names (white or black list).
        let crate_name = tcx.crate_name(LOCAL_CRATE).to_string();
        match &self.options.crate_name_list {
            CrateNameList::White(crates) if !crates.contains(&crate_name) => return,
            CrateNameList::Black(crates) if crates.contains(&crate_name) => return,
            _ => {}
        };
        if tcx.sess.opts.debugging_opts.no_codegen || !tcx.sess.opts.output_types.should_codegen() {
            return;
        }
        let cgus = tcx.collect_and_partition_mono_items(()).1;
        let instances: Vec<Instance<'tcx>> = cgus
            .into_iter()
            .flat_map(|cgu| {
                cgu.items().into_iter().filter_map(|(mono_item, _)| {
                    if let MonoItem::Fn(instance) = mono_item {
                        Some(*instance)
                    } else {
                        None
                    }
                })
            })
            .collect();
        let mut callgraph = CallGraph::new();
        let param_env = ParamEnv::reveal_all();
        callgraph.analyze(instances.clone(), tcx, param_env);
        callgraph.dot();
        let mut lockguard_instance_graph = LockGuardInstanceGraph::new();
        lockguard_instance_graph.analyze(&callgraph, tcx, param_env);
        lockguard_instance_graph.dot();
        let mut deadlock_detector = DeadLockDetector::new(tcx, param_env);
        deadlock_detector.detect(&callgraph);
        // println!("relations: {:?}", deadlock_detector.lockguard_relations);
        // for instance in instances {
        //     let body = tcx.instance_mir(instance.def);
        //     if body.source.promoted.is_some() {
        //         continue;
        //     }
        //     println!("{:?}", instance.def_id());
        //     let mut pointer_analysis = Andersen::new(body);
        //     pointer_analysis.analyze();
        //     let pts = pointer_analysis.finish();
        //     println!("{:#?}", pts);
        // }
    }
}
