//! Modified from
//! <https://github.com/rust-lang/rust/blob/aedf78e56b2279cc869962feac5153b6ba7001ed/compiler/rustc_borrowck/src/diagnostics/find_all_local_uses.rs>
extern crate rustc_middle;

use std::collections::BTreeSet;

use rustc_middle::mir::visit::Visitor;
use rustc_middle::mir::{Body, Local, Location};

use rustc_middle::mir::visit::{
    MutatingUseContext, NonMutatingUseContext, NonUseContext, PlaceContext,
};

#[derive(Eq, PartialEq, Clone)]
pub enum DefUse {
    Def,
    Use,
    Drop,
}

pub fn categorize(context: PlaceContext) -> Option<DefUse> {
    match context {
        ///////////////////////////////////////////////////////////////////////////
        // DEFS

        PlaceContext::MutatingUse(MutatingUseContext::Store) |

        // We let Call define the result in both the success and
        // unwind cases. This is not really correct, however it
        // does not seem to be observable due to the way that we
        // generate MIR. To do things properly, we would apply
        // the def in call only to the input from the success
        // path and not the unwind path. -nmatsakis
        PlaceContext::MutatingUse(MutatingUseContext::Call) |
        PlaceContext::MutatingUse(MutatingUseContext::AsmOutput) |
        PlaceContext::MutatingUse(MutatingUseContext::Yield) |

        // Storage live and storage dead aren't proper defines, but we can ignore
        // values that come before them.
        PlaceContext::NonUse(NonUseContext::StorageLive) |
        PlaceContext::NonUse(NonUseContext::StorageDead) => Some(DefUse::Def),

        ///////////////////////////////////////////////////////////////////////////
        // REGULAR USES
        //
        // These are uses that occur *outside* of a drop. For the
        // purposes of NLL, these are special in that **all** the
        // lifetimes appearing in the variable must be live for each regular use.

        PlaceContext::NonMutatingUse(NonMutatingUseContext::Projection) |
        PlaceContext::MutatingUse(MutatingUseContext::Projection) |

        // Borrows only consider their local used at the point of the borrow.
        // This won't affect the results since we use this analysis for generators
        // and we only care about the result at suspension points. Borrows cannot
        // cross suspension points so this behavior is unproblematic.
        PlaceContext::MutatingUse(MutatingUseContext::Borrow) |
        PlaceContext::NonMutatingUse(NonMutatingUseContext::SharedBorrow) |
        PlaceContext::NonMutatingUse(NonMutatingUseContext::FakeBorrow) |
        PlaceContext::NonMutatingUse(NonMutatingUseContext::PlaceMention)|

        PlaceContext::MutatingUse(MutatingUseContext::AddressOf) |
        PlaceContext::NonMutatingUse(NonMutatingUseContext::AddressOf) |
        PlaceContext::NonMutatingUse(NonMutatingUseContext::Inspect) |
        PlaceContext::NonMutatingUse(NonMutatingUseContext::Copy) |
        PlaceContext::NonMutatingUse(NonMutatingUseContext::Move) |
        PlaceContext::NonUse(NonUseContext::AscribeUserTy(_)) |
        PlaceContext::MutatingUse(MutatingUseContext::Retag) =>
            Some(DefUse::Use),

        ///////////////////////////////////////////////////////////////////////////
        // DROP USES
        //
        // These are uses that occur in a DROP (a MIR drop, not a
        // call to `std::mem::drop()`). For the purposes of NLL,
        // uses in drop are special because `#[may_dangle]`
        // attributes can affect whether lifetimes must be live.

        PlaceContext::MutatingUse(MutatingUseContext::Drop) =>
            Some(DefUse::Drop),

        // Debug info is neither def nor use.
        PlaceContext::NonUse(NonUseContext::VarDebugInfo) => None,

        PlaceContext::MutatingUse(MutatingUseContext::Deinit | MutatingUseContext::SetDiscriminant) => {
            None
        }
    }
}

pub fn find_uses(body: &Body<'_>, local: Local) -> BTreeSet<Location> {
    let mut visitor = AllLocalUsesVisitor {
        for_local: local,
        uses: BTreeSet::default(),
    };
    visitor.visit_body(body);
    visitor.uses
}

struct AllLocalUsesVisitor {
    for_local: Local,
    uses: BTreeSet<Location>,
}

impl<'tcx> Visitor<'tcx> for AllLocalUsesVisitor {
    fn visit_local(&mut self, local: Local, context: PlaceContext, location: Location) {
        if local == self.for_local {
            if let Some(DefUse::Use) = categorize(context) {
                self.uses.insert(location);
            }
        }
    }
}
