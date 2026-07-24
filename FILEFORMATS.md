## Notes on file formats

**RDML (`.rdml`)** [RDML](https://rdml.org) is an open XML/ZIP
interchange format exported by many instruments (Bio-Rad CFX, Thermo QuantStudio,
Roche LightCycler 96, Agilent AriaMx, Analytik Jena qTOWER, BMS Mic, …). openqpcr
reads `.rdml` (and bare RDML XML) into the shared model — samples, targets/dyes,
Cq, per-cycle amplification (`adp`), and melt curves (`mdp`) — and writes it back
out. The reader is version-tolerant (v1.0–1.3), locates the XML payload by content
(the member name is not standardised — e.g. Bio-Rad names it `BioRad_qPCR_melt.xml`),
and tolerates ZIP variants and Cq-only files with no raw curves. Validated against
real CFX, Applied Biosystems StepOne, and Roche LightCycler 96 exports.

**Biorad native `.pcrd`** A `.pcrd` is a ZIP container whose single inner
entry is encrypted with traditional PKWARE ZipCrypto under a fixed key baked into
CFX Manager; decrypted, it is plaintext `<experimentalData2>` XML. openqpcr reads
it directly into the shared model — instrument/serial/software metadata, plate
geometry and per-well sample/target/group/type, the thermal protocol, and the raw
per-cycle amplification (and melt) curves reconstructed from the channel-major
`plateRead` optics arrays. Files carrying an *additional* user-set open password
(a separate, per-file feature) still report as encrypted. The `inspect` command
lists the internal entries without interpreting them.

## Usage

```sh
# Summarise a run (a single export CSV/XLSX, or an export directory)
openqpcr summary path/to/export_dir/
openqpcr summary run.xlsx

# Full run as JSON
openqpcr json run.csv --pretty

# Inspect the internals of a native .pcrd archive
openqpcr inspect experiment.pcrd
```

