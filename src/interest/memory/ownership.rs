// extern crate rustc_hir;
// extern crate rustc_middle;

use stable_mir::mir::mono::Instance;
// use rustc_hir::def_id::DefId;
// use rustc_middle::ty::TyCtxt;

// use rustc_middle::ty::{GenericArg, List};

/// y = Arc::clone(x) | y = Rc::clone(x)
pub fn is_arc_or_rc_clone(
    instance: &Instance
) -> bool {
    let fn_name = instance.name();
    if fn_name != "std::clone::Clone::clone" {
        return false;
    }
    let args = instance.args();
    let arg_ty_name = format!("{:?}", args);
    println!("{arg_ty_name}");
    if is_arc(&arg_ty_name) || is_rc(&arg_ty_name) {
        return true;
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
pub fn is_ptr_read(instance: &Instance) -> bool {
    instance.name().starts_with("std::ptr::read::<")
}

/// z = <_ as Index<_>>::index(x, y)
#[inline]
pub fn is_index(instance: &Instance) -> bool {
    instance.name().ends_with("::index")
}
