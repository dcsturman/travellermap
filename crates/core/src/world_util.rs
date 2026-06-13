//! Pure decoders for the world data sheet, ported from the reference
//! `world_util.js` (`prepareWorld` and its tables). Turns a [`World`]'s terse T5
//! Second Survey fields (UWP, PBG, remarks, extensions, stellar data, …) into
//! human-readable text for the client-side world-detail panel.
//!
//! **Port-exact:** every table/blurb/threshold here is copied verbatim from
//! `world_util.js` — do not paraphrase or "improve" the wording; the reference
//! site's sheet is the spec. I/O-free and `wasm`-safe (the two code tables are
//! `include_str!`-baked from `res/t5ss/`), so it lives in `tmap-core`.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::dto::World;

/// eHex (extended hex) alphabet — `world_util.js` `Util.fromHex`. Returns the
/// digit value, or `-1` when the character isn't a valid eHex digit.
pub fn from_hex(c: char) -> i32 {
    const ALPHABET: &str = "0123456789ABCDEFGHJKLMNPQRSTUVW";
    ALPHABET
        .find(c.to_ascii_uppercase())
        .map(|i| i as i32)
        .unwrap_or(-1)
}

// ── Single-glyph blurb tables (verbatim from world_util.js) ──────────────────

fn starport_blurb(c: char) -> Option<&'static str> {
    Some(match c {
        'A' => "Excellent",
        'B' => "Good",
        'C' => "Routine",
        'D' => "Poor",
        'E' => "Frontier Installation",
        'X' => "None or Unknown",
        'F' => "Good",
        'G' => "Poor",
        'H' => "Primitive",
        'Y' => "None",
        '?' => "Unknown",
        _ => return None,
    })
}

fn siz_blurb(c: char) -> Option<&'static str> {
    Some(match c {
        '0' => "Asteroid Belt",
        'S' => "Small World",
        '1' => "1,600km (0.12g)",
        '2' => "3,200km (0.25g)",
        '3' => "4,800km (0.38g)",
        '4' => "6,400km (0.50g)",
        '5' => "8,000km (0.63g)",
        '6' => "9,600km (0.75g)",
        '7' => "11,200km (0.88g)",
        '8' => "12,800km (1.0g)",
        '9' => "14,400km (1.12g)",
        'A' => "16,000km (1.25g)",
        'B' => "17,600km (1.38g)",
        'C' => "19,200km (1.50g)",
        'D' => "20,800km (1.63g)",
        'E' => "22,400km (1.75g)",
        'F' => "24,000km (2.0g)",
        'X' => "Unknown",
        '?' => "Unknown",
        _ => return None,
    })
}

fn atm_blurb(c: char) -> Option<&'static str> {
    Some(match c {
        '0' => "No atmosphere",
        '1' => "Trace",
        '2' => "Very thin; Tainted",
        '3' => "Very thin",
        '4' => "Thin; Tainted",
        '5' => "Thin",
        '6' => "Standard",
        '7' => "Standard; Tainted",
        '8' => "Dense",
        '9' => "Dense; Tainted",
        'A' => "Exotic",
        'B' => "Corrosive",
        'C' => "Insidious",
        'D' => "Dense, high",
        'E' => "Thin, low",
        'F' => "Unusual",
        'X' => "Unknown",
        '?' => "Unknown",
        _ => return None,
    })
}

fn hyd_blurb(c: char) -> Option<&'static str> {
    Some(match c {
        '0' => "Desert World",
        '1' => "10%",
        '2' => "20%",
        '3' => "30%",
        '4' => "40%",
        '5' => "50%",
        '6' => "60%",
        '7' => "70%",
        '8' => "80%",
        '9' => "90%",
        'A' => "Water World",
        'X' => "Unknown",
        '?' => "Unknown",
        _ => return None,
    })
}

fn pop_blurb(c: char) -> Option<&'static str> {
    Some(match c {
        '0' => "Unpopulated",
        '1' => "Tens",
        '2' => "Hundreds",
        '3' => "Thousands",
        '4' => "Tens of thousands",
        '5' => "Hundreds of thousands",
        '6' => "Millions",
        '7' => "Tens of millions",
        '8' => "Hundreds of millions",
        '9' => "Billions",
        'A' => "Tens of billions",
        'B' => "Hundreds of billions",
        'C' => "Trillions",
        'D' => "Tens of trillions",
        'E' => "Hundreds of tillions",
        'F' => "Quadrillions",
        'X' => "Unknown",
        '?' => "Unknown",
        _ => return None,
    })
}

fn gov_blurb(c: char) -> Option<&'static str> {
    Some(match c {
        '0' => "No Government Structure",
        '1' => "Company/Corporation",
        '2' => "Participating Democracy",
        '3' => "Self-Perpetuating Oligarchy",
        '4' => "Representative Democracy",
        '5' => "Feudal Technocracy",
        '6' => "Captive Government / Colony",
        '7' => "Balkanization",
        '8' => "Civil Service Bureaucracy",
        '9' => "Impersonal Bureaucracy",
        'A' => "Charismatic Dictator",
        'B' => "Non-Charismatic Dictator",
        'C' => "Charismatic Oligarchy",
        'D' => "Religious Dictatorship",
        'E' => "Religious Autocracy",
        'F' => "Totalitarian Oligarchy",
        'X' => "Unknown",
        '?' => "Unknown",
        // Legacy / Non-Human
        'G' => "Small Station or Facility",
        'H' => "Split Clan Control",
        'J' => "Single On-world Clan Control",
        'K' => "Single Multi-world Clan Control",
        'L' => "Major Clan Control",
        'M' => "Vassal Clan Control",
        'N' => "Major Vassal Clan Control",
        'P' => "Small Station or Facility",
        'Q' => "Krurruna or Krumanak Rule for Off-world Steppelord",
        'R' => "Steppelord On-world Rule",
        'S' => "Sept",
        'T' => "Unsupervised Anarchy",
        'U' => "Supervised Anarchy",
        'W' => "Committee",
        _ => return None,
    })
}

fn law_blurb(c: char) -> Option<&'static str> {
    Some(match c {
        '0' => "No prohibitions",
        '1' => "Body pistols, explosives, and poison gas prohibited",
        '2' => "Portable energy weapons prohibited",
        '3' => "Machine guns, automatic rifles prohibited",
        '4' => "Light assault weapons prohibited",
        '5' => "Personal concealable weapons prohibited",
        '6' => "All firearms except shotguns prohibited",
        '7' => "Shotguns prohibited",
        '8' => "Long bladed weapons controlled; open possession prohibited",
        '9' => "Possession of weapons outside the home prohibited",
        'A' => "Weapon possession prohibited",
        'B' => "Rigid control of civilian movement",
        'C' => "Unrestricted invasion of privacy",
        'D' => "Paramilitary law enforcement",
        'E' => "Full-fledged police state",
        'F' => "All facets of daily life regularly legislated and controlled",
        'G' => "Severe punishment for petty infractions",
        'H' => "Legalized oppressive practices",
        'J' => "Routinely oppressive and restrictive",
        'K' => "Excessively oppressive and restrictive",
        'L' => "Totally oppressive and restrictive",
        'S' => "Special/Variable situation",
        'X' => "Unknown",
        '?' => "Unknown",
        _ => return None,
    })
}

fn tech_blurb(c: char) -> Option<&'static str> {
    Some(match c {
        '0' => "Stone Age",
        '1' => "Bronze, Iron",
        '2' => "Printing Press",
        '3' => "Basic Science",
        '4' => "External Combustion",
        '5' => "Mass Production",
        '6' => "Nuclear Power",
        '7' => "Miniaturized Electronics",
        '8' => "Quality Computers",
        '9' => "Anti-Gravity",
        'A' => "Interstellar community",
        'B' => "Lower Average Imperial",
        'C' => "Average Imperial",
        'D' => "Above Average Imperial",
        'E' => "Above Average Imperial",
        'F' => "Technical Imperial Maximum",
        'G' => "Robots",
        'H' => "Artificial Intelligence",
        'J' => "Personal Disintegrators",
        'K' => "Plastic Metals",
        'L' => "Comprehensible only as technological magic",
        'X' => "Unknown",
        '?' => "Unknown",
        _ => return None,
    })
}

fn ix_imp_blurb(s: &str) -> Option<&'static str> {
    Some(match s {
        "-3" | "-2" => "Very unimportant",
        "-1" | "0" => "Unimportant",
        "1" | "2" | "3" => "Ordinary",
        "4" => "Important",
        "5" => "Very important",
        "?" => "Unknown",
        _ => return None,
    })
}

fn ex_resources_blurb(c: char) -> Option<&'static str> {
    Some(match c {
        '2' | '3' => "Very scarce",
        '4' | '5' => "Scarce",
        '6' | '7' => "Few",
        '8' | '9' => "Moderate",
        'A' | 'B' => "Abundant",
        'C' | 'D' => "Very abundant",
        'E' | 'F' | 'G' | 'H' | 'J' => "Extremely abundant",
        '?' => "Unknown",
        _ => return None,
    })
}

fn ex_infrastructure_blurb(c: char) -> Option<&'static str> {
    Some(match c {
        '0' => "Non-existent",
        '1' | '2' => "Extremely limited",
        '3' | '4' => "Very limited",
        '5' | '6' => "Limited",
        '7' | '8' => "Generally available",
        '9' | 'A' => "Extensive",
        'B' | 'C' => "Very extensive",
        'D' | 'E' => "Comprehensive",
        'F' | 'G' | 'H' => "Very comprehensive",
        '?' => "Unknown",
        _ => return None,
    })
}

fn ex_efficiency_blurb(s: &str) -> Option<&'static str> {
    Some(match s {
        "-5" => "Extremely poor",
        "-4" => "Very poor",
        "-3" => "Poor",
        "-2" => "Fair",
        "-1" | "0" | "+1" => "Average",
        "+2" => "Good",
        "+3" => "Improved",
        "+4" => "Advanced",
        "+5" => "Very advanced",
        "?" => "Unknown",
        _ => return None,
    })
}

fn cx_heterogeneity_blurb(c: char) -> Option<&'static str> {
    Some(match c {
        '0' => "N/A",
        '1' | '2' | '3' => "Monolithic",
        '4' | '5' | '6' => "Harmonious",
        '7' | '8' | '9' | 'A' | 'B' => "Discordant",
        'C' | 'D' | 'E' | 'F' | 'G' => "Fragmented",
        '?' => "Unknown",
        _ => return None,
    })
}

fn cx_acceptance_blurb(c: char) -> Option<&'static str> {
    Some(match c {
        '0' => "N/A",
        '1' => "Extremely xenophobic",
        '2' => "Very xenophobic",
        '3' => "Xenophobic",
        '4' => "Extremely aloof",
        '5' => "Very aloof",
        '6' | '7' => "Aloof",
        '8' | '9' => "Friendly",
        'A' => "Very friendly",
        'B' => "Extremely friendly",
        'C' => "Xenophilic",
        'D' => "Very Xenophilic",
        'E' | 'F' => "Extremely xenophilic",
        '?' => "Unknown",
        _ => return None,
    })
}

fn cx_strangeness_blurb(c: char) -> Option<&'static str> {
    Some(match c {
        '0' => "N/A",
        '1' => "Very typical",
        '2' => "Typical",
        '3' => "Somewhat typical",
        '4' => "Somewhat distinct",
        '5' => "Distinct",
        '6' => "Very distinct",
        '7' => "Confusing",
        '8' => "Very confusing",
        '9' => "Extremely confusing",
        'A' => "Incomprehensible",
        '?' => "Unknown",
        _ => return None,
    })
}

fn cx_symbols_blurb(c: char) -> Option<&'static str> {
    Some(match c {
        '0' | '1' => "Extremely concrete",
        '2' | '3' => "Very concrete",
        '4' | '5' => "Concrete",
        '6' | '7' => "Somewhat concrete",
        '8' | '9' => "Somewhat abstract",
        'A' | 'B' => "Abstract",
        'C' | 'D' => "Very abstract",
        'E' | 'F' | 'G' => "Extremely abstract",
        'H' | 'J' | 'K' | 'L' => "Incomprehensibly abstract",
        '?' => "Unknown",
        _ => return None,
    })
}

fn nobility_blurb(c: char) -> &'static str {
    match c {
        'B' => "Knight",
        'c' => "Baronet",
        'C' => "Baron",
        'D' => "Marquis",
        'e' => "Viscount",
        'E' => "Count",
        'f' => "Duke",
        'F' => "Subsector Duke",
        'G' => "Archduke",
        'H' => "Emperor",
        '?' => "Unknown",
        _ => "???",
    }
}

fn base_blurb(c: char) -> Option<&'static str> {
    Some(match c {
        'C' => "Corsair Base",
        'D' => "Naval Depot",
        'E' => "Embassy",
        'F' => "Ruins",
        'H' => "Hiver Supply Base",
        'I' => "Interface",
        'K' => "Naval Base",
        'L' => "Naval Base",
        'M' => "Military Base",
        'N' => "Naval Base",
        'O' => "Naval Outpost",
        'R' => "Clan Base",
        'S' => "Scout Base",
        'T' => "Tlauku Base",
        'V' => "Exploration Base",
        'W' => "Way Station",
        'X' => "Relay Station",
        'Z' => "Naval/Military Base",
        _ => return None,
    })
}

/// Exact-match remark codes (`REMARKS_TABLE` in world_util.js).
fn remarks_table(code: &str) -> Option<&'static str> {
    Some(match code {
        // Planetary
        "As" => "Asteroid Belt",
        "De" => "Desert",
        "Fl" => "Fluid Hydrographics (in place of water)",
        "Ga" => "Garden World",
        "He" => "Hellworld",
        "Ic" => "Ice Capped",
        "Oc" => "Ocean World",
        "Va" => "Vacuum World",
        "Wa" => "Water World",
        // Population
        "Di" => "Dieback",
        "Ba" => "Barren",
        "Lo" => "Low Population",
        "Ni" => "Non-Industrial",
        "Ph" => "Pre-High Population",
        "Hi" => "High Population",
        // Economic
        "Pa" => "Pre-Agricultural",
        "Ag" => "Agricultural",
        "Na" => "Non-Agricultural",
        "Px" => "Prison, Exile Camp",
        "Pi" => "Pre-Industrial",
        "In" => "Industrialized",
        "Po" => "Poor",
        "Pr" => "Pre-Rich",
        "Ri" => "Rich",
        // Climate
        "Fr" => "Frozen",
        "Ho" => "Hot",
        "Co" => "Cold",
        "Lk" => "Locked",
        "Tr" => "Tropic",
        "Tu" => "Tundra",
        "Tz" => "Twilight Zone",
        // Secondary
        "Fa" => "Farming",
        "Mi" => "Mining",
        "Mr" => "Military Rule",
        "Pe" => "Penal Colony",
        "Re" => "Reserve",
        // Political
        "Cp" => "Subsector Capital",
        "Cs" => "Sector Capital",
        "Cx" => "Capital",
        "Cy" => "Colony",
        // Special
        "Sa" => "Satellite",
        "Fo" => "Forbidden",
        "Pz" => "Puzzle",
        "Da" => "Danger",
        "Ab" => "Data Repository",
        "An" => "Ancient Site",
        "Rs" => "Research Station",
        "RsA" => "Research Station Alpha",
        "RsB" => "Research Station Beta",
        "RsG" => "Research Station Gamma",
        "RsD" => "Research Station Delta",
        "RsE" => "Research Station Epsilon",
        "RsZ" => "Research Station Zeta",
        "RsH" => "Research Station Eta",
        "RsT" => "Research Station Theta",
        // Legacy
        "Nh" => "Non-Hiver Population",
        "Nk" => "Non-K'kree Population",
        "Tp" => "Terra-prime",
        "Tn" => "Terra-norm",
        "Lt" => "Low Technology",
        "Ht" => "High Technology",
        "St" => "Steppeworld",
        "Ex" => "Exile Camp",
        "Xb" => "Xboat Station",
        "Cr" => "Reserve Capital",
        _ => return None,
    })
}

/// Ordinary/compact stellar luminosity → class (`STELLAR_TABLE`).
fn stellar_blurb(code: &str) -> Option<&'static str> {
    Some(match code {
        "Ia" | "Ib" => "Supergiant",
        "II" | "III" => "Giant",
        "IV" => "Subgiant",
        "V" => "Dwarf (Main Sequence)",
        "VI" => "Subdwarf",
        "VII" => "White Dwarf",
        "D" => "White Dwarf",
        "BD" => "Brown Dwarf",
        "BH" => "Black Hole",
        "PSR" => "Pulsar",
        "NS" => "Neutron Star",
        _ => return None,
    })
}

// ── Embedded code tables (baked from res/t5ss, wasm-safe) ────────────────────

/// Legacy single-character sophont codes (`SOPHONT_TABLE` in world_util.js),
/// extended at runtime with the T5SS multi-char codes from
/// `res/t5ss/sophont_codes.tab`.
fn sophont_name(code: &str) -> Option<String> {
    static TABLE: OnceLock<HashMap<String, String>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut m: HashMap<String, String> = [
            ("A", "Aslan"),
            ("C", "Chirper"),
            ("D", "Droyne"),
            ("F", "Non-Hiver"),
            ("H", "Hiver"),
            ("I", "Ithklur"),
            ("M", "Human"),
            ("V", "Vargr"),
            ("X", "Addaxur"),
            ("Z", "Zhodani"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        // Extend with T5SS codes: `Code <tab> Name <tab> Location`, skip header.
        let raw = include_str!("../../../res/t5ss/sophont_codes.tab");
        for line in raw.lines().skip(1) {
            let mut it = line.split('\t');
            if let (Some(code), Some(name)) = (it.next(), it.next()) {
                let (code, name) = (code.trim(), name.trim());
                if !code.is_empty() && !name.is_empty() {
                    m.insert(code.to_string(), name.to_string());
                }
            }
        }
        m
    });
    table.get(code).cloned()
}

/// Allegiance code → full display name, from `res/t5ss/allegiance_codes.tab`
/// (`Code <tab> Legacy <tab> BaseCode <tab> Name <tab> Location`, skip header).
///
/// Mirrors the reference `SecondSurvey.GetStockAllegianceFromCode` priority so
/// **base/legacy** codes used by sector borders (e.g. `As`, `Im`, `So`, `Zh`)
/// resolve, not just full T5 codes: T5 code → legacy→T5 overrides → hardcoded
/// legacy stock (no generic T5 code) → Legacy-column fallback.
pub fn allegiance_name(code: &str) -> Option<String> {
    static T5: OnceLock<HashMap<String, String>> = OnceLock::new();
    static LEGACY: OnceLock<HashMap<String, String>> = OnceLock::new();
    let raw = include_str!("../../../res/t5ss/allegiance_codes.tab");
    // T5 code (col 0) → name.
    let t5 = T5.get_or_init(|| {
        let mut m = HashMap::new();
        for line in raw.lines().skip(1) {
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() >= 4 {
                let (c, name) = (cols[0].trim(), cols[3].trim());
                if !c.is_empty() && !name.is_empty() {
                    m.insert(c.to_string(), name.to_string());
                }
            }
        }
        m
    });
    if let Some(name) = t5.get(code) {
        return Some(name.clone());
    }
    // Legacy → T5 overrides (reference `s_legacyAllegianceToT5Overrides`).
    let override_t5 = match code {
        "J-" | "Jp" | "Ju" => Some("JuPr"),
        "Na" => Some("NaHu"),
        "So" => Some("SoCf"),
        "Va" => Some("NaVa"),
        "Zh" => Some("ZhCo"),
        "??" | "--" => Some("XXXX"),
        _ => None,
    };
    if let Some(name) = override_t5.and_then(|t| t5.get(t)) {
        return Some(name.clone());
    }
    // Hardcoded legacy stock — major polities with no generic T5 code
    // (reference `s_legacyAllegiances`).
    if let Some(name) = match code {
        "As" => Some("Aslan Hierate"),
        "Dr" => Some("Droyne"),
        "Im" => Some("Third Imperium"),
        "Kk" => Some("The Two Thousand Worlds"),
        _ => None,
    } {
        return Some(name.to_string());
    }
    // Legacy-column (col 1) → name, first occurrence wins (matches the
    // reference `GroupBy(LegacyCode).First()`).
    let legacy = LEGACY.get_or_init(|| {
        let mut m = HashMap::new();
        for line in raw.lines().skip(1) {
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() >= 4 {
                let (lc, name) = (cols[1].trim(), cols[3].trim());
                if !lc.is_empty() && !name.is_empty() {
                    m.entry(lc.to_string()).or_insert_with(|| name.to_string());
                }
            }
        }
        m
    });
    legacy.get(code).cloned()
}

// ── Decoded output types ─────────────────────────────────────────────────────

/// A decoded glyph: the raw character/code plus its human-readable blurb.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    pub code: String,
    pub blurb: String,
}

/// Fully decoded UWP (`StSAHPGL-T`), each field with its blurb.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedUwp {
    pub starport: Decoded,
    pub size: Decoded,
    pub atmosphere: Decoded,
    pub hydrographics: Decoded,
    pub population: Decoded,
    pub government: Decoded,
    pub law: Decoded,
    pub tech: Decoded,
}

/// Population/Belts/Gas-giants. `belts`/`gas_giants` are `None` for the eHex
/// sentinel (the reference renders these as `???`).
#[derive(Debug, Clone, PartialEq)]
pub struct Pbg {
    pub pop_mult: i32,
    pub belts: Option<i32>,
    pub gas_giants: Option<i32>,
}

/// Economic extension `(Ex)` split into Resources/Labor/Infrastructure/Efficiency.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedEx {
    pub resources: Decoded,
    pub labor: Decoded,
    pub infrastructure: Decoded,
    /// Efficiency, with `-` shown as U+2212 MINUS SIGN.
    pub efficiency: Decoded,
}

/// Cultural extension `[Cx]` split into the four facets.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedCx {
    pub heterogeneity: Decoded,
    pub acceptance: Decoded,
    pub strangeness: Decoded,
    pub symbols: Decoded,
}

/// Travel zone, mirroring world_util.js's `world.Zone` object.
#[derive(Debug, Clone, PartialEq)]
pub struct Zone {
    pub rule: &'static str,
    pub rating: &'static str,
    pub class_name: &'static str,
}

/// Importance extension `{Ix}`: the numeric value (display form, U+2212 minus)
/// plus its blurb.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedIx {
    pub imp: String,
    pub blurb: Option<String>,
}

/// The complete decoded world sheet.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedWorld {
    pub is_placeholder: bool,
    pub uwp: DecodedUwp,
    pub pbg: Pbg,
    pub total_population: Option<String>,
    pub importance: Option<DecodedIx>,
    pub economics: Option<DecodedEx>,
    pub culture: Option<DecodedCx>,
    pub nobility: Vec<Decoded>,
    pub remarks: Vec<Decoded>,
    pub bases: Vec<String>,
    pub stars: Vec<Decoded>,
    pub zone: Zone,
    pub worlds: Option<u32>,
    pub other_worlds: Option<u32>,
    pub allegiance_name: Option<String>,
}

/// U+2212 MINUS SIGN — the reference swaps ASCII `-` for this in Ix/Eff display.
const UNICODE_MINUS: char = '\u{2212}';

// ── Field decoders ───────────────────────────────────────────────────────────

/// Split a UWP string `StSAHPGL-T` into its eight fields with blurbs. Mirrors
/// `splitUWP`: tech is taken from index 8 (skipping the `-` separator at 7).
pub fn split_uwp(uwp: &str) -> DecodedUwp {
    let ch = |i: usize| uwp.chars().nth(i).unwrap_or('?');
    let dec = |c: char, f: fn(char) -> Option<&'static str>| Decoded {
        code: c.to_string(),
        blurb: f(c).unwrap_or("").to_string(),
    };
    DecodedUwp {
        starport: dec(ch(0), starport_blurb),
        size: dec(ch(1), siz_blurb),
        atmosphere: dec(ch(2), atm_blurb),
        hydrographics: dec(ch(3), hyd_blurb),
        population: dec(ch(4), pop_blurb),
        government: dec(ch(5), gov_blurb),
        law: dec(ch(6), law_blurb),
        tech: dec(ch(8), tech_blurb),
    }
}

/// Split a 3-digit PBG (`splitPBG`): Pop mult, Belts, Gas-giants; eHex `-1` → None.
pub fn split_pbg(pbg: &str) -> Pbg {
    let digit = |i: usize| pbg.chars().nth(i).map(from_hex).unwrap_or(-1);
    let fix = |v: i32| if v == -1 { None } else { Some(v) };
    Pbg {
        pop_mult: digit(0).max(0),
        belts: fix(digit(1)),
        gas_giants: fix(digit(2)),
    }
}

/// Tokenize a raw remarks string into individual codes (`splitRemarks`):
/// parenthesised/bracketed sophont groups (optionally `Di`-prefixed or
/// population-suffixed), `{comments}`, and plain whitespace tokens.
pub fn split_remarks(remarks: &str) -> Vec<String> {
    use regex::Regex;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(Di)?\([^)]*\)[0-9?]?|\[[^\]]*\][0-9?]?|\{[^}]*\}|\S+").unwrap()
    });
    re.find_iter(remarks).map(|m| m.as_str().to_string()).collect()
}

/// Decode one remark token to its detail string (`REMARKS_TABLE` then
/// `REMARKS_PATTERNS`, in order). Empty string for `{comments}`; `???` if unknown.
fn decode_remark(code: &str) -> String {
    use regex::Regex;
    if let Some(d) = remarks_table(code) {
        return d.to_string();
    }
    static RES: OnceLock<Vec<Regex>> = OnceLock::new();
    let res = RES.get_or_init(|| {
        [
            r"^Rs\w$",
            r"^Rw:?\w$",
            r"^O:\d\d\d\d$",
            r"^O:\d\d\d\d-\w+$",
            r"^O:\w\w$",
            r"^Mr:\d\d\d\d$",
            r"^\[.*\]\??$",
            r"^\(.*\)\??$",
            r"^\(.*\)(\d)$",
            r"^Di\(.*\)$",
            r"^([A-Z][A-Za-z']{3})([0-9W?])$",
            r"^([ACDFHIMVXZ])([0-9w])$",
            r"^\{.*\}$",
        ]
        .iter()
        .map(|p| Regex::new(p).unwrap())
        .collect()
    });
    for (i, re) in res.iter().enumerate() {
        let Some(caps) = re.captures(code) else { continue };
        return match i {
            0 => "Research Station".to_string(),
            1 => "Refugee World".to_string(),
            2 | 3 | 4 => "Controlled".to_string(),
            5 => "Military rule".to_string(),
            6 | 7 => "Homeworld".to_string(),
            8 => format!("Homeworld, Population {}0%", &caps[1]),
            9 => "Homeworld, Extinct".to_string(),
            10 | 11 => decode_sophont_population(&caps[1], &caps[2]),
            12 => String::new(),
            _ => "???".to_string(),
        };
    }
    "???".to_string()
}

/// `decodeSophontPopulation`: sophont name + population band from the suffix.
fn decode_sophont_population(code: &str, pop: &str) -> String {
    let name = sophont_name(code).unwrap_or_else(|| "Sophont".to_string());
    let pop = match pop {
        "0" => "< 10%".to_string(),
        "W" | "w" => "100%".to_string(),
        "?" => "Unknown".to_string(),
        other => format!("{other}0%"),
    };
    format!("{name}, Population {pop}")
}

/// Insert thousands separators into a non-negative integer (`numberWithCommas`).
fn number_with_commas(n: u128) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

/// Parse the `{Ix}` value, stripping the braces (`parseIx`). Returns the integer
/// importance, or `None` if it isn't a number.
fn parse_ix(ix: &str) -> Option<i64> {
    ix.trim().trim_start_matches('{').trim_end_matches('}').trim().parse::<i64>().ok()
}

/// Decode stellar data into a list of `(code, detail)` stars (`world.Stars`).
fn decode_stars(stellar: &str) -> Vec<Decoded> {
    use regex::Regex;
    static COLLAPSE: OnceLock<Regex> = OnceLock::new();
    static COLOR: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    static OVERRIDE_MS: OnceLock<Regex> = OnceLock::new();
    // Collapse "<spectral><digit> D" → "D" (white-dwarf shorthand).
    let collapse = COLLAPSE.get_or_init(|| Regex::new(r"[OBAFGKM][0-9] D").unwrap());
    let colors = COLOR.get_or_init(|| {
        vec![
            ("Blue", Regex::new(r"^[OB][0-9] ").unwrap()),
            ("White", Regex::new(r"^A[0-9] ").unwrap()),
            ("Yellow-White", Regex::new(r"^F[0-9] ").unwrap()),
            ("Yellow", Regex::new(r"^G[0-9] ").unwrap()),
            ("Orange", Regex::new(r"^K[0-9] ").unwrap()),
            ("Red", Regex::new(r"^M[0-9] ").unwrap()),
        ]
    });
    let override_ms = OVERRIDE_MS.get_or_init(|| Regex::new(r"^[OBA][0-9] V$").unwrap());

    let collapsed = collapse.replace_all(stellar, "D");
    const LUM: &[&str] = &["Ia", "Ib", "II", "III", "IV", "V", "VI", "VII"];
    // Group tokens into stars: a luminosity-class token attaches to the current
    // star; anything else starts a new one (mirrors the JS lookahead split).
    let mut stars: Vec<String> = Vec::new();
    for tok in collapsed.split_whitespace() {
        if LUM.contains(&tok) && !stars.is_empty() {
            let last = stars.last_mut().unwrap();
            last.push(' ');
            last.push_str(tok);
        } else {
            stars.push(tok.to_string());
        }
    }
    stars
        .into_iter()
        .map(|code| {
            let last = code.split_whitespace().last().unwrap_or("");
            let mut detail = stellar_blurb(last).unwrap_or("").to_string();
            // Avoid "blue/white dwarf" for O/B/A main-sequence stars.
            if override_ms.is_match(&code) {
                detail = "Main Sequence".to_string();
            }
            // Prepend the color for ordinary stars.
            for (name, re) in colors {
                if re.is_match(&code) {
                    detail = format!("{name} {detail}");
                }
            }
            Decoded { code, blurb: detail }
        })
        .collect()
}

/// Travel-zone decode (`world.Zone`).
fn decode_zone(zone: &str) -> Zone {
    match zone {
        "A" => Zone { rule: "Caution", rating: "Amber", class_name: "amber" },
        "R" => Zone { rule: "Restricted", rating: "Red", class_name: "red" },
        "B" => Zone {
            rule: "Technologically Elevated Dictatorship",
            rating: "c/o Coalition Data Services",
            class_name: "ted",
        },
        "F" => Zone {
            rule: "Forbidden",
            rating: "c/o Consulate Data Services",
            class_name: "forbidden",
        },
        "U" => Zone {
            rule: "Unabsorbed",
            rating: "c/o Consulate Data Services",
            class_name: "unabsorbed",
        },
        _ => Zone { rule: "No Restrictions", rating: "Green", class_name: "green" },
    }
}

/// Bases decode (`world.Bases`), incl. the Zhodani `KM`/`W` special case and the
/// reserve/exile/research remarks the reference also appends to the base list.
fn decode_bases(bases: &str, allegiance: &str, remarks: &[Decoded]) -> Vec<String> {
    let mut out: Vec<String> = if allegiance.starts_with("Zh") && bases == "KM" {
        vec!["Zhodani Base".to_string()]
    } else if allegiance.starts_with("Zh") && bases == "W" {
        vec!["Zhodani Relay Station".to_string()]
    } else {
        bases.chars().filter_map(|c| base_blurb(c).map(str::to_string)).collect()
    };
    for r in remarks {
        if matches!(r.code.as_str(), "Re" | "Px" | "Ex") || r.code.starts_with("Rs") {
            out.push(r.blurb.clone());
        }
    }
    out
}

/// Decode every field of a [`World`] into a [`DecodedWorld`] for the data sheet.
pub fn decode_world(world: &World) -> DecodedWorld {
    let is_placeholder = world.uwp == "XXXXXXX-X" || world.uwp == "???????-?";
    let uwp = split_uwp(&world.uwp);
    let pbg = split_pbg(&world.pbg);

    // Total population = PopMult · 10^PopExp (with the PopExp>0 & PopMult==0 → 1
    // adjustment), formatted with thousands separators.
    let pop_exp = from_hex(world.uwp.chars().nth(4).unwrap_or('?'));
    let pop_mult = if pop_exp > 0 && pbg.pop_mult == 0 { 1 } else { pbg.pop_mult };
    let total_population = if pop_exp >= 0 && pop_mult >= 0 {
        let total = (pop_mult as u128).saturating_mul(10u128.saturating_pow(pop_exp as u32));
        Some(number_with_commas(total))
    } else {
        None
    };

    let importance = world.importance.as_deref().and_then(parse_ix).map(|ix| {
        let imp = ix.to_string();
        let blurb = ix_imp_blurb(&imp).map(str::to_string);
        DecodedIx { imp: imp.replace('-', &UNICODE_MINUS.to_string()), blurb }
    });

    let economics = world.economic.as_deref().map(|ex| {
        let inner = ex.trim().trim_start_matches('(').trim_end_matches(')').trim();
        let ch = |i: usize| inner.chars().nth(i).unwrap_or('?');
        let eff: String = inner.chars().skip(3).collect();
        DecodedEx {
            resources: Decoded {
                code: ch(0).to_string(),
                blurb: ex_resources_blurb(ch(0)).unwrap_or("").to_string(),
            },
            labor: Decoded {
                code: ch(1).to_string(),
                blurb: pop_blurb(ch(1)).unwrap_or("").to_string(), // EX_LABOR_TABLE = POP_TABLE
            },
            infrastructure: Decoded {
                code: ch(2).to_string(),
                blurb: ex_infrastructure_blurb(ch(2)).unwrap_or("").to_string(),
            },
            efficiency: Decoded {
                code: eff.replace('-', &UNICODE_MINUS.to_string()),
                blurb: ex_efficiency_blurb(&eff).unwrap_or("").to_string(),
            },
        }
    });

    let culture = world.cultural.as_deref().map(|cx| {
        let inner = cx.trim().trim_start_matches('[').trim_end_matches(']').trim();
        let ch = |i: usize| inner.chars().nth(i).unwrap_or('?');
        DecodedCx {
            heterogeneity: Decoded {
                code: ch(0).to_string(),
                blurb: cx_heterogeneity_blurb(ch(0)).unwrap_or("").to_string(),
            },
            acceptance: Decoded {
                code: ch(1).to_string(),
                blurb: cx_acceptance_blurb(ch(1)).unwrap_or("").to_string(),
            },
            strangeness: Decoded {
                code: ch(2).to_string(),
                blurb: cx_strangeness_blurb(ch(2)).unwrap_or("").to_string(),
            },
            symbols: Decoded {
                code: ch(3).to_string(),
                blurb: cx_symbols_blurb(ch(3)).unwrap_or("").to_string(),
            },
        }
    });

    let nobility = world
        .nobility
        .as_deref()
        .map(|n| {
            n.chars()
                .map(|c| Decoded { code: c.to_string(), blurb: nobility_blurb(c).to_string() })
                .collect()
        })
        .unwrap_or_default();

    let remarks: Vec<Decoded> = split_remarks(&world.remarks)
        .into_iter()
        .map(|code| {
            let blurb = decode_remark(&code);
            Decoded { code, blurb }
        })
        .collect();

    let bases = decode_bases(&world.bases, &world.allegiance, &remarks);
    let stars = decode_stars(&world.stellar);
    let zone = decode_zone(&world.zone);

    let worlds = world.worlds.map(u32::from);
    let other_worlds = match (worlds, pbg.belts, pbg.gas_giants) {
        (Some(w), Some(b), Some(g)) => {
            Some((w as i64 - 1 - b as i64 - g as i64).max(0) as u32)
        }
        _ => None,
    };

    let allegiance_name = if world.allegiance.is_empty() {
        None
    } else {
        allegiance_name(&world.allegiance)
    };

    DecodedWorld {
        is_placeholder,
        uwp,
        pbg,
        total_population,
        importance,
        economics,
        culture,
        nobility,
        remarks,
        bases,
        stars,
        zone,
        worlds,
        other_worlds,
        allegiance_name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world(uwp: &str) -> World {
        World {
            hex: "0101".into(),
            name: "Test".into(),
            uwp: uwp.into(),
            ..Default::default()
        }
    }

    #[test]
    fn from_hex_extended() {
        assert_eq!(from_hex('0'), 0);
        assert_eq!(from_hex('9'), 9);
        assert_eq!(from_hex('A'), 10);
        assert_eq!(from_hex('G'), 16); // I and O skipped
        assert_eq!(from_hex('W'), 30);
        assert_eq!(from_hex('-'), -1);
    }

    #[test]
    fn regina_uwp() {
        // Regina A788899-C
        let u = split_uwp("A788899-C");
        assert_eq!(u.starport.blurb, "Excellent");
        assert_eq!(u.size.code, "7");
        assert_eq!(u.atmosphere.blurb, "Dense");
        assert_eq!(u.hydrographics.blurb, "80%");
        assert_eq!(u.population.blurb, "Hundreds of millions");
        assert_eq!(u.government.blurb, "Impersonal Bureaucracy");
        assert_eq!(u.law.blurb, "Possession of weapons outside the home prohibited");
        assert_eq!(u.tech.code, "C");
        assert_eq!(u.tech.blurb, "Average Imperial");
    }

    #[test]
    fn pbg_and_population() {
        // Regina PBG 703, UWP pop digit 8 → 7 · 10^8 = 700,000,000
        let mut w = world("A788899-C");
        w.pbg = "703".into();
        let d = decode_world(&w);
        assert_eq!(d.pbg.pop_mult, 7);
        assert_eq!(d.pbg.belts, Some(0));
        assert_eq!(d.pbg.gas_giants, Some(3));
        assert_eq!(d.total_population.as_deref(), Some("700,000,000"));
    }

    #[test]
    fn population_mult_zero_bumped_to_one() {
        // Pop exponent > 0 but PBG pop mult 0 → treated as 1.
        let mut w = world("A100500-A"); // pop digit 5 → 10^5
        w.pbg = "003".into();
        let d = decode_world(&w);
        assert_eq!(d.total_population.as_deref(), Some("100,000")); // 1·10^5
    }

    #[test]
    fn placeholder_detected() {
        assert!(decode_world(&world("XXXXXXX-X")).is_placeholder);
        assert!(decode_world(&world("???????-?")).is_placeholder);
        assert!(!decode_world(&world("A788899-C")).is_placeholder);
    }

    #[test]
    fn allegiance_full_name() {
        assert_eq!(allegiance_name("ImDd").as_deref(), Some("Third Imperium, Domain of Deneb"));
        assert_eq!(allegiance_name("ZhCo").as_deref(), Some("Zhodani Consulate"));
        assert_eq!(allegiance_name("ZZZZ"), None);
        // Base/legacy codes used by sector borders must resolve too.
        assert_eq!(allegiance_name("As").as_deref(), Some("Aslan Hierate")); // hardcoded stock
        assert_eq!(allegiance_name("Im").as_deref(), Some("Third Imperium"));
        assert_eq!(allegiance_name("So").as_deref(), Some("Solomani Confederation")); // legacy override
        assert_eq!(allegiance_name("Zh").as_deref(), Some("Zhodani Consulate"));
    }

    #[test]
    fn ix_decode() {
        let mut w = world("A788899-C");
        w.importance = Some("{ 4 }".into());
        let d = decode_world(&w).importance.unwrap();
        assert_eq!(d.imp, "4");
        assert_eq!(d.blurb.as_deref(), Some("Important"));

        w.importance = Some("{ -1 }".into());
        let d = decode_world(&w).importance.unwrap();
        assert_eq!(d.imp, "\u{2212}1"); // unicode minus
        assert_eq!(d.blurb.as_deref(), Some("Unimportant"));
    }

    #[test]
    fn ex_decode() {
        let mut w = world("A788899-C");
        w.economic = Some("(D7E+5)".into());
        let ex = decode_world(&w).economics.unwrap();
        assert_eq!(ex.resources.blurb, "Very abundant");
        assert_eq!(ex.labor.blurb, "Tens of millions"); // POP_TABLE[7]
        assert_eq!(ex.infrastructure.blurb, "Comprehensive"); // E
        assert_eq!(ex.efficiency.code, "+5");
        assert_eq!(ex.efficiency.blurb, "Very advanced");

        w.economic = Some("(953-2)".into());
        let ex = decode_world(&w).economics.unwrap();
        assert_eq!(ex.efficiency.code, "\u{2212}2");
        assert_eq!(ex.efficiency.blurb, "Fair");
    }

    #[test]
    fn cx_decode() {
        let mut w = world("A788899-C");
        w.cultural = Some("[7779]".into());
        let cx = decode_world(&w).culture.unwrap();
        assert_eq!(cx.heterogeneity.blurb, "Discordant");
        assert_eq!(cx.acceptance.blurb, "Aloof");
        assert_eq!(cx.strangeness.blurb, "Confusing");
        assert_eq!(cx.symbols.blurb, "Somewhat abstract");
    }

    #[test]
    fn remarks_table_and_sophont() {
        let mut w = world("A788899-C");
        w.remarks = "Hi In (Aslan)4".into();
        let d = decode_world(&w);
        let codes: Vec<&str> = d.remarks.iter().map(|r| r.code.as_str()).collect();
        assert_eq!(codes, vec!["Hi", "In", "(Aslan)4"]);
        assert_eq!(d.remarks[0].blurb, "High Population");
        assert_eq!(d.remarks[1].blurb, "Industrialized");
        assert_eq!(d.remarks[2].blurb, "Homeworld, Population 40%");
    }

    #[test]
    fn remarks_sophont_code() {
        // 4-char T5SS sophont code + population digit.
        let mut w = world("A788899-C");
        w.remarks = "Asla4".into();
        let d = decode_world(&w);
        assert_eq!(d.remarks[0].blurb, "Aslan, Population 40%");
    }

    #[test]
    fn nobility_decode() {
        let mut w = world("A788899-C");
        w.nobility = Some("BcC".into());
        let n = decode_world(&w).nobility;
        assert_eq!(n[0].blurb, "Knight");
        assert_eq!(n[1].blurb, "Baronet");
        assert_eq!(n[2].blurb, "Baron");
    }

    #[test]
    fn bases_decode_and_zhodani() {
        let mut w = world("A788899-C");
        w.bases = "NS".into();
        assert_eq!(decode_world(&w).bases, vec!["Naval Base", "Scout Base"]);

        w.bases = "KM".into();
        w.allegiance = "ZhCo".into();
        assert_eq!(decode_world(&w).bases, vec!["Zhodani Base"]);
    }

    #[test]
    fn stars_decode() {
        let mut w = world("A788899-C");
        w.stellar = "K9 V M2 V".into();
        let s = decode_world(&w).stars;
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].code, "K9 V");
        assert_eq!(s[0].blurb, "Orange Dwarf (Main Sequence)");
        assert_eq!(s[1].code, "M2 V");
        assert_eq!(s[1].blurb, "Red Dwarf (Main Sequence)");

        // O/B/A main sequence → "Main Sequence" override, with color.
        w.stellar = "A0 V".into();
        let s = decode_world(&w).stars;
        assert_eq!(s[0].blurb, "White Main Sequence");
    }

    #[test]
    fn zone_decode() {
        let mut w = world("A788899-C");
        w.zone = "R".into();
        assert_eq!(decode_world(&w).zone.rating, "Red");
        w.zone = "A".into();
        assert_eq!(decode_world(&w).zone.rating, "Amber");
        w.zone = "".into();
        assert_eq!(decode_world(&w).zone.rating, "Green");
    }

    #[test]
    fn other_worlds_arithmetic() {
        let mut w = world("A788899-C");
        w.pbg = "703".into(); // belts 0, GG 3
        w.worlds = Some(8);
        let d = decode_world(&w);
        // 8 - 1 - 0 - 3 = 4
        assert_eq!(d.other_worlds, Some(4));
    }
}
