# Sample data sources: Agilent/Stratagene, Analytik Jena, Bio-Molecular Systems (Mic)

Survey of concrete, verifiable sources of **real** qPCR sample files for three
instrument families, plus what was actually downloaded into `samples/`.
Compiled 2026-07-19.

## TL;DR

| Vendor / instrument | Native format | Native = proprietary/closed? | Practical open route | Real file downloaded? |
|---|---|---|---|---|
| Stratagene / Agilent Mx3000P/Mx3005P | `.mxp` | **Yes** — OLE Compound Document (binary) | MxPro text/Excel export **or** parse `.mxp` (open-source parser exists) | **Yes** (native `.mxp` + text exports) |
| Agilent AriaMx / AriaDx | `.amxd` (+ project DB) | **Yes** — proprietary | **RDML** export (also Excel/text/CSV/LIMS) | No open real file found — RDML is the route |
| Analytik Jena qTOWER (qPCRsoft) | qPCRsoft project | **Yes** — proprietary | CSV/Excel export, also RDML | **Yes** (real qPCRsoft CSV exports) |
| Bio-Molecular Systems **Mic** (micPCR) | micPCR run/assay files | **Yes** — proprietary | **RDML** export (well supported), also CSV | No open real file found — RDML is the route |

**Key format facts confirmed by inspection:**
- `.mxp` is a Microsoft **OLE2 / Composite Document (CDF V2)** binary container, *not* a zip.
  `file 3005.mxp` -> `Composite Document File V2 Document`. Treat as closed; use the
  open-source parser below or MxPro exports.
- **RDML is a ZIP archive** containing a single `rdml_data.xml`. Verified:
  `unzip -l example.rdml` -> one entry `rdml_data.xml`. Any RDML reader must unzip first.
- **Geometry:** Mx / AriaMx / qTOWER are **plate**-based (wells `A1..H12`, etc.).
  **Mic is tube-based: 48 tubes**, loaded as 0.1 mL strips of 4 tubes (12 strips × 4),
  the first tube of each strip has an orientation tab. This is a genuine difference from
  the plate instruments and should be modelled as a 48-tube carousel, not a plate.

---

## 1. Agilent / Stratagene

### 1a. Mx3000P / Mx3005P (MxPro software) — DOWNLOADED

| # | Description | Direct URL | Format | Data carried | Geometry | License | Verified |
|---|---|---|---|---|---|---|---|
| A1 | `3000.mxp`, `3005.mxp` — native MxPro project files (test fixtures) | https://github.com/IMTMarburg/mxp/tree/master/mxp/testfiles | **Native `.mxp`** (OLE2 binary, proprietary) | Full run incl. raw fluorescence + results | 96-well plate | MIT | Yes — downloaded, `file` = Composite Document V2 |
| A2 | `3000/3005 - Text Report Data.txt` — MxPro **text report** export | same repo | Export (TSV/text) | Per-well Ct table: Well, Well Name, Dye, Assay, Replicate, Threshold(dR), Ct(dR), Ct Avg/SD | 96-well (`A1..`) | MIT | Yes — downloaded, human-readable |
| A3 | `3000/3005 - Instrument Data - Text Format 1.txt` — MxPro **amplification data** export | same repo | Export (text, ~0.8 MB) | Per-cycle raw fluorescence per well | 96-well | MIT | Yes — downloaded |
| A4 | `HL60 SKI GFP 19.04.2018.mxp`, `U937 cells ski and vector 7.05.2018.mxp` — real user experiments | same repo | Native `.mxp` | Real gene-expression runs | 96-well | MIT | Yes — downloaded (~160 KB each) |
| A5 | **IMTMarburg/mxp** Python parser (reads `.mxp` from 300/305 machines) | https://github.com/IMTMarburg/mxp | code | reference impl. for the closed `.mxp` container | — | MIT | Yes — this is where A1–A4 come from |
| A6 | `Stratagene_MxPro_to_table` — MxPro **vertical-chart export** massaging (protein thermal shift) | https://github.com/kugatomodai/Stratagene_MxPro_to_table | Export (MxPro chart/table) | example of MxPro tabular export layout | plate | (repo, check) | Partially — repo listing only |

MxPro export menu supports `.xls`, `.ppt`, `.bmp`, `.txt`, `.xml`. The `.mxp` native
format is closed; A5 is the practical reference implementation.

### 1b. AriaMx / AriaDx (Aria software) — NO OPEN REAL FILE FOUND; RDML route

| # | Description | Direct URL | Format | Notes | Verified |
|---|---|---|---|---|---|
| B1 | "How to Export AriaMx Data to Excel, Text, LIMS, or **RDML**" (Agilent qPCR portal) | https://community.agilent.com/knowledge/qpcr-portal/kmp/qpcr-articles/kp933.how-to-export-ariamx-data-to-an-excel-text-lims-data-or-rdml-file | doc | Confirms AriaMx exports RDML/Excel/text/CSV; native `.amxd` is opened only by Aria software | Yes (doc) |
| B2 | Aria Real-Time PCR Software User Manual (K8930-90013) | https://www.agilent.com/cs/library/usermanuals/public/K8930-90013.pdf | PDF | Documents export layouts / RDML fields | Yes (doc) |
| B3 | Custom export configurations in AriaMx | https://community.agilent.com/knowledge/qpcr-portal/kmp/qpcr-articles/kp1256.using-custom-configurations-for-data-export-in-the-ariamx-software | doc | Column layout of AriaMx text/CSV exports | Yes (doc) |

No openly-licensed real AriaMx `.rdml`/CSV was located on GitHub/Zenodo (Zenodo `q=AriaMx`
-> 0 hits; GitHub repo search `ariamx`/`agilent aria pcr` -> 0 repos). **AriaMx RDML is a
standard RDML zip**, so the generic RDML references staged in `samples/mic/` (see §3) double
as an AriaMx-format stand-in for parser work until a real AriaMx export is obtained (best
sources: RDMLdb / rdml.org depositions, or export one from Aria software).

---

## 2. Analytik Jena — qTOWER series (qPCRsoft) — DOWNLOADED

| # | Description | Direct URL | Format | Data carried | Geometry | License | Verified |
|---|---|---|---|---|---|---|---|
| J1 | `BAX.csv`, `Bcl-2.csv`, `IL-10.csv`, `IL-6.csv`, `TNF.csv` — real **qPCRsoft CSV exports** | https://github.com/alexcornwell/Ct | Export (CSV) | Genuine RT-qPCR run: header block (`Title`, `Date/Time`, `Device: qTOWER⁸⁴ /G, 3107G-0413`, `Operator`, `Colors+Dyes` FAM/JOE/TAMRA/ROX/Cy5, `Heated Lid`, `TC Protocol` steps, `Melt active`) followed by per-well/per-cycle data | plate (qTOWER 84; strips report says 384-well plate loaded) | GPL-3.0 | Yes — downloaded, header confirms qTOWER + qPCRsoft |
| J2 | Repo README (sample prep, primers, instrument statement) | https://github.com/alexcornwell/Ct/blob/master/README.md | doc | "Raw files were obtained by QTower 84 AnalytikJena" | — | GPL-3.0 | Yes |

The qPCRsoft CSV export is a distinctive layout: a metadata preamble (colon-delimited
key/value rows), a `Colors+Dyes` table, a `TC Protocol` step table, then the fluorescence
matrix. Encoding note: the files contain Latin-1 bytes (`°C`, superscript `⁸⁴` render as
`\xNN`); read as latin-1/cp1252, not strict UTF-8. qTOWER RDML export also exists (qPCRsoft)
but no real qTOWER `.rdml` was found openly.

---

## 3. Bio-Molecular Systems — Mic (micPCR) — NO OPEN REAL FILE FOUND; RDML route

**Mic is tube-based (48 tubes)**, magnetic induction cycler. micPCR is the only software;
its native run/assay files are proprietary. RDML export is well supported (BMS explicitly
promotes RDML/MIQE compliance) and is the practical open path.

| # | Description | Direct URL | Format | Notes | Verified |
|---|---|---|---|---|---|
| M1 | Mic qPCR analysis software page (RDML/MIQE export claims) | https://biomolecularsystems.com/mic-pcr/software/ | doc | Confirms RDML export + assay/run file model | Yes (doc) |
| M2 | micPCR software download (installer bundles demo runs) | https://biomolecularsystems.com/media-downloads/micqpcr-downloads/ | installer | **Requires free registration** (page gated); installer typically ships example runs | Gated — not downloadable unauthenticated |
| M3 | Mic User Manual v2.10 (tube geometry, export workflow) | https://www.slideshare.net/ItzelLpezGonzlez1/mic-user-manual-version-210pdf | doc | Documents 48-tube layout, 4-tube strips, RDML export | Yes (doc) |
| M4 | RDMLdb — online RDML repository (Mic depositions common) | https://www.rdmldb.org/ | RDML files | Central repo; files referenced by unique ID in papers. NOTE: TLS cert currently mismatched (`*.cmgg.be`), and download needs the record ID/login | Partially — site reachable, cert/login issue |
| M5 | RDML consortium tools + spec (RDML-Ninja, validator) | https://rdml.org/ | tools/spec | Reference for RDML zip structure Mic emits | Yes (doc) |

**Staged format stand-ins** (real RDML, but NOT Mic-emitted — see `samples/mic/README.md`):
generic RDML v1.1 and v1.3 example files from the RDML consortium site
(`RDML-consortium/rdml-consortium.github.io`, GPL-2.0). They are valid RDML zips
(`rdml_data.xml` inside) usable to develop the RDML reader that AriaMx/qTOWER/Mic all need.
Replace with a genuine Mic export (micPCR -> Export -> RDML) or an RDMLdb Mic deposition
when available.

---

## Downloaded files (in `samples/`)

```
samples/agilent/           (MIT — IMTMarburg/mxp)
  3000.mxp                                   254 KB  native OLE2 (proprietary)
  3005.mxp                                   266 KB  native OLE2 (proprietary)
  3000 - Instrument Data - Text Format 1.txt 795 KB  MxPro amplification export
  3005 - Instrument Data - Text Format 1.txt 812 KB  MxPro amplification export
  3000 - Text Report Data.txt                3.9 KB  MxPro Ct text report
  3005 - Text Report Data.txt                4.5 KB  MxPro Ct text report
  HL60 SKI GFP 19.04.2018.mxp                161 KB  native, real experiment
  U937 cells ski and vector 7.05.2018.mxp    165 KB  native, real experiment
  LICENSE.mxp-repo.txt                               MIT license text

samples/analytikjena/      (GPL-3.0 — alexcornwell/Ct)
  BAX.csv Bcl-2.csv IL-10.csv IL-6.csv TNF.csv  ~15 KB each  real qPCRsoft CSV exports
  LICENSE.Ct-repo.txt / README.Ct-repo.md              provenance

samples/mic/               (GPL-2.0 — RDML consortium; GENERIC, not Mic-native)
  _GENERIC_rdml-consortium_v1.1_example.rdml  74 KB  valid RDML zip (format stand-in)
  _GENERIC_rdml-consortium_v1.3_example.rdml  74 KB  valid RDML zip (format stand-in)
  README.md                                          why these are stand-ins + how to get real Mic files
```

## Recommendations for the parser

1. **`.mxp` (Mx) and `.amxd` (AriaMx) are proprietary/closed** — do not target them directly
   except via the MIT `IMTMarburg/mxp` reference for `.mxp`. Prefer **exports**: MxPro
   text/`.txt` (have real samples) for Mx; RDML/CSV for AriaMx.
2. **RDML is the single highest-leverage importer**: it is the practical/only open path for
   **AriaMx, qTOWER, and Mic**, and RDML files are just zips of `rdml_data.xml`. One robust
   RDML reader covers three vendors (plus Roche/Bio-Rad/ABI, which dominate the open RDML
   corpora at `PCRuniversum/RDML`, `ramiromagno/rdml`, `RDML-consortium/rdmlpython`).
3. **qPCRsoft CSV** (qTOWER) has a real, ready sample set — good first non-RDML export target;
   remember latin-1 decoding.
4. **Model Mic as 48 tubes**, not a plate, in geometry/UI.
