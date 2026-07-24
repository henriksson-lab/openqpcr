//! RDML (Real-time PCR Data Markup Language) reader and writer.
//!
//! RDML (<https://rdml.org>) is an open XML interchange standard exported by many
//! qPCR instruments (Bio-Rad CFX, Thermo QuantStudio, Agilent AriaMx, …). A
//! `.rdml` file is a **ZIP archive** holding one RDML XML document (the RDML XML
//! per the XSD); an uncompressed `.xml` document is also accepted.
//!
//! The XML member is conventionally named `rdml_data.xml`, but real exports do
//! not always follow that (e.g. Bio-Rad CFX writes `BioRad_qPCR_melt.xml`), so on
//! read we pick the archive member whose root element is `<rdml>` regardless of
//! its file name. On write we emit the conventional `rdml_data.xml`.
//!
//! We target **RDML v1.2** on write and parse leniently on read across v1.0–v1.3
//! (the `version` attribute on the root `<rdml>` is not enforced, and element
//! names are matched on their local name, so default-namespace and prefixed
//! documents both work).
//!
//! ## What we map
//!
//! | RDML                                   | [`QpcrRun`] field                    |
//! |----------------------------------------|--------------------------------------|
//! | `react/@id` (numeric row-major or A1)  | [`Well::row`] / [`Well::col`]        |
//! | `react/sample/@id` → top-level sample  | [`Well::sample`] (id = name in 1.2)  |
//! | sample `type` (unkn/std/ntc/nrt/pos/…) | [`Well::sample_type`]                |
//! | sample `quantity/value`                | [`Well::starting_quantity`]          |
//! | `data/tar/@id` → target `dyeId`        | [`Channel::fluorophore`]             |
//! | `data/tar/@id`                         | [`Channel::target`]                  |
//! | `data/cq`                              | [`Channel::cq`]                      |
//! | `data/adp` (`cyc`,`fluor`)             | [`Channel::amplification`]           |
//! | `data/mdp` (`tmp`,`fluor`)             | [`MeltCurve::rfu`] / temperature     |
//!
//! Melt `derivative` and `peaks` are left empty on read (RDML stores only raw
//! melt points; we do not fabricate a derivative). Many optional RDML elements
//! (experimenter, thermal cycling conditions, sequences, …) are ignored.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::Path;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use zip::write::{SimpleFileOptions, ZipWriter};
use zip::{CompressionMethod, ZipArchive};

use crate::error::{QpcrError, Result};
use crate::model::{
    Channel, MeltCurve, PlateFormat, QpcrRun, RunMetadata, SampleType, Well, parse_well_label,
    row_label,
};
use crate::protocol::{GradientStep, Measure, ProtocolStep, TemperatureStep, ThermalProtocol};

/// The conventional XML document name from the RDML file-format notes. We write
/// this name; on read we accept any member whose root element is `<rdml>`.
const RDML_ENTRY: &str = "rdml_data.xml";
const RDML_NS: &str = "http://www.rdml.org";
const RDML_VERSION: &str = "1.2";

// ===========================================================================
// Reading
// ===========================================================================

/// Parse an RDML file (`.rdml` ZIP archive, or a bare `.xml` document) into a
/// [`QpcrRun`].
pub fn read_rdml(path: &Path) -> Result<QpcrRun> {
    let xml = load_rdml_xml(path)?;
    let mut run = parse_rdml_xml(&xml)?;
    run.metadata.source_file = Some(path.display().to_string());
    Ok(run)
}

/// Does `path` point at a ZIP archive that contains an RDML XML member (any
/// `*.xml` entry whose root element is `<rdml>`)?
///
/// Used by the top-level format dispatcher to prefer the RDML reader over the
/// native `.pcrd` path for ZIP files that are really RDML.
pub fn is_rdml_zip(path: &Path) -> Result<bool> {
    let file = File::open(path)?;
    let mut archive = match ZipArchive::new(file) {
        Ok(a) => a,
        Err(_) => return Ok(false),
    };
    Ok(find_rdml_member(&mut archive)?.is_some())
}

/// Read the RDML XML text, transparently handling both the zipped container and
/// a bare XML document.
///
/// We attempt to open the file as a ZIP first rather than sniffing the `PK\x03\x04`
/// magic: some writers (e.g. Bio-Rad) prefix the optional ZIP spanning signature
/// `PK\x07\x08`, so a strict local-header magic check misclassifies a valid RDML
/// container as bare XML and then chokes reading its bytes as UTF-8.
fn load_rdml_xml(path: &Path) -> Result<String> {
    if let Ok(mut archive) = ZipArchive::new(File::open(path)?) {
        return find_rdml_member(&mut archive)?.ok_or_else(|| {
            QpcrError::UnsupportedFormat(format!(
                "{} is a ZIP archive but contains no RDML XML member (<rdml> root)",
                path.display()
            ))
        });
    }

    // Not a ZIP archive: treat as a bare RDML XML document.
    let mut buf = String::new();
    File::open(path)?.read_to_string(&mut buf)?;
    if !buf.trim_start().starts_with('<') {
        return Err(QpcrError::UnsupportedFormat(format!(
            "{} is neither a ZIP archive nor an XML document",
            path.display()
        )));
    }
    Ok(buf)
}

/// Locate the RDML XML member inside a ZIP archive and return its text.
///
/// The member name is not standardised (e.g. Bio-Rad writes `BioRad_qPCR_*.xml`),
/// so we inspect every `*.xml` entry — preferring the conventional
/// `rdml_data.xml` — and return the first whose root element is `<rdml>`.
fn find_rdml_member<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<Option<String>> {
    // Gather candidate `.xml` member names (via the raw index, which never
    // decrypts), then order them so the conventional name is tried first.
    let mut names: Vec<String> = (0..archive.len())
        .filter_map(|i| archive.by_index_raw(i).ok().map(|z| z.name().to_string()))
        .filter(|n| n.to_ascii_lowercase().ends_with(".xml"))
        .collect();
    names.sort_by_key(|n| !n.eq_ignore_ascii_case(RDML_ENTRY));

    for name in names {
        let mut buf = String::new();
        if archive
            .by_name(&name)
            .ok()
            .and_then(|mut e| e.read_to_string(&mut buf).ok())
            .is_none()
        {
            continue;
        }
        if is_rdml_document(&buf) {
            return Ok(Some(buf));
        }
    }
    Ok(None)
}

/// Is `xml`'s first element the RDML root `<rdml>`?
fn is_rdml_document(xml: &str) -> bool {
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                return e.local_name().as_ref() == b"rdml";
            }
            Ok(Event::Eof) | Err(_) => return false,
            _ => {}
        }
    }
}

/// Parse an in-memory RDML XML document into a [`QpcrRun`].
pub fn parse_rdml_xml(xml: &str) -> Result<QpcrRun> {
    let mut p = Parser::default();
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event()? {
            Event::Start(e) => {
                let name = local_name(&e);
                p.start(&name, &e)?;
                p.stack.push(name);
            }
            Event::Empty(e) => {
                let name = local_name(&e);
                p.start(&name, &e)?;
                p.end(&name)?;
            }
            Event::End(e) => {
                let name = local_name_end(&e);
                p.stack.pop();
                p.end(&name)?;
            }
            Event::Text(e) => {
                let t = e.unescape()?;
                p.text.push_str(t.as_ref());
            }
            Event::CData(e) => {
                p.text.push_str(&String::from_utf8_lossy(e.as_ref()));
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(p.finish())
}

/// Accumulator for one `data` element inside a `react`.
#[derive(Default)]
struct DataAcc {
    tar: Option<String>,
    cq: Option<f64>,
    adps: Vec<(f64, f64)>,
    mdps: Vec<(f64, f64)>,
}

/// Accumulator for one `react` element.
struct ReactAcc {
    raw_id: Option<String>,
    sample: Option<String>,
    datas: Vec<DataAcc>,
}

#[derive(Default)]
struct Parser {
    stack: Vec<String>,
    text: String,

    // Top-level definition tables.
    samples: BTreeMap<String, (String, Option<f64>)>, // id -> (type code, quantity)
    targets: BTreeMap<String, String>,                // id -> dye id (fluorophore)

    // Plate geometry from the run's pcrFormat.
    rows: Option<u8>,
    cols: Option<u8>,
    instrument: Option<String>,

    // In-progress sample definition.
    cur_sample: Option<(String, String, Option<f64>)>, // id, type, quantity
    in_quantity: bool,

    // In-progress target definition.
    cur_target: Option<(String, Option<String>)>, // id, dye id

    // In-progress react / data / points.
    reacts: Vec<ReactAcc>,
    cur_data: Option<DataAcc>,
    cur_adp: Option<(Option<f64>, Option<f64>)>, // cyc, fluor
    cur_mdp: Option<(Option<f64>, Option<f64>)>, // tmp, fluor

    // In-progress thermalCyclingConditions → ThermalProtocol.
    protocol: Option<ThermalProtocol>,
    cur_temp: Option<TemperatureStep>,
    cur_grad: Option<GradientStep>,
    cur_loop: Option<(usize, u32)>, // goto (1-based nr), repeat
    cur_pause: Option<f64>,
}

impl Parser {
    fn parent(&self) -> Option<&str> {
        self.stack.last().map(|s| s.as_str())
    }

    fn start(&mut self, name: &str, e: &BytesStart) -> Result<()> {
        self.text.clear();
        let parent = self.parent();
        match (parent, name) {
            (Some("rdml"), "sample") => {
                let id = attr(e, b"id").unwrap_or_default();
                self.cur_sample = Some((id, "unkn".to_string(), None));
                self.in_quantity = false;
            }
            (Some("sample"), "quantity") => self.in_quantity = true,
            (Some("rdml"), "target") => {
                let id = attr(e, b"id").unwrap_or_default();
                self.cur_target = Some((id, None));
            }
            (Some("target"), "dyeId") => {
                if let Some(t) = self.cur_target.as_mut() {
                    t.1 = attr(e, b"id");
                }
            }
            (Some("run"), "react") => {
                self.reacts.push(ReactAcc {
                    raw_id: attr(e, b"id"),
                    sample: None,
                    datas: Vec::new(),
                });
            }
            (Some("react"), "sample") => {
                if let Some(r) = self.reacts.last_mut() {
                    r.sample = attr(e, b"id");
                }
            }
            (Some("react"), "data") => self.cur_data = Some(DataAcc::default()),
            (Some("data"), "tar") => {
                if let Some(d) = self.cur_data.as_mut() {
                    d.tar = attr(e, b"id");
                }
            }
            (Some("data"), "adp") => self.cur_adp = Some((None, None)),
            (Some("data"), "mdp") => self.cur_mdp = Some((None, None)),

            // thermalCyclingConditions → ThermalProtocol.
            (Some("rdml"), "thermalCyclingConditions") => {
                self.protocol = Some(ThermalProtocol {
                    name: attr(e, b"id"),
                    ..Default::default()
                });
            }
            (Some("step"), "temperature") => {
                self.cur_temp = Some(TemperatureStep {
                    target_c: 0.0,
                    hold_secs: None,
                    ramp_c_per_s: None,
                    temperature_change: 0.0,
                    duration_change: 0.0,
                    measure: Measure::None,
                });
            }
            (Some("step"), "gradient") => {
                self.cur_grad = Some(GradientStep {
                    high_c: 0.0,
                    low_c: 0.0,
                    hold_secs: None,
                    ramp_c_per_s: None,
                    measure: Measure::None,
                });
            }
            (Some("step"), "loop") => self.cur_loop = Some((0, 0)),
            (Some("step"), "pause") => self.cur_pause = Some(0.0),
            _ => {}
        }
        Ok(())
    }

    fn end(&mut self, name: &str) -> Result<()> {
        let parent = self.parent();
        let text = self.text.trim().to_string();
        match (parent, name) {
            (Some("sample"), "type") => {
                if let Some(s) = self.cur_sample.as_mut()
                    && !text.is_empty()
                {
                    s.1 = text;
                }
            }
            (Some("quantity"), "value") => {
                if self.in_quantity
                    && let Some(s) = self.cur_sample.as_mut()
                {
                    s.2 = parse_f64(&text);
                }
            }
            (Some("sample"), "quantity") => self.in_quantity = false,
            (Some("rdml"), "sample") => {
                if let Some((id, ty, qty)) = self.cur_sample.take()
                    && !id.is_empty()
                {
                    self.samples.insert(id, (ty, qty));
                }
            }
            (Some("rdml"), "target") => {
                if let Some((id, dye)) = self.cur_target.take()
                    && !id.is_empty()
                {
                    self.targets.insert(id.clone(), dye.unwrap_or(id));
                }
            }
            (Some("run"), "instrument") => {
                if !text.is_empty() {
                    self.instrument = Some(text);
                }
            }
            (Some("pcrFormat"), "rows") => self.rows = parse_f64(&text).map(|v| v as u8),
            (Some("pcrFormat"), "columns") => self.cols = parse_f64(&text).map(|v| v as u8),
            (Some("data"), "cq") => {
                if let Some(d) = self.cur_data.as_mut() {
                    d.cq = parse_f64(&text);
                }
            }
            (Some("adp"), "cyc") => {
                if let Some(a) = self.cur_adp.as_mut() {
                    a.0 = parse_f64(&text);
                }
            }
            (Some("adp"), "fluor") => {
                if let Some(a) = self.cur_adp.as_mut() {
                    a.1 = parse_f64(&text);
                }
            }
            (Some("mdp"), "tmp") => {
                if let Some(m) = self.cur_mdp.as_mut() {
                    m.0 = parse_f64(&text);
                }
            }
            (Some("mdp"), "fluor") => {
                if let Some(m) = self.cur_mdp.as_mut() {
                    m.1 = parse_f64(&text);
                }
            }
            (Some("data"), "adp") => {
                if let (Some((Some(c), Some(f))), Some(d)) =
                    (self.cur_adp.take(), self.cur_data.as_mut())
                {
                    d.adps.push((c, f));
                }
            }
            (Some("data"), "mdp") => {
                if let (Some((Some(t), Some(f))), Some(d)) =
                    (self.cur_mdp.take(), self.cur_data.as_mut())
                {
                    d.mdps.push((t, f));
                }
            }
            (Some("react"), "data") => {
                if let (Some(d), Some(r)) = (self.cur_data.take(), self.reacts.last_mut()) {
                    r.datas.push(d);
                }
            }

            // --- thermalCyclingConditions → ThermalProtocol ---
            (Some("thermalCyclingConditions"), "description") => {
                if let Some(p) = self.protocol.as_mut()
                    && !text.is_empty()
                {
                    p.description = Some(text);
                }
            }
            (Some("thermalCyclingConditions"), "lidTemperature") => {
                if let Some(p) = self.protocol.as_mut() {
                    p.lid_temperature = parse_f64(&text);
                }
            }
            // Values inside a `temperature` step.
            (Some("temperature"), "temperature") => {
                if let (Some(t), Some(v)) = (self.cur_temp.as_mut(), parse_f64(&text)) {
                    t.target_c = v;
                }
            }
            (Some("temperature"), "duration") => {
                if let Some(t) = self.cur_temp.as_mut() {
                    t.hold_secs = parse_f64(&text);
                }
            }
            (Some("temperature"), "ramp") => {
                if let Some(t) = self.cur_temp.as_mut() {
                    t.ramp_c_per_s = parse_f64(&text);
                }
            }
            (Some("temperature"), "temperatureChange") => {
                if let (Some(t), Some(v)) = (self.cur_temp.as_mut(), parse_f64(&text)) {
                    t.temperature_change = v;
                }
            }
            (Some("temperature"), "durationChange") => {
                if let (Some(t), Some(v)) = (self.cur_temp.as_mut(), parse_f64(&text)) {
                    t.duration_change = v;
                }
            }
            (Some("temperature"), "measure") => {
                if let Some(t) = self.cur_temp.as_mut() {
                    t.measure = parse_measure(&text);
                }
            }
            // Values inside a `gradient` step.
            (Some("gradient"), "highTemperature") => {
                if let (Some(g), Some(v)) = (self.cur_grad.as_mut(), parse_f64(&text)) {
                    g.high_c = v;
                }
            }
            (Some("gradient"), "lowTemperature") => {
                if let (Some(g), Some(v)) = (self.cur_grad.as_mut(), parse_f64(&text)) {
                    g.low_c = v;
                }
            }
            (Some("gradient"), "duration") => {
                if let Some(g) = self.cur_grad.as_mut() {
                    g.hold_secs = parse_f64(&text);
                }
            }
            (Some("gradient"), "ramp") => {
                if let Some(g) = self.cur_grad.as_mut() {
                    g.ramp_c_per_s = parse_f64(&text);
                }
            }
            (Some("gradient"), "measure") => {
                if let Some(g) = self.cur_grad.as_mut() {
                    g.measure = parse_measure(&text);
                }
            }
            // Values inside `loop` / `pause`.
            (Some("loop"), "goto") => {
                if let (Some(l), Some(v)) = (self.cur_loop.as_mut(), parse_f64(&text)) {
                    l.0 = v as usize;
                }
            }
            (Some("loop"), "repeat") => {
                if let (Some(l), Some(v)) = (self.cur_loop.as_mut(), parse_f64(&text)) {
                    l.1 = v as u32;
                }
            }
            (Some("pause"), "temperature") => {
                if let (Some(p), Some(v)) = (self.cur_pause.as_mut(), parse_f64(&text)) {
                    *p = v;
                }
            }
            // A step child ended → push the completed `ProtocolStep`.
            (Some("step"), "temperature") => {
                if let Some(t) = self.cur_temp.take()
                    && let Some(p) = self.protocol.as_mut()
                {
                    p.steps.push(ProtocolStep::Hold(t));
                }
            }
            (Some("step"), "gradient") => {
                if let Some(g) = self.cur_grad.take()
                    && let Some(p) = self.protocol.as_mut()
                {
                    p.steps.push(ProtocolStep::Gradient(g));
                }
            }
            (Some("step"), "loop") => {
                if let Some((goto, repeat)) = self.cur_loop.take()
                    && let Some(p) = self.protocol.as_mut()
                {
                    p.steps.push(ProtocolStep::Loop { goto, repeat });
                }
            }
            (Some("step"), "pause") => {
                if let Some(temperature) = self.cur_pause.take()
                    && let Some(p) = self.protocol.as_mut()
                {
                    p.steps.push(ProtocolStep::Pause { temperature });
                }
            }
            (Some("step"), "lidOpen") => {
                if let Some(p) = self.protocol.as_mut() {
                    p.steps.push(ProtocolStep::LidOpen);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(self) -> QpcrRun {
        let cols = self.cols.filter(|c| *c > 0);
        let rows = self.rows.filter(|r| *r > 0);

        let mut wells: BTreeMap<(u8, u8), Well> = BTreeMap::new();
        for react in &self.reacts {
            let Some((row, col)) = resolve_position(react.raw_id.as_deref(), cols) else {
                continue;
            };

            let sample_ref = react.sample.as_deref().filter(|s| !s.is_empty());
            let (sample_type, quantity) = match sample_ref.and_then(|s| self.samples.get(s)) {
                Some((ty, qty)) => (SampleType::parse(ty), *qty),
                None => (SampleType::Unknown, None),
            };

            let well = wells.entry((row, col)).or_insert_with(|| Well {
                row,
                col,
                ..Default::default()
            });
            if let Some(s) = sample_ref {
                well.sample = Some(s.to_string());
            }
            well.sample_type = sample_type;
            well.starting_quantity = quantity;

            for data in &react.datas {
                let tar = data.tar.clone().unwrap_or_default();
                let fluorophore = self
                    .targets
                    .get(&tar)
                    .cloned()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| tar.clone());
                let target = if tar.is_empty() { None } else { Some(tar) };

                let amplification = build_amplification(&data.adps);
                let melt = build_melt(&data.mdps);
                // RDML uses a negative Cq (typically -1.0) to signal "Not Available".
                let cq = data.cq.filter(|v| *v >= 0.0);

                well.channels.push(Channel {
                    fluorophore,
                    target,
                    cq,
                    amplification,
                    melt,
                });
            }
        }

        let plate = match (rows, cols) {
            (Some(r), Some(c)) => PlateFormat { rows: r, cols: c },
            _ => {
                let max_row = wells.keys().map(|(r, _)| *r).max().unwrap_or(7);
                let max_col = wells.keys().map(|(_, c)| *c).max().unwrap_or(11);
                if max_row >= 8 || max_col >= 12 {
                    PlateFormat::P384
                } else {
                    PlateFormat::P96
                }
            }
        };

        let cycle_count = wells
            .values()
            .flat_map(|w| w.channels.iter())
            .map(|c| c.amplification.len())
            .max()
            .filter(|n| *n > 0);

        QpcrRun {
            metadata: RunMetadata {
                instrument: self.instrument,
                cycle_count,
                ..Default::default()
            },
            plate,
            wells: wells.into_values().collect(),
            protocol: self.protocol,
        }
    }
}

/// Turn amplification points into an RFU-by-cycle vector (1-based cycle = index
/// 0), sorting by cycle and filling any missing cycles with NaN.
fn build_amplification(adps: &[(f64, f64)]) -> Vec<f64> {
    if adps.is_empty() {
        return Vec::new();
    }
    let max_cyc = adps
        .iter()
        .map(|(c, _)| c.round() as i64)
        .max()
        .unwrap_or(0)
        .max(0) as usize;
    let mut out = vec![f64::NAN; max_cyc];
    for (cyc, fluor) in adps {
        let idx = cyc.round() as i64 - 1;
        if idx >= 0 && (idx as usize) < out.len() {
            out[idx as usize] = *fluor;
        }
    }
    out
}

/// Turn melt points into a [`MeltCurve`] (rfu by temperature, sorted). Derivative
/// and peaks are left empty — RDML stores only raw melt points.
fn build_melt(mdps: &[(f64, f64)]) -> Option<MeltCurve> {
    if mdps.is_empty() {
        return None;
    }
    let mut pts = mdps.to_vec();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    Some(MeltCurve {
        temperature: pts.iter().map(|(t, _)| *t).collect(),
        rfu: pts.iter().map(|(_, f)| *f).collect(),
        derivative: Vec::new(),
        peaks: Vec::new(),
    })
}

/// Resolve a `react/@id` to a zero-based `(row, col)`.
///
/// Accepts a numeric row-major id (1-based; needs the plate column count) or a
/// CFX-flavoured `A1`-style position label.
fn resolve_position(raw: Option<&str>, cols: Option<u8>) -> Option<(u8, u8)> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(n) = raw.parse::<u64>() {
        if n == 0 {
            return None;
        }
        let cols = cols.unwrap_or(12).max(1) as u64;
        let idx = n - 1;
        let row = idx / cols;
        let col = idx % cols;
        return Some((u8::try_from(row).ok()?, u8::try_from(col).ok()?));
    }
    parse_well_label(raw)
}

/// Map an RDML `measure` value to our [`Measure`] enum.
fn parse_measure(text: &str) -> Measure {
    match text.trim().to_ascii_lowercase().as_str() {
        "real" => Measure::Real,
        "meltcurve" | "melt" => Measure::Meltcurve,
        _ => Measure::None,
    }
}

/// The RDML `measure` code for a [`Measure`], or `None` for no plate read.
fn measure_code(m: Measure) -> Option<&'static str> {
    match m {
        Measure::None => None,
        Measure::Real => Some("real"),
        Measure::Meltcurve => Some("meltcurve"),
    }
}

/// Write a `thermalCyclingConditions` element for `protocol`.
fn push_thermal_cycling_conditions(out: &mut String, protocol: &ThermalProtocol) {
    let id = protocol
        .name
        .clone()
        .unwrap_or_else(|| "protocol1".to_string());
    out.push_str(&format!(
        "  <thermalCyclingConditions id=\"{}\">\n",
        esc_attr(&id)
    ));
    if let Some(desc) = &protocol.description {
        out.push_str(&format!(
            "    <description>{}</description>\n",
            esc_text(desc)
        ));
    }
    if let Some(lid) = protocol.lid_temperature {
        out.push_str(&format!(
            "    <lidTemperature>{}</lidTemperature>\n",
            fmt_f64(lid)
        ));
    }
    for (i, step) in protocol.steps.iter().enumerate() {
        let nr = i + 1;
        out.push_str(&format!("    <step>\n      <nr>{nr}</nr>\n"));
        match step {
            ProtocolStep::Hold(t) => push_temperature_step(out, t),
            ProtocolStep::Gradient(g) => push_gradient_step(out, g),
            ProtocolStep::Loop { goto, repeat } => out.push_str(&format!(
                "      <loop>\n        <goto>{goto}</goto>\n        <repeat>{repeat}</repeat>\n      </loop>\n"
            )),
            ProtocolStep::Pause { temperature } => out.push_str(&format!(
                "      <pause>\n        <temperature>{}</temperature>\n      </pause>\n",
                fmt_f64(*temperature)
            )),
            ProtocolStep::LidOpen => out.push_str("      <lidOpen/>\n"),
            ProtocolStep::Melt(m) => {
                // Lower the Melt convenience step to a meltcurve-measured hold.
                let t = TemperatureStep {
                    target_c: m.start_c,
                    hold_secs: Some(m.hold_secs),
                    ramp_c_per_s: None,
                    temperature_change: 0.0,
                    duration_change: 0.0,
                    measure: Measure::Meltcurve,
                };
                push_temperature_step(out, &t);
            }
        }
        out.push_str("    </step>\n");
    }
    out.push_str("  </thermalCyclingConditions>\n");
}

fn push_temperature_step(out: &mut String, t: &TemperatureStep) {
    out.push_str("      <temperature>\n");
    out.push_str(&format!(
        "        <temperature>{}</temperature>\n",
        fmt_f64(t.target_c)
    ));
    if let Some(d) = t.hold_secs {
        out.push_str(&format!("        <duration>{}</duration>\n", fmt_f64(d)));
    }
    if let Some(r) = t.ramp_c_per_s {
        out.push_str(&format!("        <ramp>{}</ramp>\n", fmt_f64(r)));
    }
    if t.temperature_change != 0.0 {
        out.push_str(&format!(
            "        <temperatureChange>{}</temperatureChange>\n",
            fmt_f64(t.temperature_change)
        ));
    }
    if t.duration_change != 0.0 {
        out.push_str(&format!(
            "        <durationChange>{}</durationChange>\n",
            fmt_f64(t.duration_change)
        ));
    }
    if let Some(code) = measure_code(t.measure) {
        out.push_str(&format!("        <measure>{code}</measure>\n"));
    }
    out.push_str("      </temperature>\n");
}

fn push_gradient_step(out: &mut String, g: &GradientStep) {
    out.push_str("      <gradient>\n");
    out.push_str(&format!(
        "        <highTemperature>{}</highTemperature>\n",
        fmt_f64(g.high_c)
    ));
    out.push_str(&format!(
        "        <lowTemperature>{}</lowTemperature>\n",
        fmt_f64(g.low_c)
    ));
    if let Some(d) = g.hold_secs {
        out.push_str(&format!("        <duration>{}</duration>\n", fmt_f64(d)));
    }
    if let Some(r) = g.ramp_c_per_s {
        out.push_str(&format!("        <ramp>{}</ramp>\n", fmt_f64(r)));
    }
    if let Some(code) = measure_code(g.measure) {
        out.push_str(&format!("        <measure>{code}</measure>\n"));
    }
    out.push_str("      </gradient>\n");
}

// ===========================================================================
// Writing
// ===========================================================================

/// Serialize a [`QpcrRun`] to a `.rdml` ZIP archive containing `rdml_data.xml`.
pub fn write_rdml(run: &QpcrRun, path: &Path) -> Result<()> {
    let xml = build_rdml_xml(run);
    let file = File::create(path)?;
    let mut zip = ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file(RDML_ENTRY, opts)?;
    zip.write_all(xml.as_bytes())?;
    zip.finish()?;
    Ok(())
}

/// Build the `rdml_data.xml` document (RDML v1.2) for a [`QpcrRun`].
pub fn build_rdml_xml(run: &QpcrRun) -> String {
    // Collect sample and target/dye definitions referenced by the wells.
    let mut samples: BTreeMap<String, (SampleType, Option<f64>)> = BTreeMap::new();
    let mut targets: BTreeMap<String, String> = BTreeMap::new(); // target id -> dye id
    let mut dyes: BTreeMap<String, ()> = BTreeMap::new();

    for well in &run.wells {
        let sid = sample_id(well);
        samples.insert(sid, (well.sample_type, well.starting_quantity));
        for ch in &well.channels {
            let tid = target_id(ch);
            let dye = if ch.fluorophore.is_empty() {
                tid.clone()
            } else {
                ch.fluorophore.clone()
            };
            targets.insert(tid, dye.clone());
            dyes.insert(dye, ());
        }
    }

    let cols = run.plate.cols.max(1);
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!(
        "<rdml version=\"{RDML_VERSION}\" xmlns=\"{RDML_NS}\">\n"
    ));
    out.push_str(
        "  <id>\n    <publisher>openqpcr</publisher>\n    <serialNumber>1</serialNumber>\n  </id>\n",
    );

    // Dye definitions.
    for dye in dyes.keys() {
        out.push_str(&format!("  <dye id=\"{}\"/>\n", esc_attr(dye)));
    }

    // Sample definitions (id doubles as the display name in RDML 1.2).
    for (id, (ty, qty)) in &samples {
        out.push_str(&format!("  <sample id=\"{}\">\n", esc_attr(id)));
        out.push_str(&format!("    <type>{}</type>\n", sample_type_code(*ty)));
        if let Some(q) = qty {
            out.push_str("    <quantity>\n");
            out.push_str(&format!("      <value>{}</value>\n", fmt_f64(*q)));
            out.push_str("      <unit>cop</unit>\n");
            out.push_str("    </quantity>\n");
        }
        out.push_str("  </sample>\n");
    }

    // Target definitions.
    for (id, dye) in &targets {
        out.push_str(&format!("  <target id=\"{}\">\n", esc_attr(id)));
        out.push_str("    <type>toi</type>\n");
        out.push_str(&format!("    <dyeId id=\"{}\"/>\n", esc_attr(dye)));
        out.push_str("  </target>\n");
    }

    // Thermal cycling program (top-level, per RDML schema order).
    if let Some(protocol) = &run.protocol {
        push_thermal_cycling_conditions(&mut out, protocol);
    }

    // One experiment / one run holding every react.
    out.push_str("  <experiment id=\"experiment1\">\n");
    out.push_str("    <run id=\"run1\">\n");
    if let Some(instr) = &run.metadata.instrument {
        out.push_str(&format!(
            "      <instrument>{}</instrument>\n",
            esc_text(instr)
        ));
    }
    out.push_str("      <pcrFormat>\n");
    out.push_str(&format!("        <rows>{}</rows>\n", run.plate.rows));
    out.push_str(&format!("        <columns>{}</columns>\n", run.plate.cols));
    out.push_str("        <rowLabel>ABC</rowLabel>\n");
    out.push_str("        <columnLabel>123</columnLabel>\n");
    out.push_str("      </pcrFormat>\n");

    for well in &run.wells {
        let id = well.row as u32 * cols as u32 + well.col as u32 + 1;
        out.push_str(&format!("      <react id=\"{id}\">\n"));
        out.push_str(&format!(
            "        <sample id=\"{}\"/>\n",
            esc_attr(&sample_id(well))
        ));
        for ch in &well.channels {
            out.push_str("        <data>\n");
            out.push_str(&format!(
                "          <tar id=\"{}\"/>\n",
                esc_attr(&target_id(ch))
            ));
            if let Some(cq) = ch.cq {
                out.push_str(&format!("          <cq>{}</cq>\n", fmt_f64(cq)));
            }
            for (i, fluor) in ch.amplification.iter().enumerate() {
                if fluor.is_nan() {
                    continue;
                }
                out.push_str("          <adp>\n");
                out.push_str(&format!("            <cyc>{}</cyc>\n", i + 1));
                out.push_str(&format!("            <fluor>{}</fluor>\n", fmt_f64(*fluor)));
                out.push_str("          </adp>\n");
            }
            if let Some(melt) = &ch.melt {
                for (t, f) in melt.temperature.iter().zip(melt.rfu.iter()) {
                    if t.is_nan() || f.is_nan() {
                        continue;
                    }
                    out.push_str("          <mdp>\n");
                    out.push_str(&format!("            <tmp>{}</tmp>\n", fmt_f64(*t)));
                    out.push_str(&format!("            <fluor>{}</fluor>\n", fmt_f64(*f)));
                    out.push_str("          </mdp>\n");
                }
            }
            out.push_str("        </data>\n");
        }
        out.push_str("      </react>\n");
    }

    out.push_str("    </run>\n");
    out.push_str("  </experiment>\n");
    out.push_str("</rdml>\n");
    out
}

/// The RDML sample id (= display name in 1.2) for a well: its sample name, or a
/// position-derived fallback so every react resolves to a definition.
fn sample_id(well: &Well) -> String {
    match &well.sample {
        Some(s) if !s.trim().is_empty() => s.clone(),
        _ => format!("{}{}", row_label(well.row), well.col as usize + 1),
    }
}

/// The RDML target id for a channel: its target name, else the fluorophore.
fn target_id(ch: &Channel) -> String {
    match &ch.target {
        Some(t) if !t.trim().is_empty() => t.clone(),
        _ if !ch.fluorophore.is_empty() => ch.fluorophore.clone(),
        _ => "target".to_string(),
    }
}

fn sample_type_code(ty: SampleType) -> &'static str {
    match ty {
        SampleType::Unknown | SampleType::Empty => "unkn",
        SampleType::Standard => "std",
        SampleType::Ntc => "ntc",
        SampleType::Nrt => "nrt",
        SampleType::PositiveControl => "pos",
        SampleType::NegativeControl => "neg",
    }
}

// ===========================================================================
// Small helpers
// ===========================================================================

fn local_name(e: &BytesStart) -> String {
    String::from_utf8_lossy(e.local_name().as_ref()).into_owned()
}

fn local_name_end(e: &quick_xml::events::BytesEnd) -> String {
    String::from_utf8_lossy(e.local_name().as_ref()).into_owned()
}

fn attr(e: &BytesStart, key: &[u8]) -> Option<String> {
    for a in e.attributes().flatten() {
        if a.key.local_name().as_ref() == key {
            return a
                .unescape_value()
                .ok()
                .map(|c| c.into_owned())
                .or_else(|| Some(String::from_utf8_lossy(&a.value).into_owned()));
        }
    }
    None
}

fn parse_f64(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    t.parse().ok()
}

/// Format an f64 for XML using Rust's shortest round-trippable representation.
fn fmt_f64(x: f64) -> String {
    if x.is_finite() {
        format!("{x}")
    } else {
        "0".to_string()
    }
}

fn esc_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

fn esc_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rdml version="1.2" xmlns="http://www.rdml.org">
  <dye id="FAM"/>
  <sample id="Ctrl">
    <type>unkn</type>
  </sample>
  <sample id="StdA">
    <type>std</type>
    <quantity><value>1000</value><unit>cop</unit></quantity>
  </sample>
  <sample id="Blank">
    <type>ntc</type>
  </sample>
  <target id="GAPDH">
    <type>toi</type>
    <dyeId id="FAM"/>
  </target>
  <experiment id="exp1">
    <run id="run1">
      <pcrFormat>
        <rows>8</rows>
        <columns>12</columns>
        <rowLabel>ABC</rowLabel>
        <columnLabel>123</columnLabel>
      </pcrFormat>
      <react id="1">
        <sample id="Ctrl"/>
        <data>
          <tar id="GAPDH"/>
          <cq>24.5</cq>
          <adp><cyc>1</cyc><fluor>10</fluor></adp>
          <adp><cyc>2</cyc><fluor>20</fluor></adp>
          <adp><cyc>3</cyc><fluor>40</fluor></adp>
          <mdp><tmp>70.0</tmp><fluor>500</fluor></mdp>
          <mdp><tmp>71.0</tmp><fluor>450</fluor></mdp>
        </data>
      </react>
      <react id="B1">
        <sample id="StdA"/>
        <data>
          <tar id="GAPDH"/>
          <adp><cyc>1</cyc><fluor>11</fluor></adp>
        </data>
      </react>
      <react id="3">
        <sample id="Blank"/>
        <data>
          <tar id="GAPDH"/>
        </data>
      </react>
    </run>
  </experiment>
</rdml>
"#;

    #[test]
    fn parses_hand_written_rdml() {
        let run = parse_rdml_xml(SAMPLE_XML).unwrap();
        assert_eq!(run.plate.rows, 8);
        assert_eq!(run.plate.cols, 12);
        assert_eq!(run.wells.len(), 3);

        // react id=1 -> A1 (numeric row-major).
        let a1 = run.wells.iter().find(|w| w.position() == "A1").unwrap();
        assert_eq!(a1.sample.as_deref(), Some("Ctrl"));
        assert_eq!(a1.sample_type, SampleType::Unknown);
        let ch = &a1.channels[0];
        assert_eq!(ch.fluorophore, "FAM"); // via target's dyeId
        assert_eq!(ch.target.as_deref(), Some("GAPDH"));
        assert_eq!(ch.cq, Some(24.5));
        assert_eq!(ch.amplification, vec![10.0, 20.0, 40.0]);
        let melt = ch.melt.as_ref().unwrap();
        assert_eq!(melt.temperature, vec![70.0, 71.0]);
        assert_eq!(melt.rfu, vec![500.0, 450.0]);
        assert!(melt.derivative.is_empty());

        // react id=B1 -> A1-style label parses to B1.
        let b1 = run.wells.iter().find(|w| w.position() == "B1").unwrap();
        assert_eq!(b1.sample_type, SampleType::Standard);
        assert_eq!(b1.starting_quantity, Some(1000.0));

        // react id=3 -> A3: NTC, missing cq -> None, missing adp -> empty.
        let a3 = run.wells.iter().find(|w| w.position() == "A3").unwrap();
        assert_eq!(a3.sample_type, SampleType::Ntc);
        assert_eq!(a3.channels[0].cq, None);
        assert!(a3.channels[0].amplification.is_empty());
        assert!(a3.channels[0].melt.is_none());
    }

    #[test]
    fn amplification_gap_fill() {
        // Missing cyc 2 becomes NaN between the two provided points.
        let out = build_amplification(&[(1.0, 10.0), (3.0, 30.0)]);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], 10.0);
        assert!(out[1].is_nan());
        assert_eq!(out[2], 30.0);
    }

    #[test]
    fn cq_only_file_has_no_curves() {
        // A valid RDML with only <cq> (no adp/mdp) must parse without error.
        let xml = r#"<rdml version="1.3" xmlns="http://www.rdml.org">
  <sample id="S1"><type>unkn</type></sample>
  <target id="T1"><type>toi</type><dyeId id="FAM"/></target>
  <experiment id="e"><run id="r">
    <pcrFormat><rows>16</rows><columns>24</columns><rowLabel>ABC</rowLabel><columnLabel>123</columnLabel></pcrFormat>
    <react id="1"><sample id="S1"/><data><tar id="T1"/><cq>31.2</cq></data></react>
  </run></experiment>
</rdml>"#;
        let run = parse_rdml_xml(xml).unwrap();
        assert_eq!(run.plate.rows, 16);
        assert_eq!(run.plate.cols, 24);
        let ch = &run.wells[0].channels[0];
        assert_eq!(ch.cq, Some(31.2));
        assert!(ch.amplification.is_empty());
        assert!(ch.melt.is_none());
        assert_eq!(run.metadata.cycle_count, None);
    }

    #[test]
    fn rotor_single_column_numeric_ids() {
        // columns=1 => a rotor/tube instrument; numeric ids map to successive rows.
        let xml = r#"<rdml version="1.1" xmlns="http://www.rdml.org">
  <sample id="S1"><type>unkn</type></sample>
  <target id="T1"><type>toi</type><dyeId id="FAM"/></target>
  <experiment id="e"><run id="r">
    <pcrFormat><rows>36</rows><columns>1</columns><rowLabel>123</rowLabel><columnLabel>123</columnLabel></pcrFormat>
    <react id="1"><sample id="S1"/><data><tar id="T1"/><cq>20</cq></data></react>
    <react id="5"><sample id="S1"/><data><tar id="T1"/><cq>21</cq></data></react>
  </run></experiment>
</rdml>"#;
        let run = parse_rdml_xml(xml).unwrap();
        assert_eq!(run.plate.cols, 1);
        // id 1 -> row 0, col 0 (A1); id 5 -> row 4, col 0 (E1).
        assert!(run.wells.iter().any(|w| w.position() == "A1"));
        assert!(run.wells.iter().any(|w| w.position() == "E1"));
    }

    #[test]
    fn reads_zip_with_nonstandard_member_name() {
        // Bio-Rad names the member BioRad_qPCR_melt.xml, not rdml_data.xml.
        let path =
            std::env::temp_dir().join(format!("openqpcr_rdml_biorad_{}.rdml", std::process::id()));
        {
            let file = File::create(&path).unwrap();
            let mut zip = ZipWriter::new(file);
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            zip.start_file("BioRad_qPCR_melt.xml", opts).unwrap();
            zip.write_all(SAMPLE_XML.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        assert!(is_rdml_zip(&path).unwrap());
        let run = read_rdml(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(run.wells.len(), 3);
    }

    #[test]
    fn negative_cq_is_not_available() {
        let xml = SAMPLE_XML.replace("<cq>24.5</cq>", "<cq>-1.0</cq>");
        let run = parse_rdml_xml(&xml).unwrap();
        let a1 = run.wells.iter().find(|w| w.position() == "A1").unwrap();
        assert_eq!(a1.channels[0].cq, None);
    }

    #[test]
    fn round_trip_via_zip() {
        let run = QpcrRun {
            metadata: RunMetadata {
                instrument: Some("CFX Opus 96".to_string()),
                ..Default::default()
            },
            plate: PlateFormat::P96,
            wells: vec![
                Well {
                    row: 0,
                    col: 0,
                    sample: Some("Ctrl".to_string()),
                    sample_type: SampleType::Unknown,
                    starting_quantity: None,
                    channels: vec![Channel {
                        fluorophore: "FAM".to_string(),
                        target: Some("GAPDH".to_string()),
                        cq: Some(22.1),
                        amplification: vec![1.0, 2.5, 8.0, 30.0],
                        melt: Some(MeltCurve {
                            temperature: vec![65.0, 66.0, 67.0],
                            rfu: vec![900.0, 700.0, 300.0],
                            derivative: Vec::new(),
                            peaks: Vec::new(),
                        }),
                    }],
                    ..Default::default()
                },
                Well {
                    row: 1,
                    col: 3,
                    sample: Some("StdA".to_string()),
                    sample_type: SampleType::Standard,
                    starting_quantity: Some(1e4),
                    channels: vec![Channel {
                        fluorophore: "HEX".to_string(),
                        target: Some("ACTB".to_string()),
                        cq: None,
                        amplification: vec![0.5, 1.0],
                        melt: None,
                    }],
                    ..Default::default()
                },
            ],
            protocol: None,
        };

        let path = std::env::temp_dir().join(format!(
            "openqpcr_rdml_roundtrip_{}.rdml",
            std::process::id()
        ));
        write_rdml(&run, &path).unwrap();
        assert!(is_rdml_zip(&path).unwrap());
        let back = read_rdml(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(back.wells.len(), 2);
        assert_eq!(back.metadata.instrument.as_deref(), Some("CFX Opus 96"));

        let a1 = back.wells.iter().find(|w| w.position() == "A1").unwrap();
        assert_eq!(a1.sample_type, SampleType::Unknown);
        assert_eq!(a1.starting_quantity, None);
        let ch = &a1.channels[0];
        assert_eq!(ch.fluorophore, "FAM");
        assert_eq!(ch.target.as_deref(), Some("GAPDH"));
        assert_eq!(ch.cq, Some(22.1));
        assert_eq!(ch.amplification, vec![1.0, 2.5, 8.0, 30.0]);
        let melt = ch.melt.as_ref().unwrap();
        assert_eq!(melt.rfu, vec![900.0, 700.0, 300.0]);
        assert_eq!(melt.temperature, vec![65.0, 66.0, 67.0]);

        let b4 = back.wells.iter().find(|w| w.position() == "B4").unwrap();
        assert_eq!(b4.sample_type, SampleType::Standard);
        assert_eq!(b4.starting_quantity, Some(1e4));
        assert_eq!(b4.channels[0].fluorophore, "HEX");
        assert_eq!(b4.channels[0].cq, None);
        assert_eq!(b4.channels[0].amplification, vec![0.5, 1.0]);
    }

    #[test]
    fn thermal_protocol_round_trips_through_rdml() {
        use crate::protocol::{GradientStep, ProtocolStep, TemperatureStep};
        // A protocol exercising Hold (with ramp/touchdown/measure), a cycling
        // Loop, a Gradient, a Pause, and a LidOpen — no Melt (which is lossy on
        // write by design). Named so its RDML id round-trips.
        let protocol = ThermalProtocol {
            name: Some("test-protocol".into()),
            description: Some("round-trip".into()),
            lid_temperature: Some(105.0),
            steps: vec![
                ProtocolStep::Hold(TemperatureStep {
                    target_c: 95.0,
                    hold_secs: Some(180.0),
                    ramp_c_per_s: None,
                    temperature_change: 0.0,
                    duration_change: 0.0,
                    measure: Measure::None,
                }),
                ProtocolStep::Hold(TemperatureStep {
                    target_c: 60.0,
                    hold_secs: Some(30.0),
                    ramp_c_per_s: Some(2.5),
                    temperature_change: -0.5,
                    duration_change: 1.0,
                    measure: Measure::Real,
                }),
                ProtocolStep::Loop {
                    goto: 2,
                    repeat: 40,
                },
                ProtocolStep::Gradient(GradientStep {
                    high_c: 65.0,
                    low_c: 55.0,
                    hold_secs: Some(20.0),
                    ramp_c_per_s: None,
                    measure: Measure::Real,
                }),
                ProtocolStep::Pause { temperature: 4.0 },
                ProtocolStep::LidOpen,
            ],
        };
        let run = QpcrRun {
            protocol: Some(protocol.clone()),
            ..Default::default()
        };
        let xml = build_rdml_xml(&run);
        assert!(xml.contains("<thermalCyclingConditions"));
        let back = parse_rdml_xml(&xml).unwrap();
        assert_eq!(back.protocol.as_ref(), Some(&protocol));
    }
}
