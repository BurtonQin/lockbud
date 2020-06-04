use std::env;
pub enum CrateNameLists {
    White(Vec<String>),
    Black(Vec<String>),
}

pub enum LockDetectorType {
    DoubleLockDetector,
    ConflictLockDetector,
}

pub struct LockDetectorConfig {
    pub lock_detector_type: LockDetectorType,
    pub crate_name_lists: CrateNameLists,
}

impl LockDetectorConfig {
    pub fn from_env() -> Result<Self, &'static str> {
        let lock_detector_type = "RUST_LOCK_DETECTOR_TYPE";
        let black_crate_name_lists = "RUST_LOCK_DETECTOR_BLACK_LISTS";
        let white_crate_name_lists = "RUST_LOCK_DETECTOR_WHITE_LISTS";
        let lock_detector_type = match env::var(lock_detector_type) {
            Ok(detector) => {
                if &detector == "DoubleLockDetector" {
                    LockDetectorType::DoubleLockDetector
                } else if &detector == "ConflictLockDetector" {
                    LockDetectorType::ConflictLockDetector
                } else {
                    return Err("Env var \"RUST_LOCK_DETECTOR_TYPE\" is not set or provided with wrong value.\nPlease set it to \"DoubleLockDetector\" or \"ConflictLockDetector\"");
                }
            },
            Err(_) => return Err("Env var \"RUST_LOCK_DETECTOR_TYPE\" is not set or provided with wrong value.\nPlease set it to \"DoubleLockDetector\" or \"ConflictLockDetector\""),
        };
        let black_crate_name_lists: Vec<String> = match env::var(black_crate_name_lists) {
            Ok(black_crate_name_lists) => black_crate_name_lists
                .split(',')
                .map(|s| s.to_string())
                .collect(),
            Err(_) => Vec::new(),
        };
        let white_crate_name_lists: Vec<String> = match env::var(white_crate_name_lists) {
            Ok(white_crate_name_lists) => white_crate_name_lists
                .split(',')
                .map(|s| s.to_string())
                .collect(),
            Err(_) => Vec::new(),
        };
        if !black_crate_name_lists.is_empty() && !white_crate_name_lists.is_empty() {
            Err("Env var \"RUST_LOCK_DETECTOR_BLACK_LISTS\" and \"RUST_LOCK_DETECTOR_WHITE_LISTS\" are \nboth provided values. Please clear the values in one of them")
        } else if !black_crate_name_lists.is_empty() {
            Ok(Self {
                lock_detector_type,
                crate_name_lists: CrateNameLists::Black(black_crate_name_lists),
            })
        } else {
            Ok(Self {
                lock_detector_type,
                crate_name_lists: CrateNameLists::White(white_crate_name_lists),
            })
        }
    }
}

/// limit the callchain depth when doing inter-procedural analysis
pub const CALLCHAIN_DEPTH: usize = 4;

/// limit the GenKill iteration inside one function
pub const RUN_LIMIT: u32 = 10000;
