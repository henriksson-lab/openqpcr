//! Error types for openqpcr.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum QpcrError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("zip archive error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("XML error: {0}")]
    Xml(#[from] quick_xml::Error),

    #[error("CSV error: {0}")]
    Csv(#[from] csv::Error),

    #[error("spreadsheet error: {0}")]
    Xlsx(#[from] calamine::Error),

    /// The `.pcrd` archive appears to be encrypted (Security Edition).
    #[error(
        "this .pcrd file looks encrypted (Bio-Rad Security Edition); native parsing is not possible — use CFX exports instead"
    )]
    Encrypted,

    /// We could open the container but did not recognise its internal layout.
    #[error("unrecognised file layout: {0}")]
    UnknownLayout(String),

    #[error("unsupported or unrecognised file format: {0}")]
    UnsupportedFormat(String),

    #[error("{0}")]
    Parse(String),
}

pub type Result<T> = std::result::Result<T, QpcrError>;
