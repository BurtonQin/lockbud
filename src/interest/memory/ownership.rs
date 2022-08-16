extern crate rustc_hir;
extern crate rustc_middle;

use rustc_hir::def_id::DefId;
use rustc_middle::ty::{SubstsRef, TyCtxt};

/// y = Arc::clone(x)
pub fn is_arc_or_rc_clone<'tcx>(def_id: DefId, substs: SubstsRef<'tcx>, tcx: TyCtxt<'tcx>) -> bool {
    let fn_name = tcx.def_path_str(def_id);
    if fn_name != "std::clone::Clone::clone" {
        return false;
    }
    if let &[arg] = substs.as_ref() {
        let arg_ty_name = format!("{:?}", arg);
        if is_arc(&arg_ty_name) || is_rc(&arg_ty_name) {
            return true;
        }
    }
    false
}

#[inline]
pub fn is_arc(arg_ty_name: &str) -> bool {
    arg_ty_name.starts_with("std::sync::Arc<")
}

#[inline]
pub fn is_rc(arg_ty_name: &str) -> bool {
    arg_ty_name.starts_with("std::rc::Rc<")
}

/// y = std::ptr::read::<T>(x)
#[inline]
pub fn is_ptr_read(def_id: DefId, tcx: TyCtxt<'_>) -> bool {
    tcx.def_path_str(def_id).starts_with("std::ptr::read::<")
}
