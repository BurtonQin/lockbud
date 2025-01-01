//! Find atomic functions and classify them into read, write, read-write.
// extern crate rustc_hir;
// extern crate rustc_middle;

use once_cell::sync::Lazy;
use regex::Regex;

use rustc_hash::FxHashMap;
use stable_mir::mir::mono::Instance;

static ATOMIC_API_REGEX: Lazy<FxHashMap<&'static str, Regex>> = Lazy::new(|| {
    macro_rules! atomic_api_prefix {
        () => {
            r"^(std|core)::sync::atomic[:a-zA-Z0-9]*::"
        };
    }
    let mut m = FxHashMap::default();
    m.insert(
        "AtomicRead",
        Regex::new(std::concat!(atomic_api_prefix!(), r"load")).unwrap(),
    );
    m.insert(
        "AtomicWrite",
        Regex::new(std::concat!(atomic_api_prefix!(), r"store")).unwrap(),
    );
    m.insert(
        "AtomicReadWrite",
        Regex::new(std::concat!(
            atomic_api_prefix!(),
            r"(compare|fetch)_[a-zA-Z0-9]*"
        ))
        .unwrap(),
    );
    m
});

#[cfg(test)]
mod tests {
    use super::ATOMIC_API_REGEX;
    #[test]
    fn test_atomic_api_regex() {
        assert!(ATOMIC_API_REGEX["AtomicRead"].is_match("std::sync::atomic::AtomicUsize::load"));
        assert!(ATOMIC_API_REGEX["AtomicWrite"].is_match("std::sync::atomic::AtomicUsize::store"));
        assert!(ATOMIC_API_REGEX["AtomicReadWrite"]
            .is_match("std::sync::atomic::AtomicUsize::compare_and_swap"));
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum AtomicApi {
    Read,
    Write,
    ReadWrite,
}

impl AtomicApi {
    pub fn from_instance(instance: &Instance) -> Option<Self> {
        let path = instance.name();
        if ATOMIC_API_REGEX["AtomicRead"].is_match(&path) {
            Some(AtomicApi::Read)
        } else if ATOMIC_API_REGEX["AtomicWrite"].is_match(&path) {
            Some(AtomicApi::Write)
        } else if ATOMIC_API_REGEX["AtomicReadWrite"].is_match(&path) {
            Some(AtomicApi::ReadWrite)
        } else {
            None
        }
    }
}

// AtomicPtr::store(&self, ptr: *mut T, order: Ordering)
// Alias: self = ptr
static ATOMIC_PTR_STORE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(std|core)::sync::atomic::AtomicPtr::<.*>::store").unwrap());

pub fn is_atomic_ptr_store<'tcx>(
    instance: &Instance
) -> bool {
    let path = instance.name();
    ATOMIC_PTR_STORE.is_match(&path)
}

#[cfg(test)]
mod tests2 {
    use super::*;

    #[test]
    fn test_atomic_ptr_store() {
        assert!(ATOMIC_PTR_STORE.is_match("std::sync::atomic::AtomicPtr::<T>::store"));
        assert!(!ATOMIC_PTR_STORE.is_match("std::sync::atomic::AtomicUsize::store"));
        assert!(!ATOMIC_PTR_STORE.is_match("std::sync::atomic::AtomicPtr::<T>::load"));
    }
}
