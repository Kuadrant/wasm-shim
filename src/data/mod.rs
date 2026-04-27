pub mod attribute;
pub mod cel;
mod headers;

pub use cel::{populate_ctx_with_request_attributes, Expression};
pub use headers::Headers;

#[cfg(feature = "debug-host-behaviour")]
pub use cel::debug_all_well_known_attributes;
