//! Spoolman's search (`GET /search`), ported from Spoolman (Donkie/Spoolman, MIT) at `eccb05e`:
//! `spoolman/database/search.py`, `spoolman/colors.py` and the colour half of
//! `spoolman/math.py` (`hex_to_rgb`, `rgb_to_lab`, `delta_e`), and `find_by_color` from
//! `spoolman/database/filament.py`.
//!
//! It works on the items as the API gives them (`Spool.from_db` and so on), the hub's
//! `Work::all`. The SQL half of each search (the candidates: every term somewhere in a
//! column, the newest 200) runs here as the reference's database runs it, SQLite: `ilike` is
//! `lower(column) LIKE lower(pattern)`, with Spoolman's own `lower` (Python's `str.lower`,
//! `_register_sqlite_functions` in `spoolman/database/database.py`) in place of SQLite's.

use serde_json::{Map, Value, json};

/// `_CANDIDATE_CAP`, `_MAX_TERMS` and the rank buckets (`search.py`).
const CANDIDATE_CAP: usize = 200;
const MAX_TERMS: usize = 8;
const RANK_ID_OR_COLOR: i64 = -1;
const BASE_NATIVE: i64 = 0;
const BASE_VENDOR: i64 = 2;
const BASE_EXTRA: i64 = 4;
const WEAK_PENALTY: i64 = 1;

/// Python's `str.isspace`, what `str.split()` and `str.strip()` take as whitespace.
fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

fn py_strip(s: &str) -> &str {
    s.trim_matches(py_space)
}

/// `_split_terms`: lower-cased, de-duplicated search terms, in query order.
fn split_terms(q: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    for raw in q.to_lowercase().split(py_space).filter(|t| !t.is_empty()) {
        if !terms.iter().any(|t| t == raw) {
            terms.push(raw.to_owned());
        }
    }
    terms.truncate(MAX_TERMS);
    terms
}

/// One field a row is classified on: its label, its value, its base rank.
type Field = (String, Option<String>, i64);

/// `_classify_match`: `(match_field, rank)` for a row, or `None` if some term doesn't match.
fn classify(terms: &[String], fields: &[Field]) -> Option<(String, i64)> {
    let mut best: Option<(String, i64)> = None;
    let mut worst = RANK_ID_OR_COLOR;
    for term in terms {
        let mut term_best: Option<(String, i64)> = None;
        for (label, value, base) in fields {
            let Some(value) = value.as_deref().filter(|v| !v.is_empty()) else {
                continue;
            };
            let v = value.to_lowercase();
            if !v.contains(term.as_str()) {
                continue;
            }
            let rank = if v.starts_with(term.as_str()) {
                *base
            } else {
                base + WEAK_PENALTY
            };
            if term_best.as_ref().is_none_or(|(_, r)| rank < *r) {
                term_best = Some((label.clone(), rank));
            }
        }
        let term_best = term_best?;
        worst = worst.max(term_best.1);
        if best.as_ref().is_none_or(|(_, r)| term_best.1 < *r) {
            best = Some(term_best);
        }
    }
    best.map(|(label, _)| (label, worst))
}

/// `col.ilike(f"%{escape_like(term)}%", escape="/")` on SQLite: `lower(col) LIKE lower(pattern)`,
/// the term, wildcards escaped, anywhere in the value. Spoolman swaps SQLite's ASCII-only `lower`
/// for Python's `str.lower` on every connection (`_register_sqlite_functions`, database.py), so
/// every letter is folded, not only ASCII.
fn sql_ilike(value: Option<&str>, term: &str) -> bool {
    value.is_some_and(|v| v.to_lowercase().contains(&term.to_lowercase()))
}

fn text(item: &Value, key: &str) -> Option<String> {
    item.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// The row's text and single-choice extra fields (`_extra_values`), as classifier fields: the
/// stored value as it is, a JSON text. In key order, the order of the field table's key.
fn extras(item: &Value, keys: &[String]) -> Vec<Field> {
    let mut out: Vec<Field> = Vec::new();
    if let Some(extra) = item.get("extra").and_then(Value::as_object) {
        let mut present: Vec<&String> = keys.iter().filter(|k| extra.contains_key(*k)).collect();
        present.sort();
        for key in present {
            out.push((
                format!("extra.{key}"),
                extra[key].as_str().map(str::to_owned),
                BASE_EXTRA,
            ));
        }
    }
    out
}

/// The SQL candidates: every term in a column or an extra field, newest first, at most 200.
fn candidates<'a>(
    rows: impl Iterator<Item = &'a Value>,
    terms: &[String],
    columns: &dyn Fn(&Value) -> Vec<Option<String>>,
    keys: &[String],
) -> Vec<&'a Value> {
    let mut hit: Vec<&Value> = rows
        .filter(|row| {
            let cols = columns(row);
            let ext = extras(row, keys);
            terms.iter().all(|term| {
                cols.iter().any(|c| sql_ilike(c.as_deref(), term))
                    || ext.iter().any(|(_, v, _)| sql_ilike(v.as_deref(), term))
            })
        })
        .collect();
    hit.sort_by_key(|row| std::cmp::Reverse(id(row)));
    hit.truncate(CANDIDATE_CAP);
    hit
}

fn id(item: &Value) -> i64 {
    item["id"].as_i64().unwrap_or_default()
}

/// Rank and trim: by (rank, then the colour order for colour hits, then id).
fn ranked(mut hits: Vec<(i64, String, i64, usize)>, limit: usize) -> Vec<(i64, String)> {
    hits.sort_by_key(|(id, _, rank, colour)| (*rank, *colour, *id));
    hits.truncate(limit);
    hits.into_iter().map(|(id, f, _, _)| (id, f)).collect()
}

// ---- colour -------------------------------------------------------------------------

include!("search_colors.rs");

/// `resolve_color`: a hex code (`#rgb`, `#rrggbb` or without `#`) or a CSS colour name to
/// `rrggbb`, or `None`.
fn resolve_color(query: &str) -> Option<String> {
    let normalized = py_strip(query).to_lowercase();
    let compact = normalized.replace(' ', "");
    let hex = compact.strip_prefix('#').unwrap_or(&compact);
    if (hex.len() == 3 || hex.len() == 6)
        && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        if hex.len() == 3 {
            return Some(hex.chars().flat_map(|c| [c, c]).collect());
        }
        return Some(hex.to_owned());
    }
    CSS_COLORS
        .iter()
        .find(|(name, _)| *name == compact)
        .map(|(_, hex)| (*hex).to_owned())
}

/// `hex_to_rgb`; `None` where Python's `int(..., 16)` raises.
fn hex_to_rgb(hex: &str) -> Option<[i64; 3]> {
    let hex = hex.trim_start_matches('#');
    let part = |a: usize| {
        hex.get(a..a + 2)
            .and_then(|p| i64::from_str_radix(p, 16).ok())
    };
    Some([part(0)?, part(2)?, part(4)?])
}

/// `rgb_to_lab`, in its order of operations.
fn rgb_to_lab(rgb: [i64; 3]) -> [f64; 3] {
    let lin = |c: f64| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (
        lin(rgb[0] as f64 / 255.0),
        lin(rgb[1] as f64 / 255.0),
        lin(rgb[2] as f64 / 255.0),
    );
    let x = (r * 0.4124 + g * 0.3576 + b * 0.1805) / 0.95047;
    let y = (r * 0.2126 + g * 0.7152 + b * 0.0722) / 1.00000;
    let z = (r * 0.0193 + g * 0.1192 + b * 0.9505) / 1.08883;
    let f = |t: f64| {
        if t > 0.008856 {
            t.powf(1.0 / 3.0)
        } else {
            (7.787 * t) + 16.0 / 116.0
        }
    };
    let (x, y, z) = (f(x), f(y), f(z));
    [(116.0 * y) - 16.0, 500.0 * (x - y), 200.0 * (y - z)]
}

/// `delta_e` (CIE94), in its order of operations.
fn delta_e(a: [f64; 3], b: [f64; 3]) -> f64 {
    let delta_l = a[0] - b[0];
    let delta_a = a[1] - b[1];
    let delta_b = a[2] - b[2];
    let c1 = (a[1] * a[1] + a[2] * a[2]).sqrt();
    let c2 = (b[1] * b[1] + b[2] * b[2]).sqrt();
    let delta_c = c1 - c2;
    let delta_h = delta_a * delta_a + delta_b * delta_b - delta_c * delta_c;
    let delta_h = if delta_h > 0.0 { delta_h.sqrt() } else { 0.0 };
    let sc = 1.0 + 0.045 * c1;
    let sh = 1.0 + 0.015 * c1;
    let delta_l_kl_sl = delta_l / 1.0;
    let delta_c_kc_sc = delta_c / sc;
    let delta_h_kh_sh = delta_h / sh;
    let i = delta_l_kl_sl * delta_l_kl_sl
        + delta_c_kc_sc * delta_c_kc_sc
        + delta_h_kh_sh * delta_h_kh_sh;
    if i > 0.0 { i.sqrt() } else { 0.0 }
}

/// A filament's colours: `color_hex`, else `multi_color_hexes` split on commas.
fn colours(filament: &Value) -> Option<Vec<String>> {
    if let Some(c) = text(filament, "color_hex") {
        return Some(vec![c]);
    }
    text(filament, "multi_color_hexes").map(|m| m.split(',').map(str::to_owned).collect())
}

/// `_filament_color_distance`: the smallest delta-E to any of the filament's colours.
fn distance(filament: &Value, query: [f64; 3]) -> f64 {
    colours(filament).map_or(f64::INFINITY, |cs| {
        cs.iter()
            .filter_map(|c| hex_to_rgb(c))
            .map(|c| delta_e(query, rgb_to_lab(c)))
            .fold(f64::INFINITY, f64::min)
    })
}

/// `_color_matches` with `find_by_color`: the filaments with a colour within `threshold`, in
/// id order, then closest first.
fn colour_matches<'a>(
    filaments: &'a [Value],
    hex: &str,
    threshold: f64,
    limit: usize,
) -> Vec<&'a Value> {
    let Some(rgb) = hex_to_rgb(hex) else {
        return Vec::new();
    };
    let query = rgb_to_lab(rgb);
    let mut matched: Vec<&Value> = filaments
        .iter()
        .filter(|f| {
            colours(f).is_some_and(|cs| {
                cs.iter()
                    .filter_map(|c| hex_to_rgb(c))
                    .any(|c| delta_e(query, rgb_to_lab(c)) <= threshold)
            })
        })
        .collect();
    matched.sort_by_key(|f| id(f));
    matched.sort_by(|a, b| distance(a, query).total_cmp(&distance(b, query)));
    matched.truncate(limit);
    matched
}

// ---- the search ---------------------------------------------------------------------

/// What `GET /search` takes.
pub struct Query<'a> {
    pub q: &'a str,
    pub color_similarity_threshold: f64,
    pub limit: usize,
    pub allow_archived: bool,
    pub spools_per_filament: usize,
}

/// The text and single-choice extra field keys of each entity (`_text_extra_keys`).
pub struct Keys {
    pub spool: Vec<String>,
    pub filament: Vec<String>,
    pub vendor: Vec<String>,
}

fn archived(spool: &Value) -> bool {
    spool["archived"].as_bool().unwrap_or(false)
}

/// `search` and the route's `SearchResults`, from every spool, filament and vendor as the
/// API gives them (in id order).
pub fn search(
    q: &Query,
    keys: &Keys,
    spools: &[Value],
    filaments: &[Value],
    vendors: &[Value],
) -> Value {
    let query = py_strip(q.q);
    let terms = split_terms(query);
    if terms.is_empty() {
        return json!({"spools": [], "filaments": [], "vendors": [], "is_color_query": false});
    }
    let colour = resolve_color(query);
    let colour_hits = match &colour {
        Some(hex) => colour_matches(filaments, hex, q.color_similarity_threshold, q.limit),
        None => Vec::new(),
    };

    // Spools.
    let spool_columns =
        |s: &Value| vec![text(s, "comment"), text(s, "location"), text(s, "lot_nr")];
    let open = spools.iter().filter(|s| q.allow_archived || !archived(s));
    let mut hits = Vec::new();
    for s in candidates(open, &terms, &spool_columns, &keys.spool) {
        let mut fields: Vec<Field> = ["comment", "location", "lot_nr"]
            .iter()
            .map(|k| ((*k).to_owned(), text(s, k), BASE_NATIVE))
            .collect();
        fields.extend(extras(s, &keys.spool));
        if let Some((field, rank)) = classify(&terms, &fields) {
            hits.push((id(s), field, rank, 0));
        }
    }
    let mut spool_hits = ranked(hits, q.limit);

    // Filaments: the colour hits first, in their order.
    let mut hits: Vec<(i64, String, i64, usize)> = colour_hits
        .iter()
        .enumerate()
        .map(|(i, f)| (id(f), "color".to_owned(), RANK_ID_OR_COLOR, i))
        .collect();
    let vendor_name = |f: &Value| {
        f["vendor"]
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let filament_columns = |f: &Value| {
        vec![
            text(f, "name"),
            text(f, "material"),
            text(f, "article_number"),
            text(f, "comment"),
            vendor_name(f),
        ]
    };
    for f in candidates(filaments.iter(), &terms, &filament_columns, &keys.filament) {
        if hits.iter().any(|(i, ..)| *i == id(f)) {
            continue;
        }
        let mut fields: Vec<Field> = ["name", "material", "article_number", "comment"]
            .iter()
            .map(|k| ((*k).to_owned(), text(f, k), BASE_NATIVE))
            .collect();
        fields.push(("vendor.name".to_owned(), vendor_name(f), BASE_VENDOR));
        fields.extend(extras(f, &keys.filament));
        if let Some((field, rank)) = classify(&terms, &fields) {
            hits.push((id(f), field, rank, 0));
        }
    }
    let filament_hits = ranked(hits, q.limit);

    // Vendors.
    let vendor_columns = |v: &Value| vec![text(v, "name"), text(v, "comment")];
    let mut hits = Vec::new();
    for v in candidates(vendors.iter(), &terms, &vendor_columns, &keys.vendor) {
        let mut fields: Vec<Field> = ["name", "comment"]
            .iter()
            .map(|k| ((*k).to_owned(), text(v, k), BASE_NATIVE))
            .collect();
        fields.extend(extras(v, &keys.vendor));
        if let Some((field, rank)) = classify(&terms, &fields) {
            hits.push((id(v), field, rank, 0));
        }
    }
    let vendor_hits = ranked(hits, q.limit);

    // A numeric query: the spool with that id at the very top.
    if !query.is_empty()
        && query.chars().all(|c| c.is_ascii_digit())
        && let Some(spool) = query
            .parse::<i64>()
            .ok()
            .and_then(|n| spools.iter().find(|s| id(s) == n))
        && (q.allow_archived || !archived(spool))
    {
        spool_hits.retain(|(i, _)| *i != id(spool));
        spool_hits.insert(0, (id(spool), "id".to_owned()));
        spool_hits.truncate(q.limit);
    }

    let by_id = |items: &[Value], i: i64| {
        items
            .iter()
            .find(|x| id(x) == i)
            .cloned()
            .unwrap_or(Value::Null)
    };
    let spools_out: Vec<Value> = spool_hits
        .iter()
        .map(|(i, f)| json!({"spool": by_id(spools, *i), "match_field": f}))
        .collect();
    let filaments_out: Vec<Value> = filament_hits
        .iter()
        .map(|(i, f)| {
            let filament = by_id(filaments, *i);
            let mut m = Map::new();
            m.insert("filament".into(), filament.clone());
            m.insert("match_field".into(), json!(f));
            if q.spools_per_filament > 0 {
                // `_attach_filament_spools`: its spools by id, the first few, and how many.
                let of: Vec<&Value> = spools
                    .iter()
                    .filter(|s| s["filament"]["id"].as_i64() == Some(*i))
                    .filter(|s| q.allow_archived || !archived(s))
                    .collect();
                let few: Vec<Value> = of
                    .iter()
                    .take(q.spools_per_filament)
                    .map(|s| filament_spool(s, filament["weight"].as_f64()))
                    .collect();
                m.insert("spools".into(), Value::Array(few));
                m.insert("spool_count".into(), json!(of.len()));
            }
            Value::Object(m)
        })
        .collect();
    let vendors_out: Vec<Value> = vendor_hits
        .iter()
        .map(|(i, f)| json!({"vendor": by_id(vendors, *i), "match_field": f}))
        .collect();
    json!({
        "spools": spools_out,
        "filaments": filaments_out,
        "vendors": vendors_out,
        "is_color_query": colour.is_some(),
    })
}

/// `SearchResultFilamentSpool.from_db`: id, the remaining weight from the spool's initial
/// weight or else the filament's (left out when neither), location, archived.
fn filament_spool(spool: &Value, filament_weight: Option<f64>) -> Value {
    let used = spool["used_weight"].as_f64().unwrap_or(0.0);
    let remaining = spool["initial_weight"]
        .as_f64()
        .or(filament_weight)
        .map(|w| (w - used).max(0.0));
    let mut m = Map::new();
    m.insert("id".into(), json!(id(spool)));
    if let Some(r) = remaining {
        m.insert("remaining_weight".into(), json!(r));
    }
    if let Some(l) = spool.get("location").filter(|l| !l.is_null()) {
        m.insert("location".into(), l.clone());
    }
    m.insert("archived".into(), json!(archived(spool)));
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_resolve_as_spoolman_resolves_them() {
        assert_eq!(resolve_color("#FF0000").as_deref(), Some("ff0000"));
        assert_eq!(resolve_color(" f0a ").as_deref(), Some("ff00aa"));
        assert_eq!(resolve_color("Dark Red").as_deref(), Some("8b0000"));
        assert_eq!(resolve_color("petg"), None);
        assert_eq!(CSS_COLORS.len(), 148);
    }

    #[test]
    fn terms_split_as_python_splits_them() {
        assert_eq!(split_terms("PLA  pla Red"), vec!["pla", "red"]);
        assert_eq!(split_terms("a b c d e f g h i j").len(), 8);
    }
}
