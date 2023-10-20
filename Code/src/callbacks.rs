//! The main functionality: callbacks for rustc plugin systems.
//! Inspired by <https://github.com/facebookexperimental/MIRAI/blob/9cf3067309d591894e2d0cd9b1ee6e18d0fdd26c/checker/src/callbacks.rs>
extern crate rustc_driver;
extern crate rustc_hir;

use std::path::PathBuf;

use crate::analysis::pointsto::AliasAnalysis;
use crate::detector::memory::{InvalidFreeDetector, UseAfterFreeDetector};
use crate::options::{CrateNameList, DetectorKind, Options};
use log::{debug, warn};
use rustc_driver::Compilation;
use rustc_hir::def_id::LOCAL_CRATE;
use rustc_interface::interface;
use rustc_middle::mir::mono::MonoItem;
use rustc_middle::ty::{Instance, ParamEnv, TyCtxt};

use crate::analysis::callgraph::CallGraph;

use crate::detector::atomic::AtomicityViolationDetector;
use crate::detector::lock::DeadlockDetector;
use crate::detector::panic::PanicDetector;
use crate::detector::report::Report;

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
        debug!("Processing input file: {}", self.file_name);
        if config.opts.test {
            debug!("in test only mode");
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
        queries.global_ctxt().unwrap().enter(|tcx| {
            self.analyze_with_lockbud(compiler, tcx);
        });
        if self.test_run {
            // We avoid code gen for test cases because LLVM is not used in a thread safe manner.
            Compilation::Stop
        } else {
            // Although LockBud is only a checker, cargo still needs code generation to work.
            Compilation::Continue
        }
    }
}

impl LockBudCallbacks {
    fn analyze_with_lockbud<'tcx>(&mut self, _compiler: &interface::Compiler, tcx: TyCtxt<'tcx>) {
        // Skip crates by names (white or black list).
        let crate_name = tcx.crate_name(LOCAL_CRATE).to_string();
        match &self.options.crate_name_list {
            CrateNameList::White(crates) if !crates.is_empty() && !crates.contains(&crate_name) => {
                return
            }
            CrateNameList::Black(crates) if crates.contains(&crate_name) => return,
            _ => {}
        };
        if tcx.sess.opts.unstable_opts.no_codegen || !tcx.sess.opts.output_types.should_codegen() {
            return;
        }
        let cgus = tcx.collect_and_partition_mono_items(()).1;
        let instances: Vec<Instance<'tcx>> = cgus
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
        let mut callgraph = CallGraph::new();
        let param_env = ParamEnv::reveal_all();
        callgraph.analyze(instances.clone(), tcx, param_env);
        let mut alias_analysis = AliasAnalysis::new(tcx, &callgraph);
        match self.options.detector_kind {
            DetectorKind::Deadlock => {
                debug!("Detecting deadlock");
                let mut deadlock_detector = DeadlockDetector::new(tcx, param_env);
                let reports = deadlock_detector.detect(&callgraph, &mut alias_analysis);
                if !reports.is_empty() {
                    let j = serde_json::to_string_pretty(&reports).unwrap();
                    warn!("{}", j);
                    let stats = report_stats(&crate_name, &reports);
                    warn!("{}", stats);
                }
            }
            DetectorKind::AtomicityViolation => {
                debug!("Detecting atomicity violation");
                let mut atomicity_violation_detector = AtomicityViolationDetector::new(tcx);
                let reports = atomicity_violation_detector.detect(&callgraph, &mut alias_analysis);
                if !reports.is_empty() {
                    let j = serde_json::to_string_pretty(&reports).unwrap();
                    warn!("{}", j);
                    let stats = report_stats(&crate_name, &reports);
                    warn!("{}", stats);
                }
            }
            DetectorKind::Memory => {
                debug!("Detecting memory bugs");
                let mut reports = {
                    let invalid_free_detector = InvalidFreeDetector::new(tcx);
                    invalid_free_detector.detect(&callgraph, &mut alias_analysis)
                };
                let reports2 = {
                    let use_after_free_detector = UseAfterFreeDetector::new(tcx);
                    use_after_free_detector.detect(&callgraph, &mut alias_analysis)
                };
                reports.extend(reports2.into_iter());
                if !reports.is_empty() {
                    let j = serde_json::to_string_pretty(&reports).unwrap();
                    warn!("{}", j);
                    let stats = report_stats(&crate_name, &reports);
                    warn!("{}", stats);
                }
            }
            DetectorKind::All => {
                debug!("Detecting all bugs");
                let mut reports;
                {
                    let mut deadlock_detector = DeadlockDetector::new(tcx, param_env);
                    reports = deadlock_detector.detect(&callgraph, &mut alias_analysis);
                }
                {
                    let mut atomicity_violation_detector = AtomicityViolationDetector::new(tcx);
                    reports.extend(
                        atomicity_violation_detector
                            .detect(&callgraph, &mut alias_analysis)
                            .into_iter(),
                    );
                }
                {
                    let invalid_free_detector = InvalidFreeDetector::new(tcx);
                    reports.extend(
                        invalid_free_detector
                            .detect(&callgraph, &mut alias_analysis)
                            .into_iter(),
                    );
                }
                {
                    let use_after_free_detector = UseAfterFreeDetector::new(tcx);
                    reports.extend(
                        use_after_free_detector
                            .detect(&callgraph, &mut alias_analysis)
                            .into_iter(),
                    );
                }
                if !reports.is_empty() {
                    let j = serde_json::to_string_pretty(&reports).unwrap();
                    warn!("{}", j);
                    let stats = report_stats(&crate_name, &reports);
                    warn!("{}", stats);
                }
            }
            DetectorKind::Panic => {
                debug!("Detecting panic sites");
                let mut detector = PanicDetector::new(tcx);
                for instance in instances {
                    detector.detect(instance);
                }
                for (i, (k, v)) in detector.result().iter().enumerate() {
                    println!(
                        "PANIC[{}#{}]: {:?}: span[{:?}], outermost_span[{:?}], {:?}",
                        tcx.crate_name(LOCAL_CRATE),
                        i,
                        k,
                        v.0,
                        v.1,
                        v.2
                    );
                }
                for (panic_api, cnt) in detector.statistics() {
                    println!("{}: {:?}: {}", tcx.crate_name(LOCAL_CRATE), panic_api, cnt);
                }
            }
        }
    }
}

fn report_stats(crate_name: &str, reports: &[Report]) -> String {
    let (
        mut doublelock_probably,
        mut doublelock_possibly,
        mut conflictlock_probably,
        mut conflictlock_possibly,
        mut condvar_deadlock_probably,
        mut condvar_deadlock_possibly,
        mut atomicity_violation_possibly,
        mut invalid_free_possibly,
        mut use_after_free_possibly,
    ) = (0, 0, 0, 0, 0, 0, 0, 0, 0);
    for report in reports {
        match report {
            Report::DoubleLock(doublelock) => match doublelock.possibility.as_str() {
                "Probably" => doublelock_probably += 1,
                "Possibly" => doublelock_possibly += 1,
                _ => {}
            },
            Report::ConflictLock(conflictlock) => match conflictlock.possibility.as_str() {
                "Probably" => conflictlock_probably += 1,
                "Possibly" => conflictlock_possibly += 1,
                _ => {}
            },
            Report::CondvarDeadlock(condvar_deadlock) => {
                match condvar_deadlock.possibility.as_str() {
                    "Probably" => condvar_deadlock_probably += 1,
                    "Possibly" => condvar_deadlock_possibly += 1,
                    _ => {}
                }
            }
            Report::AtomicityViolation(_) => {
                atomicity_violation_possibly += 1;
            }
            Report::InvalidFree(_) => {
                invalid_free_possibly += 1;
            }
            Report::UseAfterFree(_) => {
                use_after_free_possibly += 1;
            }
        }
    }
    format!("crate {} contains bugs: {{ probably: {}, possibly: {} }}, conflictlock: {{ probably: {}, possibly: {} }}, condvar_deadlock: {{ probably: {}, possibly: {} }}, atomicity_violation: {{ possibly: {} }}, invalid_free: {{ possibly: {} }}, use_after_free: {{ possibly: {} }}", crate_name, doublelock_probably, doublelock_possibly, conflictlock_probably, conflictlock_possibly, condvar_deadlock_probably, condvar_deadlock_possibly, atomicity_violation_possibly, invalid_free_possibly, use_after_free_possibly)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_report_stats() {
        assert_eq!(report_stats("dummy", &[]), format!("crate {} contains bugs: {{ probably: {}, possibly: {} }}, conflictlock: {{ probably: {}, possibly: {} }}, condvar_deadlock: {{ probably: {}, possibly: {} }}, atomicity_violation: {{ possibly: {} }}, invalid_free: {{ possibly: {} }}, use_after_free: {{ possibly: {} }}", "dummy", 0, 0, 0, 0, 0, 0, 0, 0, 0));
    }
}
