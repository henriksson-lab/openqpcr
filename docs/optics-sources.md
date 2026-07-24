# CFX optics reference — public sources

The optical-channel and fluorophore data in `src/optics.rs` is a **clean-room
reconstruction from public documentation**. It does not use, and does not require,
Bio-Rad's proprietary `referencedata-*.xml` configuration files. This file records
the public sources for each value so the table can be maintained and audited
independently.

## Channel → filter (ex/em range) → fluorophore

**Primary source — Bio-Rad CFX Maestro Software User Guide, "Table 6. Factory
calibrated fluorophores, channels, and instruments" (Bio-Rad Bulletin 10000068703).**
Canonical URL: `https://www.bio-rad.com/webroot/web/pdf/lsr/literature/10000068703.pdf`
(Bio-Rad official; bio-rad.com blocks automated fetches, so it was read via the
Internet Archive Wayback Machine at that canonical URL).

| Ch | Excitation (nm) | Emission (nm) | Factory fluorophores |
|----|-----------------|---------------|----------------------|
| 1 | 450–490 | 515–530 | FAM, SYBR Green I |
| 2 | 515–535 | 560–580 | VIC, HEX, CAL Fluor Gold 540, CAL Fluor Orange 560 |
| 3 | 560–590 | 610–650 | ROX, Texas Red, CAL Fluor Red 610, TEX 615 |
| 4 | 620–650 | 675–690 | Cy5, Quasar 670 |
| 5 | 672–684 | 705–730 | Quasar 705, Cy5.5 |
| 6 | FRET | FRET | dedicated FRET channel — "does not require calibration for specific dyes" |

**Corroborating sources:**
- Bio-Rad official *Excitation and Detection Wavelength Charts* for the CFX96 Touch
  (figures on `bio-rad.com/.../amp_excit_emiss_cfx96touch.html`) — matches Table 6
  except channel-1 emission, shown as 510–530 on the chart vs 515–530 in Table 6
  (we use Table 6's 515–530 as the more formal spec).
- LGC Biosearch Technologies, *Thermal cycler spectral calibration instructions
  (CAL Fluor and Quasar dyes)* — confirms these are the CFX factory dyes and their
  channel assignments (dye-vendor official).
- Bio-Rad CFX Opus User Guide (Bulletin 10000119983) and CFX Opus 96 datasheet
  (Bulletin 7299) — instrument optics ranges (450–730 nm; 6 channels incl. FRET on
  the 96-well line, 5 on the 384).

## Fluorophore → excitation/emission maxima

- **Bio-Rad Fluorophore Reference Guide, Bulletin 2421**
  (`bio-rad.com/.../Bulletin_2421.pdf`, Bio-Rad official): FAM 492/517, SYBR Green I
  494/521, TET 520/540, HEX 530/556, ROX 567/591, Texas Red 596/615, Cy5 650/670,
  Cy5.5 675/694.
- **LGC Biosearch Technologies** dye pages (vendor official, for their proprietary
  dyes): CAL Fluor Gold 540 522/544, CAL Fluor Orange 560 538/559, CAL Fluor Red 610
  590/610, Quasar 670 647/670, Quasar 705 690/705.
- **VIC** (Applied Biosystems, proprietary) has no single authoritative public
  Bio-Rad number; the ~538/554 value is from community spectral databases (FPbase /
  AAT Bioquest). It reads on channel 2 per Table 6.

## What is NOT publicly documented (and not needed here)

- Exact filter **centre wavelength + bandwidth** per channel (e.g. "485/20"): Bio-Rad
  publishes ranges, not centre/FWHM pairs. The ranges are sufficient for our purpose
  (labelling channels, matching dyes to channels).
- Per-dye **spectral deconvolution / pure-dye calibration matrices**: instrument-
  internal calibration data, never published — and not part of the channel/dye map.
  These live on the instrument and in each run file's `calibrationCollection`.
