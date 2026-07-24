//! Reader for CFX Maestro / CFX Manager **exports** (CSV and Excel).
//!
//! CFX's "Export All Data Sheets" produces a family of tables. This reader
//! recognises each table by the shape of its header and merges them into one
//! [`QpcrRun`]:
//!
//! | Table                                   | Header shape                              | Populates                    |
//! |-----------------------------------------|-------------------------------------------|------------------------------|
//! | Quantification Amplification Results    | `…, Cycle, A1, A2, …` (wide, per-cycle)   | `Channel::amplification`     |
//! | Quantification Cq Results               | `Well, Fluor, Target, Content, Sample, Cq…`| `Channel::cq`, sample info  |
//! | Melt Curve RFU Results                   | `…, Temperature, A1, A2, …` (wide)        | `MeltCurve::rfu`             |
//! | Melt Curve Derivative Results            | `…, Temperature, A1, A2, …` (wide)        | `MeltCurve::derivative`      |
//! | Melt Curve Peak Results                  | `Well, …, Melt Temperature, …`            | `MeltCurve::peaks`           |
//! | Plate layout / Run Information           | `Well, …, Sample, Target`                 | sample / target / type       |
//!
//! Fluorophore identity for the wide tables comes from the file/sheet name
//! (`…Results_FAM.csv`), since those tables don't repeat it per column.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;

use calamine::{Data, Reader};

use crate::error::{QpcrError, Result};
use crate::model::{
    Channel, MeltCurve, PlateFormat, QpcrRun, RunMetadata, SampleType, Well, parse_well_label,
};

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Parse a single CFX export CSV into a (typically partial) [`QpcrRun`].
pub fn read_csv(path: &Path) -> Result<QpcrRun> {
    let rows = read_csv_rows(path)?;
    let fluor = fluor_from_name(path);
    let mut builder = RunBuilder::new();
    builder.metadata.source_file = Some(path.display().to_string());
    builder.ingest_table(&rows, fluor.as_deref(), &table_label(path))?;
    if !builder.has_wells() {
        return Err(QpcrError::UnsupportedFormat(format!(
            "no data-bearing CFX export table in {}",
            path.display()
        )));
    }
    Ok(builder.finish())
}

/// Parse an Excel workbook (`.xlsx`/`.xls`) of CFX exports into one [`QpcrRun`],
/// classifying each sheet by its header.
pub fn read_xlsx(path: &Path) -> Result<QpcrRun> {
    let mut workbook = calamine::open_workbook_auto(path)
        .map_err(|e| QpcrError::Parse(format!("opening {}: {e}", path.display())))?;
    let mut builder = RunBuilder::new();
    builder.metadata.source_file = Some(path.display().to_string());

    let sheet_names = workbook.sheet_names().to_vec();
    let mut found = false;
    let mut errors = Vec::new();
    for name in sheet_names {
        let range = match workbook.worksheet_range(&name) {
            Ok(r) => r,
            Err(e) => {
                errors.push(format!("{name}: {e}"));
                continue;
            }
        };
        let rows: Vec<Vec<String>> = range
            .rows()
            .map(|r| r.iter().map(cell_to_string).collect())
            .collect();
        if rows.is_empty() {
            continue;
        }
        let fluor = fluor_from_str(&name);
        match builder.ingest_table(&rows, fluor.as_deref(), &name) {
            Ok(_) => found = true,
            Err(e) if is_optional_empty_table(&name, &rows, &e) => {}
            Err(e) => errors.push(format!("{name}: {e}")),
        }
    }
    if !errors.is_empty() {
        return Err(QpcrError::Parse(format!(
            "failed to parse one or more CFX export sheets in {}:\n{}",
            path.display(),
            errors.join("\n")
        )));
    }
    if !found || !builder.has_wells() {
        return Err(QpcrError::UnsupportedFormat(format!(
            "no data-bearing CFX export sheets in {}",
            path.display()
        )));
    }
    Ok(builder.finish())
}

/// Parse every `.csv` in a directory (a full "Export All Data Sheets" dump) into
/// one merged [`QpcrRun`].
pub fn read_export_dir(dir: &Path) -> Result<QpcrRun> {
    let mut builder = RunBuilder::new();
    builder.metadata.source_file = Some(dir.display().to_string());
    let mut found = false;
    let mut errors = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("csv"))
            != Some(true)
        {
            continue;
        }
        let rows = read_csv_rows(&path)?;
        let fluor = fluor_from_name(&path);
        let label = table_label(&path);
        match builder.ingest_table(&rows, fluor.as_deref(), &label) {
            Ok(_) => found = true,
            Err(e) if is_optional_empty_table(&label, &rows, &e) => {}
            Err(e) => errors.push(format!("{}: {e}", path.display())),
        }
    }
    if !errors.is_empty() {
        return Err(QpcrError::Parse(format!(
            "failed to parse one or more CFX export CSVs in {}:\n{}",
            dir.display(),
            errors.join("\n")
        )));
    }
    if !found || !builder.has_wells() {
        return Err(QpcrError::UnsupportedFormat(format!(
            "no data-bearing CFX export CSVs in {}",
            dir.display()
        )));
    }
    Ok(builder.finish())
}

// ---------------------------------------------------------------------------
// Table classification & ingestion
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TableKind {
    Amplification,
    MeltRfu,
    MeltDerivative,
    CqResults,
    MeltPeaks,
    PlateLayout,
    RunInfo,
    Unknown,
}

struct RunBuilder {
    metadata: RunMetadata,
    plate: Option<PlateFormat>,
    wells: BTreeMap<(u8, u8), Well>,
}

impl RunBuilder {
    fn new() -> Self {
        RunBuilder {
            metadata: RunMetadata::default(),
            plate: None,
            wells: BTreeMap::new(),
        }
    }

    fn well_mut(&mut self, row: u8, col: u8) -> &mut Well {
        self.wells.entry((row, col)).or_insert_with(|| Well {
            row,
            col,
            ..Default::default()
        })
    }

    fn channel_mut(&mut self, row: u8, col: u8, fluor: &str) -> &mut Channel {
        let well = self.well_mut(row, col);
        if let Some(idx) = well.channels.iter().position(|c| c.fluorophore == fluor) {
            &mut well.channels[idx]
        } else {
            well.channels.push(Channel {
                fluorophore: fluor.to_string(),
                ..Default::default()
            });
            well.channels.last_mut().unwrap()
        }
    }

    fn note_well(&mut self, row: u8, col: u8) {
        let inferred = if row >= 8 || col >= 12 {
            PlateFormat::P384
        } else {
            PlateFormat::P96
        };
        if self
            .plate
            .is_none_or(|p| p.well_count() < inferred.well_count())
        {
            self.plate = Some(inferred);
        }
    }

    fn has_wells(&self) -> bool {
        !self.wells.is_empty()
    }

    fn finish(mut self) -> QpcrRun {
        // Infer plate format from the largest well coordinates observed.
        let plate = self.plate.unwrap_or_else(|| {
            let max_row = self.wells.keys().map(|(r, _)| *r).max().unwrap_or(7);
            let max_col = self.wells.keys().map(|(_, c)| *c).max().unwrap_or(11);
            if max_row >= 8 || max_col >= 12 {
                PlateFormat::P384
            } else {
                PlateFormat::P96
            }
        });
        // Number of amplification cycles seen anywhere → metadata.
        if self.metadata.cycle_count.is_none() {
            let cyc = self
                .wells
                .values()
                .flat_map(|w| w.channels.iter())
                .map(|c| c.amplification.len())
                .max()
                .unwrap_or(0);
            if cyc > 0 {
                self.metadata.cycle_count = Some(cyc);
            }
        }
        QpcrRun {
            metadata: self.metadata,
            plate,
            wells: self.wells.into_values().collect(),
            protocol: None,
        }
    }

    fn ingest_table(
        &mut self,
        rows: &[Vec<String>],
        fluor_hint: Option<&str>,
        label: &str,
    ) -> Result<()> {
        let Some(header) = rows.iter().find(|r| r.iter().any(|c| !c.trim().is_empty())) else {
            return Err(QpcrError::UnsupportedFormat(format!(
                "empty export table: {label}"
            )));
        };
        let header_idx = rows
            .iter()
            .position(|r| std::ptr::eq(r, header))
            .unwrap_or(0);
        let body = &rows[header_idx + 1..];
        let kind = classify(header, label);
        match kind {
            TableKind::Amplification => self.ingest_matrix(header, body, fluor_hint, Axis::Cycle),
            TableKind::MeltRfu => self.ingest_matrix(header, body, fluor_hint, Axis::MeltRfu),
            TableKind::MeltDerivative => {
                self.ingest_matrix(header, body, fluor_hint, Axis::MeltDerivative)
            }
            TableKind::CqResults => self.ingest_cq(header, body),
            TableKind::MeltPeaks => self.ingest_peaks(header, body),
            TableKind::PlateLayout => self.ingest_layout(header, body),
            TableKind::RunInfo => {
                self.ingest_run_info(rows);
                Ok(())
            }
            TableKind::Unknown => Err(QpcrError::UnsupportedFormat(format!(
                "unrecognised export table: {label}"
            ))),
        }
    }

    fn ingest_matrix(
        &mut self,
        header: &[String],
        body: &[Vec<String>],
        fluor_hint: Option<&str>,
        axis: Axis,
    ) -> Result<()> {
        let axis_name = axis.axis_name();
        let axis_col = find_col(header, &[axis_name]).ok_or_else(|| {
            QpcrError::Parse(format!("matrix table missing '{axis_name}' column"))
        })?;
        // Every column whose header is a well label is a data column.
        let mut well_cols: Vec<(usize, (u8, u8))> = Vec::new();
        for (i, h) in header.iter().enumerate() {
            if let Some(rc) = parse_cfx_well_label(h)? {
                well_cols.push((i, rc));
            }
        }
        if well_cols.is_empty() {
            return Err(QpcrError::Parse("matrix table has no well columns".into()));
        }
        let fluor = fluor_hint.unwrap_or("Unknown").to_string();

        // Collect (axis_value, per-well value) then sort by axis for stable order.
        let mut records: Vec<(f64, Vec<Option<f64>>)> = Vec::with_capacity(body.len());
        for row in body {
            let Some(av) = row.get(axis_col).and_then(|s| parse_num(s)) else {
                continue;
            };
            let vals = well_cols
                .iter()
                .map(|(i, _)| row.get(*i).and_then(|s| parse_num(s)))
                .collect();
            records.push((av, vals));
        }
        if records.is_empty() {
            return Err(QpcrError::Parse(format!(
                "{axis_name} matrix table has no data rows"
            )));
        }
        records.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        let mut data_columns = 0usize;
        for (j, (_, rc)) in well_cols.iter().enumerate() {
            if records.iter().all(|(_, v)| v[j].is_none()) {
                continue;
            }
            data_columns += 1;
            let (r, c) = *rc;
            self.note_well(r, c);
            let series: Vec<f64> = records
                .iter()
                .map(|(_, v)| v[j].unwrap_or(f64::NAN))
                .collect();
            let axis_vals: Vec<f64> = records.iter().map(|(a, _)| *a).collect();
            let ch = self.channel_mut(r, c, &fluor);
            match axis {
                Axis::Cycle => ch.amplification = series,
                Axis::MeltRfu => {
                    let m = ch.melt.get_or_insert_with(MeltCurve::default);
                    merge_melt_temperature(&mut m.temperature, &axis_vals)?;
                    m.rfu = series;
                }
                Axis::MeltDerivative => {
                    let m = ch.melt.get_or_insert_with(MeltCurve::default);
                    merge_melt_temperature(&mut m.temperature, &axis_vals)?;
                    m.derivative = series;
                }
            }
        }
        if data_columns == 0 {
            return Err(QpcrError::Parse(format!(
                "{axis_name} matrix table has no populated well columns"
            )));
        }
        Ok(())
    }

    fn ingest_cq(&mut self, header: &[String], body: &[Vec<String>]) -> Result<()> {
        let well_c = find_col(header, &["well"]);
        let fluor_c = find_col(header, &["fluor", "fluorophore", "dye"]);
        let target_c = find_col(header, &["target", "gene"]);
        let content_c = find_col(header, &["content", "type"]);
        let sample_c = find_col(header, &["sample", "sample name"]);
        let cq_c = find_col(header, &["cq", "ct", "cq value"]);
        let sq_c = find_col(
            header,
            &["starting quantity (sq)", "starting quantity", "sq"],
        );
        let bio_c = find_col(
            header,
            &["biological set name", "biological group", "sample group"],
        );

        let mut data_rows = 0usize;
        for row in body {
            let Some((r, c)) = well_c
                .and_then(|i| row.get(i))
                .map(|s| parse_cfx_well_label(s))
                .transpose()?
                .flatten()
            else {
                continue;
            };

            let fluor = cell(row, fluor_c);
            let target = cell(row, target_c);
            let cq = cq_c.and_then(|i| row.get(i)).and_then(|s| parse_num(s));
            let sample = cell(row, sample_c);
            let content = cell(row, content_c);
            let bio = cell(row, bio_c);
            let sq = sq_c.and_then(|i| row.get(i)).and_then(|s| parse_num(s));
            let has_channel_data = target.is_some() || cq.is_some();
            let has_well_data =
                sample.is_some() || content.is_some() || bio.is_some() || sq.is_some();
            if !has_channel_data && !has_well_data {
                continue;
            }

            data_rows += 1;
            self.note_well(r, c);

            // Well-level fields.
            if let Some(s) = sample {
                self.well_mut(r, c).sample = Some(s);
            }
            if let Some(s) = content {
                self.well_mut(r, c).sample_type = SampleType::parse(&s);
            }
            if let Some(s) = bio {
                self.well_mut(r, c).biological_group = Some(s);
            }
            if let Some(sq) = sq {
                self.well_mut(r, c).starting_quantity = Some(sq);
            }

            // Channel-level fields.
            if has_channel_data {
                let fluor = fluor.unwrap_or_else(|| "Unknown".to_string());
                let ch = self.channel_mut(r, c, &fluor);
                if target.is_some() {
                    ch.target = target;
                }
                if cq.is_some() {
                    ch.cq = cq;
                }
            }
        }
        if data_rows == 0 {
            return Err(QpcrError::Parse(
                "Cq results table has no data rows".to_string(),
            ));
        }
        Ok(())
    }

    fn ingest_peaks(&mut self, header: &[String], body: &[Vec<String>]) -> Result<()> {
        let well_c = find_col(header, &["well"])
            .ok_or_else(|| QpcrError::Parse("melt peaks table missing Well column".to_string()))?;
        let fluor_c = find_col(header, &["fluor", "fluorophore", "dye"]);
        let tm_c = find_col(header, &["melt temperature", "tm", "peak"]).ok_or_else(|| {
            QpcrError::Parse("melt peaks table missing Melt Temperature column".to_string())
        })?;

        let mut valid_well_rows = 0usize;
        let mut data_rows = 0usize;
        for row in body {
            let Some((r, c)) = row
                .get(well_c)
                .map(|s| parse_cfx_well_label(s))
                .transpose()?
                .flatten()
            else {
                continue;
            };
            valid_well_rows += 1;
            let tm_cell = row.get(tm_c).map(|s| s.trim()).unwrap_or("");
            if tm_cell.is_empty() {
                continue;
            }
            let Some(tm) = parse_num(tm_cell) else {
                return Err(QpcrError::Parse(format!(
                    "melt peaks table has invalid Melt Temperature value {tm_cell:?}"
                )));
            };
            {
                data_rows += 1;
                self.note_well(r, c);
                let fluor = fluor_c
                    .and_then(|i| row.get(i))
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "Unknown".to_string());
                let ch = self.channel_mut(r, c, &fluor);
                ch.melt
                    .get_or_insert_with(MeltCurve::default)
                    .peaks
                    .push(tm);
            }
        }
        if valid_well_rows == 0 {
            return Err(QpcrError::Parse(
                "melt peaks table has no valid Well rows".to_string(),
            ));
        }
        if data_rows == 0 {
            return Err(QpcrError::Parse(
                "melt peaks table has no data rows".to_string(),
            ));
        }
        Ok(())
    }

    fn ingest_layout(&mut self, header: &[String], body: &[Vec<String>]) -> Result<()> {
        let well_c = find_col(header, &["well"]).ok_or_else(|| {
            QpcrError::Parse("plate layout table missing Well column".to_string())
        })?;
        let fluor_c = find_col(header, &["fluor", "fluorophore", "dye"]);
        let target_c = find_col(header, &["target", "gene"]);
        let content_c = find_col(header, &["content", "type"]);
        let sample_c = find_col(header, &["sample", "sample name"]);
        if sample_c.is_none() && content_c.is_none() && target_c.is_none() {
            return Err(QpcrError::Parse(
                "plate layout table missing Sample, Content, or Target column".to_string(),
            ));
        }

        let mut valid_well_rows = 0usize;
        let mut data_rows = 0usize;
        for row in body {
            let Some((r, c)) = row
                .get(well_c)
                .map(|s| parse_cfx_well_label(s))
                .transpose()?
                .flatten()
            else {
                continue;
            };
            valid_well_rows += 1;
            let sample = cell(row, sample_c);
            let content = cell(row, content_c);
            let target = cell(row, target_c);
            if sample.is_none() && content.is_none() && target.is_none() {
                continue;
            }

            data_rows += 1;
            self.note_well(r, c);
            if let Some(s) = sample {
                self.well_mut(r, c).sample = Some(s);
            }
            if let Some(s) = content {
                self.well_mut(r, c).sample_type = SampleType::parse(&s);
            }
            if let Some(target) = target {
                let fluor = fluor_c
                    .and_then(|i| row.get(i))
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "Unknown".to_string());
                self.channel_mut(r, c, &fluor).target = Some(target);
            }
        }
        if valid_well_rows == 0 {
            return Err(QpcrError::Parse(
                "plate layout table has no valid Well rows".to_string(),
            ));
        }
        if data_rows == 0 {
            return Err(QpcrError::Parse(
                "plate layout table has no data rows".to_string(),
            ));
        }
        Ok(())
    }

    fn ingest_run_info(&mut self, rows: &[Vec<String>]) {
        for row in rows {
            for window in row.windows(2) {
                let key = window[0].trim().to_ascii_lowercase();
                let value = window[1].trim();
                if key.contains("instrument") && self.metadata.instrument.is_none() {
                    self.metadata.instrument = non_empty(value);
                } else if key.contains("serial") && self.metadata.serial_number.is_none() {
                    self.metadata.serial_number = non_empty(value);
                } else if key.contains("software") && self.metadata.software_version.is_none() {
                    self.metadata.software_version = non_empty(value);
                } else if (key.contains("started") || key.contains("start time"))
                    && self.metadata.run_started.is_none()
                {
                    self.metadata.run_started = non_empty(value);
                } else if (key.contains("ended") || key.contains("end time"))
                    && self.metadata.run_ended.is_none()
                {
                    self.metadata.run_ended = non_empty(value);
                } else if key.contains("operator") && self.metadata.operator.is_none() {
                    self.metadata.operator = non_empty(value);
                } else if key.contains("plate id") && self.metadata.plate_id.is_none() {
                    self.metadata.plate_id = non_empty(value);
                } else if key.contains("plate type") && self.metadata.plate_type.is_none() {
                    self.metadata.plate_type = non_empty(value);
                } else if key.contains("plate") && key.contains("format") {
                    self.note_plate_text(value);
                }
            }
            for cell in row {
                self.note_plate_text(cell);
            }
        }
    }

    fn note_plate_text(&mut self, value: &str) {
        let value = value.to_ascii_lowercase();
        if value.contains("384") {
            self.plate = Some(PlateFormat::P384);
        } else if value.contains("96") && self.plate.is_none() {
            self.plate = Some(PlateFormat::P96);
        }
    }
}

#[derive(Clone, Copy)]
enum Axis {
    Cycle,
    MeltRfu,
    MeltDerivative,
}

impl Axis {
    fn axis_name(&self) -> &'static str {
        match self {
            Axis::Cycle => "cycle",
            Axis::MeltRfu | Axis::MeltDerivative => "temperature",
        }
    }
}

/// Decide what a table is from its header row and its file/sheet name.
fn classify(header: &[String], label: &str) -> TableKind {
    let l = label.to_ascii_lowercase();
    let has = |names: &[&str]| find_col(header, names).is_some();
    let has_well_cols = header.iter().any(|h| parse_well_label(h).is_some());

    // Wide (matrix) tables: an axis column plus well-labelled data columns.
    if has(&["cycle"]) && has_well_cols {
        return TableKind::Amplification;
    }
    if has(&["temperature"]) && has_well_cols {
        // RFU vs derivative disambiguated by the file/sheet name.
        if l.contains("deriv") {
            return TableKind::MeltDerivative;
        }
        return TableKind::MeltRfu;
    }

    // Tall (per-well) tables keyed by a "Well" column.
    if has(&["well"]) {
        if has(&["cq", "ct"]) {
            return TableKind::CqResults;
        }
        if has(&["melt temperature", "peak"]) || l.contains("peak") {
            return TableKind::MeltPeaks;
        }
        if has(&["sample", "target", "content"]) {
            return TableKind::PlateLayout;
        }
    }
    if l.contains("run information") || l.contains("run info") {
        return TableKind::RunInfo;
    }
    TableKind::Unknown
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/// Case-insensitive header lookup; returns the first matching column index.
fn find_col(header: &[String], names: &[&str]) -> Option<usize> {
    header.iter().position(|h| {
        let h = h.trim().to_ascii_lowercase();
        names.iter().any(|n| h == *n)
    })
}

fn cell(row: &[String], col: Option<usize>) -> Option<String> {
    col.and_then(|i| row.get(i))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn parse_cfx_well_label(label: &str) -> Result<Option<(u8, u8)>> {
    let Some((row, col)) = parse_well_label(label) else {
        return Ok(None);
    };
    if row >= 16 || col >= 24 {
        return Err(QpcrError::Parse(format!(
            "well label {label:?} is outside supported CFX plate bounds A1-P24"
        )));
    }
    Ok(Some((row, col)))
}

fn non_empty(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn is_optional_empty_table(label: &str, rows: &[Vec<String>], err: &QpcrError) -> bool {
    let QpcrError::Parse(msg) = err else {
        return false;
    };
    let label = label.to_ascii_lowercase();
    let compact_label: String = label
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let optional_label = label.contains("peak") || compact_label.contains("platelayout");
    if !optional_label {
        return false;
    }
    match msg.as_str() {
        "melt peaks table has no data rows" | "plate layout table has no data rows" => true,
        "melt peaks table has no valid Well rows" | "plate layout table has no valid Well rows" => {
            rows.iter()
                .skip(1)
                .all(|row| row.iter().all(|cell| cell.trim().is_empty()))
        }
        _ => false,
    }
}

fn merge_melt_temperature(existing: &mut Vec<f64>, incoming: &[f64]) -> Result<()> {
    if existing.is_empty() {
        existing.extend_from_slice(incoming);
        return Ok(());
    }
    if existing.len() != incoming.len()
        || existing
            .iter()
            .zip(incoming.iter())
            .any(|(a, b)| (*a - *b).abs() > f64::EPSILON)
    {
        return Err(QpcrError::Parse(
            "melt RFU and derivative tables use different temperature axes".into(),
        ));
    }
    Ok(())
}

fn parse_num(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    // CFX uses "N/A" / "NaN" for uncalled Cq etc.
    if t.eq_ignore_ascii_case("n/a")
        || t.eq_ignore_ascii_case("na")
        || t.eq_ignore_ascii_case("nan")
    {
        return None;
    }
    // Tolerate comma decimal separators from localised exports.
    t.replace(',', ".").parse().ok()
}

/// Extract a fluorophore hint from a file path like `…Results_FAM.csv`.
fn fluor_from_name(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|s| s.to_str())
        .and_then(fluor_from_str)
}

fn fluor_from_str(stem: &str) -> Option<String> {
    let after = stem.rsplit('_').next()?.trim();
    // Guard against tables with no fluor suffix (the part after '_' would be words).
    let known = [
        "fam",
        "hex",
        "vic",
        "sybr",
        "cy5",
        "cy5.5",
        "rox",
        "texasred",
        "texas red",
        "quasar",
        "tex615",
        "atto",
        "cal",
        "cal red",
        "gold540",
        "green",
        "yellow",
        "orange",
        "red",
        "crimson",
    ];
    let low = after.to_ascii_lowercase();
    if known.iter().any(|k| low == *k || low.contains(k))
        || (after.len() <= 8 && !after.contains(' '))
    {
        Some(after.to_string())
    } else {
        None
    }
}

/// A human label for a table (used to disambiguate melt RFU vs derivative).
fn table_label(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string()
}

fn read_csv_rows(path: &Path) -> Result<Vec<Vec<String>>> {
    let file = File::open(path)?;
    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .has_headers(false)
        .from_reader(file);
    let mut rows = Vec::new();
    for rec in rdr.records() {
        let rec = rec?;
        rows.push(rec.iter().map(|s| s.to_string()).collect());
    }
    Ok(rows)
}

fn cell_to_string(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Float(f) => {
            // Print integers cleanly (well labels sometimes arrive as numbers).
            if f.fract() == 0.0 {
                format!("{}", *f as i64)
            } else {
                f.to_string()
            }
        }
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(d) => d.to_string(),
        Data::DateTimeIso(s) => s.clone(),
        Data::DurationIso(s) => s.clone(),
        Data::Error(e) => format!("{e:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(cols: &[&str]) -> Vec<String> {
        cols.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn amplification_matrix() {
        let rows = vec![
            r(&["", "Cycle", "A1", "A2"]),
            r(&["1", "1", "10.0", "20.0"]),
            r(&["2", "2", "11.0", "21.0"]),
        ];
        let mut b = RunBuilder::new();
        b.ingest_table(
            &rows,
            Some("FAM"),
            "Quantification Amplification Results_FAM",
        )
        .unwrap();
        let run = b.finish();
        let a1 = run.wells.iter().find(|w| w.position() == "A1").unwrap();
        assert_eq!(a1.channels[0].fluorophore, "FAM");
        assert_eq!(a1.channels[0].amplification, vec![10.0, 11.0]);
        assert_eq!(run.metadata.cycle_count, Some(2));
    }

    #[test]
    fn amplification_matrix_skips_empty_well_columns() {
        let rows = vec![
            r(&["", "Cycle", "A1", "A2"]),
            r(&["1", "1", "10.0", ""]),
            r(&["2", "2", "11.0", ""]),
        ];
        let mut b = RunBuilder::new();
        b.ingest_table(
            &rows,
            Some("FAM"),
            "Quantification Amplification Results_FAM",
        )
        .unwrap();
        let run = b.finish();
        assert!(run.wells.iter().any(|w| w.position() == "A1"));
        assert!(!run.wells.iter().any(|w| w.position() == "A2"));
    }

    #[test]
    fn rejects_header_only_amplification_matrix() {
        let rows = vec![r(&["", "Cycle", "A1"])];
        let mut b = RunBuilder::new();
        let err = b
            .ingest_table(
                &rows,
                Some("FAM"),
                "Quantification Amplification Results_FAM",
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("cycle matrix table has no data rows"));
        assert!(!b.has_wells());
    }

    #[test]
    fn rejects_matrix_with_no_populated_well_columns() {
        let rows = vec![r(&["", "Cycle", "A1"]), r(&["1", "1", ""])];
        let mut b = RunBuilder::new();
        let err = b
            .ingest_table(
                &rows,
                Some("FAM"),
                "Quantification Amplification Results_FAM",
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("cycle matrix table has no populated well columns"));
        assert!(!b.has_wells());
    }

    #[test]
    fn rejects_matrix_headers_outside_cfx_plate_bounds() {
        let rows = vec![r(&["", "Cycle", "Z99"]), r(&["1", "1", "10.0"])];
        let mut b = RunBuilder::new();
        let err = b
            .ingest_table(
                &rows,
                Some("FAM"),
                "Quantification Amplification Results_FAM",
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("outside supported CFX plate bounds"));
        assert!(!b.has_wells());
    }

    #[test]
    fn rejects_header_only_melt_matrix() {
        let rows = vec![r(&["", "Temperature", "A1"])];
        let mut b = RunBuilder::new();
        let err = b
            .ingest_table(&rows, Some("SYBR"), "Melt Curve RFU Results_SYBR")
            .unwrap_err()
            .to_string();
        assert!(err.contains("temperature matrix table has no data rows"));
        assert!(!b.has_wells());
    }

    #[test]
    fn cq_results() {
        let rows = vec![
            r(&["Well", "Fluor", "Target", "Content", "Sample", "Cq"]),
            r(&["A1", "FAM", "GAPDH", "Unkn", "Ctrl", "24.5"]),
            r(&["A2", "FAM", "GAPDH", "NTC", "", "N/A"]),
        ];
        let mut b = RunBuilder::new();
        b.ingest_table(&rows, None, "Quantification Cq Results")
            .unwrap();
        let run = b.finish();
        let a1 = run.wells.iter().find(|w| w.position() == "A1").unwrap();
        assert_eq!(a1.sample.as_deref(), Some("Ctrl"));
        assert_eq!(a1.channels[0].target.as_deref(), Some("GAPDH"));
        assert_eq!(a1.channels[0].cq, Some(24.5));
        let a2 = run.wells.iter().find(|w| w.position() == "A2").unwrap();
        assert_eq!(a2.sample_type, SampleType::Ntc);
        assert_eq!(a2.channels[0].cq, None);
    }

    #[test]
    fn rejects_cq_wells_outside_cfx_plate_bounds() {
        let rows = vec![
            r(&["Well", "Fluor", "Target", "Cq"]),
            r(&["Z99", "FAM", "GAPDH", "24.5"]),
        ];
        let mut b = RunBuilder::new();
        let err = b
            .ingest_table(&rows, None, "Quantification Cq Results")
            .unwrap_err()
            .to_string();
        assert!(err.contains("outside supported CFX plate bounds"));
        assert!(!b.has_wells());
    }

    #[test]
    fn rejects_cq_rows_without_measurement_or_metadata() {
        let rows = vec![r(&["Well", "Cq"]), r(&["A1", ""])];
        let mut b = RunBuilder::new();
        let err = b
            .ingest_table(&rows, None, "Quantification Cq Results")
            .unwrap_err()
            .to_string();
        assert!(err.contains("Cq results table has no data rows"));
        assert!(!b.has_wells());
    }

    #[test]
    fn rejects_cq_rows_with_only_fluorophore() {
        let rows = vec![r(&["Well", "Fluor", "Cq"]), r(&["A1", "SYBR", ""])];
        let mut b = RunBuilder::new();
        let err = b
            .ingest_table(&rows, None, "Quantification Cq Results")
            .unwrap_err()
            .to_string();
        assert!(err.contains("Cq results table has no data rows"));
        assert!(!b.has_wells());
    }

    #[test]
    fn melt_rfu_and_derivative_merge() {
        let rfu = vec![
            r(&["", "Temperature", "A1"]),
            r(&["1", "65.0", "100.0"]),
            r(&["2", "66.0", "90.0"]),
        ];
        let der = vec![
            r(&["", "Temperature", "A1"]),
            r(&["1", "65.0", "1.0"]),
            r(&["2", "66.0", "5.0"]),
        ];
        let mut b = RunBuilder::new();
        b.ingest_table(&rfu, Some("SYBR"), "Melt Curve RFU Results_SYBR")
            .unwrap();
        b.ingest_table(&der, Some("SYBR"), "Melt Curve Derivative Results_SYBR")
            .unwrap();
        let run = b.finish();
        let a1 = run.wells.iter().find(|w| w.position() == "A1").unwrap();
        let melt = a1.channels[0].melt.as_ref().unwrap();
        assert_eq!(melt.temperature, vec![65.0, 66.0]);
        assert_eq!(melt.rfu, vec![100.0, 90.0]);
        assert_eq!(melt.derivative, vec![1.0, 5.0]);
    }

    #[test]
    fn rejects_empty_melt_peaks_table() {
        let rows = vec![
            r(&["Well", "Fluor", "Melt Temperature"]),
            r(&["A1", "SYBR", ""]),
        ];
        let mut b = RunBuilder::new();
        let err = b
            .ingest_table(&rows, None, "Melt Peak Results")
            .unwrap_err()
            .to_string();
        assert!(err.contains("melt peaks table has no data rows"));
        assert!(!b.has_wells());
    }

    #[test]
    fn rejects_empty_plate_layout_table() {
        let rows = vec![
            r(&["Well", "Sample", "Content", "Target"]),
            r(&["A1", "", "", ""]),
        ];
        let mut b = RunBuilder::new();
        let err = b
            .ingest_table(&rows, None, "Plate Layout")
            .unwrap_err()
            .to_string();
        assert!(err.contains("plate layout table has no data rows"));
        assert!(!b.has_wells());
    }

    #[test]
    fn rejects_malformed_melt_peaks_table() {
        let rows = vec![r(&["Well", "Fluor"]), r(&["A1", "SYBR"])];
        let mut b = RunBuilder::new();
        let err = b
            .ingest_table(&rows, None, "Melt Peak Results")
            .unwrap_err()
            .to_string();
        assert!(err.contains("melt peaks table missing Melt Temperature column"));
        assert!(!b.has_wells());
    }

    #[test]
    fn rejects_optional_tables_without_valid_wells() {
        let peaks = vec![
            r(&["Well", "Fluor", "Melt Temperature"]),
            r(&["not-a-well", "SYBR", "80.0"]),
        ];
        let layout = vec![
            r(&["Well", "Sample", "Content", "Target"]),
            r(&["not-a-well", "S1", "Unkn", "GAPDH"]),
        ];
        let mut b = RunBuilder::new();
        let peaks_err = b
            .ingest_table(&peaks, None, "Melt Peak Results")
            .unwrap_err()
            .to_string();
        let layout_err = b
            .ingest_table(&layout, None, "Plate Layout")
            .unwrap_err()
            .to_string();
        assert!(peaks_err.contains("melt peaks table has no valid Well rows"));
        assert!(layout_err.contains("plate layout table has no valid Well rows"));
        assert!(!b.has_wells());
    }

    #[test]
    fn rejects_mismatched_melt_temperature_axes_rfu_first() {
        let rfu = vec![
            r(&["", "Temperature", "A1"]),
            r(&["1", "65.0", "100.0"]),
            r(&["2", "66.0", "90.0"]),
        ];
        let der = vec![
            r(&["", "Temperature", "A1"]),
            r(&["1", "65.5", "1.0"]),
            r(&["2", "66.5", "5.0"]),
        ];
        let mut b = RunBuilder::new();
        b.ingest_table(&rfu, Some("SYBR"), "Melt Curve RFU Results_SYBR")
            .unwrap();
        let err = b
            .ingest_table(&der, Some("SYBR"), "Melt Curve Derivative Results_SYBR")
            .unwrap_err()
            .to_string();
        assert!(err.contains("different temperature axes"));
    }

    #[test]
    fn rejects_mismatched_melt_temperature_axes_derivative_first() {
        let der = vec![
            r(&["", "Temperature", "A1"]),
            r(&["1", "65.0", "1.0"]),
            r(&["2", "66.0", "5.0"]),
        ];
        let rfu = vec![
            r(&["", "Temperature", "A1"]),
            r(&["1", "65.0", "100.0"]),
            r(&["2", "67.0", "90.0"]),
        ];
        let mut b = RunBuilder::new();
        b.ingest_table(&der, Some("SYBR"), "Melt Curve Derivative Results_SYBR")
            .unwrap();
        let err = b
            .ingest_table(&rfu, Some("SYBR"), "Melt Curve RFU Results_SYBR")
            .unwrap_err()
            .to_string();
        assert!(err.contains("different temperature axes"));
    }

    #[test]
    fn classify_basics() {
        assert_eq!(
            classify(&r(&["", "Cycle", "A1"]), "Amplification"),
            TableKind::Amplification
        );
        assert_eq!(
            classify(&r(&["", "Temperature", "A1"]), "Melt Curve Derivative"),
            TableKind::MeltDerivative
        );
        assert_eq!(
            classify(&r(&["Well", "Fluor", "Cq"]), "Cq Results"),
            TableKind::CqResults
        );
    }

    #[test]
    fn run_info_sets_plate_format() {
        let rows = vec![
            r(&["Run Information", ""]),
            r(&["Instrument", "CFX Opus"]),
            r(&["Plate Format", "384 Wells"]),
        ];
        let mut b = RunBuilder::new();
        b.ingest_table(&rows, None, "Run Information").unwrap();
        let run = b.finish();
        assert_eq!(run.metadata.instrument.as_deref(), Some("CFX Opus"));
        assert_eq!(run.plate.rows, 16);
        assert_eq!(run.plate.cols, 24);
    }
}
