//! Unified library search/filter engine (Phase 8).
//!
//! One pure, allocation-light query engine over a flat index of every factory
//! pattern (legacy + v2), consumed by BOTH the TUI (`ui::library`) and the GUI
//! (a Tauri command) so the two frontends can never diverge on what matches or in
//! what order. The index is built once at `Library::load`; filtering is a single
//! linear scan with a precomputed lowercased haystack — no database, fast for
//! thousands of entries.
//!
//! Metadata degrades gracefully: v2 patterns populate the rich fields from their
//! envelope metadata; legacy patterns derive what they can (identity, kind, length,
//! family/function, feel-from-microtiming, mono/poly) and leave the rest `Unknown`.
//! A filter targeting an absent field simply doesn't match — it never errors.

use crate::pattern::library::PatternFunction;
use crate::pattern::model::{LaneKind, PatternData};
use crate::pattern::refs::PatternRef;
use crate::pattern::store::Favorites;

/// Timing feel bucket (filterable). Derived from the v2 `timing` template or, for
/// legacy patterns, from whether any note carries microtiming.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feel {
    Straight,
    Swing,
    Triplet,
    Unknown,
}

/// Coarse energy level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Energy {
    Low,
    Mid,
    High,
    Unknown,
}

/// Coarse density.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Density {
    Sparse,
    Core,
    Busy,
    Unknown,
}

/// Monophonic vs. chord-capable (drums are treated as N/A → `Mono`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Poly {
    Mono,
    Poly,
}

impl Feel {
    pub fn parse(s: &str) -> Feel {
        // Accept the v2 `timing` template name or a free `feel` string.
        let s = s.to_ascii_lowercase();
        if s.contains("triplet") {
            Feel::Triplet
        } else if s.contains("swing")
            || s.contains("shuffle")
            || s.contains("laid")
            || s.contains("push")
            || s.contains("human")
        {
            Feel::Swing
        } else if s.contains("straight") {
            Feel::Straight
        } else {
            Feel::Unknown
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Feel::Straight => "straight",
            Feel::Swing => "swing",
            Feel::Triplet => "triplet",
            Feel::Unknown => "unknown",
        }
    }
}

impl Energy {
    pub fn parse(s: &str) -> Energy {
        match s.to_ascii_lowercase().as_str() {
            "low" => Energy::Low,
            "mid" | "medium" => Energy::Mid,
            "high" => Energy::High,
            _ => Energy::Unknown,
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Energy::Low => "low",
            Energy::Mid => "mid",
            Energy::High => "high",
            Energy::Unknown => "unknown",
        }
    }
}

impl Density {
    pub fn parse(s: &str) -> Density {
        match s.to_ascii_lowercase().as_str() {
            "sparse" => Density::Sparse,
            "core" | "mid" => Density::Core,
            "dense" | "busy" | "high" => Density::Busy,
            _ => Density::Unknown,
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Density::Sparse => "sparse",
            Density::Core => "core",
            Density::Busy => "dense",
            Density::Unknown => "unknown",
        }
    }
}

/// A flat, filterable record for one factory pattern. Identity `(role, genre, name)`
/// matches `PatternRef::Vendored`, so favorites/audition/crate/load all still work.
#[derive(Clone, Debug)]
pub struct Record {
    pub role: crate::pattern::library::LibRole,
    pub genre: String,
    pub name: String,
    pub factory_id: Option<String>,
    pub kind: LaneKind,
    pub length: usize,
    pub family: Option<String>,
    pub function: Option<PatternFunction>,
    pub feel: Feel,
    pub poly: Poly,
    pub subgenre: Option<String>,
    pub bpm: Option<(u16, u16)>,
    pub energy: Energy,
    pub density: Density,
    pub harmonic: Option<String>,
    pub tags: Vec<String>,
    pub author: Option<String>,
    pub source: Option<String>,
    pub desc: String,
    /// Precomputed lowercased search haystack (name + desc + tags + genre + subgenre
    /// + family). Built once so per-keystroke filtering is allocation-free.
    pub haystack: String,
}

impl Record {
    /// The `PatternRef` identity of this record (always Vendored).
    pub fn pattern_ref(&self) -> PatternRef {
        PatternRef::Vendored {
            role: self.role.as_str().to_string(),
            genre: self.genre.clone(),
            name: self.name.clone(),
        }
    }

    fn build_haystack(&mut self) {
        let mut h = String::new();
        h.push_str(&self.name.to_lowercase());
        h.push(' ');
        h.push_str(&self.desc.to_lowercase());
        h.push(' ');
        h.push_str(&self.genre.to_lowercase());
        if let Some(s) = &self.subgenre {
            h.push(' ');
            h.push_str(&s.to_lowercase());
        }
        if let Some(f) = &self.family {
            h.push(' ');
            h.push_str(&f.to_lowercase());
        }
        for t in &self.tags {
            h.push(' ');
            h.push_str(&t.to_lowercase());
        }
        self.haystack = h;
    }
}

/// Poly derived from the pattern data: any melodic step with 2+ notes → Poly.
pub fn poly_of(data: &PatternData) -> Poly {
    match data {
        PatternData::Melodic(steps) if steps.iter().any(|s| s.len() >= 2) => Poly::Poly,
        _ => Poly::Mono,
    }
}

/// Feel derived from pattern data alone (legacy fallback): any nonzero micro → Swing.
pub fn feel_from_data(data: &PatternData) -> Feel {
    let has_micro = match data {
        PatternData::Drums(s) => s.iter().flatten().any(|h| h.micro != 0),
        PatternData::Melodic(s) => s.iter().flat_map(|x| x.iter()).any(|n| n.micro != 0),
    };
    if has_micro {
        Feel::Swing
    } else {
        Feel::Straight
    }
}

/// Assemble a record from already-derived parts, precomputing the haystack.
#[allow(clippy::too_many_arguments)]
pub fn make_record(
    role: crate::pattern::library::LibRole,
    genre: String,
    name: String,
    factory_id: Option<String>,
    kind: LaneKind,
    length: usize,
    family: Option<String>,
    function: Option<PatternFunction>,
    feel: Feel,
    poly: Poly,
    subgenre: Option<String>,
    bpm: Option<(u16, u16)>,
    energy: Energy,
    density: Density,
    harmonic: Option<String>,
    tags: Vec<String>,
    author: Option<String>,
    source: Option<String>,
    desc: String,
) -> Record {
    let mut r = Record {
        role,
        genre,
        name,
        factory_id,
        kind,
        length,
        family,
        function,
        feel,
        poly,
        subgenre,
        bpm,
        energy,
        density,
        harmonic,
        tags,
        author,
        source,
        desc,
        haystack: String::new(),
    };
    r.build_haystack();
    r
}

/// A library query: free-text terms (AND) plus optional exact facets.
#[derive(Clone, Debug, Default)]
pub struct Query {
    /// Lowercased search terms; every term must appear in the record's haystack.
    pub terms: Vec<String>,
    pub role: Option<crate::pattern::library::LibRole>,
    pub genre: Option<String>,
    pub function: Option<PatternFunction>,
    pub feel: Option<Feel>,
    pub energy: Option<Energy>,
    pub density: Option<Density>,
    pub poly: Option<Poly>,
    pub length: Option<(usize, usize)>,
    pub favorites_only: bool,
}

impl Query {
    /// Build the term list from a raw search string (lowercased, whitespace-split).
    pub fn with_text(mut self, text: &str) -> Query {
        self.terms = text
            .to_lowercase()
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        self
    }

    /// True when no filters or terms are set (matches everything).
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
            && self.role.is_none()
            && self.genre.is_none()
            && self.function.is_none()
            && self.feel.is_none()
            && self.energy.is_none()
            && self.density.is_none()
            && self.poly.is_none()
            && self.length.is_none()
            && !self.favorites_only
    }
}

fn function_rank(f: Option<PatternFunction>) -> u8 {
    match f {
        Some(PatternFunction::Core) => 0,
        Some(PatternFunction::VariationA) => 1,
        Some(PatternFunction::VariationB) => 2,
        Some(PatternFunction::Fill) => 3,
        Some(PatternFunction::Breakdown) => 4,
        Some(PatternFunction::Peak) => 5,
        None => 6,
    }
}

fn role_rank(r: crate::pattern::library::LibRole) -> u8 {
    match r {
        crate::pattern::library::LibRole::Drums => 0,
        crate::pattern::library::LibRole::Bass => 1,
        crate::pattern::library::LibRole::Chords => 2,
        crate::pattern::library::LibRole::Synth => 3,
    }
}

/// One record passes the query.
fn matches(r: &Record, q: &Query, favs: &Favorites) -> bool {
    if let Some(role) = q.role {
        if r.role != role {
            return false;
        }
    }
    if let Some(g) = &q.genre {
        if &r.genre != g {
            return false;
        }
    }
    if let Some(f) = q.function {
        if r.function != Some(f) {
            return false;
        }
    }
    if let Some(feel) = q.feel {
        if r.feel != feel {
            return false;
        }
    }
    if let Some(e) = q.energy {
        if r.energy != e {
            return false;
        }
    }
    if let Some(d) = q.density {
        if r.density != d {
            return false;
        }
    }
    if let Some(p) = q.poly {
        if r.poly != p {
            return false;
        }
    }
    if let Some((lo, hi)) = q.length {
        if r.length < lo || r.length > hi {
            return false;
        }
    }
    if q.favorites_only && !favs.contains(&r.pattern_ref()) {
        return false;
    }
    // Free text: every term must be a substring of the prebuilt lowercased haystack.
    for t in &q.terms {
        if !r.haystack.contains(t) {
            return false;
        }
    }
    true
}

/// Filter + deterministically sort the index. Returns indices into `index`.
///
/// Total order: role → genre → family (None last) → function (Core..Peak, None
/// last) → name. `sort_by` is stable, so equal keys keep the index's load order.
pub fn filter(index: &[Record], q: &Query, favs: &Favorites) -> Vec<usize> {
    let mut out: Vec<usize> = index
        .iter()
        .enumerate()
        .filter(|(_, r)| matches(r, q, favs))
        .map(|(i, _)| i)
        .collect();
    out.sort_by(|&a, &b| {
        let ra = &index[a];
        let rb = &index[b];
        role_rank(ra.role)
            .cmp(&role_rank(rb.role))
            .then_with(|| ra.genre.cmp(&rb.genre))
            .then_with(|| match (&ra.family, &rb.family) {
                (Some(x), Some(y)) => x.cmp(y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| function_rank(ra.function).cmp(&function_rank(rb.function)))
            .then_with(|| ra.name.cmp(&rb.name))
    });
    out
}

/// Re-locate a selection by identity after the result set changes: returns the new
/// position of `(role, genre, name)` within `results`, else clamps to the same
/// ordinal (min(prev_pos, len-1)), else `None` when empty. This gives stable,
/// non-jumping selection across filter changes for both frontends.
pub fn restable_selection(
    index: &[Record],
    results: &[usize],
    prev_identity: Option<(&str, &str)>,
    prev_pos: usize,
) -> Option<usize> {
    if results.is_empty() {
        return None;
    }
    if let Some((genre, name)) = prev_identity {
        if let Some(pos) = results
            .iter()
            .position(|&i| index[i].genre == genre && index[i].name == name)
        {
            return Some(pos);
        }
    }
    Some(prev_pos.min(results.len() - 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::library::LibRole;

    #[allow(clippy::too_many_arguments)]
    fn rec(
        role: LibRole,
        genre: &str,
        name: &str,
        feel: Feel,
        energy: Energy,
        density: Density,
        poly: Poly,
        length: usize,
        func: Option<PatternFunction>,
        tags: &[&str],
        desc: &str,
    ) -> Record {
        make_record(
            role,
            genre.into(),
            name.into(),
            None,
            LaneKind::Drums,
            length,
            None,
            func,
            feel,
            poly,
            None,
            None,
            energy,
            density,
            None,
            tags.iter().map(|s| s.to_string()).collect(),
            None,
            None,
            desc.into(),
        )
    }

    fn sample() -> Vec<Record> {
        vec![
            rec(
                LibRole::Drums,
                "techno",
                "Four on Floor",
                Feel::Straight,
                Energy::High,
                Density::Core,
                Poly::Mono,
                16,
                Some(PatternFunction::Core),
                &["driving"],
                "warehouse kick",
            ),
            rec(
                LibRole::Drums,
                "boom-bap",
                "Dusty Core",
                Feel::Swing,
                Energy::Mid,
                Density::Core,
                Poly::Mono,
                16,
                Some(PatternFunction::Core),
                &["hip-hop", "dusty"],
                "swung backbeat",
            ),
            rec(
                LibRole::Bass,
                "techno",
                "Octave Pulse",
                Feel::Straight,
                Energy::Mid,
                Density::Core,
                Poly::Mono,
                16,
                Some(PatternFunction::Core),
                &[],
                "rolling sub",
            ),
            rec(
                LibRole::Synth,
                "amapiano",
                "Jazzy Keys",
                Feel::Swing,
                Energy::Low,
                Density::Sparse,
                Poly::Poly,
                16,
                Some(PatternFunction::Core),
                &["jazz"],
                "Rhodes CAFÉ chords",
            ),
        ]
    }

    #[test]
    fn empty_query_returns_all_sorted() {
        let idx = sample();
        let favs = Favorites::default();
        let out = filter(&idx, &Query::default(), &favs);
        assert_eq!(out.len(), 4);
        // Order: role (drums<bass<synth), then genre. Drums: boom-bap < techno.
        let names: Vec<&str> = out.iter().map(|&i| idx[i].name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Dusty Core", "Four on Floor", "Octave Pulse", "Jazzy Keys"]
        );
    }

    #[test]
    fn facet_filters_are_anded() {
        let idx = sample();
        let favs = Favorites::default();
        let q = Query {
            role: Some(LibRole::Drums),
            feel: Some(Feel::Swing),
            ..Default::default()
        };
        let out = filter(&idx, &q, &favs);
        assert_eq!(out.len(), 1);
        assert_eq!(idx[out[0]].name, "Dusty Core");
    }

    #[test]
    fn text_is_case_and_unicode_insensitive() {
        let idx = sample();
        let favs = Favorites::default();
        // mixed case + accented term present in desc ("CAFÉ")
        let out = filter(&idx, &Query::default().with_text("café"), &favs);
        assert_eq!(out.len(), 1);
        assert_eq!(idx[out[0]].name, "Jazzy Keys");
        // multi-term AND across name + tag
        let out = filter(&idx, &Query::default().with_text("DUSTY hip-hop"), &favs);
        assert_eq!(out.len(), 1);
        // no match → empty
        assert!(filter(&idx, &Query::default().with_text("nonexistent"), &favs).is_empty());
    }

    #[test]
    fn poly_and_length_and_density_filters() {
        let idx = sample();
        let favs = Favorites::default();
        assert_eq!(
            filter(
                &idx,
                &Query {
                    poly: Some(Poly::Poly),
                    ..Default::default()
                },
                &favs
            )
            .len(),
            1
        );
        assert_eq!(
            filter(
                &idx,
                &Query {
                    density: Some(Density::Sparse),
                    ..Default::default()
                },
                &favs
            )
            .len(),
            1
        );
        assert_eq!(
            filter(
                &idx,
                &Query {
                    length: Some((1, 16)),
                    ..Default::default()
                },
                &favs
            )
            .len(),
            4
        );
        assert_eq!(
            filter(
                &idx,
                &Query {
                    length: Some((17, 64)),
                    ..Default::default()
                },
                &favs
            )
            .len(),
            0
        );
    }

    #[test]
    fn favorites_only_filter() {
        let idx = sample();
        let mut favs = Favorites::default();
        favs.toggle(idx[1].pattern_ref()); // favorite "Dusty Core"
        let out = filter(
            &idx,
            &Query {
                favorites_only: true,
                ..Default::default()
            },
            &favs,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(idx[out[0]].name, "Dusty Core");
    }

    #[test]
    fn stable_selection_tracks_identity_then_clamps() {
        let idx = sample();
        let favs = Favorites::default();
        let all = filter(&idx, &Query::default(), &favs);
        // "Octave Pulse" is at position 2 in the full result.
        let pos = restable_selection(&idx, &all, Some(("techno", "Octave Pulse")), 0);
        assert_eq!(pos, Some(2));
        // After narrowing to drums, the identity is gone → clamp to min(prev, len-1).
        let drums = filter(
            &idx,
            &Query {
                role: Some(LibRole::Drums),
                ..Default::default()
            },
            &favs,
        );
        let pos = restable_selection(&idx, &drums, Some(("techno", "Octave Pulse")), 5);
        assert_eq!(pos, Some(drums.len() - 1));
        // Empty result → None.
        let none = filter(&idx, &Query::default().with_text("zzz"), &favs);
        assert_eq!(restable_selection(&idx, &none, None, 0), None);
    }
}
