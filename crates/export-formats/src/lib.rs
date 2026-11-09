//! The file formats of datarig's export that need a library of their own: an Excel workbook
//! ([`xlsx`]) and a Parquet file ([`parquet`]). Each is a [`RowSink`] that the export job feeds
//! row by row, as it does for the text formats of `datarig_core::export`.
//!
//! [`RowSink`]: datarig_core::export::stream::RowSink

pub mod parquet;
pub mod xlsx;
