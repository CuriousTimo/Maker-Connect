//! A Buddy printer's tools over Prusa Connect, read onto a tool tree as Prusa's own server reads
//! them: `reference/prusa-connect` `internal/connector/xbuddy` (`telemetrytools.go`, `tools.go`)
//! and the tree's contract in `sdk/connectorapi/tools.go` (read only: OCL, nothing copied). The
//! tree is kept in [`Telemetry::tool_tree`] (the server's state tree, which outlives a link); what
//! the server keeps per session (the layout an INFO gave, the readings written, the head the top
//! target went to, the active members) is in [`Session`], in memory per machine, made only for a
//! printer whose token is known and dropped with the machine (`Legacy::forget_machine`).
//!
//! Ours (the cost of our persistence): the server's session is memory-only, and so is ours, but
//! our tree is stored with the telemetry. After a hub restart the tree is back and the session is
//! empty until the next INFO: the layout falls back to `currentLayout`'s default, the one head to
//! head 0, and the active members and readings are not known. Nothing rebuilds the session from
//! the tree (the server has no such rebuild).
//!
//! Not covered: what an INFO writes onto the tree besides the active members (tool count,
//! enabled, type, features, materials; our `Details` keeps INFO's material, `to_details`), the
//! MMU's progress code and command (`numberedUnit`, `treeUnit`), and the legacy slot paths.

use maker_connect_protocol::{Telemetry, ToolHeat};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Tool members (`sdk/connectorapi/tools.go:57-137`).
const ACTIVE: &str = "active";
const NOZZLE_TEMP: &str = "nozzle_temp";
const NOZZLE_TARGET_TEMP: &str = "nozzle_target_temp";
const PRINT_FAN_RPM: &str = "print_fan_rpm";
const HEATBREAK_FAN_RPM: &str = "heatbreak_fan_rpm";
pub(crate) const MATERIAL: &str = "material_package_instance/package/material/abbreviation";

/// The members of a tree-shaped telemetry tool that are passed through (`treeTelemetryMembers`,
/// `telemetrytools.go:55-66`).
const TREE_TELEMETRY_MEMBERS: [&str; 8] = [
    NOZZLE_TEMP,
    NOZZLE_TARGET_TEMP,
    PRINT_FAN_RPM,
    HEATBREAK_FAN_RPM,
    "consumed_weight",
    "odometer_id",
    "odometer_e",
    "odometer_weight",
];

/// A slot block entry's members and the head members they are (`numberedSlotMembers`,
/// `telemetrytools.go:45-53`).
const SLOT_MEMBERS: [(&str, &str); 3] = [
    ("temp", NOZZLE_TEMP),
    ("fan_print", PRINT_FAN_RPM),
    ("fan_hotend", HEATBREAK_FAN_RPM),
];

/// How the numbered entries sit on the tree (`tools.go:52-57`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Layout {
    #[default]
    Unknown,
    Heads,
    Feeder,
}

/// What the server keeps per printer session about its tools (`Session` fields `layout`,
/// `soleHead`, `heldTools`, `readings`, `targetHead`, `activity`).
#[derive(Debug, Clone, Default)]
pub(crate) struct Session {
    layout: Layout,
    /// The one enabled head an INFO named, where it named exactly one (`toolReport.sole`).
    sole_head: Option<String>,
    /// The tools the last INFO named, enabled or not (`heldTools`); None before any.
    info_tools: Option<BTreeSet<String>>,
    /// Tools a tree-shaped frame wrote readings for (`readings`).
    readings: BTreeSet<String>,
    /// The head the top target of a numbered frame was last written to (`targetHead`).
    target_head: String,
    /// The active member of each tool as last reported (`activity.members`).
    active: BTreeMap<String, bool>,
    /// What an INFO wrote onto the tree, put on it with the next telemetry frame (ours: the tree
    /// lives in the telemetry, which an event does not write).
    pending: Vec<(String, &'static str, Value)>,
    /// The telemetry as the last frame left it, so that the next frame is taken in over it under
    /// the hub's lock (ours: the server writes patches onto one tree; ours is the telemetry). None
    /// after a restart: the stored one then.
    pub(crate) last: Option<Telemetry>,
    /// A label printer on DYMO's CUPS driver: the `reason` of its last `STATE_CHANGED`, the PPD's own
    /// text (docs/design/prusa-sdk-extension.md, part 3), shown as its problem; None for any other
    /// machine, whose problem this leaves alone.
    pub(crate) label_reason: Option<Option<String>>,
    /// What the server's connector keeps per session about the print (`crate::devices::connect_job`).
    pub(crate) job: crate::devices::connect_job::Job,
}

/// The tree in a telemetry, written as the server writes patches: a null removes the member.
fn report(tree: &mut BTreeMap<String, Map<String, Value>>, id: &str, member: &str, value: Value) {
    if value.is_null() {
        if let Some(tool) = tree.get_mut(id) {
            tool.remove(member);
            if tool.is_empty() {
                tree.remove(id);
            }
        }
    } else {
        tree.entry(id.to_owned())
            .or_default()
            .insert(member.to_owned(), value);
    }
}

/// A member a frame carries a value for, written (`writer.member`, `tools.go:120-128`: a null or a
/// missing key writes nothing).
fn member(
    tree: &mut BTreeMap<String, Map<String, Value>>,
    id: &str,
    name: &str,
    value: Option<&Value>,
) {
    if let Some(value) = value.filter(|v| !v.is_null()) {
        report(tree, id, name, value.clone());
    }
}

/// `canonicalNumber` (`sdk/connectorapi/tools.go:340-353`).
fn canonical_number(text: &str) -> bool {
    !text.is_empty()
        && !(text.len() > 1 && text.starts_with('0'))
        && text.bytes().all(|b| b.is_ascii_digit())
}

/// `ToolID.Valid` (`tools.go:204-219`).
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.split('.').all(canonical_number)
}

/// `ToolID.Head` (`tools.go:231-236`).
fn head_of(id: &str) -> &str {
    id.split('.').next().unwrap_or("")
}

/// `ToolID.Compare` (`tools.go:241-253`): part by part, as numbers.
fn compare_ids(a: &str, b: &str) -> std::cmp::Ordering {
    let (left, right): (Vec<&str>, Vec<&str>) = (a.split('.').collect(), b.split('.').collect());
    for (l, r) in left.iter().zip(&right) {
        let c = l.len().cmp(&r.len()).then_with(|| l.cmp(r));
        if c.is_ne() {
            return c;
        }
    }
    left.len().cmp(&right.len())
}

/// `ActiveToolOf` (`tools.go:296-336`): the deepest active tool whose parents are all active, the
/// lower id between two of the same depth.
fn active_tool_of(active: &BTreeMap<String, bool>) -> Option<String> {
    let above = |id: &str| {
        let mut id = id;
        while let Some(at) = id.rfind('.') {
            id = &id[..at];
            if active.get(id) != Some(&true) {
                return false;
            }
        }
        true
    };
    let depth = |id: &str| id.matches('.').count();
    let mut best: Option<&str> = None;
    for (id, on) in active {
        if !on || !valid_id(id) || !above(id) {
            continue;
        }
        let deeper =
            |b: &str| depth(id) > depth(b) || (depth(id) == depth(b) && compare_ids(id, b).is_lt());
        if best.is_none_or(deeper) {
            best = Some(id);
        }
    }
    best.map(str::to_owned)
}

/// `wireNumber` (`tools.go:821-829`): a key the numbered shape counts from 1.
fn wire_number(key: &str) -> Option<usize> {
    let n: usize = key.parse().ok()?;
    (n >= 1 && n.to_string() == key).then_some(n)
}

/// `wholeNumber` (`tools.go:831-838`).
fn whole_number(value: Option<&Value>) -> Option<usize> {
    let v = value?.as_f64()?;
    (v >= 0.0 && v.fract() == 0.0).then_some(v as usize)
}

/// `normalizeMaterial` (`tools.go:612-626`): the firmware's `"---"` and an empty name are no
/// material (`connectorapi.NoMaterial`, the empty text), other text passes. A JSON null reads as
/// the empty text there (Go's `Unmarshal` of null into a string), so it is no material too; a
/// value that is not text passes.
pub(crate) fn normalize_material(raw: &Value) -> Value {
    match raw {
        Value::Null => Value::String(String::new()),
        Value::String(s) if matches!(s.trim(), "" | "---") => Value::String(String::new()),
        other => other.clone(),
    }
}

/// `headCounts` and `headsOf` (`tools.go:59-81`, the models of `buildvolume.go:32-47`): how many
/// heads a model has, None for a model the server has no figures for.
fn heads_of(model: &str) -> Option<usize> {
    const MANY: [(&str, usize); 3] = [("XL", 5), ("CoreOne_INDX", 8), ("CoreOneL_INDX", 8)];
    const KNOWN: [&str; 13] = [
        "MINI",
        "MK3.5",
        "MK3.5S",
        "MK3.9",
        "MK3.9S",
        "MK4",
        "MK4S",
        "XL",
        "CoreOne",
        "CoreOneL",
        "IX",
        "CoreOne_INDX",
        "CoreOneL_INDX",
    ];
    let model = model.trim();
    if let Some((_, n)) = MANY
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(model))
    {
        return Some(*n);
    }
    KNOWN
        .iter()
        .any(|name| name.eq_ignore_ascii_case(model))
        .then_some(1)
}

impl Session {
    /// `currentLayout` (`telemetrytools.go:68-81`): what the last INFO said, and before any INFO,
    /// slots on a model with one head and heads on any other.
    fn current_layout(&self, model: &str) -> Layout {
        if self.layout != Layout::Unknown {
            return self.layout;
        }
        if heads_of(model) == Some(1) {
            Layout::Feeder
        } else {
            Layout::Heads
        }
    }

    /// `reportActivity` (`telemetrytools.go:492-531`): the active member of each tool a frame
    /// names, written only when it changed.
    fn report_activity(
        &mut self,
        tree: &mut BTreeMap<String, Map<String, Value>>,
        said: Option<BTreeMap<String, bool>>,
    ) {
        for (id, on) in said.unwrap_or_default() {
            if self.active.get(&id) == Some(&on) {
                continue;
            }
            self.active.insert(id.clone(), on);
            report(tree, &id, ACTIVE, Value::Bool(on));
        }
    }

    /// What one INFO event says about the tools (`reportTools`, `tools.go:672-722`): the layout of
    /// the numbered slot block, the one head, and which tools are active (`toolReport.activity`,
    /// `:724-733`); a tool it no longer names or names as disabled loses its members
    /// (`staleToolPaths`, `writer.disable`, `forgetActivity`).
    pub(crate) fn info(&mut self, data: &Value, model: &str) {
        let Some(report) = info_tools(data, model) else {
            return;
        };
        if report.layout != Layout::Unknown {
            self.layout = report.layout;
        }
        let Some(named) = report.tools else {
            return;
        };
        self.sole_head = report.sole.clone();
        let previous = self.info_tools.replace(named.clone());
        let gone = previous
            .into_iter()
            .flatten()
            .filter(|id| !named.contains(id))
            .chain(report.disabled.iter().cloned());
        for id in gone.collect::<Vec<_>>() {
            self.active.remove(&id);
            self.pending.push((id, "", Value::Null));
        }
        self.active.retain(|id, _| named.contains(id));
        let said = match (report.layout, &report.sole) {
            (Layout::Feeder, _) => Some(BTreeMap::from([("0".to_owned(), true)])),
            (Layout::Heads, Some(sole)) => Some(BTreeMap::from([(sole.clone(), true)])),
            _ => None,
        };
        for (id, on) in said.unwrap_or_default() {
            if self.active.get(&id) != Some(&on) {
                self.active.insert(id.clone(), on);
                self.pending.push((id, ACTIVE, Value::Bool(on)));
            }
        }
    }

    /// `reportToolTelemetry` (`telemetrytools.go:101-144`): one telemetry frame's tools onto the
    /// tree, in the shape it has.
    pub(crate) fn telemetry(&mut self, t: &mut Telemetry, fields: &Value, model: &str) {
        let tree = &mut t.tool_tree;
        for (id, name, value) in std::mem::take(&mut self.pending) {
            if name.is_empty() {
                tree.remove(&id);
            } else {
                report(tree, &id, name, value);
            }
        }
        if let Some(tools) = fields.get("tools") {
            let (written, nulls, said) = tree_telemetry(tree, tools);
            self.stale_readings(tree, &written, &nulls);
            self.report_activity(tree, said);
        } else if let Some(slot) = fields.get("slot") {
            let layout = self.current_layout(model);
            let said = numbered_slot(tree, slot, layout);
            self.report_activity(tree, said);
            self.report_top_readings(tree, fields, layout);
        } else {
            // One path: one head, the top of the frame is all it says of it; before an INFO names
            // the head, head 0 (`currentSoleHead`, `telemetrytools.go:83-99`, `:134-143`).
            let head = self.sole_head.clone().unwrap_or_else(|| "0".to_owned());
            report_head(tree, fields, &head, true);
        }
        show(t);
    }

    /// `staleReadings` (`telemetrytools.go:350-391`): the tools a tree frame wrote readings for
    /// are remembered; a tool the frame reports as null loses its readings, only when an earlier
    /// frame wrote them.
    fn stale_readings(
        &mut self,
        tree: &mut BTreeMap<String, Map<String, Value>>,
        written: &BTreeSet<String>,
        nulls: &[String],
    ) {
        self.readings.extend(written.iter().cloned());
        for id in nulls {
            if !self.readings.remove(id) {
                continue;
            }
            for name in TREE_TELEMETRY_MEMBERS {
                report(tree, id, name, Value::Null);
            }
        }
    }

    /// `reportTopReadings` (`telemetrytools.go:146-182`): in front of an MMU the top of the frame
    /// is the one head's; on a toolchanger only the target at the top is read, onto the picked
    /// head, and the head it was written to before loses it.
    fn report_top_readings(
        &mut self,
        tree: &mut BTreeMap<String, Map<String, Value>>,
        fields: &Value,
        layout: Layout,
    ) {
        if layout == Layout::Feeder {
            report_head(tree, fields, "0", false);
            return;
        }
        let Some(target) = fields.get("target_nozzle") else {
            return;
        };
        let head = active_tool_of(&self.active)
            .map(|id| head_of(&id).to_owned())
            .unwrap_or_default();
        let stale = std::mem::replace(&mut self.target_head, head.clone());
        if !stale.is_empty() && stale != head {
            report(tree, &stale, NOZZLE_TARGET_TEMP, Value::Null);
        }
        if !head.is_empty() {
            report(tree, &head, NOZZLE_TARGET_TEMP, target.clone());
        }
    }
}

/// What an INFO says about the tools (`toolReport`, `tools.go:100-116`).
struct InfoReport {
    layout: Layout,
    sole: Option<String>,
    /// Every tool it names, enabled or not; None when it says nothing of the heads.
    tools: Option<BTreeSet<String>>,
    disabled: Vec<String>,
}

/// `infoTools` (`tools.go:151-172`) as far as the layout, the one head and the tools named go.
fn info_tools(data: &Value, model: &str) -> Option<InfoReport> {
    let info = data.as_object()?;
    if tree_shaped(info) {
        // `treeInfoTools` (`tools.go:351-...`): its own ids; a null tool is disabled.
        let mut tools = BTreeSet::new();
        let mut disabled = Vec::new();
        let mut heads = false;
        for (key, raw) in info
            .get("tools")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            if !valid_id(key) {
                continue;
            }
            heads |= !key.contains('.');
            tools.insert(key.clone());
            if raw.is_null() {
                disabled.push(key.clone());
            }
        }
        return Some(InfoReport {
            layout: Layout::Unknown,
            sole: None,
            tools: heads.then_some(tools),
            disabled,
        });
    }
    // `numberedInfoTools` (`tools.go:217-279`).
    let Some(numbered) = info.get("tools").and_then(Value::as_object) else {
        return Some(InfoReport {
            layout: Layout::Unknown,
            sole: None,
            tools: None,
            disabled: Vec::new(),
        });
    };
    let listed: BTreeSet<usize> = numbered.keys().filter_map(|k| wire_number(k)).collect();
    let highest = listed.iter().max().copied().unwrap_or(0);
    // `toolLayoutFor` (`tools.go:310-331`).
    let layout = match info.get("mmu") {
        Some(mmu) if !mmu.is_null() => {
            if mmu["enabled"].as_bool() == Some(true) && !listed.is_empty() {
                Layout::Feeder
            } else {
                Layout::Heads
            }
        }
        _ if heads_of(model) == Some(1) && listed.len() > 1 => Layout::Feeder,
        _ => Layout::Heads,
    };
    let mut tools = BTreeSet::new();
    let mut disabled = Vec::new();
    let sole = if layout == Layout::Feeder {
        // `numberedFeeder` (`tools.go:281-308`): head 0, its slots `0.<n-1>`.
        tools.insert("0".to_owned());
        for n in 1..=highest {
            let slot = format!("0.{}", n - 1);
            if !listed.contains(&n) {
                disabled.push(slot.clone());
            }
            tools.insert(slot);
        }
        Some("0".to_owned())
    } else {
        let count = heads_of(model).unwrap_or(0).max(highest);
        for n in 1..=count {
            let id = (n - 1).to_string();
            if !listed.contains(&n) {
                disabled.push(id.clone());
            }
            tools.insert(id);
        }
        (listed.len() == 1).then(|| (highest - 1).to_string())
    };
    Some(InfoReport {
        layout,
        sole,
        tools: Some(tools),
        disabled,
    })
}

/// `treeShaped` (`tools.go:174-215`).
fn tree_shaped(info: &Map<String, Value>) -> bool {
    if info.contains_key("tool_count") {
        return true;
    }
    info.get("tools")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .any(|(key, raw)| {
            wire_number(key).is_none()
                || raw.is_null()
                || raw
                    .as_object()
                    .is_some_and(|e| e.contains_key("features") || e.contains_key("type"))
        })
}

/// `treeTelemetryPatches` and `treeReadings` (`telemetrytools.go:393-461`): each tool's readings
/// (a null target is 0, any other null removes the member, a member left out says nothing), the
/// tools written, the null ones, and each tool's active member.
fn tree_telemetry(
    tree: &mut BTreeMap<String, Map<String, Value>>,
    tools: &Value,
) -> (
    BTreeSet<String>,
    Vec<String>,
    Option<BTreeMap<String, bool>>,
) {
    let mut written = BTreeSet::new();
    let mut nulls = Vec::new();
    let mut said: Option<BTreeMap<String, bool>> = None;
    for (id, value) in tools.as_object().into_iter().flatten() {
        if !valid_id(id) {
            continue;
        }
        if value.is_null() {
            nulls.push(id.clone());
            continue;
        }
        let Some(members) = value.as_object() else {
            continue;
        };
        for name in TREE_TELEMETRY_MEMBERS {
            match members.get(name) {
                None => {}
                Some(Value::Null) if name == NOZZLE_TARGET_TEMP => {
                    report(tree, id, name, Value::from(0));
                    written.insert(id.clone());
                }
                Some(Value::Null) => report(tree, id, name, Value::Null),
                Some(reading) => {
                    report(tree, id, name, reading.clone());
                    written.insert(id.clone());
                }
            }
        }
        if let Some(active) = members.get(ACTIVE).and_then(Value::as_bool) {
            said.get_or_insert_default().insert(id.clone(), active);
        }
    }
    nulls.sort_by(|a, b| compare_ids(a, b));
    (written, nulls, said)
}

/// `numberedSlotPatches`, `numberedEntries` and `numberedActivity` (`telemetrytools.go:203-348`):
/// on a toolchanger entry `n` is head `n-1` with its readings and material; in front of an MMU
/// entry `n` is slot `0.<n-1>` with its material, and the head's fans are the selected entry's
/// (the first when none is). The activity is what `active` says (from 1, 0 for none).
fn numbered_slot(
    tree: &mut BTreeMap<String, Map<String, Value>>,
    slot: &Value,
    layout: Layout,
) -> Option<BTreeMap<String, bool>> {
    let block = slot.as_object()?;
    let feeder = layout == Layout::Feeder;
    let path_of = |n: usize| {
        if feeder {
            format!("0.{}", n - 1)
        } else {
            (n - 1).to_string()
        }
    };
    let mut entries: BTreeMap<usize, &Map<String, Value>> = BTreeMap::new();
    for (key, value) in block {
        let (Some(n), Some(members)) = (wire_number(key), value.as_object()) else {
            continue;
        };
        entries.insert(n, members);
        let id = path_of(n);
        if let Some(material) = members.get("material").filter(|m| !m.is_null()) {
            report(tree, &id, MATERIAL, normalize_material(material));
        }
        if !feeder {
            for (from, to) in SLOT_MEMBERS {
                member(tree, &id, to, members.get(from));
            }
        }
    }
    let active = whole_number(block.get("active"));
    if feeder && !entries.is_empty() {
        let pick = active
            .filter(|a| entries.contains_key(a))
            .or_else(|| entries.keys().min().copied())
            .unwrap_or(0);
        for from in ["fan_print", "fan_hotend"] {
            let to = SLOT_MEMBERS
                .iter()
                .find(|(f, _)| *f == from)
                .map(|(_, t)| *t);
            if let (Some(to), Some(entry)) = (to, entries.get(&pick)) {
                member(tree, "0", to, entry.get(from));
            }
        }
    }
    let active = active?;
    let mut said = BTreeMap::new();
    if feeder {
        said.insert("0".to_owned(), true);
    }
    for n in entries.keys() {
        said.insert(path_of(*n), *n == active);
    }
    if active > 0 {
        said.insert(path_of(active), true);
    }
    Some(said)
}

/// `reportHead` (`telemetrytools.go:184-201`): the nozzle and target at the top of a frame onto one
/// head, and its material when asked; each as carried (a null removes it).
fn report_head(
    tree: &mut BTreeMap<String, Map<String, Value>>,
    fields: &Value,
    head: &str,
    with_material: bool,
) {
    for (key, name) in [
        ("temp_nozzle", NOZZLE_TEMP),
        ("target_nozzle", NOZZLE_TARGET_TEMP),
    ] {
        if let Some(value) = fields.get(key) {
            report(tree, head, name, value.clone());
        }
    }
    if with_material && let Some(material) = fields.get("material") {
        report(tree, head, MATERIAL, normalize_material(material));
    }
}

/// Our telemetry's tool values, read off the tree. The tool printing is the tree's active tool
/// (`ToolTree.Active`, `sdk/connectorapi/tooltree.go:22-24`, from `ActiveToolOf`). Ours, where the
/// server has nothing (it has no "top" of a tool tree):
/// - the nozzle, target and fans at the top are the active tool's head, or the only head's when
///   none is active;
/// - the material at the top is the active tool's (in front of an MMU: the loaded slot), or that
///   head's; no material (`""`) shows as none;
/// - the list of heads only when there is more than one, head `"n"` numbered `n + 1` on the page.
///
/// An empty tree shows no tool values: nothing is carried over from the frame before.
fn show(t: &mut Telemetry) {
    let tree = &t.tool_tree;
    let mut heads: Vec<&String> = tree.keys().filter(|id| !id.contains('.')).collect();
    heads.sort_by(|a, b| compare_ids(a, b));
    let active: BTreeMap<String, bool> = tree
        .iter()
        .filter_map(|(id, m)| Some((id.clone(), m.get(ACTIVE)?.as_bool()?)))
        .collect();
    let active = active_tool_of(&active);
    let shown = active.as_deref().map(head_of).or(match heads.as_slice() {
        [only] => Some(only.as_str()),
        _ => None,
    });
    let number = |id: &str, name: &str| {
        tree.get(id)
            .and_then(|m| m.get(name))
            .and_then(Value::as_f64)
    };
    let head = |name: &str| shown.and_then(|id| number(id, name));
    t.nozzle_c = head(NOZZLE_TEMP).map(|v| v as f32);
    t.nozzle_target_c = head(NOZZLE_TARGET_TEMP).map(|v| v as f32);
    t.print_fan_rpm = head(PRINT_FAN_RPM).map(|v| v as u32);
    t.hotend_fan_rpm = head(HEATBREAK_FAN_RPM).map(|v| v as u32);
    let material_of = |id: &str| {
        tree.get(id)
            .and_then(|m| m.get(MATERIAL))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    t.material = active
        .as_deref()
        .and_then(material_of)
        .or_else(|| shown.and_then(material_of))
        .filter(|m| !m.is_empty());
    t.tools = if heads.len() > 1 {
        heads
            .iter()
            .map(|id| ToolHeat {
                number: id.parse::<u32>().map_or(0, |n| n + 1),
                nozzle_c: number(id, NOZZLE_TEMP).map(|v| v as f32),
                nozzle_target_c: number(id, NOZZLE_TARGET_TEMP).map(|v| v as f32),
                active: tree
                    .get(id.as_str())
                    .and_then(|m| m.get(ACTIVE))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
            .collect()
    } else {
        Vec::new()
    };
}
