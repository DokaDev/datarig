//! The files [`super::ParquetWriter`] writes, read back for tests with the parquet crate's own
//! reader: the schema, the row groups and every row's values.

use parquet::basic::{LogicalType, Type as PhysicalType};
use parquet::file::reader::{FileReader, SerializedFileReader};
pub use parquet::record::Field;

/// A column of the schema: its name, physical and logical type, and whether it may be NULL.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub name: String,
    pub physical: PhysicalType,
    pub logical: Option<LogicalType>,
    pub optional: bool,
}

/// A file read back.
#[derive(Clone, Debug, PartialEq)]
pub struct File {
    pub columns: Vec<Column>,
    /// The rows of each row group.
    pub groups: Vec<i64>,
    /// Every row's values, in order.
    pub rows: Vec<Vec<Field>>,
}

/// The file in `bytes`, or what the reader said of it.
pub fn file(bytes: &[u8]) -> Result<File, String> {
    let reader = SerializedFileReader::new(bytes::Bytes::copy_from_slice(bytes)).map_err(|e| e.to_string())?;
    let meta = reader.metadata();
    let columns = meta
        .file_metadata()
        .schema_descr()
        .columns()
        .iter()
        .map(|c| Column {
            name: c.name().to_string(),
            physical: c.physical_type(),
            logical: c.logical_type_ref().cloned(),
            optional: c.max_def_level() == 1,
        })
        .collect();
    let groups = meta.row_groups().iter().map(|g| g.num_rows()).collect();
    let rows = reader
        .get_row_iter(None)
        .map_err(|e| e.to_string())?
        .map(|r| r.map(|r| r.into_columns().into_iter().map(|(_, f)| f).collect()).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    Ok(File { columns, groups, rows })
}
