#!/usr/bin/env python3
"""Generate a realistic CFX-style export dataset for demoing openqpcr.

Writes the standard "Export All Data Sheets" CSV family into
examples/demo_export/. Not used at runtime — just produces sample data.

The plate is deliberately sectioned so every analysis tab has live data:

  Row A         Standard curve   GAPDH / FAM dilution series (SQ 1e6..1e1, dup)
  Rows B-E      Gene expression  multiplex FAM=GAPDH (ref) + HEX=IL6 (target),
                                 12 samples x 4 replicates, IL6 up/down-regulated
  Rows F-G      Allelic discrim. FAM=allele 1 / HEX=allele 2 SNP genotyping,
                                 a mix of hom-ref / hom-alt / het / no-call
  Row H         Controls         8 NTC (flat) + 4 positive controls

Two fluorophores are used (FAM, HEX) so the reader emits one amplification /
melt file per fluorophore, exactly like a real CFX multiplex export.
"""
import math
import os
import random

random.seed(42)
OUT = os.path.join(os.path.dirname(__file__), "demo_export")
os.makedirs(OUT, exist_ok=True)

ROWS = "ABCDEFGH"
COLS = range(1, 13)
NCYCLES = 40
FLUORS = ["FAM", "HEX"]

# Tm per target: GAPDH amplicon melts ~84C, IL6 ~80C.
TM_GAPDH, TM_IL6 = 84.0, 80.0

# --- plate model -----------------------------------------------------------
# wells[(row,col)] = dict(content, sample, bio, channels=[...])
# each channel = dict(fluor, target, cq (None = "N/A"), sq ("" = none), tm (None = no melt))
wells = {}


def std_cq(sq):
    # Higher starting quantity -> earlier Cq. Slope ~ -3.32 (100% efficiency).
    return 40.0 - 3.32 * math.log10(sq)


# Row A: GAPDH standard dilution series, 6 levels (1e6..1e1) in duplicate.
for j, c in enumerate(COLS):
    sq = 10 ** (6 - (j // 2))
    cq = std_cq(sq) + random.uniform(-0.1, 0.1)
    wells[("A", c)] = dict(
        content="Std", sample="", bio="",
        channels=[dict(fluor="FAM", target="GAPDH", cq=cq, sq=sq, tm=TM_GAPDH)],
    )

# Rows B-E: gene-expression panel. Each column is one sample (Sample01..12),
# rows B-E are 4 replicates. GAPDH (reference) is ~constant; IL6 shifts per
# sample to give a spread of relative expression. Calibrator = Sample01 (the
# alphabetically-first sample) -> its IL6 delta is 0 so its rel-expr is 1.0.
IL6_DELTA = [0.0, -2.0, -1.5, -1.0, -0.5, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, -0.3]
for r in "BCDE":
    for j, c in enumerate(COLS):
        sample = f"Sample{c:02d}"
        gapdh_cq = 20.0 + random.uniform(-0.15, 0.15)
        il6_cq = 20.0 + IL6_DELTA[j] + random.uniform(-0.15, 0.15)
        wells[(r, c)] = dict(
            content="Unkn", sample=sample, bio="Treatment" if c % 2 else "Control",
            channels=[
                dict(fluor="FAM", target="GAPDH", cq=gapdh_cq, sq="", tm=TM_GAPDH),
                dict(fluor="HEX", target="IL6", cq=il6_cq, sq="", tm=TM_IL6),
            ],
        )

# Rows F-G: allelic-discrimination (SNP genotyping). FAM = allele 1, HEX =
# allele 2. Both channels always run; genotype is read from which one crosses
# threshold (carries a Cq). Targets are left blank so these wells stay out of
# the gene-expression table.
GENO = ["A1"] * 8 + ["A2"] * 8 + ["HET"] * 5 + ["NONE"] * 3  # 24 wells
gi = 0
for r in "FG":
    for c in COLS:
        g = GENO[gi]
        gi += 1
        fam_cq = random.uniform(22, 28) if g in ("A1", "HET") else None
        hex_cq = random.uniform(22, 28) if g in ("A2", "HET") else None
        wells[(r, c)] = dict(
            content="Unkn", sample=f"Geno{gi:02d}", bio="",
            channels=[
                dict(fluor="FAM", target="", cq=fam_cq, sq="", tm=None),
                dict(fluor="HEX", target="", cq=hex_cq, sq="", tm=None),
            ],
        )

# Row H: controls. Cols 1-8 NTC (flat, no Cq); cols 9-12 positive controls
# (both channels amplify). Blank targets keep them out of gene expression.
for c in COLS:
    if c <= 8:
        wells[("H", c)] = dict(
            content="NTC", sample="", bio="",
            channels=[
                dict(fluor="FAM", target="", cq=None, sq="", tm=None),
                dict(fluor="HEX", target="", cq=None, sq="", tm=None),
            ],
        )
    else:
        wells[("H", c)] = dict(
            content="Pos Ctrl", sample="PC", bio="",
            channels=[
                dict(fluor="FAM", target="", cq=random.uniform(19, 21), sq="", tm=None),
                dict(fluor="HEX", target="", cq=random.uniform(19, 21), sq="", tm=None),
            ],
        )

order = [(r, c) for r in ROWS for c in COLS]


def wl(r, c):
    return f"{r}{c}"


def chan(r, c, fluor):
    """The channel dict for (well, fluor), or None if absent."""
    for ch in wells[(r, c)]["channels"]:
        if ch["fluor"] == fluor:
            return ch
    return None


# --- amplification RFU (one file per fluorophore) --------------------------
def amp_rfu(cycle, cq):
    if cq is None:
        return 15 + random.uniform(-2, 2)  # flat baseline noise (no call)
    plateau = 3000 + random.uniform(-200, 200)
    baseline = 12
    k = 0.65
    return baseline + plateau / (1 + math.exp(-k * (cycle - cq)))


for fluor in FLUORS:
    fname = f"demo - Quantification Amplification Results_{fluor}.csv"
    with open(os.path.join(OUT, fname), "w") as f:
        f.write("," + "Cycle," + ",".join(wl(r, c) for (r, c) in order) + "\n")
        for cyc in range(1, NCYCLES + 1):
            cells = []
            for (r, c) in order:
                ch = chan(r, c, fluor)
                cells.append("" if ch is None else f"{amp_rfu(cyc, ch['cq']):.2f}")
            f.write(f"{cyc},{cyc}," + ",".join(cells) + "\n")

# --- Cq results (one row per well x fluorophore) ---------------------------
with open(os.path.join(OUT, "demo - Quantification Cq Results.csv"), "w") as f:
    f.write("Well,Fluor,Target,Content,Sample,Biological Set Name,Cq,Starting Quantity (SQ)\n")
    for (r, c) in order:
        w = wells[(r, c)]
        for ch in w["channels"]:
            cq = "N/A" if ch["cq"] is None else f"{ch['cq']:.2f}"
            sq = "" if ch["sq"] == "" else f"{ch['sq']:.0f}"
            f.write(
                f"{wl(r,c)},{ch['fluor']},{ch['target']},{w['content']},"
                f"{w['sample']},{w['bio']},{cq},{sq}\n"
            )

# --- melt curves (one RFU + one derivative file per fluorophore) -----------
TSTART, TEND, TSTEP = 65.0, 95.0, 0.5
temps = [TSTART + i * TSTEP for i in range(int((TEND - TSTART) / TSTEP) + 1)]


def melt_rfu(t, tm):
    if tm is None:
        return 20 + random.uniform(-2, 2)
    return 200 / (1 + math.exp(1.2 * (t - tm))) + 20


def melt_deriv(t, tm):
    if tm is None:
        return random.uniform(-0.2, 0.2)
    h = 0.25
    return -(melt_rfu(t + h, tm) - melt_rfu(t - h, tm)) / (2 * h)


for fluor in FLUORS:
    rfu_name = f"demo - Melt Curve RFU Results_{fluor}.csv"
    der_name = f"demo - Melt Curve Derivative Results_{fluor}.csv"
    with open(os.path.join(OUT, rfu_name), "w") as f:
        f.write(",Temperature," + ",".join(wl(r, c) for (r, c) in order) + "\n")
        for i, t in enumerate(temps):
            cells = []
            for (r, c) in order:
                ch = chan(r, c, fluor)
                cells.append("" if ch is None or ch["tm"] is None
                             else f"{melt_rfu(t, ch['tm']):.2f}")
            f.write(f"{i},{t:.1f}," + ",".join(cells) + "\n")
    with open(os.path.join(OUT, der_name), "w") as f:
        f.write(",Temperature," + ",".join(wl(r, c) for (r, c) in order) + "\n")
        for i, t in enumerate(temps):
            cells = []
            for (r, c) in order:
                ch = chan(r, c, fluor)
                cells.append("" if ch is None or ch["tm"] is None
                             else f"{melt_deriv(t, ch['tm']):.3f}")
            f.write(f"{i},{t:.1f}," + ",".join(cells) + "\n")

# --- melt peaks (one row per well x fluorophore that has a melt) ------------
with open(os.path.join(OUT, "demo - Melt Curve Peak Results.csv"), "w") as f:
    f.write("Well,Fluor,Target,Content,Sample,Melt Temperature,Peak Height\n")
    for (r, c) in order:
        w = wells[(r, c)]
        for ch in w["channels"]:
            if ch["tm"] is None:
                continue
            f.write(
                f"{wl(r,c)},{ch['fluor']},{ch['target']},{w['content']},"
                f"{w['sample']},{ch['tm']:.1f},120.0\n"
            )

print("wrote demo export to", OUT)
