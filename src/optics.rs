//! CFX optical-system reference data — fluorophore ↔ detection-channel assignments
//! and excitation/emission wavelengths.
//!
//! **Clean-room / public sources.** Every value here is reconstructed from publicly
//! available Bio-Rad and dye-vendor documentation, not from any proprietary Bio-Rad
//! configuration file. Full citations are in `docs/optics-sources.md`. The primary
//! source is the Bio-Rad CFX Maestro Software User Guide, "Table 6. Factory
//! calibrated fluorophores, channels, and instruments" (Bulletin 10000068703),
//! cross-checked against Bio-Rad's published excitation/detection wavelength charts,
//! Bio-Rad Bulletin 2421 (dye ex/em maxima), and LGC Biosearch Technologies dye
//! pages (CAL Fluor / Quasar dyes).
//!
//! Bio-Rad publishes filter *ranges* per channel (e.g. 450–490 nm), not the exact
//! filter centre/bandwidth pairs, so the ranges here are the public, citable form —
//! sufficient to label channels and match dyes to channels, which is all we need.

/// One optical detection channel of the CFX shuttle optics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpticalChannel {
    /// 1-based channel number as CFX labels it.
    pub number: u8,
    /// Excitation passband (nm), `None` for the dedicated FRET channel.
    pub excitation_nm: Option<(u16, u16)>,
    /// Emission/detection passband (nm), `None` for the FRET channel.
    pub emission_nm: Option<(u16, u16)>,
    /// Factory-calibrated fluorophores read on this channel.
    pub fluorophores: &'static [&'static str],
}

/// The CFX detection channels (CFX96 / CFX384 / CFX Connect / CFX Opus family).
/// Channel 6 is the dedicated FRET channel — it detects a donor/acceptor pair
/// rather than a single dye and "does not require calibration for specific dyes",
/// so it carries no fixed filter passband here.
pub const CHANNELS: &[OpticalChannel] = &[
    OpticalChannel {
        number: 1,
        excitation_nm: Some((450, 490)),
        emission_nm: Some((515, 530)),
        fluorophores: &["FAM", "SYBR", "SYBR Green I"],
    },
    OpticalChannel {
        number: 2,
        excitation_nm: Some((515, 535)),
        emission_nm: Some((560, 580)),
        fluorophores: &["VIC", "HEX", "TET", "CAL Fluor Gold 540", "CAL Fluor Orange 560"],
    },
    OpticalChannel {
        number: 3,
        excitation_nm: Some((560, 590)),
        emission_nm: Some((610, 650)),
        fluorophores: &["ROX", "Texas Red", "CAL Fluor Red 610", "TEX 615"],
    },
    OpticalChannel {
        number: 4,
        excitation_nm: Some((620, 650)),
        emission_nm: Some((675, 690)),
        fluorophores: &["Cy5", "Quasar 670"],
    },
    OpticalChannel {
        number: 5,
        excitation_nm: Some((672, 684)),
        emission_nm: Some((705, 730)),
        fluorophores: &["Quasar 705", "Cy5.5"],
    },
    OpticalChannel {
        number: 6,
        excitation_nm: None,
        emission_nm: None,
        fluorophores: &["FRET"],
    },
];

/// A fluorophore and its excitation/emission maxima (nm).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fluorophore {
    pub name: &'static str,
    /// CFX detection channel (1-based), if it is a factory-calibrated CFX dye.
    pub channel: Option<u8>,
    /// Excitation maximum (nm).
    pub excitation_max_nm: u16,
    /// Emission maximum (nm).
    pub emission_max_nm: u16,
}

/// Common CFX fluorophores with excitation/emission maxima. Ex/em maxima are from
/// Bio-Rad Bulletin 2421 and LGC Biosearch dye pages (see `docs/optics-sources.md`);
/// VIC (proprietary) uses a community-database value.
pub const FLUOROPHORES: &[Fluorophore] = &[
    Fluorophore { name: "FAM", channel: Some(1), excitation_max_nm: 492, emission_max_nm: 517 },
    Fluorophore { name: "SYBR Green I", channel: Some(1), excitation_max_nm: 494, emission_max_nm: 521 },
    Fluorophore { name: "TET", channel: Some(2), excitation_max_nm: 520, emission_max_nm: 540 },
    Fluorophore { name: "HEX", channel: Some(2), excitation_max_nm: 530, emission_max_nm: 556 },
    Fluorophore { name: "VIC", channel: Some(2), excitation_max_nm: 538, emission_max_nm: 554 },
    Fluorophore { name: "CAL Fluor Gold 540", channel: Some(2), excitation_max_nm: 522, emission_max_nm: 544 },
    Fluorophore { name: "CAL Fluor Orange 560", channel: Some(2), excitation_max_nm: 538, emission_max_nm: 559 },
    Fluorophore { name: "ROX", channel: Some(3), excitation_max_nm: 567, emission_max_nm: 591 },
    Fluorophore { name: "Texas Red", channel: Some(3), excitation_max_nm: 596, emission_max_nm: 615 },
    Fluorophore { name: "CAL Fluor Red 610", channel: Some(3), excitation_max_nm: 590, emission_max_nm: 610 },
    Fluorophore { name: "Cy5", channel: Some(4), excitation_max_nm: 650, emission_max_nm: 670 },
    Fluorophore { name: "Quasar 670", channel: Some(4), excitation_max_nm: 647, emission_max_nm: 670 },
    Fluorophore { name: "Quasar 705", channel: Some(5), excitation_max_nm: 690, emission_max_nm: 705 },
    Fluorophore { name: "Cy5.5", channel: Some(5), excitation_max_nm: 675, emission_max_nm: 694 },
];

/// Normalize a fluorophore name for matching: lowercase, drop spaces/hyphens, and
/// fold the common "SYBR"/"SYBR Green I"/"SYBR Green" spellings together.
fn normalize(name: &str) -> String {
    let n: String = name
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .flat_map(|c| c.to_lowercase())
        .collect();
    if n.starts_with("sybr") { "sybr".to_string() } else { n }
}

/// The 1-based CFX detection channel a fluorophore is read on, matched
/// case/spacing-insensitively. `None` if the dye is not a known CFX factory dye.
pub fn channel_for_fluorophore(name: &str) -> Option<u8> {
    let target = normalize(name);
    CHANNELS
        .iter()
        .find(|ch| ch.fluorophores.iter().any(|f| normalize(f) == target))
        .map(|ch| ch.number)
}

/// Look up a channel by its 1-based number.
pub fn channel(number: u8) -> Option<&'static OpticalChannel> {
    CHANNELS.iter().find(|ch| ch.number == number)
}

/// Look up a fluorophore's ex/em record by name (case/spacing-insensitive).
pub fn fluorophore(name: &str) -> Option<&'static Fluorophore> {
    let target = normalize(name);
    FLUOROPHORES.iter().find(|f| normalize(f.name) == target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_dyes_to_channels() {
        assert_eq!(channel_for_fluorophore("FAM"), Some(1));
        assert_eq!(channel_for_fluorophore("SYBR"), Some(1));
        assert_eq!(channel_for_fluorophore("sybr green i"), Some(1));
        assert_eq!(channel_for_fluorophore("HEX"), Some(2));
        assert_eq!(channel_for_fluorophore("VIC"), Some(2));
        assert_eq!(channel_for_fluorophore("Texas Red"), Some(3));
        assert_eq!(channel_for_fluorophore("Cy5"), Some(4));
        assert_eq!(channel_for_fluorophore("Quasar 705"), Some(5));
        assert_eq!(channel_for_fluorophore("Cy5.5"), Some(5));
        assert_eq!(channel_for_fluorophore("nonsense"), None);
    }

    #[test]
    fn channel_lookup_and_fret() {
        let c1 = channel(1).unwrap();
        assert_eq!(c1.excitation_nm, Some((450, 490)));
        assert!(c1.fluorophores.contains(&"FAM"));
        // Channel 6 is the FRET channel with no fixed passband.
        let fret = channel(6).unwrap();
        assert_eq!(fret.excitation_nm, None);
        assert_eq!(fret.emission_nm, None);
        assert!(channel(9).is_none());
    }

    #[test]
    fn fluorophore_maxima_are_consistent_with_channel() {
        let fam = fluorophore("fam").unwrap();
        assert_eq!(fam.channel, Some(1));
        assert_eq!((fam.excitation_max_nm, fam.emission_max_nm), (492, 517));
        // Every fluorophore's declared channel agrees with the channel table.
        for f in FLUOROPHORES {
            if let Some(ch) = f.channel {
                assert_eq!(
                    channel_for_fluorophore(f.name),
                    Some(ch),
                    "{} channel mismatch",
                    f.name
                );
            }
        }
    }
}
