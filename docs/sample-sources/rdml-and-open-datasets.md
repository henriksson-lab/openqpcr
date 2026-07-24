# RDML and open qPCR raw-data sources

A corpus of **real, openly-available RDML files** (and related open qPCR raw-data
datasets) for testing the `openqpcr` RDML reader/writer. RDML (rdml.org) is an
open XML-in-ZIP interchange format exported by many vendors (Bio-Rad CFX, Applied
Biosystems / Life Technologies StepOne / QuantStudio, Roche LightCycler, Agilent
AriaMx, Analytik Jena qTOWER, Bio Molecular Systems Mic, etc.), so one collection
of RDML files gives cross-vendor coverage from a single parser.

Compiled 2026-07-19. "Verified?" = link resolved to a real file and (for
downloaded files) the ZIP contained a parseable `rdml_data.xml` (or equivalent).

## Format notes learned while verifying

- An `.rdml` file is a **ZIP archive**. Inspect with `unzip -l file.rdml`.
- The XML member is **usually** named `rdml_data.xml`, but the spec allows any
  name. Bio-Rad CFX names it after the run, e.g. `BioRad_qPCR_melt.xml`. A robust
  reader must read the first `*.xml` member, not hard-code `rdml_data.xml`.
- Root element is `<rdml ... version="X.Y">`; versions in the wild here are
  **1.0, 1.1, 1.3** (1.2 also exists per the spec).
- Raw per-cycle amplification curves live in `<adp>` (amplification data point)
  elements; raw melt curves live in `<mdp>` (melt data point). **Many RDML files
  omit raw curves** and carry only `<cq>` values (e.g. large plate exports /
  database deposits) — a reader must not assume `adp`/`mdp` are present.

## Primary source repositories

| Source | Repo / site | License | Notes |
|---|---|---|---|
| RDML-Python reference implementation | `github.com/RDML-consortium/rdmlpython` | MIT | `test/` and `experiments/` ship real `.rdml` fixtures; canonical parser |
| RDML-Tools (GEAR) web apps | `github.com/RDML-consortium/rdml-tools` | GPL-3.0 | `client/src/static/bin/*.rdml` demo files; also served at gear-genomics.com |
| RDML R package (PCRuniversum) | `github.com/PCRuniversum/RDML` | MIT | `inst/extdata/*.rdml` real instrument exports |
| rdml (Mathematica pkg, R. Magno) | `github.com/ramiromagno/rdml` | MIT | `datasets/*.rdml` incl. RDML-consortium DB export |
| RDMLdb / RDML database | `rdml.org` (RDMLdb online repository) | per-deposit | Searchable public RDML repository; download after search |

## Detailed file table

Instrument/vendor is taken from the `<instrument>` element inside each file where
present. adp = raw amplification curves, mdp = raw melt curves, cq = quantification
cycle values only.

| File | Direct URL (raw) | RDML ver | Instrument / vendor | Data carried | Size | License | Verified? |
|---|---|---|---|---|---|---|---|
| `stepone_std.rdml` | https://raw.githubusercontent.com/PCRuniversum/RDML/master/inst/extdata/stepone_std.rdml | 1.0 | Applied Biosystems **StepOne** (Life Tech / ABI) | adp (raw amp), cq; 32 samples / 1 target | 8.7 KB | MIT | ✅ downloaded |
| `BioRad_qPCR_melt.rdml` | https://raw.githubusercontent.com/PCRuniversum/RDML/master/inst/extdata/BioRad_qPCR_melt.rdml | 1.1 | **Bio-Rad CFX** (Block 96FX, SN CC009466) | adp + mdp (raw amp **and** melt) | 73 KB | MIT | ✅ downloaded |
| `lc96_bACTXY.rdml` | https://raw.githubusercontent.com/PCRuniversum/RDML/master/inst/extdata/lc96_bACTXY.rdml | 1.0 | **Roche LightCycler 96** (SN LC96SN11301) | adp (raw amp), cq; 108 samples / 8 targets | 371 KB | MIT | ✅ downloaded |
| `test_2_raw_data.rdml` | https://raw.githubusercontent.com/RDML-consortium/rdml-tools/main/client/src/static/bin/test_2_raw_data.rdml | 1.3 | not recorded in file | adp (raw amp); 144 samples / 4 targets | 22 KB | GPL-3.0 | ✅ downloaded |
| `test_mca_1_raw_data.rdml` | https://raw.githubusercontent.com/RDML-consortium/rdmlpython/main/test/test_mca_1_raw_data.rdml | 1.3 | melt-curve analysis test set | mdp (raw melt); 100 samples / 1 target | 32 KB | MIT | ✅ downloaded |
| `1507AA03.rdml` | https://raw.githubusercontent.com/ramiromagno/rdml/master/datasets/1507AA03.rdml | 1.0 | RDML-consortium DB export | **cq only, no raw curves**; 384 targets, large plate | 73 KB | MIT | ✅ downloaded |
| `test_1_raw_data.rdml` | https://raw.githubusercontent.com/RDML-consortium/rdml-tools/main/client/src/static/bin/test_1_raw_data.rdml | 1.3 | RDML-Tools demo | adp (raw amp) | 12 KB | GPL-3.0 | link listed |
| `test_3_raw_data.rdml` | https://raw.githubusercontent.com/RDML-consortium/rdml-tools/main/client/src/static/bin/test_3_raw_data.rdml | 1.3 | RDML-Tools demo | adp (raw amp) | 52 KB | GPL-3.0 | link listed |
| `test_4_raw_data.rdml` | https://raw.githubusercontent.com/RDML-consortium/rdml-tools/main/client/src/static/bin/test_4_raw_data.rdml | 1.3 | RDML-Tools demo | adp (raw amp) | 52 KB | GPL-3.0 | link listed |
| `test_5_raw_data.rdml` | https://raw.githubusercontent.com/RDML-consortium/rdml-tools/main/client/src/static/bin/test_5_raw_data.rdml | 1.3 | RDML-Tools demo | adp (raw amp) | 10 KB | GPL-3.0 | link listed |
| `absolute.rdml` | https://raw.githubusercontent.com/RDML-consortium/rdml-tools/main/client/src/static/bin/absolute.rdml | 1.x | RDML-Tools (absolute quant demo) | adp + cq | 56 KB | GPL-3.0 | link listed |
| `relative.rdml` | https://raw.githubusercontent.com/RDML-consortium/rdml-tools/main/client/src/static/bin/relative.rdml | 1.x | RDML-Tools (relative quant demo) | adp + cq, multi-plate | 188 KB | GPL-3.0 | link listed |
| `genorm.rdml` | https://raw.githubusercontent.com/RDML-consortium/rdml-tools/main/client/src/static/bin/genorm.rdml | 1.x | RDML-Tools (geNorm demo) | cq / results | 2.9 KB | GPL-3.0 | link listed |
| `linregpcr.rdml` | https://raw.githubusercontent.com/RDML-consortium/rdml-tools/main/client/src/static/bin/linregpcr.rdml | 1.x | RDML-Tools (LinRegPCR demo) | cq / results | 3.3 KB | GPL-3.0 | link listed |
| `meltingcurveanalysis.rdml` | https://raw.githubusercontent.com/RDML-consortium/rdml-tools/main/client/src/static/bin/meltingcurveanalysis.rdml | 1.x | RDML-Tools (melt demo) | mdp (raw melt) | 43 KB | GPL-3.0 | link listed |
| `merge.rdml` / `merge_add.rdml` | https://raw.githubusercontent.com/RDML-consortium/rdml-tools/main/client/src/static/bin/merge.rdml | 1.x | RDML-Tools (tiny merge demos) | minimal skeleton | <1 KB | GPL-3.0 | link listed |
| `test_mca_5_raw_data.rdml` | https://raw.githubusercontent.com/RDML-consortium/rdmlpython/main/test/test_mca_5_raw_data.rdml | 1.3 | rdmlpython large melt test | mdp (raw melt), large | 1.7 MB | MIT | link listed |
| `data_vermeulen_raw.rdml` | https://raw.githubusercontent.com/RDML-consortium/rdmlpython/main/experiments/vermeulen/data_vermeulen_raw.rdml | 1.3 | Vermeulen et al. neuroblastoma study | adp (raw amp), large real study | 6.7 MB | MIT | link listed |
| `data_vermeulen_std_150000_15.rdml` | https://raw.githubusercontent.com/RDML-consortium/rdmlpython/main/experiments/vermeulen/data_vermeulen_std_150000_15.rdml | 1.3 | Vermeulen study (standard-curve subset) | adp + cq | 268 KB | MIT | link listed |
| `amplicon_primer_mix.rdml` | https://raw.githubusercontent.com/RDML-consortium/rdmlpython/main/experiments/untergasser/amplicon_primer_mix.rdml | 1.3 | Untergasser primer-mix experiment | adp (raw amp), large | 1.7 MB | MIT | link listed |
| `QPCRCourseApril2015_plate_1_.rdml` | https://raw.githubusercontent.com/ramiromagno/rdml/master/datasets/QPCRCourseApril2015_plate_1_.rdml | 1.x | Ruijter et al. 2015 qPCR course plate | adp + cq | 60 KB | MIT | link listed |
| `rpa.rdml` | https://raw.githubusercontent.com/ramiromagno/rdml/master/datasets/rpa.rdml | 1.x | Raquel P. Andrade Lab (CBMR, Portugal) | adp + cq | 146 KB | MIT | link listed |
| RDMLdb online repository | https://rdml.org/ | varies | Searchable deposits, many vendors | varies (often adp/mdp/cq) | varies | per-deposit | site verified |

Notes on "link listed" rows: URLs were enumerated via the GitHub API (file exists,
size known) but the file was not downloaded/opened here; RDML version shown as
"1.x" was not individually confirmed. RDML-Tools demo files (`test_1..5`,
`absolute`, `relative`, etc.) are also served over HTTP at
`https://www.gear-genomics.com/rdml-tools/static/bin/<name>.rdml`.

## Downloaded files (in `samples/rdml/`)

All six were confirmed to be valid ZIP archives containing a parseable RDML XML
member. Renamed on download with a `vendor_` prefix for clarity.

| Local filename | RDML ver | Vendor | Raw data | XML member name | Bytes |
|---|---|---|---|---|---|
| `abi_stepone_std.rdml` | 1.0 | Applied Biosystems StepOne (ABI / Life Tech) | adp + cq | `rdml_data.xml` | 8 740 |
| `biorad_cfx_melt.rdml` | 1.1 | Bio-Rad CFX (Block 96FX) | adp + mdp | `BioRad_qPCR_melt.xml` (non-standard) | 74 510 |
| `roche_lc96_bACTXY.rdml` | 1.0 | Roche LightCycler 96 | adp + cq | `rdml_data.xml` | 379 603 |
| `rdmltools_test_2_raw_data.rdml` | 1.3 | (unspecified) RDML-Tools demo | adp | `rdml_data.xml` | 22 033 |
| `rdmlpython_test_mca_1_raw_data.rdml` | 1.3 | melt-curve test set | mdp | `rdml_data.xml` | 32 733 |
| `rdmldb_1507AA03.rdml` | 1.0 | RDML-consortium DB export | cq only (no raw) | `rdml_data.xml` | 74 520 |

Vendor spread achieved: **Bio-Rad CFX (v1.1)**, **Applied Biosystems StepOne
(v1.0)**, **Roche LightCycler 96 (v1.0)**, plus vendor-neutral RDML-Tools/rdmlpython
v1.3 fixtures. A dedicated rotor/tube-instrument RDML (Qiagen Rotor-Gene, Bio
Molecular Systems Mic) was **not found** in these open repos; RDMLdb (rdml.org) is
the place to search for one if needed.

## Best test fixtures (recommended round-trip / regression set)

Pick these 5 for an RDML reader test suite — diverse vendors, versions, and data
shapes, all small:

1. **`biorad_cfx_melt.rdml`** (v1.1, Bio-Rad CFX) — the single most valuable
   fixture: real instrument, carries **both** `adp` and `mdp` raw curves, **and**
   its XML member is named `BioRad_qPCR_melt.xml` rather than `rdml_data.xml`, so
   it exercises the "read first `*.xml` member" path. ~73 KB.
2. **`abi_stepone_std.rdml`** (v1.0, ABI StepOne) — tiny (8.7 KB), real vendor,
   raw `adp` + `cq`, standard-curve run. Great fast smoke test for v1.0.
3. **`roche_lc96_bACTXY.rdml`** (v1.0, Roche LightCycler 96) — third vendor,
   multi-target (8 targets, 108 samples), lots of raw `adp`. Exercises larger
   multi-well parsing while staying under 400 KB.
4. **`rdmltools_test_2_raw_data.rdml`** (v1.3) — canonical RDML-Tools reference
   fixture for the **newest** schema version with raw `adp`; multi-target
   (4 targets). Ensures v1.3 element/attribute coverage.
5. **`rdmldb_1507AA03.rdml`** (v1.0) — a **cq-only** deposit with **no raw curves**
   and 384 targets. Essential negative/edge case: verifies the reader degrades
   gracefully when `adp`/`mdp` are absent and handles large target lists.

For a heavier stress/regression test add **`data_vermeulen_raw.rdml`** (6.7 MB,
v1.3, real published study) — not downloaded here to keep the sample dir small.
