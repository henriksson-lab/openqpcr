//! openqpcr GUI — a CFX-Maestro-style viewer for qPCR runs, built with Slint.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::{Brush, Color, ModelRc, SharedString, VecModel};

use std::time::Duration;

use openqpcr::analysis;
use openqpcr::instrument::{self, AcquisitionEvent, Instrument, RunHandle, RunState};
use openqpcr::model::{QpcrRun, SampleType, row_label};
use openqpcr::readers::rdml::write_rdml;
use openqpcr::readers::{csv_writer, export};
use openqpcr::{cq, edit, protocol, protocol_edit, stdcurve};

slint::include_modules!();

struct ViewCache {
    occupied: HashSet<(u8, u8)>,
    plate_cells: Vec<WellCell>,
    amp: Vec<Series>,
    melt: Vec<MeltSeries>,
    rows: Vec<RowEntry>,
    ncycles: usize,
    n_std: usize,
    amp_range: AxisRange,
    melt_t_range: AxisRange,
    melt_r_range: AxisRange,
    melt_d_range: AxisRange,
}

struct Series {
    row: u8,
    col: u8,
    fluor: String,
    color: Brush,
    xs: Vec<f64>,
    ys: Vec<f64>,
    stable_log_commands: String,
    stable_linear_commands: String,
}

struct MeltSeries {
    row: u8,
    col: u8,
    fluor: String,
    color: Brush,
    temperature: Vec<f64>,
    rfu: Vec<f64>,
    derivative: Vec<f64>,
    stable_rfu_commands: String,
    stable_derivative_commands: String,
}

struct RowEntry {
    row: u8,
    col: u8,
    well: SharedString,
    fluor: SharedString,
    content: SharedString,
    sample: SharedString,
    cq: SharedString,
    melt_temp: SharedString,
    has_melt: bool,
}

#[derive(Clone, Copy)]
struct AxisRange {
    min: f64,
    max: f64,
}

impl AxisRange {
    fn empty() -> Self {
        Self {
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
        }
    }

    fn include(&mut self, value: f64) {
        if value.is_finite() {
            self.min = self.min.min(value);
            self.max = self.max.max(value);
        }
    }

    fn is_valid(&self) -> bool {
        self.min.is_finite()
    }

    fn or_default(self) -> Self {
        if self.is_valid() {
            self
        } else {
            Self { min: 0.0, max: 1.0 }
        }
    }
}

impl ViewCache {
    fn new(run: &QpcrRun) -> Self {
        let mut occupied = HashSet::new();
        for w in &run.wells {
            occupied.insert((w.row, w.col));
        }

        let mut plate_cells = Vec::new();
        for r in 0..run.plate.rows {
            for c in 0..run.plate.cols {
                let well = run.wells.iter().find(|w| w.row == r && w.col == c);
                let (fill, content, occupied) = match well {
                    Some(w) => (well_fill(w.sample_type), content_code(w.sample_type), true),
                    None => (
                        Brush::SolidColor(hex(0xF4, 0xF4, 0xF4)),
                        String::new(),
                        false,
                    ),
                };
                plate_cells.push(WellCell {
                    row: r as i32,
                    col: c as i32,
                    label: SharedString::from(format!("{}{}", row_label(r), c as usize + 1)),
                    content: SharedString::from(content),
                    fill,
                    occupied,
                    selected: false,
                });
            }
        }

        let mut amp = Vec::new();
        let mut melt = Vec::new();
        let mut rows = Vec::new();
        let mut amp_range = AxisRange::empty();
        let mut melt_t_range = AxisRange::empty();
        let mut melt_r_range = AxisRange::empty();
        let mut melt_d_range = AxisRange::empty();
        let mut ncycles = run.metadata.cycle_count.unwrap_or(0);
        let mut n_std = 0usize;

        for w in &run.wells {
            if w.sample_type == SampleType::Standard {
                n_std += 1;
            }
            let well_label =
                SharedString::from(format!("{}{}", row_label(w.row), w.col as usize + 1));
            let content = SharedString::from(content_code(w.sample_type));
            let sample = SharedString::from(w.sample.clone().unwrap_or_default());
            let color = well_brush(w.row, w.col);
            for ch in &w.channels {
                if !ch.amplification.is_empty() {
                    ncycles = ncycles.max(ch.amplification.len());
                    let xs: Vec<f64> = (0..ch.amplification.len())
                        .map(|i| (i + 1) as f64)
                        .collect();
                    for &v in &ch.amplification {
                        amp_range.include(v);
                    }
                    amp.push(Series {
                        row: w.row,
                        col: w.col,
                        fluor: ch.fluorophore.clone(),
                        color: color.clone(),
                        xs,
                        ys: ch.amplification.clone(),
                        stable_log_commands: String::new(),
                        stable_linear_commands: String::new(),
                    });
                }

                let mut melt_temp = SharedString::from("N/A");
                let mut has_melt = false;
                if let Some(m) = &ch.melt {
                    has_melt = true;
                    if let Some(tm) = m.peaks.first() {
                        melt_temp = SharedString::from(format!("{tm:.1}"));
                    }
                    for &t in &m.temperature {
                        melt_t_range.include(t);
                    }
                    for &v in &m.rfu {
                        melt_r_range.include(v);
                    }
                    for &v in &m.derivative {
                        melt_d_range.include(v);
                    }
                    melt.push(MeltSeries {
                        row: w.row,
                        col: w.col,
                        fluor: ch.fluorophore.clone(),
                        color: color.clone(),
                        temperature: m.temperature.clone(),
                        rfu: m.rfu.clone(),
                        derivative: m.derivative.clone(),
                        stable_rfu_commands: String::new(),
                        stable_derivative_commands: String::new(),
                    });
                }

                rows.push(RowEntry {
                    row: w.row,
                    col: w.col,
                    well: well_label.clone(),
                    fluor: SharedString::from(ch.fluorophore.clone()),
                    content: content.clone(),
                    sample: sample.clone(),
                    cq: SharedString::from(
                        ch.cq
                            .map(|v| format!("{v:.2}"))
                            .unwrap_or_else(|| "N/A".into()),
                    ),
                    melt_temp,
                    has_melt,
                });
            }
        }

        precompute_stable_commands(
            &mut amp,
            &mut melt,
            ncycles.max(1),
            amp_range,
            melt_t_range,
            melt_r_range,
            melt_d_range,
        );

        Self {
            occupied,
            plate_cells,
            amp,
            melt,
            rows,
            ncycles: ncycles.max(1),
            n_std,
            amp_range,
            melt_t_range,
            melt_r_range,
            melt_d_range,
        }
    }
}

/// Mutable view state that drives every derived model.
struct State {
    run: QpcrRun,
    view: ViewCache,
    /// Selected wells; empty means "all wells" (show everything).
    selection: HashSet<(u8, u8)>,
    log_scale: bool,
    stable_axes: bool,
    /// Fluorophore filter for the channel-level views (amplification, Cq table,
    /// standard curve, melt). `None` = show all channels.
    fluor_filter: Option<String>,
    /// Active click-drag on the plate: `Some(true)` = selecting, `Some(false)` =
    /// deselecting (mode fixed by the first well pressed), `None` = not dragging.
    drag_select: Option<bool>,
    /// True while the plate editor is open (relaxes the selection gate so empty
    /// wells can be authored).
    edit_mode: bool,
    /// Snapshot undo/redo stacks for plate edits (bounded).
    undo_stack: Vec<QpcrRun>,
    redo_stack: Vec<QpcrRun>,
}

/// Maximum plate-edit undo depth.
const UNDO_DEPTH: usize = 20;

impl State {
    fn is_shown_coord(&self, row: u8, col: u8) -> bool {
        self.selection.is_empty() || self.selection.contains(&(row, col))
    }
    /// Whether a channel's fluorophore passes the active fluorophore filter.
    fn is_shown_fluor(&self, fluor: &str) -> bool {
        match &self.fluor_filter {
            None => true,
            Some(f) => f == fluor,
        }
    }
    fn is_selected(&self, row: u8, col: u8) -> bool {
        self.selection.contains(&(row, col))
    }
    fn is_occupied(&self, row: u8, col: u8) -> bool {
        self.view.occupied.contains(&(row, col))
    }
    /// Add or remove a well from the selection (used by drag-select). In edit
    /// mode, empty wells are selectable too (so a layout can be authored on them).
    fn set_selected(&mut self, row: u8, col: u8, selected: bool) -> bool {
        // Never select outside the plate.
        if row >= self.run.plate.rows || col >= self.run.plate.cols {
            return false;
        }
        // Outside edit mode, only occupied wells are selectable.
        if !self.edit_mode && !self.is_occupied(row, col) {
            return false;
        }
        if selected {
            self.selection.insert((row, col))
        } else {
            self.selection.remove(&(row, col))
        }
    }

    /// Snapshot the run for undo before a plate edit (bounded; clears redo).
    fn snapshot(&mut self) {
        if self.undo_stack.len() >= UNDO_DEPTH {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(self.run.clone());
        self.redo_stack.clear();
    }
}

struct Args {
    input: Option<PathBuf>,
    screenshot: Option<PathBuf>,
    tab: i32,
    stable_axes: bool,
    simulate: bool,
    open_editor: bool,
    open_protocol_editor: bool,
    copy_layout: Option<PathBuf>,
    replay: Option<PathBuf>,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut input = None;
    let mut screenshot = None;
    let mut tab = 0;
    let mut stable_axes = true;
    let mut simulate = false;
    let mut open_editor = false;
    let mut open_protocol_editor = false;
    let mut copy_layout = None;
    let mut replay = None;
    let mut it = std::env::args_os().skip(1);
    while let Some(a) = it.next() {
        if a == "--simulate" {
            simulate = true;
        } else if a == "--edit" {
            open_editor = true;
        } else if a == "--protocol-edit" {
            open_protocol_editor = true;
        } else if a == "--copy-layout" {
            let value = it
                .next()
                .ok_or_else(|| anyhow::anyhow!("--copy-layout requires a source run path"))?;
            copy_layout = Some(PathBuf::from(value));
        } else if a == "--replay" {
            let value = it
                .next()
                .ok_or_else(|| anyhow::anyhow!("--replay requires a recorded run path"))?;
            replay = Some(PathBuf::from(value));
        } else if a == "--screenshot" {
            let value = it
                .next()
                .ok_or_else(|| anyhow::anyhow!("--screenshot requires a path"))?;
            screenshot = Some(PathBuf::from(value));
        } else if a == "--tab" {
            let value = it
                .next()
                .ok_or_else(|| anyhow::anyhow!("--tab requires 0..=5"))?;
            let value = value
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("--tab must be valid UTF-8"))?;
            tab = match value {
                "0" => 0,
                "1" => 1,
                "2" => 2,
                "3" => 3,
                "4" => 4,
                "5" => 5,
                _ => anyhow::bail!("--tab must be 0..=5"),
            };
        } else if a == "--stable-axes" {
            stable_axes = true;
        } else if a == "--dynamic-axes" {
            stable_axes = false;
        } else {
            input = Some(PathBuf::from(a));
        }
    }
    Ok(Args {
        input,
        screenshot,
        tab,
        stable_axes,
        simulate,
        open_editor,
        open_protocol_editor,
        copy_layout,
        replay,
    })
}

fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    if !(0..=5).contains(&args.tab) {
        anyhow::bail!("--tab must be 0..=5");
    }
    if args.screenshot.is_some() && !has_display_server() {
        anyhow::bail!("--screenshot requires a display server; run under Xvfb on headless systems");
    }
    if args.screenshot.is_some() && std::env::var_os("SLINT_BACKEND").is_none() {
        // SAFETY: this happens at process startup, before Slint is initialized
        // or any application threads are spawned. The winit software renderer
        // still requires the display checked above, but snapshots charts
        // reliably under Xvfb.
        unsafe {
            std::env::set_var("SLINT_BACKEND", "winit-software");
        }
    }
    let mut run = match &args.input {
        Some(p) if p.is_dir() => export::read_export_dir(p)?,
        Some(p) => openqpcr::read_path(p)?,
        None => {
            // Fall back to the bundled sample export so the app is never empty.
            let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/demo_export");
            export::read_export_dir(&fixture).unwrap_or_default_run()
        }
    };
    // Compute Cq + melt peaks from raw curves for any channel that lacks them
    // (e.g. a native `.pcrd`, which stores raw fluorescence only) — the same
    // derived analysis CFX performs when it opens a run.
    cq::annotate_run(&mut run, &cq::CqParams::default());
    cq::annotate_melt(&mut run, &cq::MeltParams::default());

    // Seed a demo protocol when a run carries none, so the Run Information tab and
    // the simulated-acquisition flow have a program to show/execute.
    if run.protocol.is_none() {
        run.protocol = protocol::builtins().into_iter().nth(2);
    }

    let app = AppWindow::new()?;
    let file_name = run
        .metadata
        .source_file
        .clone()
        .unwrap_or_else(|| "(no file)".into());
    app.set_file_name(SharedString::from(shorten_path(&file_name)));
    app.set_app_version(SharedString::from(env!("CARGO_PKG_VERSION")));
    app.set_current_tab(args.tab);
    app.set_stable_axes(args.stable_axes);

    let view = ViewCache::new(&run);
    let state = Rc::new(RefCell::new(State {
        run,
        view,
        selection: HashSet::new(),
        log_scale: true,
        stable_axes: args.stable_axes,
        fluor_filter: None,
        drag_select: None,
        edit_mode: false,
        undo_stack: Vec::new(),
        redo_stack: Vec::new(),
    }));

    // Live-acquisition state. The driver spawns its own worker thread; the UI
    // drains it on a Slint timer, so nothing here leaves the event-loop thread.
    let sim_handle: Rc<RefCell<Option<Box<dyn RunHandle>>>> = Rc::new(RefCell::new(None));
    let sim_timer: Rc<RefCell<Option<slint::Timer>>> = Rc::new(RefCell::new(None));

    // Static, selection-independent models.
    push_plate_headers(&app, &state.borrow().run);
    push_fluor_options(&app, &state.borrow().run);
    push_info(&app, &state.borrow().run);

    // Interactions --------------------------------------------------------
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_toggle_log(move |on| {
            state.borrow_mut().log_scale = on;
            if let Some(app) = weak.upgrade() {
                app.set_log_scale(on);
                refresh_current_tab(&app, &state.borrow());
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_toggle_stable_axes(move |on| {
            state.borrow_mut().stable_axes = on;
            if let Some(app) = weak.upgrade() {
                app.set_stable_axes(on);
                refresh_current_tab(&app, &state.borrow());
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_fluor_selected(move |value| {
            // "All Channels" (the first option) clears the filter.
            state.borrow_mut().fluor_filter = match value.as_str() {
                "" | ALL_CHANNELS => None,
                f => Some(f.to_string()),
            };
            if let Some(app) = weak.upgrade() {
                refresh_current_tab(&app, &state.borrow());
            }
        });
    }
    // Plate click-drag selection. Pressing a well fixes the drag mode from that
    // well's current state (select if it was unselected, else deselect); dragging
    // over further wells applies the same mode without repeated clicking.
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_well_pressed(move |r, c| {
            let (r, c) = (r as u8, c as u8);
            let (changed, edit_mode) = {
                let mut st = state.borrow_mut();
                let em = st.edit_mode;
                // In edit mode, empty in-bounds wells are selectable too.
                if !em && !st.is_occupied(r, c) {
                    st.drag_select = None;
                    (false, em)
                } else {
                    let selecting = !st.is_selected(r, c);
                    st.drag_select = Some(selecting);
                    (st.set_selected(r, c, selecting), em)
                }
            };
            if changed && let Some(app) = weak.upgrade() {
                refresh_current_tab(&app, &state.borrow());
                if edit_mode {
                    push_editor_fields(&app, &state.borrow());
                }
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_well_dragged(move |r, c| {
            let (changed, edit_mode) = {
                let mut st = state.borrow_mut();
                let em = st.edit_mode;
                let changed = match st.drag_select {
                    Some(selecting) => st.set_selected(r as u8, c as u8, selecting),
                    None => false,
                };
                (changed, em)
            };
            if changed && let Some(app) = weak.upgrade() {
                refresh_current_tab(&app, &state.borrow());
                if edit_mode {
                    push_editor_fields(&app, &state.borrow());
                }
            }
        });
    }
    {
        let state = state.clone();
        app.on_well_released(move || {
            state.borrow_mut().drag_select = None;
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_request_export(move || {
            let (path, payload) = {
                let st = state.borrow();
                (export_path(&st.run), serde_json::to_string_pretty(&st.run))
            };
            let result = payload
                .map_err(|e| e.to_string())
                .and_then(|json| std::fs::write(&path, json).map_err(|e| e.to_string()));
            if let Some(app) = weak.upgrade() {
                app.set_status_state(SharedString::from(match result {
                    Ok(()) => format!("Exported → {}", path.display()),
                    Err(e) => format!("Export failed: {e}"),
                }));
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_request_export_rdml(move || {
            let path = {
                let st = state.borrow();
                export_path_with_ext(&st.run, "rdml")
            };
            let result = {
                let st = state.borrow();
                write_rdml(&st.run, &path).map_err(|e| e.to_string())
            };
            if let Some(app) = weak.upgrade() {
                app.set_status_state(SharedString::from(match result {
                    Ok(()) => format!("Exported → {}", path.display()),
                    Err(e) => format!("Export failed: {e}"),
                }));
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_request_export_csv(move || {
            let path = {
                let st = state.borrow();
                export_path_with_ext(&st.run, "csv")
            };
            let result = {
                let st = state.borrow();
                csv_writer::write_cfx_csv(&st.run, &path).map_err(|e| e.to_string())
            };
            if let Some(app) = weak.upgrade() {
                app.set_status_state(SharedString::from(match result {
                    Ok(()) => format!("Exported → {}", path.display()),
                    Err(e) => format!("Export failed: {e}"),
                }));
            }
        });
    }
    // ---- New plate from the current layout (measured data stripped) ----
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_new_from_layout(move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            {
                let mut st = state.borrow_mut();
                st.run = edit::layout_template(&st.run);
                let view = ViewCache::new(&st.run);
                st.view = view;
            }
            refresh_all(&app, &state.borrow());
            app.set_status_state(SharedString::from(
                "New plate from layout (measured data stripped)",
            ));
        });
    }
    // ---- Copy plate layout from another run file ----
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_copy_layout_from(move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let Some(path) = rfd::FileDialog::new().pick_file() else {
                return;
            };
            apply_copy_layout(&app, &state, &path);
        });
    }
    // ---- Save the current layout (data stripped) as a JSON template ----
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_save_layout_template(move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let Some(path) = rfd::FileDialog::new()
                .set_file_name("layout.json")
                .save_file()
            else {
                return;
            };
            let payload = serde_json::to_string_pretty(&edit::layout_template(&state.borrow().run));
            let result = payload
                .map_err(|e| e.to_string())
                .and_then(|json| std::fs::write(&path, json).map_err(|e| e.to_string()));
            app.set_status_state(SharedString::from(match result {
                Ok(()) => format!("Saved layout template → {}", path.display()),
                Err(e) => format!("Save layout template failed: {e}"),
            }));
        });
    }
    app.on_request_quit(|| {
        let _ = slint::quit_event_loop();
    });
    // ---- Instrument control: simulated acquisition ----
    {
        let state = state.clone();
        let weak = app.as_weak();
        let sim_handle = sim_handle.clone();
        let sim_timer = sim_timer.clone();
        app.on_request_simulate(move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            // Acquire over the current plate LAYOUT (measured data stripped), so
            // the chart visibly fills as the synthetic run progresses.
            let base = edit::layout_template(&state.borrow().run);
            // Use the run's chosen protocol, else the first bundled builtin.
            let program = state
                .borrow()
                .run
                .protocol
                .clone()
                .or_else(|| protocol::builtins().into_iter().next())
                .unwrap_or_default();
            let mut sim =
                instrument::SimulatedInstrument::default().with_tick(Duration::from_millis(60));
            if sim.load(&program, &base).is_err() {
                return;
            }
            let handle = match sim.start() {
                Ok(h) => h,
                Err(_) => return,
            };
            {
                let mut st = state.borrow_mut();
                st.run = base;
                st.run.protocol = Some(program);
                let view = ViewCache::new(&st.run);
                st.view = view;
            }
            *sim_handle.borrow_mut() = Some(handle);
            app.set_run_active(true);
            refresh_all(&app, &state.borrow());
            install_run_timer(weak.clone(), state.clone(), sim_handle.clone(), sim_timer.clone());
        });
    }
    {
        let weak = app.as_weak();
        let sim_handle = sim_handle.clone();
        app.on_request_abort(move || {
            if let Some(h) = sim_handle.borrow_mut().as_mut() {
                let _ = h.abort();
            }
            if let Some(app) = weak.upgrade() {
                app.set_status_state(SharedString::from("Aborting…"));
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_load_protocol(move |idx| {
            let name = {
                let mut st = state.borrow_mut();
                let program = protocol::builtins().into_iter().nth(idx.max(0) as usize);
                let name = program
                    .as_ref()
                    .and_then(|p| p.name.clone())
                    .unwrap_or_else(|| "protocol".to_string());
                st.run.protocol = program;
                name
            };
            if let Some(app) = weak.upgrade() {
                push_info(&app, &state.borrow().run);
                app.set_status_state(SharedString::from(format!("Loaded protocol: {name}")));
            }
        });
    }
    // ---- Plate editor ----
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_editor_open(move || {
            state.borrow_mut().edit_mode = true;
            if let Some(app) = weak.upgrade() {
                app.set_plate_editor_visible(true);
                app.set_edit_status(SharedString::from(""));
                refresh_plate(&app, &state.borrow());
                push_editor_fields(&app, &state.borrow());
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_editor_close(move || {
            state.borrow_mut().edit_mode = false;
            if let Some(app) = weak.upgrade() {
                app.set_plate_editor_visible(false);
                refresh_current_tab(&app, &state.borrow());
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_edit_apply_type(move |idx| {
            let msg = {
                let mut st = state.borrow_mut();
                let sel: Vec<(u8, u8)> = st.selection.iter().copied().collect();
                if sel.is_empty() {
                    "Select wells first.".to_string()
                } else {
                    st.snapshot();
                    edit::set_sample_type(&mut st.run, &sel, sample_type_from_index(idx));
                    "Sample type applied.".to_string()
                }
            };
            if let Some(app) = weak.upgrade() {
                commit_edit(&app, &state, &msg);
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_edit_apply_name(move |name| {
            let msg = {
                let mut st = state.borrow_mut();
                let sel: Vec<(u8, u8)> = st.selection.iter().copied().collect();
                if sel.is_empty() {
                    "Select wells first.".to_string()
                } else {
                    st.snapshot();
                    let value = (!name.is_empty()).then(|| name.to_string());
                    edit::set_sample_name(&mut st.run, &sel, value);
                    "Sample name applied.".to_string()
                }
            };
            if let Some(app) = weak.upgrade() {
                commit_edit(&app, &state, &msg);
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_edit_apply_sq(move |text| {
            let parsed = if text.trim().is_empty() {
                Ok(None)
            } else {
                text.trim().parse::<f64>().map(Some).map_err(|_| ())
            };
            let msg = {
                let mut st = state.borrow_mut();
                let sel: Vec<(u8, u8)> = st.selection.iter().copied().collect();
                if sel.is_empty() {
                    "Select wells first.".to_string()
                } else {
                    match parsed {
                        Ok(Some(q)) if !q.is_finite() => {
                            "starting quantity must be a finite number".to_string()
                        }
                        Ok(Some(q)) if q <= 0.0 => {
                            "starting quantity must be greater than zero".to_string()
                        }
                        Ok(value) => {
                            st.snapshot();
                            edit::set_starting_quantity(&mut st.run, &sel, value)
                                .map(|()| "Starting quantity applied.".to_string())
                                .unwrap_or_else(|e| e.to_string())
                        }
                        Err(()) => "Starting quantity must be a number.".to_string(),
                    }
                }
            };
            if let Some(app) = weak.upgrade() {
                commit_edit(&app, &state, &msg);
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_edit_clear(move || {
            let msg = {
                let mut st = state.borrow_mut();
                let sel: Vec<(u8, u8)> = st.selection.iter().copied().collect();
                if sel.is_empty() {
                    "Select wells first.".to_string()
                } else {
                    st.snapshot();
                    edit::clear_wells(&mut st.run, &sel);
                    "Wells cleared.".to_string()
                }
            };
            if let Some(app) = weak.upgrade() {
                commit_edit(&app, &state, &msg);
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_editor_undo(move || {
            let undone = {
                let mut st = state.borrow_mut();
                if let Some(prev) = st.undo_stack.pop() {
                    let current = std::mem::replace(&mut st.run, prev);
                    st.redo_stack.push(current);
                    true
                } else {
                    false
                }
            };
            if let Some(app) = weak.upgrade() {
                commit_edit(
                    &app,
                    &state,
                    if undone { "Undo." } else { "Nothing to undo." },
                );
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_editor_save(move || {
            let (path, payload) = {
                let st = state.borrow();
                (export_path(&st.run), serde_json::to_string_pretty(&st.run))
            };
            let result = payload
                .map_err(|e| e.to_string())
                .and_then(|json| std::fs::write(&path, json).map_err(|e| e.to_string()));
            if let Some(app) = weak.upgrade() {
                app.set_edit_status(SharedString::from(match result {
                    Ok(()) => format!("Saved → {}", path.display()),
                    Err(e) => format!("Save failed: {e}"),
                }));
            }
        });
    }
    // ---- Protocol editor ----
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_protocol_editor_open(move || {
            if let Some(app) = weak.upgrade() {
                app.set_protocol_editor_visible(true);
                app.set_protocol_status(SharedString::from(""));
                push_protocol_rows(&app, &state.borrow());
            }
        });
    }
    {
        let weak = app.as_weak();
        app.on_protocol_editor_close(move || {
            if let Some(app) = weak.upgrade() {
                app.set_protocol_editor_visible(false);
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_protocol_add_hold(move || {
            {
                let mut st = state.borrow_mut();
                let p = st
                    .run
                    .protocol
                    .get_or_insert_with(protocol::ThermalProtocol::default);
                protocol_edit::add_step(
                    p,
                    protocol::ProtocolStep::Hold(protocol::TemperatureStep::hold(95.0, 10.0)),
                );
            }
            if let Some(app) = weak.upgrade() {
                commit_protocol(&app, &state, "Added hold step (95 °C / 10 s).");
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_protocol_add_cycle(move || {
            {
                let mut st = state.borrow_mut();
                let p = st
                    .run
                    .protocol
                    .get_or_insert_with(protocol::ThermalProtocol::default);
                // Repeat the step just before this loop 40×. The user can retarget
                // `goto` by reordering; validation flags an unread loop on save.
                let goto = p.steps.len().saturating_sub(1);
                protocol_edit::add_step(p, protocol::ProtocolStep::Loop { goto, repeat: 40 });
            }
            if let Some(app) = weak.upgrade() {
                commit_protocol(&app, &state, "Added cycle (loop ×40).");
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_protocol_add_melt(move || {
            {
                let mut st = state.borrow_mut();
                let p = st
                    .run
                    .protocol
                    .get_or_insert_with(protocol::ThermalProtocol::default);
                protocol_edit::add_step(
                    p,
                    protocol::ProtocolStep::Melt(protocol::MeltStep {
                        start_c: 65.0,
                        end_c: 95.0,
                        increment_c: 0.5,
                        hold_secs: 5.0,
                    }),
                );
            }
            if let Some(app) = weak.upgrade() {
                commit_protocol(&app, &state, "Added melt (65 → 95 °C).");
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_protocol_remove(move |idx| {
            let msg = {
                let mut st = state.borrow_mut();
                let ok = if let Some(p) = st.run.protocol.as_mut() {
                    protocol_edit::remove_step(p, idx as usize)
                } else {
                    false
                };
                if ok { "Removed step." } else { "Nothing to remove." }.to_string()
            };
            if let Some(app) = weak.upgrade() {
                commit_protocol(&app, &state, &msg);
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_protocol_move(move |from, to| {
            let msg = {
                let mut st = state.borrow_mut();
                let ok = if to < 0 {
                    false
                } else if let Some(p) = st.run.protocol.as_mut() {
                    protocol_edit::move_step(p, from as usize, to as usize)
                } else {
                    false
                };
                if ok { "Moved step." } else { "Cannot move further." }.to_string()
            };
            if let Some(app) = weak.upgrade() {
                commit_protocol(&app, &state, &msg);
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_protocol_save(move || {
            // Report any validation problem, but still persist so a work-in-progress
            // program isn't lost.
            let (path, payload, warn) = {
                let st = state.borrow();
                let warn = st
                    .run
                    .protocol
                    .as_ref()
                    .and_then(|p| protocol_edit::validate(p).err())
                    .map(|e| e.to_string());
                (
                    export_path(&st.run),
                    serde_json::to_string_pretty(&st.run),
                    warn,
                )
            };
            let result = payload
                .map_err(|e| e.to_string())
                .and_then(|json| std::fs::write(&path, json).map_err(|e| e.to_string()));
            if let Some(app) = weak.upgrade() {
                app.set_protocol_status(SharedString::from(match result {
                    Ok(()) => match warn {
                        Some(w) => format!("Saved → {} (warning: {w})", path.display()),
                        None => format!("Saved → {}", path.display()),
                    },
                    Err(e) => format!("Save failed: {e}"),
                }));
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_tab_selected(move |_| {
            if let Some(app) = weak.upgrade() {
                refresh_current_tab(&app, &state.borrow());
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_quant_row_clicked(move |well| {
            if let Some((r, c)) = openqpcr::model::parse_well_label(&well) {
                let changed = toggle_well(&mut state.borrow_mut(), r, c);
                if changed && let Some(app) = weak.upgrade() {
                    refresh_current_tab(&app, &state.borrow());
                }
            }
        });
    }

    refresh_all(&app, &state.borrow());

    // Kick off a synthetic acquisition immediately when requested (headless demo
    // / screenshots). The menu action does the same at runtime.
    if args.simulate {
        app.invoke_request_simulate();
    }
    if let Some(source_path) = args.copy_layout.clone() {
        apply_copy_layout(&app, &state, &source_path);
    }
    if let Some(replay_path) = args.replay.clone() {
        start_replay(&app, &state, &sim_handle, &sim_timer, &replay_path);
    }
    if args.open_editor {
        app.invoke_editor_open();
    }
    if args.open_protocol_editor {
        app.invoke_protocol_editor_open();
    }

    // Display-backed screenshot mode: render one frame, save a PNG, then quit.
    // A live run (simulate/replay) needs longer so the snapshot captures mid-run.
    let snapshot_delay = if args.simulate || args.replay.is_some() {
        1400
    } else {
        500
    };
    let screenshot_result = Rc::new(RefCell::new(None));
    if let Some(out) = args.screenshot.clone() {
        let weak = app.as_weak();
        let screenshot_result = screenshot_result.clone();
        slint::Timer::single_shot(
            std::time::Duration::from_millis(snapshot_delay),
            move || {
                let result = if let Some(app) = weak.upgrade() {
                    match app.window().take_snapshot() {
                        Ok(buf) => save_png(&buf, &out).map(|()| {
                            println!("screenshot saved to {}", out.display());
                        }),
                        Err(e) => Err(anyhow::anyhow!("screenshot: take_snapshot failed: {e}")),
                    }
                } else {
                    Err(anyhow::anyhow!("screenshot: window closed before capture"))
                };
                *screenshot_result.borrow_mut() = Some(result.map_err(|e| e.to_string()));
                let _ = slint::quit_event_loop();
            },
        );
    }

    app.run()?;
    if args.screenshot.is_some() {
        match screenshot_result.borrow_mut().take() {
            Some(Ok(())) => {}
            Some(Err(e)) => anyhow::bail!("{e}"),
            None => anyhow::bail!("screenshot: capture did not run"),
        }
    }
    Ok(())
}

/// Encode a Slint RGBA snapshot to a PNG file.
fn save_png(buf: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, path: &Path) -> anyhow::Result<()> {
    ensure_nonblank_snapshot(buf)?;
    let img: image::RgbaImage =
        image::ImageBuffer::from_raw(buf.width(), buf.height(), buf.as_bytes().to_vec())
            .ok_or_else(|| anyhow::anyhow!("buffer size mismatch"))?;
    img.save(path)?;
    Ok(())
}

fn ensure_nonblank_snapshot(
    buf: &slint::SharedPixelBuffer<slint::Rgba8Pixel>,
) -> anyhow::Result<()> {
    let mut colors = Vec::<[u8; 4]>::new();
    for px in buf.as_bytes().chunks_exact(4) {
        let color = [px[0], px[1], px[2], px[3]];
        if !colors.contains(&color) {
            colors.push(color);
            if colors.len() > 4 {
                return Ok(());
            }
        }
    }
    anyhow::bail!(
        "screenshot: captured image appears blank ({} distinct colors); try running under Xvfb",
        colors.len()
    )
}

fn has_display_server() -> bool {
    if cfg!(not(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ))) {
        return true;
    }
    std::env::var_os("DISPLAY").is_some()
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var_os("WAYLAND_SOCKET").is_some()
}

/// Map a plate-editor sample-type dropdown index to a [`SampleType`]
/// (order matches `sample-type-names` in `app.slint`).
fn sample_type_from_index(i: i32) -> SampleType {
    match i {
        1 => SampleType::Standard,
        2 => SampleType::Ntc,
        3 => SampleType::Nrt,
        4 => SampleType::PositiveControl,
        5 => SampleType::NegativeControl,
        6 => SampleType::Empty,
        _ => SampleType::Unknown,
    }
}

fn index_from_sample_type(t: SampleType) -> i32 {
    match t {
        SampleType::Unknown => 0,
        SampleType::Standard => 1,
        SampleType::Ntc => 2,
        SampleType::Nrt => 3,
        SampleType::PositiveControl => 4,
        SampleType::NegativeControl => 5,
        SampleType::Empty => 6,
    }
}

/// Push the plate-editor panel fields for the current selection.
fn push_editor_fields(app: &AppWindow, state: &State) {
    let sel: Vec<(u8, u8)> = state.selection.iter().copied().collect();
    app.set_edit_sel_count(sel.len() as i32);
    let type_idx = openqpcr::edit::common_sample_type(&state.run, &sel)
        .map(index_from_sample_type)
        .unwrap_or(0);
    app.set_edit_sample_type_index(type_idx);
    let name = match openqpcr::edit::common_sample_name(&state.run, &sel) {
        Some(Some(n)) => n,
        _ => String::new(),
    };
    app.set_edit_sample_name(SharedString::from(name));
    // Common starting quantity across the selection (blank if mixed/none).
    let sqs: Vec<Option<f64>> = sel
        .iter()
        .map(|&(r, c)| {
            state
                .run
                .wells
                .iter()
                .find(|w| w.row == r && w.col == c)
                .and_then(|w| w.starting_quantity)
        })
        .collect();
    let common_sq = if !sqs.is_empty() && sqs.iter().all(|q| *q == sqs[0]) {
        sqs[0]
    } else {
        None
    };
    app.set_edit_start_qty(SharedString::from(
        common_sq.map(|q| format!("{q}")).unwrap_or_default(),
    ));
}

/// Rebuild the derived view after a plate edit and refresh every surface.
fn commit_edit(app: &AppWindow, state: &Rc<RefCell<State>>, status: &str) {
    {
        let mut st = state.borrow_mut();
        let view = ViewCache::new(&st.run);
        st.view = view;
    }
    let st = state.borrow();
    refresh_current_tab(app, &st);
    push_editor_fields(app, &st);
    app.set_edit_status(SharedString::from(status.to_string()));
}

/// The most amplification cycles accrued on any channel (live-run progress).
fn max_cycles(run: &QpcrRun) -> usize {
    run.wells
        .iter()
        .flat_map(|w| w.channels.iter())
        .map(|c| c.amplification.len())
        .max()
        .unwrap_or(0)
}

/// Install a UI-thread timer that drains a running [`RunHandle`], folds each
/// acquisition event into the live run, and refreshes the views — shared by the
/// synthetic simulator and the `.pcrd` replayer. On the terminal event it stops
/// the timer and clears the handle.
fn install_run_timer(
    weak: slint::Weak<AppWindow>,
    state: Rc<RefCell<State>>,
    handle_cell: Rc<RefCell<Option<Box<dyn RunHandle>>>>,
    timer_cell: Rc<RefCell<Option<slint::Timer>>>,
) {
    let timer = slint::Timer::default();
    let cb_timer = timer_cell.clone();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(40),
        move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let events = match handle_cell.borrow_mut().as_mut() {
                Some(h) => h.poll(),
                None => return,
            };
            if events.is_empty() {
                return;
            }
            let mut finished: Option<RunState> = None;
            {
                let mut st = state.borrow_mut();
                for ev in &events {
                    instrument::simulated::fold_event(&mut st.run, ev);
                    if let AcquisitionEvent::Finished(rs) = ev {
                        finished = Some(rs.clone());
                    }
                }
                let view = ViewCache::new(&st.run);
                st.view = view;
            }
            refresh_all(&app, &state.borrow());
            match finished {
                Some(rs) => {
                    app.set_run_active(false);
                    app.set_status_state(SharedString::from(match rs {
                        RunState::Complete => "Run complete".to_string(),
                        RunState::Aborted => "Run aborted".to_string(),
                        RunState::Error(e) => format!("Run error: {e}"),
                        _ => "Run finished".to_string(),
                    }));
                    *handle_cell.borrow_mut() = None;
                    // Stop (don't drop) the timer from inside its own callback.
                    if let Some(t) = cb_timer.borrow().as_ref() {
                        t.stop();
                    }
                }
                None => {
                    let cycle = max_cycles(&state.borrow().run);
                    app.set_status_state(SharedString::from(format!("Running — cycle {cycle}")));
                }
            }
        },
    );
    *timer_cell.borrow_mut() = Some(timer);
}

/// Start replaying a recorded run (e.g. a decoded `.pcrd`) as a live acquisition:
/// the plate's layout is shown immediately and its amplification/melt curves fill
/// in cycle-by-cycle, driven by the same timer as the synthetic simulator.
fn start_replay(
    app: &AppWindow,
    state: &Rc<RefCell<State>>,
    handle_cell: &Rc<RefCell<Option<Box<dyn RunHandle>>>>,
    timer_cell: &Rc<RefCell<Option<slint::Timer>>>,
    path: &Path,
) {
    let recorded = if path.is_dir() {
        export::read_export_dir(path)
    } else {
        openqpcr::read_path(path)
    };
    let recorded = match recorded {
        Ok(r) => r,
        Err(e) => {
            app.set_status_state(SharedString::from(format!("Replay load failed: {e}")));
            return;
        }
    };
    // Base = the same plate layout with measured data stripped, so the samples show
    // immediately and the curves fill as the replay progresses.
    let mut base = recorded.clone();
    for w in &mut base.wells {
        for c in &mut w.channels {
            c.amplification.clear();
            c.melt = None;
            c.cq = None;
        }
    }
    let mut inst =
        instrument::ReplayInstrument::from_run(recorded).with_tick(Duration::from_millis(60));
    let handle = match inst.start() {
        Ok(h) => h,
        Err(e) => {
            app.set_status_state(SharedString::from(format!("Replay failed: {e}")));
            return;
        }
    };
    {
        let mut st = state.borrow_mut();
        st.run = base;
        st.view = ViewCache::new(&st.run);
    }
    *handle_cell.borrow_mut() = Some(handle);
    app.set_run_active(true);
    refresh_all(app, &state.borrow());
    install_run_timer(
        app.as_weak(),
        state.clone(),
        handle_cell.clone(),
        timer_cell.clone(),
    );
}

fn toggle_well(state: &mut State, row: u8, col: u8) -> bool {
    if !state.is_occupied(row, col) {
        return false;
    }
    if state.selection.insert((row, col)) {
        true
    } else {
        state.selection.remove(&(row, col))
    }
}

/// Copy the plate layout from `source_path` onto the current run and refresh the
/// UI. Shared by the "Copy Plate Layout From…" menu action (which supplies a
/// dialog-picked path) and the `--copy-layout <path>` CLI flag (which makes the
/// same code path exercisable headlessly for screenshot checks).
fn apply_copy_layout(app: &AppWindow, state: &Rc<RefCell<State>>, source_path: &Path) {
    // Accept either a single run file or a CFX export directory, mirroring how the
    // base run is loaded in `main`.
    let source = if source_path.is_dir() {
        export::read_export_dir(source_path)
    } else {
        openqpcr::read_path(source_path)
    };
    let status = match source {
        Ok(source) => {
            let report = {
                let mut st = state.borrow_mut();
                let report = edit::copy_layout_from(&mut st.run, &source);
                st.view = ViewCache::new(&st.run);
                report
            };
            refresh_all(app, &state.borrow());
            format!(
                "Copied layout: {} wells copied, {} skipped (out of bounds)",
                report.wells_copied, report.wells_skipped_out_of_bounds
            )
        }
        Err(e) => format!("Copy layout failed: {e}"),
    };
    app.set_status_state(SharedString::from(status));
}

fn refresh_all(app: &AppWindow, state: &State) {
    refresh_plate(app, state);
    refresh_amp(app, state);
    refresh_melt(app, state);
    refresh_tables(app, state);
    refresh_std_curve(app, state);
    refresh_gene_expr(app, state);
    refresh_allelic(app, state);
    refresh_qc(app, state);
    refresh_status(app, state);
}

fn refresh_current_tab(app: &AppWindow, state: &State) {
    refresh_plate(app, state);
    match app.get_current_tab() {
        0 => {
            refresh_amp(app, state);
            refresh_quant_table(app, state);
            refresh_std_curve(app, state);
        }
        1 => {
            refresh_melt(app, state);
            refresh_melt_table(app, state);
        }
        3 => refresh_gene_expr(app, state),
        4 => refresh_allelic(app, state),
        5 => refresh_qc(app, state),
        _ => {}
    }
    refresh_status(app, state);
}

fn refresh_amp(app: &AppWindow, state: &State) {
    let log = state.log_scale;
    let raw_range = if state.stable_axes {
        state.view.amp_range
    } else {
        selected_amp_range(state)
    };
    let raw_range = if raw_range.is_valid() {
        raw_range
    } else {
        AxisRange { min: 0.0, max: 1.0 }
    };
    let (yt_min, yt_max) = value_axis(raw_range.min, raw_range.max, log);
    let xmax = state.view.ncycles as f64;
    let mut amp_traces = Vec::new();
    for series in &state.view.amp {
        if !state.is_shown_coord(series.row, series.col) || !state.is_shown_fluor(&series.fluor) {
            continue;
        }
        let dynamic_cmd;
        let cmd = if state.stable_axes {
            if log {
                &series.stable_log_commands
            } else {
                &series.stable_linear_commands
            }
        } else {
            let ys: Vec<f64> = series.ys.iter().map(|&v| ymap(v, log)).collect();
            dynamic_cmd = build_commands(&series.xs, &ys, 0.0, xmax, yt_min, yt_max);
            &dynamic_cmd
        };
        if !cmd.is_empty() {
            amp_traces.push(TraceData {
                color: series.color.clone(),
                commands: SharedString::from(cmd.as_str()),
            });
        }
    }
    app.set_amp_traces(model(amp_traces));
    app.set_amp_x_labels(x_axis_labels(xmax));
    app.set_amp_y_labels(value_axis_labels(yt_min, yt_max, log));

    let thr_val = if log {
        yt_min + 0.35 * (yt_max - yt_min)
    } else {
        raw_range.min + 0.1 * (raw_range.max - raw_range.min)
    };
    let thr_disp = if log { 10f64.powf(thr_val) } else { thr_val };
    app.set_amp_threshold_pos(frac_from_top(thr_val, yt_min, yt_max) as f32);
    app.set_amp_threshold_label(SharedString::from(format!("Threshold: {thr_disp:.0}")));
}

fn refresh_melt(app: &AppWindow, state: &State) {
    let mut melt_traces = Vec::new();
    let mut peak_traces = Vec::new();

    let (t_range, r_range, d_range) = if state.stable_axes {
        (
            state.view.melt_t_range,
            state.view.melt_r_range,
            state.view.melt_d_range,
        )
    } else {
        selected_melt_ranges(state)
    };

    if t_range.is_valid() {
        let r_range = r_range.or_default();
        let d_range = d_range.or_default();
        let (rmin, rmax) = pad(r_range.min, r_range.max);
        let (dmin, dmax) = pad(d_range.min, d_range.max);
        for series in &state.view.melt {
            if !state.is_shown_coord(series.row, series.col) || !state.is_shown_fluor(&series.fluor)
            {
                continue;
            }
            if !series.rfu.is_empty() {
                let dynamic_cmd;
                let cmd = if state.stable_axes {
                    &series.stable_rfu_commands
                } else {
                    dynamic_cmd = build_commands(
                        &series.temperature,
                        &series.rfu,
                        t_range.min,
                        t_range.max,
                        rmin,
                        rmax,
                    );
                    &dynamic_cmd
                };
                if !cmd.is_empty() {
                    melt_traces.push(TraceData {
                        color: series.color.clone(),
                        commands: SharedString::from(cmd.as_str()),
                    });
                }
            }
            if !series.derivative.is_empty() {
                let dynamic_cmd;
                let cmd = if state.stable_axes {
                    &series.stable_derivative_commands
                } else {
                    dynamic_cmd = build_commands(
                        &series.temperature,
                        &series.derivative,
                        t_range.min,
                        t_range.max,
                        dmin,
                        dmax,
                    );
                    &dynamic_cmd
                };
                if !cmd.is_empty() {
                    peak_traces.push(TraceData {
                        color: series.color.clone(),
                        commands: SharedString::from(cmd.as_str()),
                    });
                }
            }
        }
        app.set_melt_x_labels(linear_axis_labels(t_range.min, t_range.max, "°", false));
        app.set_melt_y_labels(linear_axis_labels(rmin, rmax, "", true));
        app.set_peak_x_labels(linear_axis_labels(t_range.min, t_range.max, "°", false));
        app.set_peak_y_labels(linear_axis_labels(dmin, dmax, "", true));
    } else {
        app.set_melt_x_labels(model(Vec::new()));
        app.set_melt_y_labels(model(Vec::new()));
        app.set_peak_x_labels(model(Vec::new()));
        app.set_peak_y_labels(model(Vec::new()));
    }
    app.set_melt_traces(model(melt_traces));
    app.set_peak_traces(model(peak_traces));
}

fn refresh_tables(app: &AppWindow, state: &State) {
    refresh_quant_table(app, state);
    refresh_melt_table(app, state);
}

fn refresh_quant_table(app: &AppWindow, state: &State) {
    app.set_quant_rows(build_rows(state, TableValue::Cq));
}

fn refresh_melt_table(app: &AppWindow, state: &State) {
    app.set_melt_rows(build_rows(state, TableValue::MeltTemp));
}

fn refresh_std_curve(app: &AppWindow, state: &State) {
    // Empty-state helper: clear the chart and set the caption text.
    let clear_chart = |app: &AppWindow| {
        app.set_stdcurve_traces(model(Vec::new()));
        app.set_stdcurve_x_labels(model(Vec::new()));
        app.set_stdcurve_y_labels(model(Vec::new()));
    };

    if state.view.n_std == 0 {
        clear_chart(app);
        app.set_std_curve_text(SharedString::from(
            "No wells designated as Sample Type standard.",
        ));
        return;
    }

    // Collect (starting_quantity, cq) points from standard wells. One point per
    // (standard well, channel-with-cq) gives the regression the most data.
    let mut points: Vec<(f64, f64)> = Vec::new();
    for well in &state.run.wells {
        if well.sample_type != SampleType::Standard {
            continue;
        }
        let Some(sq) = well.starting_quantity else {
            continue;
        };
        for channel in &well.channels {
            if !state.is_shown_fluor(&channel.fluorophore) {
                continue;
            }
            if let Some(cq) = channel.cq {
                points.push((sq, cq));
            }
        }
    }

    match stdcurve::fit(&points) {
        Some(f) => {
            app.set_std_curve_text(SharedString::from(format!(
                "n={}   slope {:.3}   eff {:.1}%   R² {:.4}",
                f.n,
                f.slope,
                f.efficiency * 100.0,
                f.r_squared,
            )));

            // Plot in (x = log10(SQ), y = Cq) space. The fit is
            // Cq = slope·log10(SQ) + intercept, so it is a straight line here.
            let mut xr = AxisRange::empty();
            let mut yr = AxisRange::empty();
            let mut xy: Vec<(f64, f64)> = Vec::new();
            for &(sq, cq) in &points {
                if sq <= 0.0 || !cq.is_finite() {
                    continue;
                }
                let x = sq.log10();
                if !x.is_finite() {
                    continue;
                }
                xr.include(x);
                yr.include(cq);
                xy.push((x, cq));
            }
            if xy.is_empty() || !xr.is_valid() || !yr.is_valid() {
                clear_chart(app);
                return;
            }
            let (xmin, xmax) = pad(xr.min, xr.max);
            let (ymin, ymax) = pad(yr.min, yr.max);

            // Scatter: diamond marks for each standard point.
            let mut scatter = String::new();
            for &(x, y) in &xy {
                let px = if (xmax - xmin).abs() < f64::EPSILON {
                    500.0
                } else {
                    (x - xmin) / (xmax - xmin) * 1000.0
                };
                let py = frac_from_top(y, ymin, ymax) * 1000.0;
                scatter.push_str(&format!(
                    "M {px:.1} {:.1} L {:.1} {py:.1} L {px:.1} {:.1} L {:.1} {py:.1} Z ",
                    py - 9.0,
                    px + 9.0,
                    py + 9.0,
                    px - 9.0
                ));
            }

            // Fit line: y = slope·x + intercept across the x-range.
            let line_xs = [xmin, xmax];
            let line_ys = [
                f.slope * xmin + f.intercept,
                f.slope * xmax + f.intercept,
            ];
            let line = build_commands(&line_xs, &line_ys, xmin, xmax, ymin, ymax);

            let traces = vec![
                TraceData {
                    color: Brush::SolidColor(hex(0xC0, 0x39, 0x2B)),
                    commands: SharedString::from(line.as_str()),
                },
                TraceData {
                    color: Brush::SolidColor(hex(0x1B, 0x4F, 0xE0)),
                    commands: SharedString::from(scatter.as_str()),
                },
            ];
            app.set_stdcurve_traces(model(traces));
            app.set_stdcurve_x_labels(linear_axis_labels(xmin, xmax, "", false));
            app.set_stdcurve_y_labels(linear_axis_labels(ymin, ymax, "", true));
        }
        None => {
            clear_chart(app);
            app.set_std_curve_text(SharedString::from(format!(
                "{} standard wells, but too few valid (SQ, Cq) points to fit a curve.",
                state.view.n_std
            )));
        }
    }
}

// ---------------------------------------------------------------------------
// Analysis tabs (Gene Expression / Allelic Discrimination / QC)
//
// These three tabs present plate-level summaries, so for v1 they compute over
// the *whole* run rather than the interactive plate selection — a selection
// filter would make ΔΔCq references, genotype clouds and QC counts confusing.
// ---------------------------------------------------------------------------

fn refresh_gene_expr(app: &AppWindow, state: &State) {
    let res = analysis::gene_expression(&state.run);
    let label = match (&res.ref_target, res.single_target) {
        (None, _) => "No targets with a Cq available.".to_string(),
        (Some(t), true) => format!("Single target: {t} — mean Cq only (no reference)"),
        (Some(t), false) => format!(
            "Reference: {}   ·   Calibrator: {}",
            t,
            res.calibrator.as_deref().unwrap_or("—")
        ),
    };
    app.set_gene_ref_label(SharedString::from(label));

    let mut rows = Vec::new();
    for r in &res.rows {
        let value = match r.rel_expr {
            Some(v) => format!("{v:.3}"),
            None => "—".to_string(),
        };
        rows.push(TableRow {
            well: SharedString::from(String::new()),
            fluor: SharedString::from(r.target.clone()),
            content: SharedString::from(format!("{:.2}", r.mean_cq)),
            sample: SharedString::from(r.sample.clone()),
            value: SharedString::from(value),
            selected: false,
        });
    }
    app.set_gene_rows(model(rows));
}

fn refresh_allelic(app: &AppWindow, state: &State) {
    let res = analysis::allelic_discrimination(&state.run);

    // Axis titles use the fluorophore names when present.
    app.set_allelic_x_title(SharedString::from(
        res.allele_x.clone().unwrap_or_else(|| "Allele 1".into()),
    ));
    app.set_allelic_y_title(SharedString::from(
        res.allele_y.clone().unwrap_or_else(|| "Allele 2".into()),
    ));

    // Legend / status line above the table.
    let label = match (&res.allele_x, &res.allele_y) {
        (Some(x), Some(y)) => format!("◆ {x} = Allele 1   ·   ◆ {y} = Allele 2"),
        (Some(x), None) => format!("Only one fluorophore ({x}); need two for genotyping."),
        _ => "No fluorophores with amplification data.".to_string(),
    };
    app.set_allelic_label(SharedString::from(label));

    // Table: well=well, fluor=call, content=X RFU, value=Y RFU.
    let mut rows = Vec::new();
    for p in &res.points {
        rows.push(TableRow {
            well: SharedString::from(p.well_label.clone()),
            fluor: SharedString::from(p.call.label().to_string()),
            content: SharedString::from(format!("{:.0}", p.x)),
            sample: SharedString::from(String::new()),
            value: SharedString::from(format!("{:.0}", p.y)),
            selected: false,
        });
    }
    app.set_allelic_rows(model(rows));

    // Scatter: one TraceData per genotype call, each a cluster of diamond marks.
    let mut xr = AxisRange::empty();
    let mut yr = AxisRange::empty();
    for p in &res.points {
        xr.include(p.x);
        yr.include(p.y);
    }
    if res.points.is_empty() || !xr.is_valid() || !yr.is_valid() {
        app.set_allelic_traces(model(Vec::new()));
        app.set_allelic_x_labels(model(Vec::new()));
        app.set_allelic_y_labels(model(Vec::new()));
        return;
    }
    let (xmin, xmax) = pad(xr.min, xr.max);
    let (ymin, ymax) = pad(yr.min, yr.max);

    let mut cmds: std::collections::HashMap<&'static str, String> =
        std::collections::HashMap::new();
    for p in &res.points {
        let px = if (xmax - xmin).abs() < f64::EPSILON {
            500.0
        } else {
            (p.x - xmin) / (xmax - xmin) * 1000.0
        };
        let py = frac_from_top(p.y, ymin, ymax) * 1000.0;
        let key = p.call.label();
        let s = cmds.entry(key).or_default();
        // Small diamond marker centred on (px, py).
        s.push_str(&format!(
            "M {px:.1} {:.1} L {:.1} {py:.1} L {px:.1} {:.1} L {:.1} {py:.1} Z ",
            py - 9.0,
            px + 9.0,
            py + 9.0,
            px - 9.0
        ));
    }

    let color_for = |call: &str| -> Brush {
        Brush::SolidColor(match call {
            "Both" => hex(0x8E, 0x24, 0xAA),
            "Allele 1" => hex(0x1B, 0x4F, 0xE0),
            "Allele 2" => hex(0x1F, 0x9E, 0x1F),
            _ => hex(0x90, 0x90, 0x90),
        })
    };
    // Deterministic trace order.
    let mut traces = Vec::new();
    for key in ["Both", "Allele 1", "Allele 2", "NTC/None"] {
        if let Some(cmd) = cmds.get(key) {
            traces.push(TraceData {
                color: color_for(key),
                commands: SharedString::from(cmd.as_str()),
            });
        }
    }
    app.set_allelic_traces(model(traces));
    app.set_allelic_x_labels(linear_axis_labels(xmin, xmax, "", false));
    app.set_allelic_y_labels(linear_axis_labels(ymin, ymax, "", true));
}

fn refresh_qc(app: &AppWindow, state: &State) {
    let rows: Vec<InfoRow> = analysis::qc_metrics(&state.run)
        .into_iter()
        .map(|(k, v)| InfoRow {
            key: SharedString::from(k),
            value: SharedString::from(v),
        })
        .collect();
    app.set_qc_rows(model(rows));
}

fn refresh_status(app: &AppWindow, state: &State) {
    app.set_status_analysis_mode(SharedString::from(if state.log_scale {
        "Baseline Subtracted Curve Fit (Log)"
    } else {
        "Baseline Subtracted Curve Fit"
    }));
}

// ---------------------------------------------------------------------------
// Model builders
// ---------------------------------------------------------------------------

fn refresh_plate(app: &AppWindow, state: &State) {
    app.set_wells(build_wells(state));
}

fn build_wells(state: &State) -> ModelRc<WellCell> {
    let mut cells = state.view.plate_cells.clone();
    for cell in &mut cells {
        cell.selected = state.is_selected(cell.row as u8, cell.col as u8);
    }
    model(cells)
}

enum TableValue {
    Cq,
    MeltTemp,
}

fn build_rows(state: &State, which: TableValue) -> ModelRc<TableRow> {
    let mut rows = Vec::new();
    for entry in &state.view.rows {
        if !state.is_shown_coord(entry.row, entry.col) || !state.is_shown_fluor(&entry.fluor) {
            continue;
        }
        if matches!(which, TableValue::MeltTemp) && !entry.has_melt {
            continue;
        }
        let value = match which {
            TableValue::Cq => entry.cq.clone(),
            TableValue::MeltTemp => entry.melt_temp.clone(),
        };
        rows.push(TableRow {
            well: entry.well.clone(),
            fluor: entry.fluor.clone(),
            content: entry.content.clone(),
            sample: entry.sample.clone(),
            value,
            selected: state.is_selected(entry.row, entry.col),
        });
    }
    model(rows)
}

fn selected_amp_range(state: &State) -> AxisRange {
    let mut range = AxisRange::empty();
    for series in &state.view.amp {
        if !state.is_shown_coord(series.row, series.col) || !state.is_shown_fluor(&series.fluor) {
            continue;
        }
        for &v in &series.ys {
            range.include(v);
        }
    }
    range
}

fn selected_melt_ranges(state: &State) -> (AxisRange, AxisRange, AxisRange) {
    let mut t_range = AxisRange::empty();
    let mut r_range = AxisRange::empty();
    let mut d_range = AxisRange::empty();
    for series in &state.view.melt {
        if !state.is_shown_coord(series.row, series.col) || !state.is_shown_fluor(&series.fluor) {
            continue;
        }
        for &t in &series.temperature {
            t_range.include(t);
        }
        for &v in &series.rfu {
            r_range.include(v);
        }
        for &v in &series.derivative {
            d_range.include(v);
        }
    }
    (t_range, r_range, d_range)
}

/// The sentinel first entry of the fluorophore dropdown: clears the filter.
const ALL_CHANNELS: &str = "All Channels";

/// Populate the toolbar's fluorophore dropdown with `All Channels` followed by
/// the distinct fluorophores present in the run (sorted). Resets the selection
/// to `All Channels`.
fn push_fluor_options(app: &AppWindow, run: &QpcrRun) {
    let mut fluors: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for w in &run.wells {
        for ch in &w.channels {
            if !ch.fluorophore.is_empty() {
                fluors.insert(ch.fluorophore.as_str());
            }
        }
    }
    let mut options = vec![SharedString::from(ALL_CHANNELS)];
    options.extend(fluors.into_iter().map(SharedString::from));
    app.set_fluor_options(model(options));
    app.set_current_fluor_index(0);
}

fn push_plate_headers(app: &AppWindow, run: &QpcrRun) {
    app.set_plate_rows(run.plate.rows as i32);
    app.set_plate_cols(run.plate.cols as i32);
    let xs: Vec<SharedString> = (1..=run.plate.cols)
        .map(|c| SharedString::from(c.to_string()))
        .collect();
    let ys: Vec<SharedString> = (0..run.plate.rows)
        .map(|r| SharedString::from(row_label(r)))
        .collect();
    app.set_plate_x_headers(model(xs));
    app.set_plate_y_headers(model(ys));
}

/// A one-line human description of a protocol step (for the Run Info tab).
fn describe_step(step: &protocol::ProtocolStep) -> String {
    use protocol::ProtocolStep as S;
    match step {
        S::Hold(t) => {
            let hold = t
                .hold_secs
                .map(|s| format!("{s:.0} s"))
                .unwrap_or_else(|| "hold".into());
            let read = match t.measure {
                protocol::Measure::Real => "  ●read",
                protocol::Measure::Meltcurve => "  ●melt",
                protocol::Measure::None => "",
            };
            format!("Hold {:.1} °C for {hold}{read}", t.target_c)
        }
        S::Gradient(g) => format!("Gradient {:.1}–{:.1} °C", g.low_c, g.high_c),
        S::Loop { goto, repeat } => format!("Go to step {goto} ×{repeat}"),
        S::Pause { temperature } => format!("Pause at {temperature:.1} °C"),
        S::LidOpen => "Open lid".to_string(),
        S::Melt(m) => format!(
            "Melt {:.1}→{:.1} °C by {:.1}",
            m.start_c, m.end_c, m.increment_c
        ),
    }
}

/// Short one-word label for a step's kind, shown in the protocol-editor table.
fn step_kind(step: &protocol::ProtocolStep) -> &'static str {
    use protocol::ProtocolStep as S;
    match step {
        S::Hold(_) => "Hold",
        S::Gradient(_) => "Gradient",
        S::Loop { .. } => "Cycle",
        S::Pause { .. } => "Pause",
        S::LidOpen => "Lid Open",
        S::Melt(_) => "Melt",
    }
}

/// Rebuild the protocol-editor step-table model from the run's program.
fn push_protocol_rows(app: &AppWindow, state: &State) {
    let rows: Vec<ProtocolRow> = state
        .run
        .protocol
        .as_ref()
        .map(|p| {
            p.steps
                .iter()
                .enumerate()
                .map(|(i, s)| ProtocolRow {
                    index: i as i32,
                    kind: SharedString::from(step_kind(s)),
                    summary: SharedString::from(describe_step(s)),
                })
                .collect()
        })
        .unwrap_or_default();
    app.set_protocol_rows(model(rows));
}

/// Refresh the protocol-editor table + the Run-Information tab after a program
/// edit, and set the editor's status line.
fn commit_protocol(app: &AppWindow, state: &Rc<RefCell<State>>, status: &str) {
    let st = state.borrow();
    push_protocol_rows(app, &st);
    push_info(app, &st.run);
    app.set_protocol_status(SharedString::from(status.to_string()));
}

fn push_info(app: &AppWindow, run: &QpcrRun) {
    let m = &run.metadata;
    app.set_status_plate_type(SharedString::from(
        m.plate_type.clone().unwrap_or_else(|| "Unknown".into()),
    ));
    let mut rows = Vec::new();
    let mut add = |k: &str, v: Option<String>| {
        if let Some(v) = v
            && !v.is_empty()
        {
            rows.push(InfoRow {
                key: SharedString::from(k.to_string()),
                value: SharedString::from(v),
            });
        }
    };
    add("Source file", m.source_file.clone());
    add("Instrument", m.instrument.clone());
    add("Serial number", m.serial_number.clone());
    add("Software", m.software_version.clone());
    add("Run started", m.run_started.clone());
    add("Run ended", m.run_ended.clone());
    add("Operator", m.operator.clone());
    add("Plate ID", m.plate_id.clone());
    add("Plate type", m.plate_type.clone());
    add("Cycles", m.cycle_count.map(|c| c.to_string()));
    add(
        "Plate format",
        Some(format!(
            "{}×{} ({} wells)",
            run.plate.rows,
            run.plate.cols,
            run.plate.well_count()
        )),
    );
    add("Occupied wells", Some(run.wells.len().to_string()));
    let fluors: std::collections::BTreeSet<&str> = run
        .wells
        .iter()
        .flat_map(|w| w.channels.iter())
        .map(|c| c.fluorophore.as_str())
        .collect();
    add(
        "Fluorophores",
        Some(fluors.into_iter().collect::<Vec<_>>().join(", ")),
    );
    add("Notes", m.notes.clone());
    if let Some(protocol) = &run.protocol {
        add(
            "Protocol",
            Some(protocol.name.clone().unwrap_or_else(|| "(unnamed)".into())),
        );
        if let Some(lid) = protocol.lid_temperature {
            add("Lid temperature", Some(format!("{lid:.0} °C")));
        }
        add(
            "Amplification cycles",
            Some(protocol.amplification_cycles().to_string()),
        );
        for (i, step) in protocol.steps.iter().enumerate() {
            add(&format!("Step {}", i + 1), Some(describe_step(step)));
        }
    }
    app.set_info_rows(model(rows));
}

// ---------------------------------------------------------------------------
// Chart math
// ---------------------------------------------------------------------------

/// Map a value through the current Y transform (log10 with a floor, or identity).
fn ymap(v: f64, log: bool) -> f64 {
    if log { v.max(1.0).log10() } else { v }
}

/// Choose transformed axis bounds for the value axis.
fn value_axis(ymin: f64, ymax: f64, log: bool) -> (f64, f64) {
    if log {
        let lo = ymin.max(1.0).log10().floor();
        let hi = (ymax.max(10.0).log10()).ceil();
        (lo, hi.max(lo + 1.0))
    } else {
        pad(ymin, ymax)
    }
}

fn pad(min: f64, max: f64) -> (f64, f64) {
    if !min.is_finite() || !max.is_finite() || (max - min).abs() < f64::EPSILON {
        return (min - 1.0, max + 1.0);
    }
    let m = (max - min) * 0.05;
    (min - m, max + m)
}

/// Fraction from the top (0=top,1=bottom) of a transformed value on the axis.
fn frac_from_top(v: f64, lo: f64, hi: f64) -> f64 {
    if (hi - lo).abs() < f64::EPSILON {
        0.5
    } else {
        (1.0 - (v - lo) / (hi - lo)).clamp(0.0, 1.0)
    }
}

fn precompute_stable_commands(
    amp: &mut [Series],
    melt: &mut [MeltSeries],
    ncycles: usize,
    amp_range: AxisRange,
    melt_t_range: AxisRange,
    melt_r_range: AxisRange,
    melt_d_range: AxisRange,
) {
    let amp_range = amp_range.or_default();
    let (log_min, log_max) = value_axis(amp_range.min, amp_range.max, true);
    let (linear_min, linear_max) = value_axis(amp_range.min, amp_range.max, false);
    let xmax = ncycles.max(1) as f64;
    for series in amp {
        let log_ys: Vec<f64> = series.ys.iter().map(|&v| ymap(v, true)).collect();
        series.stable_log_commands =
            build_commands(&series.xs, &log_ys, 0.0, xmax, log_min, log_max);
        series.stable_linear_commands =
            build_commands(&series.xs, &series.ys, 0.0, xmax, linear_min, linear_max);
    }

    if !melt_t_range.is_valid() {
        return;
    }
    let r_range = melt_r_range.or_default();
    let d_range = melt_d_range.or_default();
    let (rmin, rmax) = pad(r_range.min, r_range.max);
    let (dmin, dmax) = pad(d_range.min, d_range.max);
    for series in melt {
        series.stable_rfu_commands = build_commands(
            &series.temperature,
            &series.rfu,
            melt_t_range.min,
            melt_t_range.max,
            rmin,
            rmax,
        );
        series.stable_derivative_commands = build_commands(
            &series.temperature,
            &series.derivative,
            melt_t_range.min,
            melt_t_range.max,
            dmin,
            dmax,
        );
    }
}

/// Build an SVG path string in the 0..1000 viewbox (Y flipped).
fn build_commands(xs: &[f64], ys: &[f64], xmin: f64, xmax: f64, ymin: f64, ymax: f64) -> String {
    let mut s = String::new();
    let mut started = false;
    for (&x, &y) in xs.iter().zip(ys.iter()) {
        if !x.is_finite() || !y.is_finite() {
            started = false;
            continue;
        }
        let px = if (xmax - xmin).abs() < f64::EPSILON {
            0.0
        } else {
            (x - xmin) / (xmax - xmin) * 1000.0
        };
        let py = frac_from_top(y, ymin, ymax) * 1000.0;
        if started {
            s.push_str(&format!("L {px:.1} {py:.1} "));
        } else {
            s.push_str(&format!("M {px:.1} {py:.1} "));
            started = true;
        }
    }
    s
}

fn x_axis_labels(xmax: f64) -> ModelRc<AxisLabel> {
    let step = nice_step(xmax, 6);
    let mut labels = Vec::new();
    let mut t = 0.0;
    while t <= xmax + 0.5 {
        labels.push(AxisLabel {
            pos: (t / xmax) as f32,
            text: SharedString::from(format!("{t:.0}")),
        });
        t += step;
    }
    model(labels)
}

fn value_axis_labels(lo: f64, hi: f64, log: bool) -> ModelRc<AxisLabel> {
    let mut labels = Vec::new();
    if log {
        let mut d = lo;
        while d <= hi + 0.001 {
            let pos = frac_from_top(d, lo, hi) as f32;
            labels.push(AxisLabel {
                pos,
                text: SharedString::from(format!("10^{:.0}", d)),
            });
            d += 1.0;
        }
    } else {
        return linear_axis_labels(lo, hi, "", true);
    }
    model(labels)
}

/// Linear tick labels. `flip=true` places `lo` at the bottom (value axis);
/// `flip=false` places `lo` at the left (temperature/X axis).
fn linear_axis_labels(lo: f64, hi: f64, suffix: &str, flip: bool) -> ModelRc<AxisLabel> {
    let step = nice_step(hi - lo, 5);
    let mut labels = Vec::new();
    let start = (lo / step).ceil() * step;
    let mut t = start;
    while t <= hi + step * 0.001 {
        let frac = if (hi - lo).abs() < f64::EPSILON {
            0.5
        } else {
            (t - lo) / (hi - lo)
        };
        let pos = (if flip { 1.0 - frac } else { frac }).clamp(0.0, 1.0) as f32;
        let txt = if t.abs() >= 1000.0 {
            format!("{:.0}{}", t, suffix)
        } else {
            format!("{:.1}{}", t, suffix)
        };
        labels.push(AxisLabel {
            pos,
            text: SharedString::from(txt),
        });
        t += step;
    }
    model(labels)
}

fn nice_step(span: f64, target: usize) -> f64 {
    if span <= 0.0 || !span.is_finite() {
        return 1.0;
    }
    let raw = span / target as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let norm = raw / mag;
    let nice = if norm < 1.5 {
        1.0
    } else if norm < 3.0 {
        2.0
    } else if norm < 7.0 {
        5.0
    } else {
        10.0
    };
    nice * mag
}

// ---------------------------------------------------------------------------
// Colours & codes
// ---------------------------------------------------------------------------

fn hex(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb_u8(r, g, b)
}

/// A distinct trace colour per well (CFX "Random by Well" default), cycled from
/// a fixed, high-contrast palette so runs render deterministically.
fn well_brush(row: u8, col: u8) -> Brush {
    const PALETTE: [(u8, u8, u8); 12] = [
        (0x1B, 0x4F, 0xE0),
        (0x1F, 0x9E, 0x1F),
        (0xF2, 0x6B, 0x1D),
        (0x8E, 0x24, 0xAA),
        (0xD8, 0x1B, 0x60),
        (0x00, 0x93, 0x88),
        (0xC6, 0xA7, 0x00),
        (0x5D, 0x40, 0x37),
        (0x00, 0x77, 0xC2),
        (0x7C, 0xB3, 0x42),
        (0xE5, 0x39, 0x35),
        (0x3F, 0x51, 0xB5),
    ];
    let idx = (row as usize * 12 + col as usize) % PALETTE.len();
    let (r, g, b) = PALETTE[idx];
    Brush::SolidColor(hex(r, g, b))
}

fn well_fill(t: SampleType) -> Brush {
    let c = match t {
        SampleType::Unknown => hex(0xDC, 0xEB, 0xF7),
        SampleType::Standard => hex(0x8F, 0xD9, 0x8A),
        SampleType::Ntc => hex(0xF5, 0xD9, 0x3B),
        SampleType::Nrt => hex(0xF7, 0xC9, 0x8A),
        SampleType::PositiveControl | SampleType::NegativeControl => hex(0xE9, 0xC9, 0xF0),
        SampleType::Empty => hex(0xF4, 0xF4, 0xF4),
    };
    Brush::SolidColor(c)
}

fn content_code(t: SampleType) -> String {
    match t {
        SampleType::Unknown => "Unk",
        SampleType::Standard => "Std",
        SampleType::Ntc => "NTC",
        SampleType::Nrt => "NRT",
        SampleType::PositiveControl => "Pos",
        SampleType::NegativeControl => "Neg",
        SampleType::Empty => "",
    }
    .to_string()
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

fn shorten_path(p: &str) -> String {
    Path::new(p)
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| p.to_string())
}

/// Where "Export Run as JSON" writes: next to the source file/dir, else the CWD.
fn export_path(run: &QpcrRun) -> PathBuf {
    const NAME: &str = "openqpcr-export.json";
    if let Some(src) = &run.metadata.source_file {
        let p = Path::new(src);
        let dir = if p.is_dir() {
            Some(p.to_path_buf())
        } else {
            p.parent().map(Path::to_path_buf)
        };
        if let Some(dir) = dir {
            return dir.join(NAME);
        }
    }
    PathBuf::from(NAME)
}

/// Like [`export_path`] but with the given extension (e.g. "rdml", "csv").
fn export_path_with_ext(run: &QpcrRun, ext: &str) -> PathBuf {
    export_path(run).with_extension(ext)
}

/// Small extension so the no-arg launch degrades gracefully to an empty run.
trait OrDefaultRun {
    fn unwrap_or_default_run(self) -> QpcrRun;
}
impl OrDefaultRun for openqpcr::Result<QpcrRun> {
    fn unwrap_or_default_run(self) -> QpcrRun {
        self.unwrap_or_default()
    }
}
