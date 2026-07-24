//! Native Bio-Rad `.pcrd` reader.
//!
//! A `.pcrd` file is a ZIP container (magic `PK\x03\x04`) holding a single inner
//! entry — a GUID-named `.pcrd` — encrypted with **traditional PKWARE ZipCrypto**.
//! The password ([`INNER_PASSWORD`]) is a fixed constant baked into CFX Manager
//! (`BioRad.Common.dll`); it is the same across all non-user-protected files.
//! Once decrypted the inner entry is plaintext UTF-8 XML rooted at
//! `<experimentalData2>`, which [`read_file`] maps into a [`QpcrRun`].
//!
//! Reverse-engineered from the CFX Manager install and verified end-to-end
//! against real instrument files. Layout of the XML we consume:
//! * `plateSetup2` — plate geometry, plate/consumable type, scan mode, dyes;
//! * `plateSetup2` `wellSample`s — per-well target (`geneName`), sample
//!   (`conditionName`), group (`condition2Name`), type, starting quantity;
//! * `protocol2` — the thermal program (temperature / goto / melt steps);
//! * `runData/plateReadDataVector` — per-cycle raw optics: each `plateRead`
//!   carries a `PAr` array laid out **channel-major** as
//!   `[channel][well-position][mean, sd, min, max]` over `NumRows×NumCols`
//!   positions (the last row is instrument reference, not plate wells).
//!
//! A file that carries an additional user-set open password (a separate feature
//! from the global key) cannot be decrypted here and yields [`QpcrError::Encrypted`].
//! [`inspect`] still enumerates any archive without interpreting it.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use zip::ZipArchive;
use zip::result::ZipError;

use crate::error::{QpcrError, Result};
use crate::model::{Channel, MeltCurve, PlateFormat, QpcrRun, RunMetadata, SampleType, Well};
use crate::protocol::{MeltStep, ProtocolStep, TemperatureStep, ThermalProtocol};

/// Fixed ZipCrypto password for the inner `.pcrd` entry, baked into CFX Manager
/// (`BioRad.Common.dll`, constant `c_EncryptPWD` / `DefaultEncryptionPassword`).
/// The "iQ5V4" name is a historical artefact of the iQ5 software lineage; the
/// constant is unchanged in current CFX Manager builds.
///
/// **Interoperability rationale.** This is a single global obfuscation key that
/// CFX applies to *every* non-user-protected `.pcrd`; it is not a per-user secret
/// and grants access to nothing but the file's own contents. It is included here
/// solely to let an independently-developed program read qPCR runs the user
/// already owns — the classic interoperability purpose (US DMCA §1201(f); EU
/// Software Directive Art. 6). It does not defeat any per-file *user* password
/// (see [`QpcrError::Encrypted`]), and enables reading, not redistribution, of
/// Bio-Rad's format.
pub const INNER_PASSWORD: &[u8] = b"SecureCompressDecompressKeyiQ5V4Files!!##$$";

/// Number of statistics per (channel, position) in a `PAr` optics array:
/// mean fluorescence, standard deviation, minimum, maximum — in that order.
const STATS_PER_POSITION: usize = 4;

/// Cheap check: does the file start with a ZIP signature?
///
/// Accepts the whole `PK` signature family, not just the local-file-header
/// `PK\x03\x04`: some writers (e.g. Bio-Rad CFX) prefix the optional spanning /
/// data-descriptor marker `PK\x07\x08`, and an empty archive begins with the
/// end-of-central-directory `PK\x05\x06`. A strict local-header check misreports
/// such a valid `.pcrd` as "not a ZIP".
pub fn looks_like_zip(path: &Path) -> Result<bool> {
    let mut f = File::open(path)?;
    let mut magic = [0u8; 4];
    match f.read_exact(&mut magic) {
        Ok(()) => Ok(matches!(
            &magic,
            b"PK\x03\x04" | b"PK\x07\x08" | b"PK\x05\x06"
        )),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// A description of one entry inside a `.pcrd` archive.
#[derive(Debug, Clone)]
pub struct ArchiveEntry {
    pub name: String,
    pub size: u64,
    pub compressed_size: u64,
    pub is_encrypted: bool,
    /// First bytes of the (decompressed) entry, for format sniffing.
    pub head: Vec<u8>,
    /// Best-guess content kind based on `head`.
    pub kind: EntryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Xml,
    Zip,
    /// Likely a run of little-endian floats / structured binary.
    Binary,
    Text,
    Unknown,
}

impl EntryKind {
    fn sniff(head: &[u8]) -> EntryKind {
        let trimmed: &[u8] = {
            let mut i = 0;
            while i < head.len() && (head[i] as char).is_whitespace() {
                i += 1;
            }
            &head[i..]
        };
        if trimmed.starts_with(b"<?xml") || trimmed.starts_with(b"<") {
            EntryKind::Xml
        } else if head.starts_with(b"PK\x03\x04") {
            EntryKind::Zip
        } else if head.is_ascii() && !head.is_empty() {
            EntryKind::Text
        } else if head.is_empty() {
            EntryKind::Unknown
        } else {
            EntryKind::Binary
        }
    }
}

/// A full inspection report for a `.pcrd` file — the reverse-engineering entry point.
#[derive(Debug, Clone)]
pub struct Inspection {
    pub encrypted: bool,
    pub entries: Vec<ArchiveEntry>,
}

/// Enumerate the contents of a `.pcrd` archive without interpreting them.
///
/// This is the tool used to reverse-engineer the format: point it at a real
/// sample file and it prints every entry, its size, and a content sniff.
pub fn inspect(path: &Path) -> Result<Inspection> {
    let file = File::open(path)?;
    let mut archive = ZipArchive::new(file)?;

    // Pre-collect entry names via the raw index (never decrypts), so we can still
    // name an entry even when `by_index` fails on encryption.
    let names: Vec<String> = (0..archive.len())
        .map(|i| {
            archive
                .by_index_raw(i)
                .map(|z| z.name().to_string())
                .unwrap_or_else(|_| format!("<entry {i}>"))
        })
        .collect();

    let mut entries = Vec::with_capacity(archive.len());
    let mut any_encrypted = false;

    for i in 0..archive.len() {
        // `by_index` errors on encrypted entries without a password; fall back
        // to metadata-only reading so we can still report the entry.
        match archive.by_index(i) {
            Ok(mut zf) => {
                let mut head = vec![0u8; 64];
                let n = zf.read(&mut head).unwrap_or(0);
                head.truncate(n);
                let kind = EntryKind::sniff(&head);
                entries.push(ArchiveEntry {
                    name: zf.name().to_string(),
                    size: zf.size(),
                    compressed_size: zf.compressed_size(),
                    is_encrypted: false,
                    head,
                    kind,
                });
            }
            Err(zip::result::ZipError::UnsupportedArchive(msg)) if is_encryption_error(msg) => {
                any_encrypted = true;
                entries.push(ArchiveEntry {
                    name: names
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| format!("<entry {i}>")),
                    size: 0,
                    compressed_size: 0,
                    is_encrypted: true,
                    head: Vec::new(),
                    kind: EntryKind::Unknown,
                });
            }
            Err(e) => return Err(e.into()),
        }
    }

    Ok(Inspection {
        encrypted: any_encrypted,
        entries,
    })
}

/// Read the full bytes of a named entry from the archive.
pub fn read_entry<R: Read + Seek>(archive: &mut ZipArchive<R>, name: &str) -> Result<Vec<u8>> {
    let mut zf = archive.by_name(name)?;
    let mut buf = Vec::with_capacity(zf.size() as usize);
    zf.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Parse a `.pcrd` file into a [`QpcrRun`].
///
/// Decrypts the inner entry with the global [`INNER_PASSWORD`] and maps its
/// `<experimentalData2>` XML into the shared model. Returns
/// [`QpcrError::Encrypted`] when the file carries an additional user-set open
/// password (which the global key cannot unlock).
pub fn read_file(path: &Path) -> Result<QpcrRun> {
    let xml = decrypt_inner_xml(path)?.ok_or(QpcrError::Encrypted)?;
    // The inner XML is UTF-8 with a BOM; strip it so the parser sees `<?xml`.
    let text = String::from_utf8_lossy(strip_bom(&xml));
    let mut run = parse_experiment(&text)?;
    run.metadata.source_file = Some(path.display().to_string());
    Ok(run)
}

/// Open the outer ZIP and decrypt the inner run entry with [`INNER_PASSWORD`].
///
/// Returns `Ok(None)` when no entry decrypts with the global key (i.e. the file
/// is protected by a user-chosen password), so the caller can report it as
/// [`QpcrError::Encrypted`] rather than a hard error.
fn decrypt_inner_xml(path: &Path) -> Result<Option<Vec<u8>>> {
    let file = File::open(path)?;
    let mut archive = ZipArchive::new(file)?;
    for i in 0..archive.len() {
        match archive.by_index_decrypt(i, INNER_PASSWORD) {
            Ok(mut zf) => {
                let mut buf = Vec::with_capacity(zf.size() as usize);
                zf.read_to_end(&mut buf)?;
                if looks_like_xml(strip_bom(&buf)) {
                    return Ok(Some(buf));
                }
                // Not the XML payload (e.g. an auxiliary entry) — keep scanning.
            }
            // Wrong password for this entry: the file uses user protection.
            Err(ZipError::InvalidPassword) => return Ok(None),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(None)
}

fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)
}

fn looks_like_xml(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(64)];
    let trimmed = head
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .map_or(&[][..], |i| &head[i..]);
    trimmed.starts_with(b"<?xml") || trimmed.starts_with(b"<experiment") || trimmed.starts_with(b"<")
}

/// One raw optics read (one cycle of one program step across the whole plate).
struct PlateReadData {
    /// Program-step index this read belongs to (amplification vs melt vs …).
    step: i32,
    /// 1-based ordinal within the step (amplification cycle, or melt point).
    cycle: i32,
    rows: usize,
    cols: usize,
    /// Flattened `PAr` values, channel-major: `[ch][pos][mean,sd,min,max]`.
    values: Vec<f64>,
}

impl PlateReadData {
    fn positions(&self) -> usize {
        self.rows * self.cols
    }

    /// Mean fluorescence for a plate position on a channel, or `None` when the
    /// index is out of range for this read's array.
    fn mean(&self, channel: usize, pos: usize) -> Option<f64> {
        let idx = (channel * self.positions() + pos) * STATS_PER_POSITION;
        self.values.get(idx).copied()
    }
}

/// Per-well sample assignment scraped from `plateSetup2`.
#[derive(Default, Clone)]
struct WellAssignment {
    target: Option<String>,
    sample: Option<String>,
    group: Option<String>,
    sample_type: SampleType,
    starting_quantity: Option<f64>,
}

/// Streaming parser over the decrypted `<experimentalData2>` document.
fn parse_experiment(xml: &str) -> Result<QpcrRun> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut meta = RunMetadata::default();
    let mut rows: usize = 0;
    let mut cols: usize = 0;
    let mut dyes: Vec<String> = Vec::new();
    // plateIndex -> ordered sample assignments (one per loaded fluor/channel).
    let mut assignments: HashMap<usize, Vec<WellAssignment>> = HashMap::new();
    let mut protocol = ThermalProtocol::default();
    let mut melt_step_number: Option<i32> = None;
    let mut reads: Vec<PlateReadData> = Vec::new();
    // Threshold cycles (Cq) keyed by plate position, when the file stored them.
    let mut cqs: HashMap<usize, f64> = HashMap::new();

    // Lightweight context tracking.
    let mut path: Vec<String> = Vec::new();
    let mut cur_read: Option<PlateReadData> = None;
    let mut cur_pars: Vec<Vec<f64>> = Vec::new();
    let mut header_seen = false;

    loop {
        match reader.read_event()? {
            Event::Start(e) => {
                let name = local_name(&e);
                {
                    let mut ctx = HandlerCtx {
                        meta: &mut meta,
                        rows: &mut rows,
                        cols: &mut cols,
                        dyes: &mut dyes,
                        assignments: &mut assignments,
                        protocol: &mut protocol,
                        melt_step_number: &mut melt_step_number,
                        cqs: &mut cqs,
                        header_seen: &mut header_seen,
                    };
                    handle_element(&e, &name, &path, &mut ctx)?;
                }
                if name == "plateRead" {
                    cur_read = Some(PlateReadData {
                        step: 0,
                        cycle: 0,
                        rows: 0,
                        cols: 0,
                        values: Vec::new(),
                    });
                    cur_pars.clear();
                }
                path.push(name);
            }
            Event::Empty(e) => {
                let name = local_name(&e);
                let mut ctx = HandlerCtx {
                    meta: &mut meta,
                    rows: &mut rows,
                    cols: &mut cols,
                    dyes: &mut dyes,
                    assignments: &mut assignments,
                    protocol: &mut protocol,
                    melt_step_number: &mut melt_step_number,
                    cqs: &mut cqs,
                    header_seen: &mut header_seen,
                };
                handle_element(&e, &name, &path, &mut ctx)?;
                // Self-contained: do not push onto the context path.
            }
            Event::End(_) => {
                let closing = path.pop();
                if closing.as_deref() == Some("plateRead") {
                    if let Some(mut rd) = cur_read.take() {
                        // The data array is the longest PAr in this read (the
                        // short one is the instrument reference reading).
                        if let Some(longest) = cur_pars
                            .iter()
                            .max_by_key(|v| v.len())
                            .filter(|v| !v.is_empty())
                        {
                            rd.values = longest.clone();
                            reads.push(rd);
                        }
                    }
                    cur_pars.clear();
                }
            }
            Event::Text(e) => {
                if let Some(field) = path.last() {
                    let text = e.unescape().unwrap_or_default();
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        apply_text(field, &text, &mut cur_read, &mut cur_pars, &mut meta);
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    assemble_run(
        meta,
        rows,
        cols,
        dyes,
        assignments,
        protocol,
        melt_step_number,
        reads,
        cqs,
    )
}

/// Mutable parser state threaded through [`handle_element`].
struct HandlerCtx<'a> {
    meta: &'a mut RunMetadata,
    rows: &'a mut usize,
    cols: &'a mut usize,
    dyes: &'a mut Vec<String>,
    assignments: &'a mut HashMap<usize, Vec<WellAssignment>>,
    protocol: &'a mut ThermalProtocol,
    melt_step_number: &'a mut Option<i32>,
    cqs: &'a mut HashMap<usize, f64>,
    header_seen: &'a mut bool,
}

fn local_name(e: &BytesStart) -> String {
    String::from_utf8_lossy(e.local_name().as_ref()).into_owned()
}

fn get_attr(e: &BytesStart, key: &str) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        (a.key.local_name().as_ref() == key.as_bytes())
            .then(|| a.unescape_value().ok().map(|v| v.into_owned()))
            .flatten()
    })
}

fn nonempty(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.is_empty())
}

fn extract_between(hay: &str, start: &str, end: &str) -> Option<String> {
    let s = hay.find(start)? + start.len();
    let e = hay[s..].find(end)? + s;
    Some(hay[s..e].to_string())
}

/// Map a CFX `wellSampleType` code (`wcSample`, `wcStandard`, `wcNTC`, …) to the
/// shared [`SampleType`]. The `wc` prefix is stripped and matched generously.
fn map_sample_type(wc: &str) -> SampleType {
    let t = wc.strip_prefix("wc").unwrap_or(wc).to_ascii_lowercase();
    match t.as_str() {
        _ if t.contains("standard") => SampleType::Standard,
        _ if t.contains("ntc") || t.contains("notemplate") => SampleType::Ntc,
        _ if t.contains("nrt") || t.contains("nort") => SampleType::Nrt,
        _ if t.contains("pos") => SampleType::PositiveControl,
        _ if t.contains("neg") => SampleType::NegativeControl,
        _ if t.contains("nosample") || t.contains("empty") || t.contains("blank") => {
            SampleType::Empty
        }
        // "wcSample" and unknown codes fall back to Unknown.
        _ => SampleType::Unknown,
    }
}

fn parse_par(text: &str) -> Vec<f64> {
    text.split(';')
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.trim().parse::<f64>().ok())
        .collect()
}

/// Dispatch a Start/Empty element into the parser state.
fn handle_element(e: &BytesStart, name: &str, path: &[String], ctx: &mut HandlerCtx) -> Result<()> {
    match name {
        "plateSetup2" => {
            if let Some(r) = get_attr(e, "rows").and_then(|s| s.parse().ok()) {
                *ctx.rows = r;
            }
            if let Some(c) = get_attr(e, "columns").and_then(|s| s.parse().ok()) {
                *ctx.cols = c;
            }
            if let Some(pt) = nonempty(get_attr(e, "plateName")) {
                ctx.meta.plate_type = Some(pt);
            }
        }
        "dyeLayer" => {
            if let Some(n) = nonempty(get_attr(e, "plateName")) {
                ctx.dyes.push(n);
            }
        }
        "protocol2" => {
            if let Some(lt) = get_attr(e, "lidTemperature").and_then(|s| s.parse().ok()) {
                ctx.protocol.lid_temperature = Some(lt);
            }
        }
        "TemperatureStep" => {
            if let Some(t) = get_attr(e, "temperatureStepTemp").and_then(|s| s.parse().ok()) {
                let h = get_attr(e, "temperatureStepHoldTime")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                ctx.protocol
                    .steps
                    .push(ProtocolStep::Hold(TemperatureStep::hold(t, h)));
            }
        }
        "GotoStep" => {
            // CFX `optionGotoStep` is a 0-based index; our `goto` is a 1-based step
            // number (matching the RDML reader). `optionGotoCycle` is the count of
            // *additional* repeats, so total iterations = cycle + 1.
            let goto = get_attr(e, "optionGotoStep")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0);
            let cyc = get_attr(e, "optionGotoCycle")
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0);
            ctx.protocol.steps.push(ProtocolStep::Loop {
                goto: goto + 1,
                repeat: cyc + 1,
            });
        }
        "MeltCurveStep" => {
            let f = |k: &str, d: f64| get_attr(e, k).and_then(|s| s.parse().ok()).unwrap_or(d);
            ctx.protocol.steps.push(ProtocolStep::Melt(MeltStep {
                start_c: f("meltCurveStartTemp", 0.0),
                end_c: f("meltCurveEndTemp", 0.0),
                increment_c: f("meltCurveTemperatureIncrement", 0.5),
                hold_secs: f("meltCurveHoldTime", 0.0),
            }));
            *ctx.melt_step_number = get_attr(e, "meltCurveStepNumber").and_then(|s| s.parse().ok());
        }
        "wellSample" => {
            if let Some(pi) = get_attr(e, "plateIndex").and_then(|s| s.parse::<usize>().ok()) {
                let sample_type = get_attr(e, "wellSampleType")
                    .map(|s| map_sample_type(&s))
                    .unwrap_or_default();
                let sq = get_attr(e, "sampleQuantity")
                    .and_then(|s| s.parse::<f64>().ok())
                    .filter(|v| v.is_finite());
                ctx.assignments.entry(pi).or_default().push(WellAssignment {
                    target: nonempty(get_attr(e, "geneName")),
                    sample: nonempty(get_attr(e, "conditionName")),
                    group: nonempty(get_attr(e, "condition2Name")),
                    sample_type,
                    starting_quantity: sq,
                });
            }
        }
        "computedWellData" => {
            if let (Some(pi), Some(tc)) = (
                get_attr(e, "pIndex").and_then(|s| s.parse::<usize>().ok()),
                get_attr(e, "thresholdCycle").and_then(|s| s.parse::<f64>().ok()),
            )
                && tc.is_finite() {
                    ctx.cqs.insert(pi, tc);
                }
        }
        "appFile" => {
            if get_attr(e, "name").as_deref() == Some("BioRadCFXManager.exe")
                && let Some(v) = nonempty(get_attr(e, "version")) {
                    ctx.meta.software_version = Some(v);
                }
        }
        "auditHeader" => {
            if ctx.meta.operator.is_none() {
                ctx.meta.operator =
                    nonempty(get_attr(e, "fullUserName")).or_else(|| nonempty(get_attr(e, "user")));
            }
        }
        "header" => {
            // The experiment-level header is the first one, directly under the root.
            if !*ctx.header_seen && path.last().map(String::as_str) == Some("experimentalData2") {
                *ctx.header_seen = true;
                if ctx.meta.run_started.is_none() {
                    ctx.meta.run_started = nonempty(get_attr(e, "createdDate"));
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Fold an element's text content into the current plate read (and sniff the
/// instrument model, which is embedded as escaped XML elsewhere in the document).
fn apply_text(
    field: &str,
    text: &str,
    cur_read: &mut Option<PlateReadData>,
    cur_pars: &mut Vec<Vec<f64>>,
    meta: &mut RunMetadata,
) {
    if meta.instrument.is_none()
        && let Some(model) = extract_between(text, "<BLOCKTYPE>", "</BLOCKTYPE>") {
            meta.instrument = nonempty(Some(model.trim().to_string()));
        }
    let Some(rd) = cur_read.as_mut() else {
        return;
    };
    match field {
        "Step" => {
            if let Ok(v) = text.parse() {
                rd.step = v;
            }
        }
        "Cycle" => {
            if let Ok(v) = text.parse() {
                rd.cycle = v;
            }
        }
        "NumRows" => {
            if let Ok(v) = text.parse() {
                rd.rows = v;
            }
        }
        "NumCols" => {
            if let Ok(v) = text.parse() {
                rd.cols = v;
            }
        }
        "PAr" => cur_pars.push(parse_par(text)),
        "BaseSerNum" => {
            if meta.serial_number.is_none() {
                meta.serial_number = Some(text.to_string());
            }
        }
        _ => {}
    }
}

/// Turn the scraped intermediates into a [`QpcrRun`].
#[allow(clippy::too_many_arguments)]
fn assemble_run(
    mut meta: RunMetadata,
    rows: usize,
    cols: usize,
    dyes: Vec<String>,
    assignments: HashMap<usize, Vec<WellAssignment>>,
    protocol: ThermalProtocol,
    melt_step_number: Option<i32>,
    reads: Vec<PlateReadData>,
    cqs: HashMap<usize, f64>,
) -> Result<QpcrRun> {
    // Plate geometry: prefer plateSetup2; fall back to a read's grid minus the
    // instrument reference row, then to a 96-well default.
    let (prows, pcols) = if rows > 0 && cols > 0 {
        (rows, cols)
    } else if let Some(rd) = reads.first() {
        (rd.rows.saturating_sub(1).max(1), rd.cols.max(1))
    } else {
        (8, 12)
    };
    let plate = PlateFormat {
        rows: prows as u8,
        cols: pcols as u8,
    };

    // Partition reads into melt (matching the melt program step) vs amplification.
    let (mut melt_reads, mut amp_reads): (Vec<_>, Vec<_>) = reads
        .into_iter()
        .partition(|rd| melt_step_number.is_some() && Some(rd.step) == melt_step_number);
    amp_reads.sort_by_key(|rd| rd.cycle);
    melt_reads.sort_by_key(|rd| rd.cycle);

    // Melt temperature axis from the protocol's Melt step, if present.
    let melt_axis: Option<(f64, f64)> = protocol.steps.iter().find_map(|s| match s {
        ProtocolStep::Melt(m) => Some((m.start_c, m.increment_c)),
        _ => None,
    });

    // One channel per dye; default to a single unnamed channel.
    let channel_names: Vec<String> = if dyes.is_empty() {
        vec!["Channel 1".to_string()]
    } else {
        dyes
    };

    let well_count = prows * pcols;
    let mut wells = Vec::with_capacity(well_count);
    for pos in 0..well_count {
        let row = (pos / pcols) as u8;
        let col = (pos % pcols) as u8;
        let assigns = assignments.get(&pos);
        let first = assigns.and_then(|a| a.first());

        let mut channels = Vec::with_capacity(channel_names.len());
        for (c, fluor) in channel_names.iter().enumerate() {
            let amplification: Vec<f64> = amp_reads.iter().filter_map(|rd| rd.mean(c, pos)).collect();
            let has_amp = !amp_reads.is_empty() && amplification.len() == amp_reads.len();

            let melt = if !melt_reads.is_empty() {
                let rfu: Vec<f64> = melt_reads.iter().filter_map(|rd| rd.mean(c, pos)).collect();
                if rfu.len() == melt_reads.len() {
                    let temperature = match melt_axis {
                        Some((start, inc)) => {
                            (0..rfu.len()).map(|i| start + i as f64 * inc).collect()
                        }
                        None => Vec::new(),
                    };
                    Some(MeltCurve {
                        temperature,
                        rfu,
                        derivative: Vec::new(),
                        peaks: Vec::new(),
                    })
                } else {
                    None
                }
            } else {
                None
            };

            // Skip channels that carry no data for this run (e.g. dark channels).
            if !has_amp && melt.is_none() {
                continue;
            }

            let target = assigns
                .and_then(|a| a.get(c).or_else(|| a.first()))
                .and_then(|w| w.target.clone());
            channels.push(Channel {
                fluorophore: fluor.clone(),
                target,
                cq: if c == 0 { cqs.get(&pos).copied() } else { None },
                amplification: if has_amp { amplification } else { Vec::new() },
                melt,
            });
        }

        wells.push(Well {
            row,
            col,
            sample: first.and_then(|w| w.sample.clone()),
            sample_type: first.map(|w| w.sample_type).unwrap_or_default(),
            biological_group: first.and_then(|w| w.group.clone()),
            starting_quantity: first.and_then(|w| w.starting_quantity),
            channels,
        });
    }

    meta.cycle_count = Some(amp_reads.len());
    let protocol = (!protocol.steps.is_empty()).then_some(protocol);

    Ok(QpcrRun {
        metadata: meta,
        plate,
        wells,
        protocol,
    })
}

fn is_encryption_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("password") || message.contains("encrypt") || message.contains("decrypt")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_zip_encryption_errors() {
        assert!(is_encryption_error("Password required to decrypt file"));
        assert!(is_encryption_error("encrypted file"));
        assert!(is_encryption_error("missing password"));
        assert!(!is_encryption_error("unsupported compression method"));
    }

    #[test]
    fn looks_like_zip_accepts_pk_signature_family() {
        // Bio-Rad CFX writes the optional spanning marker `PK\x07\x08` ahead of
        // the local file header; a strict `PK\x03\x04` check misreports it.
        let dir = std::env::temp_dir();
        for (name, magic, expect) in [
            ("local", &b"PK\x03\x04"[..], true),
            ("spanning", &b"PK\x07\x08"[..], true),
            ("empty_eocd", &b"PK\x05\x06"[..], true),
            ("not_zip", &b"%PDF"[..], false),
        ] {
            let path = dir.join(format!("openqpcr_pcrd_magic_{}_{name}", std::process::id()));
            std::fs::write(&path, magic).unwrap();
            let got = looks_like_zip(&path).unwrap();
            let _ = std::fs::remove_file(&path);
            assert_eq!(got, expect, "{name}");
        }
    }

    #[test]
    fn maps_cfx_well_sample_types() {
        assert_eq!(map_sample_type("wcSample"), SampleType::Unknown);
        assert_eq!(map_sample_type("wcStandard"), SampleType::Standard);
        assert_eq!(map_sample_type("wcNTC"), SampleType::Ntc);
        assert_eq!(map_sample_type("wcNRT"), SampleType::Nrt);
        assert_eq!(map_sample_type("wcPositiveControl"), SampleType::PositiveControl);
        assert_eq!(map_sample_type("wcNegativeControl"), SampleType::NegativeControl);
        assert_eq!(map_sample_type("wcNoSample"), SampleType::Empty);
    }

    #[test]
    fn parses_par_and_indexes_channel_major() {
        // 2 positions × 2 channels × 4 stats, channel-major:
        // ch0 -> positions 0,1 ; ch1 -> positions 0,1.
        let par = parse_par("10;0;9;11;20;0;19;21;30;0;29;31;40;0;39;41");
        assert_eq!(par.len(), 16);
        let rd = PlateReadData {
            step: 3,
            cycle: 1,
            rows: 1,
            cols: 2,
            values: par,
        };
        assert_eq!(rd.positions(), 2);
        assert_eq!(rd.mean(0, 0), Some(10.0)); // ch0 pos0
        assert_eq!(rd.mean(0, 1), Some(20.0)); // ch0 pos1
        assert_eq!(rd.mean(1, 0), Some(30.0)); // ch1 pos0
        assert_eq!(rd.mean(1, 1), Some(40.0)); // ch1 pos1
        assert_eq!(rd.mean(2, 0), None); // out of range
    }

    /// A compact synthetic `<experimentalData2>` exercising the full mapping
    /// without needing a real (large, non-committed) instrument file.
    fn synthetic_experiment() -> String {
        // Two amplification reads (step 3, cycles 1 & 2) and one melt read
        // (step 6), each a 1×2 grid, single channel, 4 stats/position.
        let amp1 = "100;1;99;101;110;1;109;111";
        let amp2 = "200;1;199;201;210;1;209;211";
        let melt = "50;1;49;51;51;1;50;52";
        let read = |step: i32, cycle: i32, par: &str| {
            format!(
                "<plateRead><PlateRead V=\"1\"><Hdr><PlateReadDataHeader>\
                 <BaseSerNum>BR9</BaseSerNum><Step>{step}</Step><Cycle>{cycle}</Cycle>\
                 <NumRows>1</NumRows><NumCols>2</NumCols><ChCount>1</ChCount>\
                 </PlateReadDataHeader></Hdr><Data><PAr V=\"1\">{par}</PAr></Data></PlateRead></plateRead>"
            )
        };
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
             <experimentalData2>\
             <header name=\"datafile.pcrd\" createdDate=\"2023-02-15T14:21:22\" />\
             <plateSetup2 rows=\"1\" columns=\"2\" plateName=\"BR Clear\">\
               <dyeLayersList><dyeLayer plateName=\"SYBR\" /></dyeLayersList>\
               <wellSample plateIndex=\"0\" wellSampleType=\"wcSample\" geneName=\"srp\" conditionName=\"S1\" condition2Name=\"G1\" sampleQuantity=\"NaN\" />\
               <wellSample plateIndex=\"1\" wellSampleType=\"wcStandard\" geneName=\"Taf4\" conditionName=\"S2\" condition2Name=\"G1\" sampleQuantity=\"1000\" />\
             </plateSetup2>\
             <protocol2 lidTemperature=\"105\"><protocol2BaseList>\
               <TemperatureStep temperatureStepTemp=\"95\" temperatureStepHoldTime=\"120\" />\
               <TemperatureStep temperatureStepTemp=\"60\" temperatureStepHoldTime=\"30\" />\
               <GotoStep optionGotoStep=\"0\" optionGotoCycle=\"1\" />\
               <MeltCurveStep meltCurveStartTemp=\"65\" meltCurveEndTemp=\"95\" meltCurveTemperatureIncrement=\"0.5\" meltCurveHoldTime=\"5\" meltCurveStepNumber=\"6\" />\
             </protocol2BaseList></protocol2>\
             <runReport><runReportEntry>&lt;BLOCKTYPE&gt;CFX Connect&lt;/BLOCKTYPE&gt;</runReportEntry></runReport>\
             <runData channelCount=\"1\" wellsCount=\"2\"><plateReadDataVector>{}{}{}</plateReadDataVector></runData>\
             <auditHeader user=\"andres\" fullUserName=\"Andres\" />\
             </experimentalData2>",
            read(3, 1, amp1),
            read(3, 2, amp2),
            read(6, 1, melt),
        )
    }

    #[test]
    fn parses_synthetic_experiment() {
        let run = parse_experiment(&synthetic_experiment()).unwrap();

        assert_eq!(run.metadata.instrument.as_deref(), Some("CFX Connect"));
        assert_eq!(run.metadata.serial_number.as_deref(), Some("BR9"));
        assert_eq!(run.metadata.operator.as_deref(), Some("Andres"));
        assert_eq!(run.metadata.plate_type.as_deref(), Some("BR Clear"));
        assert_eq!(run.metadata.cycle_count, Some(2));
        assert_eq!((run.plate.rows, run.plate.cols), (1, 2));
        assert_eq!(run.wells.len(), 2);

        // Protocol: two holds + a loop + a melt.
        let steps = &run.protocol.as_ref().unwrap().steps;
        assert_eq!(steps.len(), 4);
        assert!(matches!(steps[2], ProtocolStep::Loop { goto: 1, repeat: 2 }));
        assert!(matches!(steps[3], ProtocolStep::Melt(_)));
        assert_eq!(run.protocol.as_ref().unwrap().lid_temperature, Some(105.0));

        // Well A1: SYBR / srp, amplification rises 100 -> 200 across the 2 cycles.
        let a1 = &run.wells[0];
        assert_eq!(a1.sample.as_deref(), Some("S1"));
        assert_eq!(a1.biological_group.as_deref(), Some("G1"));
        assert_eq!(a1.sample_type, SampleType::Unknown);
        let ch = &a1.channels[0];
        assert_eq!(ch.fluorophore, "SYBR");
        assert_eq!(ch.target.as_deref(), Some("srp"));
        assert_eq!(ch.amplification, vec![100.0, 200.0]);
        let melt = ch.melt.as_ref().unwrap();
        assert_eq!(melt.rfu, vec![50.0]);
        assert_eq!(melt.temperature, vec![65.0]);

        // Well A2: standard with a starting quantity, target Taf4.
        let a2 = &run.wells[1];
        assert_eq!(a2.sample_type, SampleType::Standard);
        assert_eq!(a2.starting_quantity, Some(1000.0));
        assert_eq!(a2.channels[0].target.as_deref(), Some("Taf4"));
        assert_eq!(a2.channels[0].amplification, vec![110.0, 210.0]);
    }

    #[test]
    fn reads_real_sample_when_present() {
        // Optional end-to-end check against a real (non-committed) CFX file.
        let path = Path::new("onedata/2023-02_15_D18trxD9_4Ampl.pcrd");
        if !path.exists() {
            eprintln!("skipping: {} not present", path.display());
            return;
        }
        let run = read_file(path).expect("decrypt + parse real .pcrd");
        assert_eq!(run.metadata.instrument.as_deref(), Some("CFX Connect"));
        assert_eq!(run.wells.len(), 96);
        assert_eq!(run.metadata.cycle_count, Some(40));
        // Every well should have a SYBR channel with a full 40-cycle trace.
        let a1 = &run.wells[0].channels[0];
        assert_eq!(a1.fluorophore, "SYBR");
        assert_eq!(a1.amplification.len(), 40);
        assert!(a1.amplification.last() > a1.amplification.first());
    }
}
