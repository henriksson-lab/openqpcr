# Sample data sources — Thermo Fisher / Applied Biosystems (ABI) qPCR

Real-time qPCR instruments: QuantStudio family (1–12K Flex, 6/7 Pro), 7500 / 7900HT,
StepOne / StepOnePlus, ViiA 7. Software: Design & Analysis (D&A), SDS, QuantStudio.

Native project format is **`.eds`** — a plain (unencrypted) ZIP/OOXML container. The
qPCR payload lives outside the OOXML standard, under `apldbio/sds/` (EDS v1.x) or in
top-level JSON dirs (EDS v2.x). This makes `.eds` the most tractable ABI format to add
to `openqpcr`. Exports (CSV/TXT/XLSX) and RDML are secondary.

All files below were verified to resolve on 2026-07-19. Downloaded copies live in
`samples/thermo-abi/`.

## Sources

| # | Description | Direct URL | Format | Data carried | License / terms | Verified / downloaded |
|---|-------------|-----------|--------|--------------|-----------------|-----------------------|
| 1 | **qslib `test.eds`** — completed QuantStudio 3/5 run, EDS spec v1.3.2 | https://raw.githubusercontent.com/cgevans/qslib/main/tests/test.eds | native `.eds` (v1.3) | plate_setup, targets/samples, per-well raw filter data (`filterdata.xml`), multicomponent (`multicomponentdata.xml`), Rn/ΔRn + Ct (`analysis_result.txt`), per-well `.quant`, calibrations | EUPL-1.2 (`test.eds.license`) | ✅ downloaded `qslib-test.eds` (439 KB) |
| 2 | **qslib `mid-run.eds`** — a run captured mid-acquisition (partial) | https://raw.githubusercontent.com/cgevans/qslib/main/tests/mid-run.eds | native `.eds` (v1.3) | plate_setup, experiment, tcprotocol, partial `.quant` + `filter/*filterdata.xml`, messages.log | EUPL-1.2 (same repo) | ✅ downloaded `qslib-mid-run.eds` (367 KB) |
| 3 | **qslib `v2_test.eds`** — QuantStudio 6 Pro (QS6PRO), 384-well, **EDS v2.0 (JSON) layout** | https://raw.githubusercontent.com/cgevans/qslib/main/tests/v2_test.eds | native `.eds` (v2.0) | `setup/plate_setup.json`, `setup/run_method.json`, per-well `run/quant/*.quant`, `primary/multicomponent_data.json` (per-cycle fluorescence per dye), `primary/analysis_result.json` (Cq/quantity) | EUPL-1.2 (same repo) | ✅ downloaded `qslib-v2_test.eds` (185 KB) |
| 4 | **RDML R package `sce.eds`** — StepOne/7500-family file (internal header: "StepOne v2.0"), 96-well | https://raw.githubusercontent.com/kablag/RDML/master/inst/extdata/from_abi7500/sce.eds | native `.eds` (older OOXML+images layout) | experiment.xml, plate_setup.xml, tcprotocol, per-cycle raw camera `images/*.tiff`, `multicomponent_data.txt` (text), `post-roi.dat`, calibrations | MIT (`RDML` pkg `DESCRIPTION`: "MIT + file LICENSE") | ✅ downloaded `rdmlpkg-abi7500-sce.eds` (285 KB) |
| 5 | **RDML R package `stepone_std.rdml`** — StepOnePlus standard-curve run exported to RDML | https://raw.githubusercontent.com/kablag/RDML/master/inst/extdata/stepone_std.rdml | RDML (zip → `rdml_data.xml`) | RDML v1.x: samples, targets, standard-curve quantities, amplification/Cq | MIT | ✅ downloaded `rdmlpkg-stepone_std.rdml` (8.7 KB) |
| 6 | **Jay_qPCR demo `.eds`** — QuantStudio demo plate used by the `EDSReader.py` example | https://raw.githubusercontent.com/jayunruh/Jay_qPCR/HEAD/demo_plate/PER123RHV_1_06242020_1_ra.eds | native `.eds` | full real run (ΔRn + Ct via `analysis_result.txt`, plate_setup, experiment) | No explicit license (GitHub reports NOASSERTION) | ✅ link resolves (1.03 MB); **not downloaded** — license unclear |
| 7 | **Juul/eds-handler template** — an *extracted* pre-run `.eds` (no run data) + JS to generate/parse `.eds` | https://github.com/Juul/eds-handler/tree/master/template | `.eds` tree (v1.3 `apldbio/sds/*`) | `experiment.xml`, `plate_setup.xml`, `tcprotocol.xml`, `analysis_protocol.xml`, `Manifest.mf` — template only, no fluorescence | repo has no LICENSE file | ✅ tree resolves; documents structure well |
| 8 | **jayunruh/Jay_qPCR `EDSReader.py`** — reference parser for v1.x `.eds` (`analysis_result.txt`, `plate_setup.xml`, `experiment.xml`) | https://github.com/jayunruh/Jay_qPCR/blob/HEAD/code/EDSReader.py | source (reference) | documents where Ct/ΔRn live in v1.x | NOASSERTION | ✅ resolves |
| 9 | **cgevans/qslib** — Rust+Python lib that reads EDS v1.3 and (partial) v2.0; best format reference | https://github.com/cgevans/qslib | source + test data (rows 1–3) | supports Viia7, QS3/5, QS6 Flex, QS6 Pro; 96 & 384-well | EUPL-1.2 | ✅ resolves |
| 10 | **Thermo Fisher — QuantStudio Sample Data** (official demo experiment files) | https://www.thermofisher.com/us/en/home/life-science/pcr/real-time-pcr/real-time-pcr-instruments/quantstudio-systems/sample-data.html | native `.eds` / exports | official example runs per instrument | Thermo terms; may require account/region | ⚠️ page slow/timed out via fetch — open in browser; likely login-gated |
| 11 | **Zenodo 10.5281/zenodo.15072870** — RT-qPCR technical-replicate study; raw output from QuantStudio 3 & 7 Flex consolidated | https://doi.org/10.5281/zenodo.15072870 | exports (raw output files → master dataset) | Ct/Cq master table + raw instrument exports | Open (Zenodo, check record) | ✅ DOI resolves; not downloaded (export data, larger) |
| 12 | **figshare 16755340** — Supplementary Data 2: RT-qPCR results export | https://figshare.com/articles/dataset/Supplementary_Data_2_RT-qPCR_results/16755340 | export (XLSX/CSV) | Cq/results table | CC (check record) | ✅ resolves |
| 13 | **CRAN `RDML` package manual** — documents importing ABI 7500 / StepOne / QuantStudio into RDML | https://cran.r-project.org/web/packages/RDML/RDML.pdf | doc + bundled samples (rows 4–5) | how ABI formats map to RDML | GPL (pkg) | ✅ resolves |

### Note on the dead lead
`nzxzxw/edsbreaker` (an automated `.eds` parser referenced in several search results) now
returns **404** — repo removed/renamed. Excluded.

## What was downloaded

`samples/thermo-abi/` (5 real files, ~1.3 MB total):

| file | bytes | format / instrument | license |
|------|-------|---------------------|---------|
| `qslib-test.eds` | 439 264 | EDS v1.3, QuantStudio 3/5, complete run | EUPL-1.2 |
| `qslib-mid-run.eds` | 367 414 | EDS v1.3, mid-acquisition | EUPL-1.2 |
| `qslib-v2_test.eds` | 184 979 | EDS v2.0 (JSON), QuantStudio 6 Pro, 384-well | EUPL-1.2 |
| `rdmlpkg-abi7500-sce.eds` | 285 390 | EDS (StepOne/7500 v2.0, TIFF-image layout) | MIT |
| `rdmlpkg-stepone_std.rdml` | 8 740 | RDML export, StepOnePlus | MIT |
| `qslib-test.eds.license` | 108 | SPDX license text for row 1 | — |

Each `.eds` was confirmed a valid ZIP (`file` reports "Zip archive"/"Java archive";
`unzip -l` lists entries below).

## Reverse-engineering notes for `.eds`

`.eds` = ZIP. Rename to `.zip` / open with `unzip`. There are **three distinct internal
layouts** in the wild — the reader must branch on which is present.

### A. EDS v1.x — `apldbio/sds/` layout (QuantStudio D&A / SDS; rows 1, 2)
`Manifest.mf` declares the version, e.g. `Specification-Version: 1.3.2`,
`Implementation-Title: QuantStudio 3 and 5 Software`. Key entries:

- `apldbio/sds/experiment.xml` — run metadata: name, `RunState`, timestamps, chemistry
  (TaqMan/SYBR), instrument, source `.eds` path.
- `apldbio/sds/plate_setup.xml` — plate geometry (`Rows`/`Columns`, `PlateKind`
  `TYPE_8X12`), and a `FeatureMap` of per-well `Sample` / target / task / dye
  assignments (samples carry an `SP_UUID` custom property).
- `apldbio/sds/tcprotocol.xml` — thermal-cycling protocol (stages/steps/temps).
- `apldbio/sds/analysis_protocol.xml` — analysis settings (baseline, threshold, etc.).
- **Per-cycle fluorescence — two representations:**
  - `apldbio/sds/filterdata.xml` — **raw optical readings.** A `PlatePointDataCollection`
    of `PlatePointData` blocks, one per `<Stage>/<Cycle>/<Step>/<Point>` and filter set
    (`FILTER_SET` e.g. `x1-m1`), each holding a whitespace-separated `<WellData>` array of
    length rows×cols (row-major). One block per cycle × filter = the amplification curves.
  - `apldbio/sds/multicomponentdata.xml` — **dye-decomposed signal.** Header gives
    `WellCount`, `CycleCount`, `CollectionPoints`, per-well `SampleTemperatures`, `MSE`,
    and `DyeData WellIndex=…` with `DyeList` (e.g. `[FAM, ROX]`); dye component values per
    cycle follow.
- `apldbio/sds/analysis_result.txt` — **tab-separated results table** (the export the SDS
  UI shows). Header row `Well  Sample Name  Detector  Task  Ct  Avg Ct … Amp Status
  Cq Conf`; each well row is followed by two lines `Rn values  <c1> <c2> …` and
  `Delta Rn values  <c1> <c2> …` — i.e. the normalized amplification curve per well.
- `apldbio/sds/quant/S02_C0NN_T01_P0001_M?_X?_E?.quant` — one small file per well/filter.
  **Not binary**: an INI-like text file with `[instrument]`, `[head]`, `[properties]`,
  `[conditions]` sections and tab-separated columns (`BlkID Run ExcitationColor
  EmissionColor CameraBias …`). Redundant with filterdata.xml but per-well.
- `apldbio/sds/calibrations/*.ini` (roi, background, uniformity, puredye), `messages.log`.

Filenames encode indices: `S{stage}_C{cycle}_T{step}_P{point}_M{filter/emission}_X{excitation}_E{exposure}`.

### B. EDS v2.0 — top-level JSON layout (QuantStudio 6/7 Pro, D&A v2; row 3)
`Manifest.mf` + `summary.json` (`"instrumentType":"QS6PRO"`, `"blockType":"BLOCK_384W"`,
`analysis.primary.version`). Dirs:

- `setup/plate_setup.json` — plate/sample/target/task assignments.
- `setup/run_method.json` — thermal protocol.
- `primary/multicomponent_data.json` — **per-cycle fluorescence.** `collectionPoints[]`
  (stage/cycle/step/point) + `wellData[]`, each well: `wellIndex`, `dyeData[]` with
  `dyeName` + `fluorescences[]` (one value per collection point), and `temperatures[]`.
  This is the cleanest curve source in any ABI format.
- `primary/analysis_result.json` — `replicateGroupResults[]` with `cqMean/cqSD/cqSE`,
  `quantity`, `sampleName`, `targetName`, flags.
- `primary/analysis_setting.json`, `run/quant/*.quant`, `run/filter_data.json`,
  `run/run_summary.json`, `run/saturated_wells.json`, `calibrations/*.ini`.

### C. Older StepOne / 7500 layout — OOXML shell + TIFF images (row 4)
Has the full OOXML wrapper (`[Content_Types].xml`, `_rels/.rels`, `docProps/`, `xl/`)
alongside `apldbio/sds/`. Distinctive traits:

- `apldbio/sds/images/stage3-cycleNNN-pointP-D.tiff` — **raw per-cycle camera frames**
  (one TIFF per cycle × point × detector); fluorescence must be integrated from images or
  read from the pre-integrated text below.
- `apldbio/sds/multicomponent_data.txt` — text, header `StepOne v2.0 MulticomponentData`,
  columns `WELL CYCLE DYE LIST MSE SIGNAL DATA PURE_DYE_DATA`.
- `apldbio/sds/post-roi.dat` — binary ROI-integrated data; `images/temperatures.txt`.
- Path quirk: mixed-case duplicate dirs (`APLDBIO\SDS\CALIBRATIONS/…` with backslashes) —
  a reader should match case-insensitively and normalize separators.

### Practical guidance for openqpcr
- Detect layout: presence of `summary.json` ⇒ v2 (B); `apldbio/sds/experiment.xml` ⇒ v1
  (A); OOXML `xl/` + `images/*.tiff` ⇒ old StepOne/7500 (C).
- Easiest amplification curves: **B** `multicomponent_data.json`, then **A**
  `analysis_result.txt` (ΔRn) or `filterdata.xml` (raw). Avoid the `.quant` and TIFF paths
  unless raw data is required.
- Ct/Cq: **A** `analysis_result.txt` (`Ct` col); **B** `analysis_result.json` (`cqMean`).
- Best single starting sample: **`qslib-test.eds`** (row 1) — small, EUPL-licensed, a
  complete QuantStudio v1.3 run with every relevant file present.
