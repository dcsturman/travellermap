//! The travellermap.com search query language — parser, SQL-Server `LIKE`
//! matcher, and `SOUNDEX`.
//!
//! Pure logic (no I/O), ported faithfully from the reference
//! `server/search/SearchEngine.cs` (`ParseQuery`, `RE_TERMS`, the per-term
//! clause table) plus the handler-level preprocessing in
//! `server/api/SearchHandler.cs`. The backend builds an in-memory index of
//! [`SearchRecord`]s and evaluates a [`ParsedQuery`]'s [`Clause`]s against each.
//!
//! The reference pushes clauses down to SQL Server (`name LIKE @term`,
//! `SOUNDEX(name) = SOUNDEX(@term)`, …); here the same clauses run in Rust over
//! the in-memory records. All comparisons are case-insensitive — everything is
//! lowercased up front (the reference lowercases the whole query in `ParseQuery`
//! and stores lowercased index columns).

/// Which item kinds a query may return (port of `SearchResultsType`). A query
/// term can *narrow* this (e.g. any field op restricts to worlds; the bare word
/// `sector` restricts to sectors).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchTypes {
    pub sectors: bool,
    pub subsectors: bool,
    pub worlds: bool,
    pub labels: bool,
}

impl SearchTypes {
    pub const NONE: SearchTypes =
        SearchTypes { sectors: false, subsectors: false, worlds: false, labels: false };
    pub const DEFAULT: SearchTypes =
        SearchTypes { sectors: true, subsectors: true, worlds: true, labels: true };
    pub const WORLDS: SearchTypes =
        SearchTypes { sectors: false, subsectors: false, worlds: true, labels: false };
    pub const SECTORS: SearchTypes =
        SearchTypes { sectors: true, subsectors: false, worlds: false, labels: false };
    pub const SUBSECTORS: SearchTypes =
        SearchTypes { sectors: false, subsectors: true, worlds: false, labels: false };
}

/// The fields a single index record exposes to the clause matcher. Mirrors the
/// reference `worlds` table columns (the only table with per-field ops); the
/// name-only tables (sectors/subsectors/labels) leave the world fields empty.
///
/// **All strings must already be lowercased** by the index builder (the
/// reference stores lowercased columns / lowercases the query, so `LIKE` is
/// effectively case-insensitive).
pub struct SearchRecord<'a> {
    pub name: &'a str,
    pub uwp: &'a str,
    pub pbg: &'a str,
    pub zone: &'a str,
    pub alleg: &'a str,
    /// Space-delimited stellar tokens, e.g. `"m9 v"`.
    pub stellar: &'a str,
    /// Space-delimited remark tokens, e.g. `"hi in cp"`.
    pub remarks: &'a str,
    pub ex: &'a str,
    pub cx: &'a str,
    /// Importance `{Ix}` integer value, or `None` when absent.
    pub ix: Option<i32>,
    pub sector_name: &'a str,
}

/// One parsed clause. Each is an AND term (the reference joins clauses with
/// `AND`); a record matches the query iff it matches every clause.
#[derive(Debug, Clone, PartialEq)]
pub enum Clause {
    /// `name LIKE term + '%' OR name LIKE '% ' + term + '%'` — the default
    /// word-boundary match (start of name, or after a space).
    NameWordBoundary(String),
    /// `name LIKE term` — full-string LIKE (exact:, quoted, or a term with a
    /// wildcard char).
    NameLike(String),
    /// `SOUNDEX(name) = SOUNDEX(term)` — the `like:` "sounds like" match.
    NameSoundex(String),
    /// `uwp LIKE term`.
    Uwp(String),
    /// `pbg LIKE term`.
    Pbg(String),
    /// `zone LIKE term`.
    Zone(String),
    /// `alleg LIKE term`.
    Alleg(String),
    /// `ex LIKE term`.
    Ex(String),
    /// `cx LIKE term`.
    Cx(String),
    /// `ix = term` (integer compare).
    Ix(i32),
    /// stellar token match: `(' '+stellar+' ') LIKE ('% '+term+' %')`.
    Stellar(String),
    /// remark token match: `(' '+remarks+' ') LIKE ('% '+term+' %')`.
    Remark(String),
    /// `sector_name LIKE '%' + term + '%'` (the `in:` op).
    InSector(String),
}

impl Clause {
    /// Does `record` satisfy this clause?
    pub fn matches(&self, record: &SearchRecord) -> bool {
        match self {
            Clause::NameWordBoundary(t) => name_word_boundary(record.name, t),
            Clause::NameLike(t) => like_match(t, record.name),
            Clause::NameSoundex(t) => soundex(record.name) == soundex(t),
            Clause::Uwp(t) => like_match(t, record.uwp),
            Clause::Pbg(t) => like_match(t, record.pbg),
            Clause::Zone(t) => like_match(t, record.zone),
            Clause::Alleg(t) => like_match(t, record.alleg),
            Clause::Ex(t) => like_match(t, record.ex),
            Clause::Cx(t) => like_match(t, record.cx),
            Clause::Ix(n) => record.ix == Some(*n),
            Clause::Stellar(t) => token_like(record.stellar, t),
            Clause::Remark(t) => token_like(record.remarks, t),
            Clause::InSector(t) => like_match(&format!("%{t}%"), record.sector_name),
        }
    }
}

/// `(' ' + field + ' ') LIKE ('% ' + term + ' %')`: the term, treated as a LIKE
/// pattern, must match a whole space-delimited token of `field`.
fn token_like(field: &str, term: &str) -> bool {
    like_match(&format!("% {term} %"), &format!(" {field} "))
}

/// The default word-boundary name rule (`name LIKE term+'%' OR name LIKE '%
/// '+term+'%'`). Here `term` is a literal (terms with wildcards route to
/// [`Clause::NameLike`] instead), so the two LIKEs reduce to "starts with" or
/// "contains after a space".
fn name_word_boundary(name: &str, term: &str) -> bool {
    like_match(&format!("{term}%"), name) || like_match(&format!("% {term}%"), name)
}

/// A fully parsed query: the kinds to return, plus either a list of AND clauses
/// or the sector+hex shortcut. Empty `clauses` with `!sector_hex` means "no
/// match" (the reference returns no results when no clauses were produced).
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedQuery {
    pub types: SearchTypes,
    pub clauses: Vec<Clause>,
    /// Set when the query was the `Sector Name 0101` shortcut — restricts to
    /// worlds in the named sector at the given local hex.
    pub sector_hex: Option<SectorHex>,
}

/// The `^(?<sector>[A-Za-z0-9!' ]{3,}) (?<hex>\d{4})$` shortcut: worlds whose
/// `sector_name` starts with `sector` and whose local hex is `(hex_x, hex_y)`.
#[derive(Debug, Clone, PartialEq)]
pub struct SectorHex {
    pub sector_prefix: String,
    pub hex_x: i32,
    pub hex_y: i32,
}

/// The recognized op prefixes (port of `SearchEngine.OPS`, longest-first per
/// item so `ex:`/`cx:`/`ix:` don't shadow each other — they don't overlap, but
/// we test full equality of the prefix anyway).
const OPS: &[&str] = &[
    "uwp:", "pbg:", "zone:", "alleg:", "stellar:", "remark:", "exact:", "like:", "in:", "ix:",
    "ex:", "cx:",
];

/// Split a query into `(op, raw_term)` pairs, porting `RE_TERMS`:
/// `(op-prefix)?("[^"]+"|\S+)`. We scan token-by-token rather than with a regex
/// engine: skip whitespace, optionally consume a leading op prefix, then consume
/// either a `"…"` quoted run (to the next `"`, or to end-of-string — the
/// reference's `("[^"]+")` plus its trailing-`"` inference) or a non-space run.
fn parse_terms(q: &str) -> Vec<(Option<&'static str>, String)> {
    let bytes = q.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < bytes.len() {
        // Skip whitespace.
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // Optional op prefix.
        let mut op: Option<&'static str> = None;
        for &candidate in OPS {
            if q[i..].starts_with(candidate) {
                op = Some(candidate);
                i += candidate.len();
                break;
            }
        }

        // The term: a quoted run or a non-space run. `RE_TERMS` requires at
        // least one char inside quotes (`"[^"]+"`); an empty `""` falls through
        // to the `\S+` branch and is consumed as the literal token `""`.
        let term: String = if i < bytes.len() && bytes[i] == b'"' && q[i..].starts_with("\"") {
            // Find the closing quote (if any) after the opening one.
            let rest = &q[i + 1..];
            match rest.find('"') {
                Some(close) if close > 0 => {
                    // "abc" — keep the quotes; ParseQuery strips/inspects them.
                    let token = &q[i..i + 1 + close + 1];
                    i += 1 + close + 1;
                    token.to_string()
                }
                _ => {
                    // `"` with no (or immediate) closing quote: the reference's
                    // `\S+` branch consumes the run, then ParseQuery infers a
                    // trailing `"`. Consume the non-space run.
                    let start = i;
                    while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    q[start..i].to_string()
                }
            }
        } else {
            let start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            q[start..i].to_string()
        };

        if op.is_none() && term.is_empty() {
            continue;
        }
        out.push((op, term));
    }
    out
}

/// Parse a query into clauses + return types, porting `SearchEngine.ParseQuery`.
/// `query` is the handler-preprocessed string (already `*`→`%`, `?`→`_`, and
/// `uwp:`-prefixed for the UWP shortcut); we lowercase it here (the reference
/// lowercases inside `ParseQuery`). `start_types` is the caller's requested set
/// (from the `types=` param, default [`SearchTypes::DEFAULT`]).
pub fn parse_query(query: &str, start_types: SearchTypes) -> ParsedQuery {
    let mut types = start_types;
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return ParsedQuery { types, clauses: Vec::new(), sector_hex: None };
    }

    // Sector+hex shortcut: `^[A-Za-z0-9!' ]{3,} \d{4}$`.
    if let Some(sh) = parse_sector_hex(&query) {
        return ParsedQuery { types: SearchTypes::WORLDS, clauses: Vec::new(), sector_hex: Some(sh) };
    }

    let mut clauses = Vec::new();
    for (op, raw) in parse_terms(&query) {
        let mut term = raw;
        let mut quoted = false;

        // Infer a trailing `"` (reference: starts with `"` but doesn't end with
        // one, or is a lone `"`).
        if term.starts_with('"') && (!term.ends_with('"') || term.len() == 1) {
            term.push('"');
        }
        // Strip surrounding quotes.
        if term.len() >= 2 && term.starts_with('"') && term.ends_with('"') {
            quoted = true;
            term = term[1..term.len() - 1].to_string();
        }
        if term.is_empty() {
            continue;
        }

        let clause = match op {
            Some("uwp:") => {
                types = SearchTypes::WORLDS;
                Clause::Uwp(term)
            }
            Some("pbg:") => {
                types = SearchTypes::WORLDS;
                Clause::Pbg(term)
            }
            Some("ix:") => {
                types = SearchTypes::WORLDS;
                // Non-integer `ix:` value matches nothing (SQL `ix = @term`
                // with a non-numeric param errors → no rows). Use a sentinel
                // that no real importance equals.
                match term.parse::<i32>() {
                    Ok(n) => Clause::Ix(n),
                    Err(_) => Clause::Ix(i32::MIN),
                }
            }
            Some("ex:") => {
                types = SearchTypes::WORLDS;
                Clause::Ex(term)
            }
            Some("cx:") => {
                types = SearchTypes::WORLDS;
                Clause::Cx(term)
            }
            Some("zone:") => {
                types = SearchTypes::WORLDS;
                Clause::Zone(term)
            }
            Some("alleg:") => {
                types = SearchTypes::WORLDS;
                Clause::Alleg(term)
            }
            Some("stellar:") => {
                types = SearchTypes::WORLDS;
                Clause::Stellar(term)
            }
            Some("remark:") => {
                types = SearchTypes::WORLDS;
                Clause::Remark(term)
            }
            Some("in:") => {
                types = SearchTypes::WORLDS;
                Clause::InSector(term)
            }
            Some("exact:") => Clause::NameLike(term),
            Some("like:") => Clause::NameSoundex(term),
            _ if quoted => Clause::NameLike(term),
            _ if term.contains('%') || term.contains('_') => Clause::NameLike(term),
            _ if term == "sector" => {
                types = SearchTypes::SECTORS;
                continue;
            }
            _ if term == "subsector" => {
                types = SearchTypes::SUBSECTORS;
                continue;
            }
            _ if term == "world" => {
                types = SearchTypes::WORLDS;
                continue;
            }
            _ => Clause::NameWordBoundary(term),
        };
        clauses.push(clause);
    }

    ParsedQuery { types, clauses, sector_hex: None }
}

/// Port of `SECTOR_HEX_REGEX` (`^(?<sector>[A-Za-z0-9!' ]{3,}) (?<hex>\d{4})$`).
/// The last space-separated token must be exactly 4 digits; everything before
/// it (≥3 chars, from the allowed class) is the sector prefix.
fn parse_sector_hex(query: &str) -> Option<SectorHex> {
    let last_space = query.rfind(' ')?;
    let (sector, hex) = (&query[..last_space], &query[last_space + 1..]);
    if hex.len() != 4 || !hex.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if sector.len() < 3 {
        return None;
    }
    // The sector part may itself contain spaces; every char must be in the class
    // `[A-Za-z0-9!' ]`.
    if !sector.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'!' | b'\'' | b' ')) {
        return None;
    }
    let hex_num: i32 = hex.parse().ok()?;
    Some(SectorHex { sector_prefix: sector.to_string(), hex_x: hex_num / 100, hex_y: hex_num % 100 })
}

/// SQL-Server `LIKE`: `%` = zero-or-more chars, `_` = exactly one char,
/// `[...]` = char class (ranges `a-z`, negation `[^...]`), anything else
/// literal. Anchored to the whole string (SQL `LIKE` matches the entire value).
///
/// Both `pattern` and `text` are expected pre-lowercased by the caller.
/// Backtracking is via the classic two-pointer star algorithm (linear in
/// practice for these short strings).
pub fn like_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    // Backtrack anchors: where the last `%` was, and the text position to resume.
    let mut star: Option<usize> = None;
    let mut star_ti = 0usize;

    while ti < t.len() {
        if pi < p.len() {
            match p[pi] {
                '%' => {
                    star = Some(pi);
                    star_ti = ti;
                    pi += 1;
                    continue;
                }
                '_' => {
                    pi += 1;
                    ti += 1;
                    continue;
                }
                '[' => {
                    if let Some((matched, next_pi)) = match_class(&p, pi, t[ti]) {
                        if matched {
                            pi = next_pi;
                            ti += 1;
                            continue;
                        }
                    } else {
                        // Malformed class: treat `[` as a literal.
                        if p[pi] == t[ti] {
                            pi += 1;
                            ti += 1;
                            continue;
                        }
                    }
                }
                c => {
                    if c == t[ti] {
                        pi += 1;
                        ti += 1;
                        continue;
                    }
                }
            }
        }
        // Mismatch (or pattern exhausted): backtrack to the last `%` if any.
        if let Some(s) = star {
            pi = s + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    // Consume trailing `%`s.
    while pi < p.len() && p[pi] == '%' {
        pi += 1;
    }
    pi == p.len()
}

/// Match a `[...]` character class starting at `p[start]` (`p[start] == '['`)
/// against `ch`. Returns `Some((matched, index_after_class))`, or `None` if the
/// class is malformed (no closing `]`).
fn match_class(p: &[char], start: usize, ch: char) -> Option<(bool, usize)> {
    let mut i = start + 1;
    let negate = i < p.len() && p[i] == '^';
    if negate {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    while i < p.len() {
        // A `]` as the very first class char is a literal (SQL/T-SQL semantics);
        // otherwise it closes the class.
        if p[i] == ']' && !first {
            return Some((matched != negate, i + 1));
        }
        first = false;
        // Range `a-z`: current char, `-`, and a following non-`]` char.
        if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
            let (lo, hi) = (p[i], p[i + 2]);
            if lo <= ch && ch <= hi {
                matched = true;
            }
            i += 3;
        } else {
            if p[i] == ch {
                matched = true;
            }
            i += 1;
        }
    }
    // No closing bracket.
    None
}

/// American Soundex, SQL-Server semantics (4 chars: letter + 3 digits, padded
/// with `0`). Port of T-SQL `SOUNDEX`: keep the first letter, encode subsequent
/// consonants, drop vowels/`h`/`w` as separators (but `h`/`w` between two
/// same-code consonants do NOT split them), collapse adjacent duplicate codes.
///
/// Non-letters before the first letter are skipped; once 4 chars are produced we
/// stop. An input with no letters yields `"0000"` (SQL returns an empty/NULL-ish
/// result; `"0000"` is a safe sentinel that won't collide with real codes here).
pub fn soundex(s: &str) -> String {
    fn code(c: char) -> Option<u8> {
        match c.to_ascii_lowercase() {
            'b' | 'f' | 'p' | 'v' => Some(b'1'),
            'c' | 'g' | 'j' | 'k' | 'q' | 's' | 'x' | 'z' => Some(b'2'),
            'd' | 't' => Some(b'3'),
            'l' => Some(b'4'),
            'm' | 'n' => Some(b'5'),
            'r' => Some(b'6'),
            _ => None,
        }
    }

    let letters: Vec<char> = s.chars().filter(|c| c.is_ascii_alphabetic()).collect();
    if letters.is_empty() {
        return "0000".to_string();
    }

    let mut result = String::with_capacity(4);
    result.push(letters[0].to_ascii_uppercase());

    // `last_code` tracks the code of the previous *coded* letter, used to
    // collapse duplicates. `h`/`w` are transparent (don't reset it); vowels do.
    let mut last_code = code(letters[0]);

    for &c in &letters[1..] {
        let lc = c.to_ascii_lowercase();
        match code(c) {
            Some(d) => {
                if Some(d) != last_code {
                    result.push(d as char);
                    if result.len() == 4 {
                        break;
                    }
                }
                last_code = Some(d);
            }
            None => {
                // Vowels (and y) reset the duplicate tracker so a code can repeat
                // across them; `h`/`w` are transparent (keep `last_code`).
                if lc != 'h' && lc != 'w' {
                    last_code = None;
                }
            }
        }
    }

    while result.len() < 4 {
        result.push('0');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_literal_anchored() {
        assert!(like_match("sol", "sol"));
        assert!(!like_match("sol", "solomani"));
        assert!(!like_match("sol", "so"));
    }

    #[test]
    fn like_percent() {
        assert!(like_match("r%a", "regina"));
        assert!(like_match("%", "anything"));
        assert!(like_match("a%", "a"));
        assert!(like_match("%a", "a"));
        // re%in does NOT match regina (anchored full-string, no trailing %).
        assert!(!like_match("re%in", "regina"));
        // re%in% does match regina.
        assert!(like_match("re%in%", "regina"));
        assert!(like_match("a%b%c", "axxbyyc"));
    }

    #[test]
    fn like_underscore() {
        assert!(like_match("a_c", "abc"));
        assert!(!like_match("a_c", "ac"));
        assert!(!like_match("a_c", "abbc"));
        assert!(like_match("a788899-a", "a788899-a"));
    }

    #[test]
    fn like_char_class() {
        assert!(like_match("[0-5]", "3"));
        assert!(!like_match("[0-5]", "7"));
        assert!(like_match("[m-z]", "q"));
        assert!(like_match("[89abc]", "b"));
        assert!(!like_match("[89abc]", "d"));
        assert!(like_match("[^a]", "b"));
        assert!(!like_match("[^a]", "a"));
        assert!(like_match("r[ae]gina", "regina"));
    }

    #[test]
    fn soundex_examples() {
        // SQL Server: SOUNDEX('Terra') = T600, SOUNDEX('tear') = T600.
        assert_eq!(soundex("Terra"), "T600");
        assert_eq!(soundex("tear"), "T600");
        assert_eq!(soundex("Robert"), "R163");
        assert_eq!(soundex("Rupert"), "R163");
        assert_eq!(soundex("Tymm"), "T500");
        assert_eq!(soundex("Honeyman"), "H555");
    }

    #[test]
    fn parse_default_word_boundary() {
        let pq = parse_query("sol", SearchTypes::DEFAULT);
        assert_eq!(pq.clauses, vec![Clause::NameWordBoundary("sol".into())]);
        assert!(pq.types.worlds && pq.types.sectors);
    }

    #[test]
    fn parse_wildcard_full_like() {
        // % present → NameLike (full-string), not word-boundary.
        let pq = parse_query("re%in", SearchTypes::DEFAULT);
        assert_eq!(pq.clauses, vec![Clause::NameLike("re%in".into())]);
    }

    #[test]
    fn parse_ops() {
        let pq = parse_query("uwp:a%", SearchTypes::DEFAULT);
        assert_eq!(pq.clauses, vec![Clause::Uwp("a%".into())]);
        assert_eq!(pq.types, SearchTypes::WORLDS);

        let pq = parse_query("t% in:spin", SearchTypes::DEFAULT);
        assert_eq!(
            pq.clauses,
            vec![Clause::NameLike("t%".into()), Clause::InSector("spin".into())]
        );
        assert_eq!(pq.types, SearchTypes::WORLDS);
    }

    #[test]
    fn parse_exact_and_like() {
        let pq = parse_query("exact:sol", SearchTypes::DEFAULT);
        assert_eq!(pq.clauses, vec![Clause::NameLike("sol".into())]);
        let pq = parse_query("like:tear", SearchTypes::DEFAULT);
        assert_eq!(pq.clauses, vec![Clause::NameSoundex("tear".into())]);
    }

    #[test]
    fn parse_quoted() {
        let pq = parse_query("in:\"solomani rim\"", SearchTypes::DEFAULT);
        assert_eq!(pq.clauses, vec![Clause::InSector("solomani rim".into())]);
        // Stellar with quotes + inferred trailing quote.
        let pq = parse_query("stellar:\"m? i*\"", SearchTypes::DEFAULT);
        // ? and * are NOT translated here (handler does that); but in this unit
        // test they stay literal. The op + quote handling is what we check.
        assert_eq!(pq.clauses, vec![Clause::Stellar("m? i*".into())]);
    }

    #[test]
    fn parse_multi_word_and() {
        let pq = parse_query("so ri", SearchTypes::DEFAULT);
        assert_eq!(
            pq.clauses,
            vec![Clause::NameWordBoundary("so".into()), Clause::NameWordBoundary("ri".into())]
        );
    }

    #[test]
    fn parse_type_keywords() {
        let pq = parse_query("sol sector", SearchTypes::DEFAULT);
        assert_eq!(pq.clauses, vec![Clause::NameWordBoundary("sol".into())]);
        assert_eq!(pq.types, SearchTypes::SECTORS);
    }

    #[test]
    fn parse_sector_hex_shortcut() {
        let pq = parse_query("spinward marches 1910", SearchTypes::DEFAULT);
        assert_eq!(pq.types, SearchTypes::WORLDS);
        assert_eq!(
            pq.sector_hex,
            Some(SectorHex { sector_prefix: "spinward marches".into(), hex_x: 19, hex_y: 10 })
        );
        // Too-short sector part is not a shortcut.
        assert!(parse_query("ab 1910", SearchTypes::DEFAULT).sector_hex.is_none());
        // Non-4-digit trailing token is not a shortcut.
        assert!(parse_query("spinward 19", SearchTypes::DEFAULT).sector_hex.is_none());
    }

    #[test]
    fn token_match_stellar() {
        let rec = SearchRecord {
            name: "",
            uwp: "",
            pbg: "",
            zone: "",
            alleg: "",
            stellar: "m9 v",
            remarks: "",
            ex: "",
            cx: "",
            ix: None,
            sector_name: "",
        };
        assert!(Clause::Stellar("m9".into()).matches(&rec));
        assert!(Clause::Stellar("v".into()).matches(&rec));
        // wildcard token: "m?" should not match "m9" here (literal ? in core);
        // but "m_" (underscore) would.
        assert!(Clause::Stellar("m_".into()).matches(&rec));
        assert!(!Clause::Stellar("m".into()).matches(&rec));
    }
}
