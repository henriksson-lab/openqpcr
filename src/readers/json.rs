//! Reader for openqpcr's native JSON save format.
//!
//! The whole [`QpcrRun`] model derives `Serialize`/`Deserialize`, so a plate
//! saved with `serde_json::to_string_pretty(&run)` round-trips losslessly back
//! through [`read_json`]. This is the native editable save format.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use crate::error::{QpcrError, Result};
use crate::model::QpcrRun;

/// Parse an openqpcr JSON save file into a [`QpcrRun`].
pub fn read_json(path: &Path) -> Result<QpcrRun> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    serde_json::from_reader(reader)
        .map_err(|e| QpcrError::Parse(format!("parsing JSON {}: {e}", path.display())))
}
