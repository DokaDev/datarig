//! A re-export under another name, which other files would use without the scan seeing it.
pub use dialect::Dialect as Lang;

/// The same through a type alias.
pub type Plan = crate::driver::ExplainFormat;
