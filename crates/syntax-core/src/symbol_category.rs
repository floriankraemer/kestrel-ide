//! [`SymbolCategory`] and [`SymbolKind::category`] (Task 4b) — split out of
//! `lib.rs` to keep it under the file-size gate's grandfathered ceiling
//! rather than for any conceptual reason; `SymbolCategory` is re-exported
//! from `lib.rs` so callers see it as if it lived there.

use crate::SymbolKind;

/// The fixed-order group (Task 4b) a [`SymbolKind`] falls into for Class
/// View's sub-grouping: the view creates one group node per
/// `SymbolCategory` actually present among a symbol's children, in this
/// enum's declaration order, and nests the real symbol items under their
/// group instead of directly under the parent. A business rule, so it
/// lives here rather than in `ui-shell`'s C++ (CLAUDE.md's hard layering
/// rule) — the view only groups by the ordinal [`SymbolKind::category`]
/// hands it, never by the kind itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolCategory {
    Constants,
    Fields,
    Properties,
    Constructors,
    Methods,
    NestedTypes,
    Other,
}

impl SymbolKind {
    /// This kind's fixed Structure group (Task 4b). `Function` joins
    /// `Method` under `Methods`: several languages (Rust's `impl` blocks,
    /// Go, Kotlin) have one grammar node for both free functions and
    /// methods, and a function nested under a container reads as a method
    /// regardless of which node produced it. `EnumMember` joins `Constant`
    /// under `Constants`: an enum member is a named constant value, the
    /// same shape a `const`/`static` item is.
    pub fn category(self) -> SymbolCategory {
        match self {
            SymbolKind::Constant | SymbolKind::EnumMember => SymbolCategory::Constants,
            SymbolKind::Field => SymbolCategory::Fields,
            SymbolKind::Property => SymbolCategory::Properties,
            SymbolKind::Constructor => SymbolCategory::Constructors,
            SymbolKind::Method | SymbolKind::Function => SymbolCategory::Methods,
            SymbolKind::Class | SymbolKind::Struct | SymbolKind::Enum | SymbolKind::Interface => {
                SymbolCategory::NestedTypes
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_category_groups_kinds_in_the_fixed_order() {
        assert_eq!(SymbolKind::Constant.category(), SymbolCategory::Constants);
        assert_eq!(SymbolKind::EnumMember.category(), SymbolCategory::Constants);
        assert_eq!(SymbolKind::Field.category(), SymbolCategory::Fields);
        assert_eq!(SymbolKind::Property.category(), SymbolCategory::Properties);
        assert_eq!(
            SymbolKind::Constructor.category(),
            SymbolCategory::Constructors
        );
        assert_eq!(SymbolKind::Method.category(), SymbolCategory::Methods);
        assert_eq!(SymbolKind::Function.category(), SymbolCategory::Methods);
        assert_eq!(SymbolKind::Class.category(), SymbolCategory::NestedTypes);
        assert_eq!(SymbolKind::Struct.category(), SymbolCategory::NestedTypes);
        assert_eq!(SymbolKind::Enum.category(), SymbolCategory::NestedTypes);
        assert_eq!(
            SymbolKind::Interface.category(),
            SymbolCategory::NestedTypes
        );
    }
}
