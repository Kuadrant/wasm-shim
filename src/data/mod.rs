mod attribute;
mod cel;
mod property;

pub use attribute::get_attribute;
pub use attribute::store_metadata;

#[cfg(feature = "debug-host-behaviour")]
pub use cel::debug_all_well_known_attributes;

pub use cel::errors::EvaluationError;
pub use cel::Attribute;
pub use cel::AttributeOwner;
pub use cel::AttributeResolver;
pub use cel::Expression;
pub use cel::PathCache;
pub use cel::Predicate;
pub use cel::PredicateResult;
pub use cel::PredicateVec;

pub use crate::v2::data::attribute::Path as PropertyPath;
pub use crate::v2::data::attribute::PropError;
pub use crate::v2::data::attribute::PropertyError;
