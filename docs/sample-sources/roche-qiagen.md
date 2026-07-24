# Sample-data sources — Roche LightCycler & Qiagen Rotor-Gene

**Status:** research note. **Date:** 2026-07-19.
**Scope:** locate real example data files for **Roche LightCycler** and
**Qiagen Rotor-Gene** qPCR instruments so openqpcr can be tested against
non-CFX dialects.

## TL;DR

| Vendor | Native format | Open route | Real file downloaded? |
|---|---|---|---|
| **Roche LightCycler** | `.ixo` (LC480/1536, binary DB), `.lc96p` (LC96) — **closed, no open parser** | **RDML** (native on LC96; export on some LC480) + tab/XLSX text export (Cp + per-cycle fluorescence) | ✅ `samples/roche/lc96_bACTXY.rdml` (genuine LC96 RDML, MIT) |
| **Qiagen Rotor-Gene** | `.rex`, legacy `.gene` — **closed, no open parser** | **RDML** (Q-Rex export) + CSV/XLSX export | ❌ none openly-licensed found; best real source (rpoB repo) has the right file structure but the data files are empty stubs and the repo has no license |

Key semantic reminders confirmed from the files:
- **Roche uses Cp** (crossing point / 2nd-derivative-max) on LC480/1536; LC96
  reports Cq. In **RDML** both are normalised to the `<cq>` element — the LC96
  file below carries `<cq>` values plus raw `<adp>` amplification points.
- **Rotor-Gene is rotor-based** (36/72/100 integer-indexed positions, not a
  plate). Rotor-Gene CSV exports label rows by tube **No.** (1..N), which is the
  geometry signal openqpcr must not force into A1..H12 plate coordinates.

> **Network note:** the build sandbox has no outbound network; all downloads in
> this doc were run with the Bash sandbox disabled (`curl -L`). Commands are
> given so they can be reproduced.

---

## 1. Roche LightCycler

| # | Source | Direct URL | Format | Data carried | Cp/Cq semantics | License | Verified |
|---|---|---|---|---|---|---|---|
| R1 | **kablag / PCRuniversum `RDML` R package** — `inst/extdata/lc96_bACTXY.rdml` | `https://raw.githubusercontent.com/kablag/RDML/master/inst/extdata/lc96_bACTXY.rdml` | RDML v1.x (ZIP of XML) | Genuine **LightCycler 96** run: 19,200 raw amplification points (`<adp>`), `<cq>` values, melt data. Embeds Roche-only extension schemas (`roche.ch/LC96InstrumentDataSchema`, `LC96AppExtensionSchema`, `LC96CalculatedDataSchema`), instrument SN `LC96SN11301`, SW 1.1.0.1320 | Cq (LC96); Roche calculated-data blocks present | **MIT** (`License: MIT + file LICENSE`) | ✅ downloaded, unzipped, Roche metadata + Cq + adp confirmed |
| R2 | **RDML-consortium `rdmlpython`** — Vermeulen benchmark set (`experiments/vermeulen/data_vermeulen_raw.rdml` + std-curve dilutions) | `https://raw.githubusercontent.com/RDML-consortium/rdmlpython/main/experiments/vermeulen/data_vermeulen_raw.rdml` | RDML (ZIP) | Classic Vermeulen/Ruijter SYBR benchmark, full raw amplification curves, standard-curve dilution series | Historically generated on **LightCycler 480**, but the RDML file itself carries **no instrument tag** — treat Roche attribution as lineage only, not encoded | **MIT** (rdmlpython repo) | ⚠️ downloaded (6.9 MB); real qPCR raw data but instrument not encoded in-file |
| R3 | **RDML-consortium `rdmlpython`** — Untergasser experiments (`eva_green_a.rdml`, `probes.rdml`, `quantification_methods.rdml`, …) | `https://github.com/RDML-consortium/rdmlpython/tree/main/experiments/untergasser` | RDML v1.4 (ZIP) | Real amplification/melt data; dyes named `"SYBR - Roche"` (Roche **reagent**, not proof of a Roche instrument) | `<cq>` | **MIT** | ⚠️ inspected; Roche = reagent brand only, do not claim as LightCycler run |
| R4 | **Roche LC480 Operator's Manual / Quick Guides** (for the text/XLSX export layout: Cp table + per-cycle fluorescence) | `https://www.lebonheur.org/research/investigator-resources/manuals/Roche%20LightCycler%20480%20II%20manual.pdf` | PDF (documents export format, not a data file) | Column layout of LC480 tab/XLSX exports, Cp semantics | Cp (2nd-deriv-max) | Roche copyright (reference only) | ✅ reachable; documentation, not sample data |
| R5 | **RDMLdb** public qPCR-file repository (deposit-by-accession; LC96 is an RDML-exporting instrument) | `http://www.rdmldb.org` | RDML | Author-deposited real runs, citable by accession | mixed | site terms; per-file | ⚠️ site TLS cert mismatch (served under `*.cmgg.be`); browse/download needs the live site |

**Roche takeaways.** The one clean, verified, openly-licensed Roche file is
**R1 (LC96 RDML)** — now in `samples/roche/`. It is the recommended fixture for
exercising a Roche/LC96 RDML path (Roche extension schemas + `<cq>` + raw
`<adp>`). For LC480/1536 the practical route is **tab/XLSX text export** (Cp
column + per-cycle fluorescence) — no open `.ixo` parser exists; capture a real
export when a collaborator can produce one.

---

## 2. Qiagen Rotor-Gene

| # | Source | Direct URL | Format | Data carried | Rotor vs plate geometry | License | Verified |
|---|---|---|---|---|---|---|---|
| Q1 | **`nicolemalofsky/rpoB491Screening2025`** — `Main Report - SMASH Assay Rotor-Gene Data Repository/` (Expts 312/314/315/332/333/42) | `https://github.com/nicolemalofsky/rpoB491Screening2025/tree/main/Main%20Report%20-%20SMASH%20Assay%20Rotor-Gene%20Data%20Repository` | Native **`.rex`** + Rotor-Gene **"data sheet.csv"** (raw fluorescence) + **"analysis sheet.csv"** (Ct) + analyzed `.xlsx` | Real 2024 Rotor-Gene Q run structure: one `.rex` per run, a raw per-cycle CSV, and a Ct analysis CSV — exactly the Rotor-Gene Q export triplet | Rotor (documented by the export naming/structure) | **NONE** — repo has no LICENSE (all-rights-reserved by default) | ❌ **data files are 0-byte empty stubs** in the repo (only figures/PPTX/Prism have content); structure is real, bytes are not downloadable |
| Q2 | **`igg-molecular-biology-lab/pipe-t`** (Galaxy RT-qPCR tool; docs claim Rotor-Gene compatibility) | `https://github.com/igg-molecular-biology-lab/pipe-t/tree/master/examples` | tab-`.txt` | Shipped `examples/` are GEO/TaqMan-array (`GSM…`) ABI data, **not** Rotor-Gene | n/a | repo license | ✅ inspected — no actual Rotor-Gene file present |
| Q3 | **`FEUSION/Extractor`** — GUI extractor **for Q-Rex (Rotor-Gene Q) software** | `https://github.com/FEUSION/Extractor` | tool only | Confirms `.rex`/Q-Rex export handling exists | n/a | repo license | ✅ inspected — repo ships only README + demo video + logo, **no sample data** |
| Q4 | **RDMLdb** repository — Rotor-Gene Q is an RDML-exporting instrument, so deposits exist by accession | `http://www.rdmldb.org` | RDML | Author-deposited real Rotor-Gene runs | rotor (1 column, rows = rotor positions per RDML spec) | site terms | ⚠️ needs the live site (TLS cert mismatch when fetched programmatically) |
| Q5 | **Rotor-Gene Q User Manual** (export-format reference: CSV columns incl. tube **No.**, Colour, Ct) | `https://www.qiagen.com/us/resources/faq?id=37fee066-731a-4582-9752-cc75a3c3250f&lang=en` (export FAQ) / manual PDF `https://genecraftlabs.com/wp-content/uploads/2019/05/HB-0167-007_UM_RGQ_LS_0918_WW.pdf` | PDF (documents export, not a data file) | Column layout of Q-Rex CSV/Excel exports | rotor (tube No. 1..N) | Qiagen copyright (reference) | ✅ reachable; documentation only |
| Q6 | Third-party importers that accept Rotor-Gene exports (**qBase+**, **CAmpER**) — evidence of the export dialect, not a file source | (vendor/tool sites) | — | — | rotor | — | ⚠️ referenced in the tool-survey literature |

**Qiagen takeaways.** **No openly-licensed, byte-complete real Rotor-Gene file
could be downloaded.** The single best real-world source (**Q1**, a 2025 rpoB
screening study) demonstrates the exact Rotor-Gene Q export triplet
(`.rex` native + raw-data CSV + analysis CSV) with real experiment names, but
(a) the tracked data files are empty 0-byte placeholders in the repo, and (b)
the repo carries no license. To obtain a real Rotor-Gene fixture, the reliable
paths are: export **RDML** from Q-Rex, pull a deposit from **RDMLdb**, or have a
collaborator export a **CSV** (which will show integer tube/rotor positions —
the geometry openqpcr must preserve).

---

## 3. Proprietary / closed formats (export- or RDML-only routes)

Flag these to the project — there is **no open in-house parser**, so openqpcr
must consume them via text export or RDML:

| Format | Vendor / instrument | Nature | Only viable route |
|---|---|---|---|
| **`.ixo`** | Roche LightCycler 480 / 1536, Cobas z480 | Proprietary binary DB export | tab/XLSX text export, or RDML where the LC480 SW supports it |
| **`.lc96p`** | Roche LightCycler 96 | Proprietary (RDML-based) project | **RDML** (native export) — first-class, best route |
| **`.rex`** | Qiagen Rotor-Gene Q (Q-Rex) | Proprietary XML-ish, undocumented | **RDML** export, or CSV/XLSX export |
| **`.gene`** | Qiagen/Corbett Rotor-Gene 6000/3000 | Proprietary binary (legacy Corbett) | CSV/XLSX export |

None of these should be treated as parseable in-house without a real sample and
a clean-room effort; prioritise the **RDML** and **text-export** paths.

---

## 4. What was downloaded into `samples/`

| Path | Bytes | Vendor | Container | Notes |
|---|---|---|---|---|
| `samples/roche/lc96_bACTXY.rdml` | 379,603 | Roche LightCycler 96 | **ZIP** — entries: `rdml_data.xml`, `instrument_data.xml`, `app_data.xml`, `module_data.xml`, `calculated_data.xml`, `manifest.xml` | MIT (kablag/PCRuniversum `RDML`). 19,200 `<adp>` raw points + `<cq>` + Roche LC96 extension schemas. Verified genuine. |
| `samples/qiagen/` | — | — | — | **empty** — no openly-licensed real Rotor-Gene file obtainable (see §2). |

RDML files are **ZIP archives** of `rdml_data.xml` (+ vendor extension XMLs);
the LC96 file's internal entries are listed above.

### Reproduce the downloads

```bash
mkdir -p samples/roche samples/qiagen

# Roche LC96 (MIT) — the verified fixture
curl -L -o samples/roche/lc96_bACTXY.rdml \
  https://raw.githubusercontent.com/kablag/RDML/master/inst/extdata/lc96_bACTXY.rdml

# Optional: Vermeulen raw benchmark (MIT; LC480 by lineage, no instrument tag) — 6.9 MB
curl -L -o samples/roche/data_vermeulen_raw.rdml \
  https://raw.githubusercontent.com/RDML-consortium/rdmlpython/main/experiments/vermeulen/data_vermeulen_raw.rdml

# Inspect any RDML (it is a ZIP)
unzip -l samples/roche/lc96_bACTXY.rdml
unzip -p samples/roche/lc96_bACTXY.rdml rdml_data.xml | grep -o '<cq>[0-9.]*</cq>' | head
```

Rotor-Gene has no equivalent command — the real files (Q1) are empty in-repo and
unlicensed; use a Q-Rex RDML/CSV export or an RDMLdb deposit instead.
