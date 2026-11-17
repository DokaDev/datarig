//! A seam module may name the formats, but may not send the unnamed ones to a default.
use crate::driver::ExplainFormat;

impl ExplainFormat {
    /// Whether the plan is text.
    pub fn is_text(self) -> bool {
        match self {
            ExplainFormat::MySqlTree => true,
            _ => false,
        }
    }
}
