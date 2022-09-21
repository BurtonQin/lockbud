//! uninitialize:
//! 1. _1 = uninitialized::<Vec<i32>>() -> bb1;
//! 2. _1 = MaybeUninit::<Vec<i32>>::uninit() -> bb1;
//! 3. _2 = MaybeUninit::<Vec<i32>>::assume_init(move _1) -> bb4;
//! initialize:
//! 1. _2 = MaybeUninit::<Vec<i32>>::write(move _3, move _4) -> bb3;
//! 2. _2 = MaybeUninit::<Obj>::as_mut_ptr(move _3) -> bb2;
//! 3. _5 = &raw mut ((*_2).0: std::vec::Vec<i32>);
//! 4. _4 = ptr::mut_ptr::<impl *mut Vec<i32>>::write(move _5, move _6) -> bb4;
extern crate rustc_data_structures;
extern crate rustc_middle;

use once_cell::sync::Lazy;
use regex::Regex;

use rustc_data_structures::fx::FxHashMap;
use rustc_middle::ty::{Instance, TyCtxt};

static UNINIT_API_REGEX: Lazy<FxHashMap<UninitApi, Regex>> = Lazy::new(|| {
    use UninitApi::*;

    let mut m = FxHashMap::default();
    m.insert(
        MaybeUninit,
        Regex::new(r"^(std|core)::mem::MaybeUninit::<.*>::uninit").unwrap(),
    );
    m.insert(
        Uninitialized,
        Regex::new(r"^(std|core)::mem::uninitialized::<.*>").unwrap(),
    );
    m.insert(
        MaybeUninitWrite,
        Regex::new(r"^(std|core)::mem::MaybeUninit::<.*>::write").unwrap(),
    );
    m.insert(
        PtrWrite,
        Regex::new(r"^(std|core)::mem::MaybeUninit::<.*>::as_mut_ptr").unwrap(),
    );
    m.insert(
        AssumeInit,
        Regex::new(r"^(std|core)::mem::MaybeUninit::<.*>::assume_init(_mut)?").unwrap(),
    );
    m
});

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum UninitApi {
    Uninitialized,
    MaybeUninit,
    AssumeInit,
    MaybeUninitWrite,
    PtrWrite,
}

impl UninitApi {
    pub fn from_instance<'tcx>(instance: Instance<'tcx>, tcx: TyCtxt<'tcx>) -> Option<Self> {
        let path = tcx.def_path_str_with_substs(instance.def_id(), instance.substs);
        Self::from_str(&path)
    }

    #[inline]
    fn from_str(path: &str) -> Option<Self> {
        for (k, v) in UNINIT_API_REGEX.iter() {
            if v.is_match(path) {
                return Some(*k);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uninit_api() {
        use UninitApi::*;
        assert_eq!(
            MaybeUninit,
            UninitApi::from_str("std::mem::MaybeUninit::<std::vec::Vec<i32>>::uninit").unwrap()
        );
        assert_eq!(
            Uninitialized,
            UninitApi::from_str("std::mem::uninitialized::<std::vec::Vec<i32>>").unwrap()
        );
        assert_eq!(
            MaybeUninitWrite,
            UninitApi::from_str("std::mem::MaybeUninit::<std::vec::Vec<i32>>::write").unwrap()
        );
        assert_eq!(
            PtrWrite,
            UninitApi::from_str("std::mem::MaybeUninit::<std::vec::Vec<i32>>::as_mut_ptr").unwrap()
        );
        assert_eq!(
            AssumeInit,
            UninitApi::from_str("std::mem::MaybeUninit::<std::vec::Vec<i32>>::assume_init")
                .unwrap()
        );
        assert_eq!(
            AssumeInit,
            UninitApi::from_str("std::mem::MaybeUninit::<std::vec::Vec<i32>>::assume_init_mut")
                .unwrap()
        );
        assert_eq!(
            AssumeInit,
            UninitApi::from_str("std::mem::MaybeUninit::<assume_ptr_write_fp::Obj>::assume_init")
                .unwrap()
        );
    }
}
