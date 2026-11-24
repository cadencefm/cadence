//! Eurorack Modular Rack controller: 3-tab Instrument Deck state (`Panel`, `Patchbay`, `Zones`),
//! 2D Patchbay navigation, module/cable/pin workflows, and `:mod` / `:patch` / `:unpatch` / `:pin` / `:unpin` commands.
use super::*;
use cadence_core::rack::{
    cable_default_amount, compile_zone_graph, format_cable_amount, parse_cable_amount, InPort,
    ModuleKindTag, PanelPin, PatchCable, RackModule, RackPatch, RackZone, MAX_CABLES_PER_ZONE,
    MAX_MODULES_PER_ZONE, MAX_PANEL_PINS, MAX_RESONATORS_PER_ZONE, PATCHBAY_COLUMNS,
};

cadence_core::define_choice_enum! {
    /// Active tab of the Eurorack Instrument Deck (`H`/`L` cycles).
    #[derive(Default)]
    pub enum RackTab [array] {
        #[default]
        Panel => ("panel", "Panel", ["quick", "faceplate", "controls"]),
        Patchbay => ("patchbay", "Patchbay", ["patch", "cables", "graph", "modular"]),
        Zones => ("zones", "Zones", ["zone", "map", "kit", "layers"]),
    }
}

/// Entry in the horizontal Panel module strip (`None` = `"Quick"` / `"Rack"` overview pill).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RackPanelPill {
    pub module_id: Option<u16>,
    pub name: String,
    pub badge: &'static str,
}

/// Build the 4-column module ID layout (`SOURCES | MODULATORS | PROCESSORS | VCA / OUT`)
/// matching `CompiledZoneGraph::column_layout` (or module declaration order if graph is empty).
pub(crate) fn patchbay_columns(zone: &RackZone) -> [Vec<u16>; 4] {
    let mut cols: [Vec<u16>; 4] = Default::default();
    if let Ok(compiled) = compile_zone_graph(zone) {
        for (op, &col) in compiled.ops.iter().zip(compiled.column_layout.iter()) {
            let c = usize::from(col).min(3);
            cols[c].push(op.module_id);
        }
        return cols;
    }
    for m in &zone.modules {
        let c = usize::from(m.kind.tag().column()).min(3);
        cols[c].push(m.id);
    }
    cols
}

/// Build the ordered horizontal Panel pills for `(rack, zone)`.
/// Pill `0` is always the Rack/Quick overview (`module_id: None`, labeled `"Quick"` when pins
/// exist and `"Rack"` when empty) so all 5 Rack-level parameters stay accessible at all times;
/// subsequent pills correspond to every module in `zone.modules`.
pub(crate) fn rack_panel_pills(rack: &RackPatch, zone: &RackZone) -> Vec<RackPanelPill> {
    let mut pills = Vec::with_capacity(zone.modules.len() + 1);
    pills.push(RackPanelPill {
        module_id: None,
        name: if rack.panel_pins.is_empty() {
            "Rack".into()
        } else {
            "Quick".into()
        },
        badge: if rack.panel_pins.is_empty() {
            "RACK"
        } else {
            "PIN"
        },
    });
    for m in &zone.modules {
        pills.push(RackPanelPill {
            module_id: Some(m.id),
            name: m.name.clone(),
            badge: m.kind.tag().short_badge(),
        });
    }
    pills
}

/// All cables touching `module_id` (outgoing first, then incoming) in declaration order.
pub(crate) fn module_attached_cables(zone: &RackZone, module_id: u16) -> Vec<u16> {
    let mut out: Vec<u16> = zone
        .cables
        .iter()
        .filter(|c| c.from_module == module_id)
        .map(|c| c.id)
        .collect();
    for c in &zone.cables {
        if c.to_module == module_id && !out.contains(&c.id) {
            out.push(c.id);
        }
    }
    out
}

/// Unique human-friendly module name inside `zone` (e.g. `VCO`, `VCO 2`, `Filter 2`).
pub(crate) fn next_module_name(zone: &RackZone, kind: ModuleKindTag) -> String {
    let base = kind.default_name();
    if !zone
        .modules
        .iter()
        .any(|m| m.name.eq_ignore_ascii_case(base))
    {
        return base.to_string();
    }
    let mut n = 2usize;
    loop {
        let candidate = format!("{base} {n}");
        if !zone
            .modules
            .iter()
            .any(|m| m.name.eq_ignore_ascii_case(&candidate))
        {
            return candidate;
        }
        n += 1;
    }
}

impl App {
    /// Active zone index for `rack`, clamped cleanly to `0..rack.zones.len()`.
    pub(crate) fn active_zone_index(&self, rack: &RackPatch) -> usize {
        if rack.zones.is_empty() {
            0
        } else {
            self.rack_ui.zone_cursor.min(rack.zones.len() - 1)
        }
    }

    /// Active zone reference for `instrument_id`.
    pub(crate) fn active_rack_zone(&self, instrument_id: Id) -> Option<&RackZone> {
        let inst = self.instrument(instrument_id)?;
        let idx = self.active_zone_index(&inst.rack);
        inst.rack.zones.get(idx)
    }

    /// Currently active Panel pill `(pill_index, Option<module_id>)` for `instrument_id`.
    pub(crate) fn active_panel_pill(&self, instrument_id: Id) -> Option<(usize, Option<u16>)> {
        let inst = self.instrument(instrument_id)?;
        let zone = inst.rack.zones.get(self.active_zone_index(&inst.rack))?;
        let pills = rack_panel_pills(&inst.rack, zone);
        if pills.is_empty() {
            return None;
        }
        let idx = self.rack_ui.panel_slot.min(pills.len() - 1);
        Some((idx, pills[idx].module_id))
    }

    /// Select the Panel pill showing `module_id` (switching to `RackTab::Panel`).
    pub(crate) fn focus_rack_module_panel(&mut self, instrument_id: Id, module_id: u16) {
        let Some(inst) = self.instrument(instrument_id) else {
            return;
        };
        let Some(zone) = inst.rack.zones.get(self.active_zone_index(&inst.rack)) else {
            return;
        };
        let pills = rack_panel_pills(&inst.rack, zone);
        if let Some(pos) = pills.iter().position(|p| p.module_id == Some(module_id)) {
            self.rack_ui.panel_slot = pos;
            self.rack_ui.tab = RackTab::Panel;
            self.control_cursor = 0;
            self.rack_ui.patchbay_cable_id = None;
            let name = pills[pos].name.clone();
            self.status = format!("Panel · {name} · j/k controls · Alt+h/l ,/. adjust · p pin");
        }
    }

    /// Clamp `zone_cursor`, `rack_panel_slot`, and Patchbay selection against `rack`.
    pub(crate) fn clamp_rack_state(&mut self, rack: &RackPatch) {
        self.rack_ui.zone_cursor = rack.clamp_zone_index(self.rack_ui.zone_cursor);
        if let Some(zone) = rack.zones.get(self.rack_ui.zone_cursor) {
            let pills = rack_panel_pills(rack, zone);
            self.rack_ui.panel_slot = self.rack_ui.panel_slot.min(pills.len().saturating_sub(1));
            self.clamp_patchbay_state(zone);
        } else {
            self.rack_ui.panel_slot = 0;
            self.rack_ui.patchbay_col = 0;
            self.rack_ui.patchbay_row = 0;
            self.rack_ui.patchbay_cable_id = None;
        }
    }

    /// Return the currently focused module ID in `rack`'s active zone (from Panel or Patchbay).
    pub(crate) fn focused_rack_module_id(&self, rack: &RackPatch) -> Option<u16> {
        let zone = rack.zones.get(self.active_zone_index(rack))?;
        match self.rack_ui.tab {
            RackTab::Panel => {
                let pills = rack_panel_pills(rack, zone);
                pills
                    .get(self.rack_ui.panel_slot.min(pills.len().saturating_sub(1)))
                    .and_then(|p| p.module_id)
            }
            RackTab::Patchbay => {
                let cols = patchbay_columns(zone);
                let col = self.rack_ui.patchbay_col.min(3);
                cols[col]
                    .get(
                        self.rack_ui
                            .patchbay_row
                            .min(cols[col].len().saturating_sub(1)),
                    )
                    .copied()
            }
            RackTab::Zones => None,
        }
    }

    /// Clamp `patchbay_col`, `patchbay_row`, and `patchbay_cable_id` to `zone`.
    pub(crate) fn clamp_patchbay_state(&mut self, zone: &RackZone) -> Option<u16> {
        let cols = patchbay_columns(zone);
        let mut col = self.rack_ui.patchbay_col.min(3);
        if cols[col].is_empty() {
            if let Some(first) = cols.iter().position(|c| !c.is_empty()) {
                col = first;
            } else {
                self.rack_ui.patchbay_col = 0;
                self.rack_ui.patchbay_row = 0;
                self.rack_ui.patchbay_cable_id = None;
                return None;
            }
        }
        let row = self
            .rack_ui
            .patchbay_row
            .min(cols[col].len().saturating_sub(1));
        self.rack_ui.patchbay_col = col;
        self.rack_ui.patchbay_row = row;
        if let Some(cid) = self.rack_ui.patchbay_cable_id {
            if zone.cable(cid).is_none() {
                self.rack_ui.patchbay_cable_id = None;
            }
        }
        cols[col].get(row).copied()
    }

    /// Move Patchbay cursor onto a specific module ID if present in `zone`.
    pub(crate) fn focus_patchbay_module(&mut self, zone: &RackZone, module_id: u16) {
        let cols = patchbay_columns(zone);
        for (c, col) in cols.iter().enumerate() {
            if let Some(r) = col.iter().position(|&id| id == module_id) {
                self.rack_ui.patchbay_col = c;
                self.rack_ui.patchbay_row = r;
                return;
            }
        }
    }

    /// Read-only lookup of the focused Patchbay module ID for `zone`.
    pub(crate) fn focused_patchbay_module_in(&self, zone: &RackZone) -> Option<u16> {
        let cols = patchbay_columns(zone);
        let mut col = self.rack_ui.patchbay_col.min(3);
        if cols[col].is_empty() {
            col = cols.iter().position(|c| !c.is_empty())?;
        }
        let row = self
            .rack_ui
            .patchbay_row
            .min(cols[col].len().saturating_sub(1));
        cols[col].get(row).copied()
    }

    /// Cycle between `RackTab::{Panel, Patchbay, Zones}` (`H`/`L`).
    pub fn step_rack_tab(&mut self, delta: i8) {
        self.rack_ui.tab = self.rack_ui.tab.cycle(i32::from(delta));
        self.control_cursor = 0;
        self.rack_ui.patchbay_cable_id = None;
        if let Detail::Instrument(id) = self.detail {
            if let Some(zone) = self.active_rack_zone(id).cloned() {
                if self.rack_ui.tab == RackTab::Patchbay {
                    self.clamp_patchbay_state(&zone);
                }
            }
        }
        self.status = match self.rack_ui.tab {
            RackTab::Panel => {
                "Rack · Panel · h/l module · j/k param · Alt+h/l ,/. adjust · a add · x remove · p pin"
                    .into()
            }
            RackTab::Patchbay => {
                "Rack · Patchbay · hjkl move · w/b cable · Alt+h/l ,/. amt · c patch · x cut · Enter panel"
                    .into()
            }
            RackTab::Zones => {
                "Rack · Zones · h/l zone · j/k param · Alt+h/l ,/. adjust · a add · d dup · x remove · B sample"
                    .into()
            }
        };
    }

    /// Cycle horizontal module pills on the `Panel` tab (`h/l` or `[` `]`).
    pub(crate) fn step_rack_panel_pill(&mut self, instrument_id: Id, delta: isize) {
        let Some(inst) = self.instrument(instrument_id) else {
            return;
        };
        let Some(zone) = inst.rack.zones.get(self.active_zone_index(&inst.rack)) else {
            return;
        };
        let pills = rack_panel_pills(&inst.rack, zone);
        if pills.is_empty() {
            return;
        }
        let cur = self.rack_ui.panel_slot.min(pills.len() - 1) as isize;
        let next = (cur + delta).rem_euclid(pills.len() as isize) as usize;
        self.rack_ui.panel_slot = next;
        self.control_cursor = 0;
        let pill = &pills[next];
        self.status = format!(
            "Panel · [{}] {} · j/k param · Alt+h/l ,/. adjust · p pin",
            pill.badge, pill.name
        );
    }

    /// Cycle attached cables on the focused Patchbay module (`[`/`]` or `w`/`b`).
    fn cycle_patchbay_cable(&mut self, instrument_id: Id, delta: isize) {
        let Some(zone) = self.active_rack_zone(instrument_id).cloned() else {
            return;
        };
        let Some(mod_id) = self.clamp_patchbay_state(&zone) else {
            return;
        };
        let attached = module_attached_cables(&zone, mod_id);
        if attached.is_empty() {
            self.rack_ui.patchbay_cable_id = None;
            self.status = "No patch cables attached to this module · press c to patch".into();
            return;
        }
        // Positions: 0 = module focus (`None`), 1..=len = attached cables.
        let total = (attached.len() + 1) as isize;
        let cur = match self.rack_ui.patchbay_cable_id {
            None => 0isize,
            Some(cid) => attached
                .iter()
                .position(|&id| id == cid)
                .map_or(0, |i| (i + 1) as isize),
        };
        let next = (cur + delta).rem_euclid(total) as usize;
        if next == 0 {
            self.rack_ui.patchbay_cable_id = None;
            let name = zone.module(mod_id).map_or("Module", |m| m.name.as_str());
            self.status = format!("Patchbay · {name} · c patch · Enter open panel");
        } else {
            let cid = attached[next - 1];
            self.rack_ui.patchbay_cable_id = Some(cid);
            self.status = self.describe_patchbay_cable(&zone, cid);
        }
    }

    pub(crate) fn describe_patchbay_cable(&self, zone: &RackZone, cable_id: u16) -> String {
        let Some(c) = zone.cable(cable_id) else {
            return format!("Cable c{cable_id}");
        };
        let from_name = zone
            .module(c.from_module)
            .map_or_else(|| format!("m{}", c.from_module), |m| m.name.clone());
        let (to_name, to_kind) = zone.module(c.to_module).map_or_else(
            || (format!("m{}", c.to_module), ModuleKindTag::Out),
            |m| (m.name.clone(), m.kind.tag()),
        );
        let amt = format_cable_amount(to_kind, c.to_port, c.amount);
        let is_fb = compile_zone_graph(zone)
            .ok()
            .and_then(|g| g.cable_index_for_id(cable_id).map(|i| g.cables[i].feedback))
            .unwrap_or(false);
        let fb = if is_fb { " [z-1]" } else { "" };
        format!(
            "Cable c{cable_id} · {from_name} → {to_name}.{} ({amt}){fb} · Alt+h/l or ,/. adjust · c repatch · Enter exact · x cut",
            c.to_port.slug()
        )
    }

    /// `NORMAL`-mode keyboard dispatch for the Eurorack Instrument Deck (`Detail::Instrument(id)`).
    /// Returns `Some(result)` when consumed by Rack UX, `None` to fall through to global keys.
    pub(crate) fn handle_rack_deck_key(
        &mut self,
        instrument_id: Id,
        key: KeyEvent,
        count: usize,
    ) -> Option<Result<Outcome>> {
        match key.code {
            KeyCode::Char('H') => {
                self.step_rack_tab(-1);
                return Some(Ok(Outcome::default()));
            }
            KeyCode::Char('L') => {
                self.step_rack_tab(1);
                return Some(Ok(Outcome::default()));
            }
            KeyCode::Char('A') => {
                return Some(self.audition_instrument_preview(instrument_id));
            }
            KeyCode::Char('o') => {
                return Some(self.open_graph_route_instrument_picker(instrument_id));
            }
            KeyCode::Char('X') => {
                return Some(self.request_delete_instrument(instrument_id));
            }
            KeyCode::Char('r') => {
                return Some(self.rename_focused_rack_item(instrument_id));
            }
            _ => {}
        }
        match self.rack_ui.tab {
            RackTab::Panel => self.handle_rack_panel_key(instrument_id, key, count),
            RackTab::Patchbay => self.handle_rack_patchbay_key(instrument_id, key, count),
            RackTab::Zones => self.handle_rack_zones_key(instrument_id, key, count),
        }
    }

    fn rename_focused_rack_item(&mut self, instrument_id: Id) -> Result<Outcome> {
        match self.rack_ui.tab {
            RackTab::Patchbay => {
                if let Some(zone) = self.active_rack_zone(instrument_id).cloned() {
                    if let Some(mod_id) = self.focused_patchbay_module_in(&zone) {
                        return self.begin_rack_rename_module(instrument_id, zone.id, mod_id);
                    }
                }
                self.begin_workflow(WorkflowKind::RenameInstrument(instrument_id))
            }
            RackTab::Panel => {
                if let Some((_, Some(mod_id))) = self.active_panel_pill(instrument_id) {
                    if let Some(zone_id) = self.active_rack_zone(instrument_id).map(|z| z.id) {
                        return self.begin_rack_rename_module(instrument_id, zone_id, mod_id);
                    }
                }
                if let Some(ctrl) = self.selected_control() {
                    if ctrl.group == "QUICK" {
                        if let ControlTarget::Param(addr) = ctrl.target {
                            if let Some((inst_id, target)) = addr.as_instrument() {
                                if inst_id == instrument_id {
                                    if let Some(pin_idx) =
                                        self.instrument(instrument_id).and_then(|i| {
                                            i.rack
                                                .panel_pins
                                                .iter()
                                                .position(|p| p.target == target)
                                        })
                                    {
                                        return self.begin_rack_rename_pin(instrument_id, pin_idx);
                                    }
                                }
                            }
                        }
                    }
                }
                self.begin_workflow(WorkflowKind::RenameInstrument(instrument_id))
            }
            RackTab::Zones => {
                if let Some(ctrl) = self.selected_control() {
                    if matches!(ctrl.group, "ZONE" | "VOICE") {
                        return self.begin_zone_field_value_input(
                            instrument_id,
                            ZoneControl::Name,
                            String::new(),
                        );
                    }
                }
                self.begin_workflow(WorkflowKind::RenameInstrument(instrument_id))
            }
        }
    }

    fn handle_rack_panel_key(
        &mut self,
        instrument_id: Id,
        key: KeyEvent,
        count: usize,
    ) -> Option<Result<Outcome>> {
        match key.code {
            KeyCode::Char('h' | '[') | KeyCode::Left => {
                self.step_rack_panel_pill(instrument_id, -(count as isize));
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('l' | ']') | KeyCode::Right => {
                self.step_rack_panel_pill(instrument_id, count as isize);
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('a') => Some(self.open_rack_add_module_picker(instrument_id)),
            KeyCode::Char('x') => Some(self.delete_focused_panel_module(instrument_id)),
            KeyCode::Char('p') => Some(self.rack_toggle_pin_focused(instrument_id)),
            _ => None,
        }
    }

    pub(crate) fn rack_patchbay_navigate_edge(&mut self, zone: &RackZone, end: bool) {
        let cols = patchbay_columns(zone);
        let _ = self.clamp_patchbay_state(zone);
        let len = cols[self.rack_ui.patchbay_col].len();
        if len > 0 {
            self.rack_ui.patchbay_row = if end { len - 1 } else { 0 };
            self.rack_ui.patchbay_cable_id = None;
        }
        self.update_patchbay_status(zone);
    }

    pub(crate) fn rack_patchbay_navigate_page(
        &mut self,
        zone: &RackZone,
        forward: bool,
        count: usize,
    ) {
        let cols = patchbay_columns(zone);
        let _ = self.clamp_patchbay_state(zone);
        let len = cols[self.rack_ui.patchbay_col].len();
        let stride = 6usize.saturating_mul(count.max(1));
        if len > 0 {
            self.rack_ui.patchbay_row = if forward {
                self.rack_ui
                    .patchbay_row
                    .saturating_add(stride)
                    .min(len - 1)
            } else {
                self.rack_ui.patchbay_row.saturating_sub(stride)
            };
            self.rack_ui.patchbay_cable_id = None;
        }
        self.update_patchbay_status(zone);
    }

    fn handle_rack_patchbay_key(
        &mut self,
        instrument_id: Id,
        key: KeyEvent,
        count: usize,
    ) -> Option<Result<Outcome>> {
        let zone = self.active_rack_zone(instrument_id)?.clone();
        let cols = patchbay_columns(&zone);
        let focused_mod = self.clamp_patchbay_state(&zone);

        match key.code {
            KeyCode::Char('h') | KeyCode::Left => {
                let mut next_col = self.rack_ui.patchbay_col;
                for step in (0..self.rack_ui.patchbay_col).rev() {
                    if !cols[step].is_empty() {
                        next_col = step;
                        break;
                    }
                }
                if next_col != self.rack_ui.patchbay_col {
                    self.rack_ui.patchbay_col = next_col;
                    self.rack_ui.patchbay_row = self
                        .rack_ui
                        .patchbay_row
                        .min(cols[next_col].len().saturating_sub(1));
                    self.rack_ui.patchbay_cable_id = None;
                }
                self.update_patchbay_status(&zone);
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('l') | KeyCode::Right => {
                let mut next_col = self.rack_ui.patchbay_col;
                for (step, col_vec) in cols.iter().enumerate().skip(self.rack_ui.patchbay_col + 1) {
                    if !col_vec.is_empty() {
                        next_col = step;
                        break;
                    }
                }
                if next_col != self.rack_ui.patchbay_col {
                    self.rack_ui.patchbay_col = next_col;
                    self.rack_ui.patchbay_row = self
                        .rack_ui
                        .patchbay_row
                        .min(cols[next_col].len().saturating_sub(1));
                    self.rack_ui.patchbay_cable_id = None;
                }
                self.update_patchbay_status(&zone);
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('j') | KeyCode::Down => {
                let len = cols[self.rack_ui.patchbay_col].len();
                if len > 0 {
                    let next = (self.rack_ui.patchbay_row + count).min(len - 1);
                    if next != self.rack_ui.patchbay_row {
                        self.rack_ui.patchbay_row = next;
                        self.rack_ui.patchbay_cable_id = None;
                    }
                }
                self.update_patchbay_status(&zone);
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let next = self.rack_ui.patchbay_row.saturating_sub(count);
                if next != self.rack_ui.patchbay_row {
                    self.rack_ui.patchbay_row = next;
                    self.rack_ui.patchbay_cable_id = None;
                }
                self.update_patchbay_status(&zone);
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('[' | 'b') => {
                self.cycle_patchbay_cable(instrument_id, -1);
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char(']' | 'w') => {
                self.cycle_patchbay_cable(instrument_id, 1);
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('a') => Some(self.open_rack_add_module_picker(instrument_id)),
            KeyCode::Char('c') => {
                if let Some(cid) = self.rack_ui.patchbay_cable_id {
                    return Some(self.open_rack_repatch_cable_picker(instrument_id, zone.id, cid));
                }
                let Some(mod_id) = focused_mod else {
                    return Some(Ok(Outcome::default()));
                };
                Some(self.open_rack_connect_cable_picker(instrument_id, zone.id, mod_id))
            }
            KeyCode::Char('C') => {
                if let Some(cid) = self.rack_ui.patchbay_cable_id {
                    let addr = ParamAddress::Cable {
                        instrument: instrument_id,
                        zone: zone.id,
                        cable: cid,
                    };
                    return Some(self.begin_param_value_input(addr, String::new()));
                }
                let Some(mod_id) = focused_mod else {
                    return Some(Ok(Outcome::default()));
                };
                let attached = module_attached_cables(&zone, mod_id);
                if attached.is_empty() {
                    self.status = "No cables attached to this module · press c to patch".into();
                    return Some(Ok(Outcome::default()));
                }
                if attached.len() == 1 {
                    let cid = attached[0];
                    self.rack_ui.patchbay_cable_id = Some(cid);
                    let addr = ParamAddress::Cable {
                        instrument: instrument_id,
                        zone: zone.id,
                        cable: cid,
                    };
                    return Some(self.begin_param_value_input(addr, String::new()));
                }
                Some(self.open_rack_select_cable_picker(instrument_id, zone.id, &attached))
            }
            KeyCode::Char('p') => Some(self.rack_toggle_pin_patchbay_cable(instrument_id, zone.id)),
            KeyCode::Char('x') => {
                if let Some(cid) = self.rack_ui.patchbay_cable_id {
                    self.rack_ui.patchbay_cable_id = None;
                    return Some(self.apply(Action::RackDisconnectCable {
                        instrument: instrument_id,
                        zone: zone.id,
                        cable: cid,
                    }));
                }
                let Some(mod_id) = focused_mod else {
                    return Some(Ok(Outcome::default()));
                };
                Some(self.apply(Action::RackRemoveModule {
                    instrument: instrument_id,
                    zone: zone.id,
                    module: mod_id,
                }))
            }
            KeyCode::Enter => {
                if let Some(cid) = self.rack_ui.patchbay_cable_id {
                    let addr = ParamAddress::Cable {
                        instrument: instrument_id,
                        zone: zone.id,
                        cable: cid,
                    };
                    return Some(self.begin_param_value_input(addr, String::new()));
                }
                if let Some(mod_id) = focused_mod {
                    self.focus_rack_module_panel(instrument_id, mod_id);
                }
                Some(Ok(Outcome::default()))
            }
            _ => None,
        }
    }

    fn handle_rack_zones_key(
        &mut self,
        instrument_id: Id,
        key: KeyEvent,
        count: usize,
    ) -> Option<Result<Outcome>> {
        let zone_count = self
            .instrument(instrument_id)
            .map_or(0, |i| i.rack.zones.len());
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => {
                if zone_count > 0 {
                    let cur = self.rack_ui.zone_cursor.min(zone_count - 1);
                    self.rack_ui.zone_cursor = cur.saturating_sub(count);
                    self.rack_ui.panel_slot = 0;
                    self.rack_ui.patchbay_cable_id = None;
                    let len = self.control_descriptors().len();
                    self.control_cursor = self.control_cursor.min(len.saturating_sub(1));
                    self.update_zone_tab_status(instrument_id);
                }
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('l') | KeyCode::Right => {
                if zone_count > 0 {
                    let cur = self.rack_ui.zone_cursor.min(zone_count - 1);
                    self.rack_ui.zone_cursor = (cur + count).min(zone_count - 1);
                    self.rack_ui.panel_slot = 0;
                    self.rack_ui.patchbay_cable_id = None;
                    let len = self.control_descriptors().len();
                    self.control_cursor = self.control_cursor.min(len.saturating_sub(1));
                    self.update_zone_tab_status(instrument_id);
                }
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('a') => Some(self.open_rack_add_zone_picker(instrument_id)),
            KeyCode::Char('d') => Some(self.apply(Action::ZoneDuplicate {
                instrument: Some(instrument_id),
                name: None,
            })),
            KeyCode::Char('x') => Some(self.delete_active_rack_zone(instrument_id)),
            KeyCode::Char('p') => {
                if matches!(
                    self.selected_control().map(|c| c.target),
                    Some(ControlTarget::Param(_))
                ) {
                    Some(self.rack_toggle_pin_focused(instrument_id))
                } else {
                    self.status = "Focus a parameter row to pin to Quick Panel".into();
                    Some(Ok(Outcome::default()))
                }
            }
            KeyCode::Enter => Some(self.open_current()),
            _ => None,
        }
    }

    fn update_patchbay_status(&mut self, zone: &RackZone) {
        if let Some(cid) = self.rack_ui.patchbay_cable_id {
            self.status = self.describe_patchbay_cable(zone, cid);
            return;
        }
        let col_title = PATCHBAY_COLUMNS[self.rack_ui.patchbay_col.min(3)];
        if let Some(mid) = self.focused_patchbay_module_in(zone) {
            if let Some(m) = zone.module(mid) {
                let attached = module_attached_cables(zone, mid).len();
                self.status = format!(
                    "Patchbay · {col_title} · {} ({}) · {attached} cable{} · c patch · Enter panel",
                    m.name,
                    m.kind.tag().short_badge(),
                    if attached == 1 { "" } else { "s" }
                );
                return;
            }
        }
        self.status = format!("Patchbay · {col_title}");
    }

    fn update_zone_tab_status(&mut self, instrument_id: Id) {
        if let Some(inst) = self.instrument(instrument_id) {
            let idx = self.active_zone_index(&inst.rack);
            if let Some(z) = inst.rack.zones.get(idx) {
                self.status = format!(
                    "Zone {}/{}: {} ({}–{}, root {}) · h/l zone · j/k ctrl · Alt+h/l adjust · a add · d dup · x del",
                    idx + 1,
                    inst.rack.zones.len(),
                    z.name,
                    cadence_core::music::note_name(z.low_midi),
                    cadence_core::music::note_name(z.high_midi),
                    cadence_core::music::note_name(z.root_midi),
                );
            }
        }
    }

    /// Nudge the currently selected Patchbay cable amount (`Alt+h/l`, `,/.`, etc.).
    pub(crate) fn nudge_patchbay_cable(
        &mut self,
        instrument_id: Id,
        direction: i8,
        coarse: bool,
        coalesce: bool,
    ) -> Result<Outcome> {
        let Some(cable_id) = self.rack_ui.patchbay_cable_id else {
            self.status = "Select a cable with w/b to adjust its amount".into();
            return Ok(Outcome::default());
        };
        let Some(zone_id) = self.active_rack_zone(instrument_id).map(|z| z.id) else {
            return Ok(Outcome::default());
        };
        let addr = ParamAddress::Cable {
            instrument: instrument_id,
            zone: zone_id,
            cable: cable_id,
        };
        self.nudge_param(addr, direction, coarse, coalesce)
    }

    /// Reset the currently selected Patchbay cable back to its port default (`Alt+Backspace`).
    pub(crate) fn reset_patchbay_cable(&mut self, instrument_id: Id) -> Result<Outcome> {
        let Some(cable_id) = self.rack_ui.patchbay_cable_id else {
            return Ok(Outcome::default());
        };
        let Some(zone_id) = self.active_rack_zone(instrument_id).map(|z| z.id) else {
            return Ok(Outcome::default());
        };
        self.reset_param_default(ParamAddress::Cable {
            instrument: instrument_id,
            zone: zone_id,
            cable: cable_id,
        })
    }

    /// Toggle pinning the selected Patchbay cable to the Quick Panel (`p` on Patchbay).
    fn rack_toggle_pin_patchbay_cable(
        &mut self,
        instrument_id: Id,
        zone_id: u16,
    ) -> Result<Outcome> {
        let Some(cable_id) = self.rack_ui.patchbay_cable_id else {
            self.status = "Select a cable with w/b to pin to Quick Panel".into();
            return Ok(Outcome::default());
        };
        let target = InstrumentParam::Cable {
            zone: zone_id,
            cable: cable_id,
        };
        let inst = self
            .instrument(instrument_id)
            .context("Instrument missing")?;
        if let Some(pin_idx) = inst
            .rack
            .panel_pins
            .iter()
            .position(|pin| pin.target == target)
        {
            return self.apply(Action::RackUnpinControl {
                instrument: instrument_id,
                index: pin_idx,
            });
        }
        ensure!(
            inst.rack.panel_pins.len() < MAX_PANEL_PINS,
            "Quick Panel already has the maximum {MAX_PANEL_PINS} pins"
        );
        let label = self.default_pin_label(&inst.rack, target);
        self.apply(Action::RackPinControl {
            instrument: instrument_id,
            pin: PanelPin { target, label },
        })
    }

    /// Reorder the active Quick Panel pin (`Alt+j/k` when focused on the Quick pill).
    pub(crate) fn reorder_panel_pin(&mut self, instrument_id: Id, delta: isize) -> Result<Outcome> {
        let Some((_, None)) = self.active_panel_pill(instrument_id) else {
            return Ok(Outcome::default());
        };
        let Some(ControlTarget::Param(addr)) = self.selected_control().map(|c| c.target) else {
            return Ok(Outcome::default());
        };
        let Some((inst_id, target)) = addr.as_instrument() else {
            return Ok(Outcome::default());
        };
        if inst_id != instrument_id {
            return Ok(Outcome::default());
        }
        let inst = self
            .instrument(instrument_id)
            .context("Instrument missing")?;
        let Some(pos) = inst
            .rack
            .panel_pins
            .iter()
            .position(|pin| pin.target == target)
        else {
            return Ok(Outcome::default());
        };
        let len = inst.rack.panel_pins.len();
        let next = (pos as isize + delta).clamp(0, len.saturating_sub(1) as isize) as usize;
        if next == pos {
            return Ok(Outcome::default());
        }
        let pin = inst.rack.panel_pins[pos].clone();
        let label = format!("Moved pin '{}' → #{}", pin.label, next + 1);
        let out = self.commit(
            vec![Edit::PinPanelControl {
                instrument: instrument_id,
                pin,
                index: Some(next),
            }],
            label,
        )?;
        // Track the moved pin in `control_cursor`.
        if let Some(new_idx) = self
            .control_descriptors()
            .iter()
            .position(|c| matches!(c.target, ControlTarget::Param(a) if a == addr))
        {
            self.control_cursor = new_idx;
        }
        Ok(out)
    }

    /// Reorder the active Rack zone (`Alt+j/k` on the Zones tab).
    pub(crate) fn reorder_rack_zone(&mut self, instrument_id: Id, delta: isize) -> Result<Outcome> {
        let inst = self
            .instrument(instrument_id)
            .context("Instrument missing")?;
        let len = inst.rack.zones.len();
        if len <= 1 {
            return Ok(Outcome::default());
        }
        let cur = self.active_zone_index(&inst.rack);
        let next = (cur as isize + delta).clamp(0, len.saturating_sub(1) as isize) as usize;
        if next == cur {
            return Ok(Outcome::default());
        }
        let zone_id = inst.rack.zones[cur].id;
        let zname = inst.rack.zones[cur].name.clone();
        let out = self.commit(
            vec![Edit::ReorderRackZone {
                instrument: instrument_id,
                zone: zone_id,
                target_index: next,
            }],
            format!("Moved zone '{zname}' → #{}", next + 1),
        )?;
        self.rack_ui.zone_cursor = next;
        Ok(out)
    }

    fn delete_focused_panel_module(&mut self, instrument_id: Id) -> Result<Outcome> {
        let Some((_, module_opt)) = self.active_panel_pill(instrument_id) else {
            return Ok(Outcome::default());
        };
        let Some(module_id) = module_opt else {
            self.status =
                "Switch to a module pill (h/l) to delete a module · p unpins focused control"
                    .into();
            return Ok(Outcome::default());
        };
        let zone_id = self
            .active_rack_zone(instrument_id)
            .context("Active zone missing")?
            .id;
        self.apply(Action::RackRemoveModule {
            instrument: instrument_id,
            zone: zone_id,
            module: module_id,
        })
    }

    /// Pin or unpin the currently focused `ControlTarget::Param` on the active Rack.
    pub(crate) fn rack_toggle_pin_focused(&mut self, instrument_id: Id) -> Result<Outcome> {
        let Some(ctrl) = self.control_descriptors().get(self.control_cursor).cloned() else {
            bail!("Focus a parameter control to pin or unpin");
        };
        let ControlTarget::Param(addr) = ctrl.target else {
            bail!("Only Rack, Zone, Module, or Cable parameters can be pinned to Quick Panel");
        };
        let Some((inst_id, target)) = addr.as_instrument() else {
            bail!("Focused control does not belong to this Rack");
        };
        ensure!(
            inst_id == instrument_id,
            "Focused control belongs to another instrument"
        );
        let inst = self
            .instrument(instrument_id)
            .context("Instrument missing")?;
        if let Some(pin_idx) = inst
            .rack
            .panel_pins
            .iter()
            .position(|pin| pin.target == target)
        {
            return self.apply(Action::RackUnpinControl {
                instrument: instrument_id,
                index: pin_idx,
            });
        }
        ensure!(
            inst.rack.panel_pins.len() < MAX_PANEL_PINS,
            "Quick Panel already has the maximum {MAX_PANEL_PINS} pins"
        );
        let label = self.default_pin_label(&inst.rack, target);
        self.apply(Action::RackPinControl {
            instrument: instrument_id,
            pin: PanelPin { target, label },
        })
    }

    pub(crate) fn default_pin_label(&self, rack: &RackPatch, target: InstrumentParam) -> String {
        match target {
            InstrumentParam::Rack { param } => format!("Rack {}", param.label()),
            InstrumentParam::Zone { zone, param } => {
                if rack.zones.len() > 1 {
                    let zname = rack
                        .zone(zone)
                        .map_or_else(|| format!("z{zone}"), |z| z.name.clone());
                    format!("{zname} {}", param.label())
                } else {
                    param.label().into()
                }
            }
            InstrumentParam::Module {
                zone,
                module,
                param,
            } => {
                let mname = rack
                    .zone(zone)
                    .and_then(|z| z.module(module))
                    .map_or_else(|| format!("m{module}"), |m| m.name.clone());
                format!("{mname} {}", param.label())
            }
            InstrumentParam::Cable { zone, cable } => {
                if let Some(z) = rack.zone(zone) {
                    if let Some(c) = z.cable(cable) {
                        let from = z
                            .module(c.from_module)
                            .map_or_else(|| format!("m{}", c.from_module), |m| m.name.clone());
                        let to = z
                            .module(c.to_module)
                            .map_or_else(|| format!("m{}", c.to_module), |m| m.name.clone());
                        return format!("{from}→{to}.{}", c.to_port.short_label());
                    }
                }
                format!("Cable c{cable}")
            }
        }
    }

    pub(crate) fn delete_active_rack_zone(&mut self, instrument_id: Id) -> Result<Outcome> {
        let inst = self
            .instrument(instrument_id)
            .context("Instrument missing")?;
        ensure!(
            inst.rack.zones.len() > 1,
            "Cannot remove the last zone in a Rack"
        );
        let idx = self.active_zone_index(&inst.rack);
        let zone = &inst.rack.zones[idx];
        let zone_id = zone.id;
        let zname = zone.name.clone();
        let out = self.commit(
            vec![Edit::RemoveRackZone {
                instrument: instrument_id,
                zone: zone_id,
            }],
            format!("Removed zone '{zname}' (z{zone_id}) · u restores"),
        )?;
        let new_len = self
            .instrument(instrument_id)
            .map_or(1, |i| i.rack.zones.len());
        self.rack_ui.zone_cursor = idx.min(new_len.saturating_sub(1));
        self.rack_ui.panel_slot = 0;
        self.rack_ui.patchbay_cable_id = None;
        Ok(out)
    }

    // --- Action handlers for modular authoring ---

    pub(super) fn apply_rack_add_module(
        &mut self,
        instrument: Id,
        zone_id: u16,
        kind: ModuleKindTag,
        name: Option<String>,
    ) -> Result<Outcome> {
        ensure!(
            kind != ModuleKindTag::Out,
            "Each Rack zone already has a single mandatory Out module"
        );
        let inst = self.instrument(instrument).context("Instrument missing")?;
        let zone = inst
            .rack
            .zone(zone_id)
            .with_context(|| format!("Zone z{zone_id} missing"))?;
        ensure!(
            zone.modules.len() < MAX_MODULES_PER_ZONE,
            "Zone '{}' already has the maximum {MAX_MODULES_PER_ZONE} modules",
            zone.name
        );
        if kind == ModuleKindTag::Resonator {
            let res_count = zone
                .modules
                .iter()
                .filter(|m| m.kind.tag() == ModuleKindTag::Resonator)
                .count();
            ensure!(
                res_count < MAX_RESONATORS_PER_ZONE,
                "Zone '{}' already has the maximum {MAX_RESONATORS_PER_ZONE} Resonator modules",
                zone.name
            );
        }
        let mod_id = zone.next_module_id();
        let resolved_name = name
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| next_module_name(zone, kind));
        let module = RackModule {
            id: mod_id,
            name: resolved_name.clone(),
            kind: kind.default_kind(),
        };
        let label = format!("Added {} '{}' (m{mod_id})", kind.label(), resolved_name);
        let out = self.commit(
            vec![Edit::AddRackModule {
                instrument,
                zone: zone_id,
                module,
            }],
            label,
        )?;
        if let Some(updated_zone) = self
            .instrument(instrument)
            .and_then(|i| i.rack.zone(zone_id))
            .cloned()
        {
            self.focus_patchbay_module(&updated_zone, mod_id);
            if self.rack_ui.tab == RackTab::Panel {
                let rack = &self
                    .instrument(instrument)
                    .context("Instrument missing")?
                    .rack;
                let pills = rack_panel_pills(rack, &updated_zone);
                if let Some(pos) = pills.iter().position(|p| p.module_id == Some(mod_id)) {
                    self.rack_ui.panel_slot = pos;
                    self.control_cursor = 0;
                }
            }
        }
        Ok(out)
    }

    pub(super) fn apply_rack_remove_module(
        &mut self,
        instrument: Id,
        zone_id: u16,
        module_id: u16,
    ) -> Result<Outcome> {
        let inst = self.instrument(instrument).context("Instrument missing")?;
        let zone = inst
            .rack
            .zone(zone_id)
            .with_context(|| format!("Zone z{zone_id} missing"))?;
        let m = zone
            .module(module_id)
            .with_context(|| format!("Module m{module_id} missing"))?;
        ensure!(
            m.kind.tag() != ModuleKindTag::Out,
            "Cannot delete the mandatory Out module"
        );
        let name = m.name.clone();
        let out = self.commit(
            vec![Edit::RemoveRackModule {
                instrument,
                zone: zone_id,
                module: module_id,
            }],
            format!("Removed module '{name}' (m{module_id}) · u restores"),
        )?;
        if let Some(updated_zone) = self
            .instrument(instrument)
            .and_then(|i| i.rack.zone(zone_id))
            .cloned()
        {
            self.clamp_patchbay_state(&updated_zone);
            let rack = &self
                .instrument(instrument)
                .context("Instrument missing")?
                .rack;
            let pills = rack_panel_pills(rack, &updated_zone);
            self.rack_ui.panel_slot = self.rack_ui.panel_slot.min(pills.len().saturating_sub(1));
            self.control_cursor = 0;
        }
        Ok(out)
    }

    pub(super) fn apply_rack_rename_module(
        &mut self,
        instrument: Id,
        zone_id: u16,
        module_id: u16,
        name: String,
    ) -> Result<Outcome> {
        let trimmed = name.trim().to_string();
        ensure!(!trimmed.is_empty(), "Module name cannot be empty");
        let label = format!("Renamed module m{module_id} → '{trimmed}'");
        self.commit(
            vec![Edit::RenameRackModule {
                instrument,
                zone: zone_id,
                module: module_id,
                name: trimmed,
            }],
            label,
        )
    }

    pub(super) fn apply_rack_connect_cable(
        &mut self,
        instrument: Id,
        zone_id: u16,
        from_module: u16,
        to_module: u16,
        to_port: InPort,
        amount: f32,
    ) -> Result<Outcome> {
        let inst = self.instrument(instrument).context("Instrument missing")?;
        let zone = inst
            .rack
            .zone(zone_id)
            .with_context(|| format!("Zone z{zone_id} missing"))?;
        ensure!(
            zone.cables.len() < MAX_CABLES_PER_ZONE,
            "Zone '{}' already has the maximum {MAX_CABLES_PER_ZONE} cables",
            zone.name
        );
        let from_m = zone
            .module(from_module)
            .with_context(|| format!("Source module m{from_module} missing"))?;
        ensure!(
            from_m.kind.tag().has_output(),
            "Module '{}' ({}) has no patchable output",
            from_m.name,
            from_m.kind.tag().label()
        );
        let to_m = zone
            .module(to_module)
            .with_context(|| format!("Target module m{to_module} missing"))?;
        ensure!(
            to_m.kind.tag().accepts_in_port(to_port),
            "Module '{}' ({}) does not accept port '{}'",
            to_m.name,
            to_m.kind.tag().label(),
            to_port.slug()
        );
        let cable_id = zone.next_cable_id();
        let cable = PatchCable::new(cable_id, from_module, to_module, to_port, amount);
        let amt_str = format_cable_amount(to_m.kind.tag(), to_port, amount);
        let label = format!(
            "Patched {} → {}.{} ({amt_str})",
            from_m.name,
            to_m.name,
            to_port.slug()
        );
        let out = self.commit(
            vec![Edit::ConnectCable {
                instrument,
                zone: zone_id,
                cable,
            }],
            label,
        )?;
        self.rack_ui.patchbay_cable_id = Some(cable_id);
        Ok(out)
    }

    pub(super) fn apply_rack_disconnect_cable(
        &mut self,
        instrument: Id,
        zone_id: u16,
        cable_id: u16,
    ) -> Result<Outcome> {
        let out = self.commit(
            vec![Edit::DisconnectCable {
                instrument,
                zone: zone_id,
                cable: cable_id,
            }],
            format!("Cut cable c{cable_id} · u restores"),
        )?;
        if self.rack_ui.patchbay_cable_id == Some(cable_id) {
            self.rack_ui.patchbay_cable_id = None;
        }
        Ok(out)
    }

    /// Re-patch an existing cable's destination `(to_module, to_port)` in place, preserving its
    /// `cable_id` and every Quick Panel pin, Modulation, Part link, and Macro binding targeting it.
    pub(crate) fn apply_rack_repatch_cable(
        &mut self,
        instrument: Id,
        zone_id: u16,
        cable_id: u16,
        to_module: u16,
        to_port: InPort,
    ) -> Result<Outcome> {
        let mut rack = self
            .instrument(instrument)
            .context("Instrument missing")?
            .rack
            .clone();
        let zone = rack
            .zone_mut(zone_id)
            .with_context(|| format!("Zone z{zone_id} missing"))?;
        let existing = *zone
            .cable(cable_id)
            .with_context(|| format!("Cable c{cable_id} missing"))?;
        let from_module = existing.from_module;
        ensure!(
            !zone.cables.iter().any(|c| {
                c.id != cable_id
                    && c.from_module == from_module
                    && c.to_module == to_module
                    && c.to_port == to_port
            }),
            "Cable from m{from_module} → m{to_module}.{} already exists",
            to_port.slug()
        );
        let to_kind = zone
            .module(to_module)
            .with_context(|| format!("Target module m{to_module} missing"))?
            .kind
            .tag();
        let old_to_kind = zone.module(existing.to_module).map(|m| m.kind.tag());
        let (lo, hi) = cadence_core::rack::cable_amount_range(to_kind, to_port);
        let next_amount = if (lo..=hi).contains(&existing.amount)
            && ((existing.to_port == to_port && old_to_kind == Some(to_kind))
                || (existing.to_port == InPort::In) == (to_port == InPort::In))
        {
            existing.amount
        } else {
            cable_default_amount(to_kind, to_port)
        };
        let from_name = zone
            .module(from_module)
            .map_or_else(|| format!("m{from_module}"), |m| m.name.clone());
        let to_name = zone
            .module(to_module)
            .map_or_else(|| format!("m{to_module}"), |m| m.name.clone());
        if let Some(c) = zone.cable_mut(cable_id) {
            c.to_module = to_module;
            c.to_port = to_port;
            c.amount = next_amount;
        }
        rack.validate()?;
        let out = self.commit(
            vec![Edit::UpdateRackPatch {
                id: instrument,
                rack,
            }],
            format!(
                "Re-patched c{cable_id} · {from_name} → {to_name}.{} ({})",
                to_port.slug(),
                format_cable_amount(to_kind, to_port, next_amount)
            ),
        )?;
        self.rack_ui.patchbay_cable_id = Some(cable_id);
        Ok(out)
    }

    pub(super) fn apply_rack_pin_control(
        &mut self,
        instrument: Id,
        pin: PanelPin,
    ) -> Result<Outcome> {
        let display = pin.label.clone();
        self.commit(
            vec![Edit::PinPanelControl {
                instrument,
                pin,
                index: None,
            }],
            format!("Pinned '{display}' to Quick Panel"),
        )
    }

    pub(super) fn apply_rack_unpin_control(
        &mut self,
        instrument: Id,
        index: usize,
    ) -> Result<Outcome> {
        let out = self.commit(
            vec![Edit::UnpinPanelControl { instrument, index }],
            format!(
                "Unpinned control #{} from Quick Panel · u restores",
                index + 1
            ),
        )?;
        if let Some(inst) = self.instrument(instrument) {
            if let Some(zone) = inst.rack.zones.get(self.active_zone_index(&inst.rack)) {
                let pills = rack_panel_pills(&inst.rack, zone);
                self.rack_ui.panel_slot =
                    self.rack_ui.panel_slot.min(pills.len().saturating_sub(1));
                self.control_cursor = 0;
            }
        }
        Ok(out)
    }

    // --- Ex commands: `:mod add|del|rename`, `:patch`, `:unpatch`, `:pin`, `:unpin` ---

    pub(crate) fn require_active_rack_and_zone(&self) -> Result<(Id, u16)> {
        let inst_id = self
            .active_instrument()
            .or(self.selected_instrument)
            .context("Select or open a Rack instrument first")?;
        let inst = self.instrument(inst_id).context("Instrument missing")?;
        let zone = inst
            .rack
            .zones
            .get(self.active_zone_index(&inst.rack))
            .context("Rack has no zones")?;
        Ok((inst_id, zone.id))
    }

    pub(crate) fn rack_mod_subcommand(&mut self, args: &[String]) -> Result<Outcome> {
        let sub = args[0].to_ascii_lowercase();
        let (inst_id, zone_id) = self.require_active_rack_and_zone()?;
        match sub.as_str() {
            "add" => {
                ensure!(
                    args.len() >= 2,
                    "Usage: :mod add <vco|sampler|noise|env|lfo|filter|vca|drive|lofi|resonator> [NAME]"
                );
                let kind = ModuleKindTag::parse(&args[1]).with_context(|| {
                    "Unknown module kind · use vco, sampler, noise, env, lfo, filter, vca, drive, lofi, or resonator"
                })?;
                let name = if args.len() >= 3 {
                    Some(args[2..].join(" "))
                } else {
                    None
                };
                self.apply(Action::RackAddModule {
                    instrument: inst_id,
                    zone: zone_id,
                    kind,
                    name,
                })
            }
            "del" | "rm" | "remove" => {
                ensure!(args.len() == 2, "Usage: :mod del <ID_OR_NAME>");
                let zone = self
                    .instrument(inst_id)
                    .and_then(|i| i.rack.zone(zone_id))
                    .context("Zone missing")?;
                let mod_id = zone
                    .resolve_module_id(&args[1])
                    .with_context(|| format!("Unknown module '{}'", args[1]))?;
                self.apply(Action::RackRemoveModule {
                    instrument: inst_id,
                    zone: zone_id,
                    module: mod_id,
                })
            }
            "rename" => {
                ensure!(
                    args.len() >= 3,
                    "Usage: :mod rename <ID_OR_NAME> <NEW_NAME>"
                );
                let zone = self
                    .instrument(inst_id)
                    .and_then(|i| i.rack.zone(zone_id))
                    .context("Zone missing")?;
                let mod_id = zone
                    .resolve_module_id(&args[1])
                    .with_context(|| format!("Unknown module '{}'", args[1]))?;
                let new_name = args[2..].join(" ");
                self.apply(Action::RackRenameModule {
                    instrument: inst_id,
                    zone: zone_id,
                    module: mod_id,
                    name: new_name,
                })
            }
            _ => bail!(
                "Usage: :mod add <kind> [name] | :mod del <id|name> | :mod rename <id|name> <new_name>"
            ),
        }
    }

    pub(crate) fn patch_command(&mut self, args: &[String]) -> Result<Outcome> {
        ensure!(
            (2..=3).contains(&args.len()),
            "Usage: :patch <FROM> <TO[.PORT]> [AMOUNT]"
        );
        let (inst_id, zone_id) = self.require_active_rack_and_zone()?;
        let zone = self
            .instrument(inst_id)
            .and_then(|i| i.rack.zone(zone_id))
            .context("Zone missing")?
            .clone();
        let from_id = zone
            .resolve_module_id(&args[0])
            .with_context(|| format!("Unknown source module '{}'", args[0]))?;
        let from_mod = zone.module(from_id).context("Source module missing")?;
        let (to_tok, explicit_port) = match args[1].rsplit_once('.') {
            Some((m, p)) => (m, Some(p)),
            None => (args[1].as_str(), None),
        };
        let to_id = zone
            .resolve_module_id(to_tok)
            .with_context(|| format!("Unknown target module '{to_tok}'"))?;
        let to_mod = zone.module(to_id).context("Target module missing")?;
        let to_port = match explicit_port {
            Some(p) => InPort::parse(p).with_context(|| {
                format!("Unknown input port '{p}' (use in, cv, pitch, fm, pw, trig)")
            })?,
            None => to_mod
                .kind
                .tag()
                .default_in_port(from_mod.kind.tag())
                .with_context(|| {
                    format!(
                        "Module '{}' ({}) has no input ports",
                        to_mod.name,
                        to_mod.kind.tag().label()
                    )
                })?,
        };
        let amount = match args.get(2) {
            Some(raw) => parse_cable_amount(to_mod.kind.tag(), to_port, raw)?,
            None => cable_default_amount(to_mod.kind.tag(), to_port),
        };
        self.apply(Action::RackConnectCable {
            instrument: inst_id,
            zone: zone_id,
            from_module: from_id,
            to_module: to_id,
            to_port,
            amount,
        })
    }

    pub(crate) fn unpatch_command(&mut self, args: &[String]) -> Result<Outcome> {
        ensure!(
            (1..=2).contains(&args.len()),
            "Usage: :unpatch <CABLE_ID | FROM TO[.PORT]>"
        );
        let (inst_id, zone_id) = self.require_active_rack_and_zone()?;
        let zone = self
            .instrument(inst_id)
            .and_then(|i| i.rack.zone(zone_id))
            .context("Zone missing")?
            .clone();
        let cable_id = if args.len() == 1 {
            let trimmed = args[0].trim().trim_start_matches(['c', 'C', '#']);
            let id = trimmed
                .parse::<u16>()
                .with_context(|| format!("Expected cable ID (e.g. c1), got '{}'", args[0]))?;
            ensure!(
                zone.cable(id).is_some(),
                "Cable c{id} not found in active zone"
            );
            id
        } else {
            let from_id = zone
                .resolve_module_id(&args[0])
                .with_context(|| format!("Unknown source module '{}'", args[0]))?;
            let (to_tok, explicit_port) = match args[1].rsplit_once('.') {
                Some((m, p)) => (m, Some(p)),
                None => (args[1].as_str(), None),
            };
            let to_id = zone
                .resolve_module_id(to_tok)
                .with_context(|| format!("Unknown target module '{to_tok}'"))?;
            let port_filter = explicit_port
                .map(|p| InPort::parse(p).with_context(|| format!("Unknown input port '{p}'")))
                .transpose()?;
            zone.cables
                .iter()
                .find(|c| {
                    c.from_module == from_id
                        && c.to_module == to_id
                        && port_filter.is_none_or(|pf| c.to_port == pf)
                })
                .map(|c| c.id)
                .with_context(|| format!("No cable matching '{}' → '{}'", args[0], args[1]))?
        };
        self.apply(Action::RackDisconnectCable {
            instrument: inst_id,
            zone: zone_id,
            cable: cable_id,
        })
    }

    pub(crate) fn pin_command(&mut self, args: &[String]) -> Result<Outcome> {
        let (inst_id, _) = self.require_active_rack_and_zone()?;
        if args.is_empty() {
            return self.rack_toggle_pin_focused(inst_id);
        }
        let inst = self.instrument(inst_id).context("Instrument missing")?;
        if let Some(target) = InstrumentParam::parse_for_rack(&args[0], &inst.rack, false) {
            let label = if args.len() >= 2 {
                args[1..].join(" ")
            } else {
                self.default_pin_label(&inst.rack, target)
            };
            if let Some(existing) = inst.rack.panel_pins.iter().position(|p| p.target == target) {
                return self.apply(Action::RackUnpinControl {
                    instrument: inst_id,
                    index: existing,
                });
            }
            return self.apply(Action::RackPinControl {
                instrument: inst_id,
                pin: PanelPin { target, label },
            });
        }
        // Otherwise treat args as a custom label for the currently focused control!
        let ctrl = self
            .control_descriptors()
            .get(self.control_cursor)
            .cloned()
            .context("Focus a Rack parameter or pass :pin <PARAM> [LABEL]")?;
        let ControlTarget::Param(addr) = ctrl.target else {
            bail!("Focus a Rack parameter or pass :pin <PARAM> [LABEL]");
        };
        let Some((id, target)) = addr.as_instrument() else {
            bail!("Focused control does not belong to a Rack");
        };
        let label = args.join(" ");
        self.apply(Action::RackPinControl {
            instrument: id,
            pin: PanelPin { target, label },
        })
    }

    pub(crate) fn unpin_command(&mut self, args: &[String]) -> Result<Outcome> {
        let (inst_id, _) = self.require_active_rack_and_zone()?;
        let inst = self.instrument(inst_id).context("Instrument missing")?;
        ensure!(
            !inst.rack.panel_pins.is_empty(),
            "Quick Panel has no pinned controls"
        );
        let index = if args.is_empty() {
            if let Some(ControlTarget::Param(addr)) = self
                .control_descriptors()
                .get(self.control_cursor)
                .map(|c| c.target.clone())
            {
                if let Some((id, target)) = addr.as_instrument() {
                    if id == inst_id {
                        if let Some(idx) =
                            inst.rack.panel_pins.iter().position(|p| p.target == target)
                        {
                            return self.apply(Action::RackUnpinControl {
                                instrument: inst_id,
                                index: idx,
                            });
                        }
                    }
                }
            }
            inst.rack.panel_pins.len() - 1
        } else {
            let tok = args[0].trim();
            if let Ok(one_based) = tok.parse::<usize>() {
                one_based
                    .checked_sub(1)
                    .filter(|&i| i < inst.rack.panel_pins.len())
                    .with_context(|| {
                        format!("Pin index must be 1..={}", inst.rack.panel_pins.len())
                    })?
            } else if let Some(target) = InstrumentParam::parse_for_rack(tok, &inst.rack, false) {
                inst.rack
                    .panel_pins
                    .iter()
                    .position(|p| p.target == target)
                    .with_context(|| format!("Parameter '{tok}' is not pinned"))?
            } else {
                inst.rack
                    .panel_pins
                    .iter()
                    .position(|p| p.label.eq_ignore_ascii_case(tok))
                    .with_context(|| format!("No pinned control matching '{tok}'"))?
            }
        };
        self.apply(Action::RackUnpinControl {
            instrument: inst_id,
            index,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bounce::{BounceScope, BounceTarget};
    use crate::view::{DeckView, InstrumentVisualView, ModuleVisualView};
    use cadence_core::presets::InstrumentPreset;
    use cadence_core::rack::{
        module_sample_key, DriveModule, EnvModule, FilterModule, InPort, LfoModule, LoFiModule,
        ModuleKind, NoiseModule, OutModule, PanelPin, PatchCable, RackModule, RackPatch,
        RackTemplate, ResonatorModule, SamplerModule, VcaModule, VcoModule,
    };
    use cadence_core::runtime::AudioSnapshot;
    use cadence_core::sampler::SampleLoopMode;

    /// Build a minimal valid 16-bit mono PCM WAV with embedded RIFF `smpl` and `inst` chunks.
    fn write_synthetic_wav_with_riff_meta(
        path: &Path,
        frames: usize,
        root_midi: u8,
        low_midi: u8,
        high_midi: u8,
        loop_start: u32,
        loop_end: u32,
    ) {
        let mut chunks: Vec<u8> = Vec::new();
        // `fmt ` chunk (16 bytes: PCM, mono, 48000 Hz, 16-bit)
        chunks.extend_from_slice(b"fmt ");
        chunks.extend_from_slice(&16u32.to_le_bytes());
        chunks.extend_from_slice(&1u16.to_le_bytes()); // PCM
        chunks.extend_from_slice(&1u16.to_le_bytes()); // mono
        chunks.extend_from_slice(&48_000u32.to_le_bytes());
        chunks.extend_from_slice(&(48_000u32 * 2).to_le_bytes());
        chunks.extend_from_slice(&2u16.to_le_bytes());
        chunks.extend_from_slice(&16u16.to_le_bytes());

        // `data` chunk (sine burst)
        let data_bytes = (frames * 2) as u32;
        chunks.extend_from_slice(b"data");
        chunks.extend_from_slice(&data_bytes.to_le_bytes());
        for i in 0..frames {
            let s = ((i as f32 * 0.05).sin() * 12_000.0) as i16;
            chunks.extend_from_slice(&s.to_le_bytes());
        }

        // `smpl` chunk: 36-byte header + 1 x 24-byte loop = 60 bytes
        chunks.extend_from_slice(b"smpl");
        chunks.extend_from_slice(&60u32.to_le_bytes());
        chunks.extend_from_slice(&0u32.to_le_bytes()); // manufacturer
        chunks.extend_from_slice(&0u32.to_le_bytes()); // product
        chunks.extend_from_slice(&(1_000_000_000u32 / 48_000).to_le_bytes()); // sample period
        chunks.extend_from_slice(&u32::from(root_midi).to_le_bytes()); // unity note
        chunks.extend_from_slice(&0u32.to_le_bytes()); // pitch fraction
        chunks.extend_from_slice(&0u32.to_le_bytes()); // smpte format
        chunks.extend_from_slice(&0u32.to_le_bytes()); // smpte offset
        chunks.extend_from_slice(&1u32.to_le_bytes()); // num loops
        chunks.extend_from_slice(&0u32.to_le_bytes()); // sampler data
                                                       // Loop 0: forward loop
        chunks.extend_from_slice(&0u32.to_le_bytes()); // cue id
        chunks.extend_from_slice(&0u32.to_le_bytes()); // type = forward
        chunks.extend_from_slice(&loop_start.to_le_bytes());
        chunks.extend_from_slice(&loop_end.to_le_bytes());
        chunks.extend_from_slice(&0u32.to_le_bytes()); // fraction
        chunks.extend_from_slice(&0u32.to_le_bytes()); // play count

        // `inst` chunk: 7 bytes + 1 pad byte (even chunk size)
        chunks.extend_from_slice(b"inst");
        chunks.extend_from_slice(&7u32.to_le_bytes());
        chunks.push(root_midi); // unshifted note
        chunks.push(0); // fine tune
        chunks.push(0); // gain
        chunks.push(low_midi);
        chunks.push(high_midi);
        chunks.push(1); // low vel
        chunks.push(127); // high vel
        chunks.push(0); // RIFF word alignment pad

        let mut wav = Vec::with_capacity(12 + chunks.len());
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&((4 + chunks.len()) as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(&chunks);
        std::fs::write(path, wav).unwrap();
    }

    fn instrument_rack_view(app: &App) -> crate::view::RackDeckView {
        let snap = AudioSnapshot {
            sample_rate: 48_000,
            ..AudioSnapshot::default()
        };
        let fv = app.frame_view(&snap, "silent");
        fv.decks
            .into_iter()
            .find_map(|d| match d.view {
                DeckView::Instrument(inst) => match inst.visual {
                    InstrumentVisualView::Rack(r) => Some(*r),
                    InstrumentVisualView::Missing => None,
                },
                _ => None,
            })
            .expect("RackDeckView must be visible")
    }

    #[test]
    fn rack_creation_and_modular_ex_commands_roundtrip_with_undo() {
        let mut app = App::new(Project::demo(), "rack-ex.json".into()).unwrap();

        // 1. `:new synth|sampler|kit|rack|modular [name]` create unified Rack instruments.
        for (cmd, expected_name, tmpl) in [
            ("new synth LeadX", "LeadX", RackTemplate::Synth),
            ("new sampler SmpX", "SmpX", RackTemplate::Sampler),
            ("new kit KitX", "KitX", RackTemplate::Kit),
            ("new rack RackX", "RackX", RackTemplate::Modular),
            ("new modular ModX", "ModX", RackTemplate::Modular),
        ] {
            app.execute_command(cmd).unwrap();
            let inst = app
                .project()
                .instruments
                .iter()
                .find(|i| i.name == expected_name)
                .expect("created Rack instrument");
            assert_eq!(inst.rack, RackPatch::from_template(tmpl));
        }

        // Focus `LeadX` for modular authoring commands.
        app.execute_command("instrument LeadX").unwrap();
        let lead_id = app.active_instrument().unwrap();
        let initial_mods = app.instrument(lead_id).unwrap().rack.zones[0].modules.len();

        // 2. `:mod add|rename|del` + Undo / Redo.
        app.execute_command("mod add vco \"Sub Osc\"").unwrap();
        assert_eq!(
            app.instrument(lead_id).unwrap().rack.zones[0].modules.len(),
            initial_mods + 1
        );
        app.execute_command("mod rename \"Sub Osc\" \"Harm Osc\"")
            .unwrap();
        assert!(app.instrument(lead_id).unwrap().rack.zones[0]
            .modules
            .iter()
            .any(|m| m.name == "Harm Osc"));
        app.execute_command("mod add drive").unwrap();
        app.execute_command("mod add lofi").unwrap();
        app.execute_command("mod del \"Harm Osc\"").unwrap();
        assert!(!app.instrument(lead_id).unwrap().rack.zones[0]
            .modules
            .iter()
            .any(|m| m.name == "Harm Osc"));
        app.apply(Action::Undo).unwrap();
        assert!(app.instrument(lead_id).unwrap().rack.zones[0]
            .modules
            .iter()
            .any(|m| m.name == "Harm Osc"));

        // 3. `:patch` & `:unpatch` including feedback cable using ASCII `"z-1"` (never `"z⁻¹"`).
        // Default Synth zone has VCO (#1) -> Filter (#2). Patching Filter -> VCO.fm_in creates feedback!
        app.execute_command("patch 2 1.fm_in 50%").unwrap();
        let rv = instrument_rack_view(&app);
        assert!(rv.patchbay.has_feedback);
        assert_eq!(rv.patchbay.feedback_badge, "z-1");
        let fb_cable = rv
            .patchbay
            .cables
            .iter()
            .find(|c| c.from_module == 2 && c.to_module == 1 && c.to_port == InPort::FmIn)
            .expect("feedback cable must appear in PatchbayDeckView");
        assert!(fb_cable.feedback);
        assert!(
            fb_cable.label.contains("z-1"),
            "feedback label must use ASCII z-1, got {:?}",
            fb_cable.label
        );
        assert!(!fb_cable.label.contains("z⁻¹"));
        let fb_id = fb_cable.cable_id;

        // `:unpatch` by cable ID and by FROM TO.
        app.execute_command(&format!("unpatch c{fb_id}")).unwrap();
        assert!(app.instrument(lead_id).unwrap().rack.zones[0]
            .cable(fb_id)
            .is_none());
        app.execute_command("patch 2 1.fm_in 35%").unwrap();
        app.execute_command("unpatch 2 1.fm_in").unwrap();
        assert!(!instrument_rack_view(&app).patchbay.has_feedback);

        // 4. `:pin` & `:unpin`.
        let before_pins = app.instrument(lead_id).unwrap().rack.panel_pins.len();
        app.execute_command("pin z1.m1.tune \"Osc Pitch\"").unwrap();
        assert_eq!(
            app.instrument(lead_id).unwrap().rack.panel_pins.len(),
            before_pins + 1
        );
        app.execute_command("unpin \"Osc Pitch\"").unwrap();
        assert_eq!(
            app.instrument(lead_id).unwrap().rack.panel_pins.len(),
            before_pins
        );

        // 5. `:zone` full parity on ANY Rack instrument!
        app.execute_command("zone add Kick 36").unwrap();
        assert_eq!(app.instrument(lead_id).unwrap().rack.zones.len(), 2);
        app.execute_command("zone range 36 40 36").unwrap();
        app.execute_command("zone vel 10% 90%").unwrap();
        app.execute_command("zone choke 1").unwrap();
        app.execute_command("zone name \"Sub Kick\"").unwrap();
        app.execute_command("zone set gain -3dB").unwrap();
        app.execute_command("zone set pan R25").unwrap();
        let z = &app.instrument(lead_id).unwrap().rack.zones[1];
        assert_eq!(z.name, "Sub Kick");
        assert_eq!((z.low_midi, z.high_midi, z.root_midi), (36, 40, 36));
        assert!((z.low_vel - 0.1).abs() < 1e-3 && (z.high_vel - 0.9).abs() < 1e-3);
        assert_eq!(z.choke_group, Some(1));
        assert!((z.gain_db - (-3.0)).abs() < 0.05);
        assert!((z.pan - 0.25).abs() < 0.02);

        app.execute_command("zone dup Snare").unwrap();
        assert_eq!(app.instrument(lead_id).unwrap().rack.zones.len(), 3);
        app.execute_command("zone del").unwrap();
        assert_eq!(app.instrument(lead_id).unwrap().rack.zones.len(), 2);
    }

    #[test]
    fn rack_deck_three_tab_keyboard_ux_panel_patchbay_zones() {
        let mut app = App::new(Project::demo(), "rack-ux.json".into()).unwrap();
        let inst_id = app.project().instruments[0].id;
        app.open_instrument(inst_id).unwrap();
        assert_eq!(app.rack_ui.tab, RackTab::Panel);

        // H / L switch tabs: Panel -> Patchbay -> Zones -> Panel.
        app.press_keys("L").unwrap();
        assert_eq!(app.rack_ui.tab, RackTab::Patchbay);
        app.press_keys("L").unwrap();
        assert_eq!(app.rack_ui.tab, RackTab::Zones);
        app.press_keys("H").unwrap();
        assert_eq!(app.rack_ui.tab, RackTab::Patchbay);
        app.press_keys("H").unwrap();
        assert_eq!(app.rack_ui.tab, RackTab::Panel);

        // Panel tab: h/l cycles module pills; p toggles pin; a opens Add Module picker -> Enter adds module.
        let slot0 = app.rack_ui.panel_slot;
        app.press_keys("l").unwrap();
        assert_eq!(app.rack_ui.panel_slot, slot0 + 1);
        let rv_panel = instrument_rack_view(&app);
        assert!(matches!(rv_panel.visual, ModuleVisualView::Vco { .. }));

        // `p` toggles pin on focused VCO control.
        let pins_before = app.instrument(inst_id).unwrap().rack.panel_pins.len();
        app.press_keys("p").unwrap();
        let pins_after = app.instrument(inst_id).unwrap().rack.panel_pins.len();
        assert_ne!(pins_before, pins_after);

        // `a` opens Add Module picker -> pick item 1 (VCO) with Enter!
        let mods_before = app.instrument(inst_id).unwrap().rack.zones[0].modules.len();
        app.press_keys("a").unwrap();
        assert_eq!(app.input.mode, Mode::Pick);
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.input.mode, Mode::Normal);
        assert_eq!(
            app.instrument(inst_id).unwrap().rack.zones[0].modules.len(),
            mods_before + 1
        );

        // Switch to Patchbay with `L`: navigate columns with `h/l`, patch with `c` -> picker -> ValueInput -> Enter.
        app.press_keys("L").unwrap();
        assert_eq!(app.rack_ui.tab, RackTab::Patchbay);
        // Move to column 0 (SOURCES) so focused module has output jack.
        app.press_keys("hhhh").unwrap();
        let cables_before = app.instrument(inst_id).unwrap().rack.zones[0].cables.len();
        let rev_before_cancel = app.doc.revision();

        // Cancel cable workflow at ValueInput step with Esc -> ZERO undo / revision change!
        app.press_keys("c").unwrap();
        assert_eq!(app.input.mode, Mode::Pick);
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.input.mode, Mode::ValueInput);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.input.mode, Mode::Normal);
        assert_eq!(app.doc.revision(), rev_before_cancel);
        assert_eq!(
            app.instrument(inst_id).unwrap().rack.zones[0].cables.len(),
            cables_before
        );

        // Commit cable workflow: `c` -> Enter (choose target) -> type "75%" -> Enter!
        app.press_keys("c").unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.input.mode, Mode::ValueInput);
        app.press_keys("75%").unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.input.mode, Mode::Normal);
        assert_eq!(
            app.instrument(inst_id).unwrap().rack.zones[0].cables.len(),
            cables_before + 1
        );

        // Newly patched cable is automatically selected; cycling with `]` steps through attached cables!
        let new_cid = app
            .rack_ui
            .patchbay_cable_id
            .expect("newly patched cable selected");
        app.press_keys("]").unwrap(); // wraps back to module focus (None)
        assert_eq!(app.rack_ui.patchbay_cable_id, None);
        app.press_keys("]").unwrap(); // selects first attached cable
        let sel_cid = app
            .rack_ui
            .patchbay_cable_id
            .expect("attached cable selected");
        assert!(app.instrument(inst_id).unwrap().rack.zones[0]
            .cable(new_cid)
            .is_some());
        let amt_before = app.instrument(inst_id).unwrap().rack.zones[0]
            .cable(sel_cid)
            .unwrap()
            .amount;
        app.press_keys("<m-l>").unwrap();
        assert_eq!(app.input.mode, Mode::Normal);
        let amt_after = app.instrument(inst_id).unwrap().rack.zones[0]
            .cable(sel_cid)
            .unwrap()
            .amount;
        assert_ne!(amt_before, amt_after);
        app.press_keys("x").unwrap();
        assert!(app.instrument(inst_id).unwrap().rack.zones[0]
            .cable(sel_cid)
            .is_none());

        // Enter on focused module in Patchbay (no cable selected) jumps straight to its Panel!
        app.rack_ui.patchbay_cable_id = None;
        let focused_pb_mod = instrument_rack_view(&app).patchbay.selected_module.unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.rack_ui.tab, RackTab::Panel);
        let rv_after_jump = instrument_rack_view(&app);
        assert!(rv_after_jump
            .modules
            .iter()
            .any(|p| p.active && p.module_id == Some(focused_pb_mod)));

        // Zones tab: `a` opens `"Add Zone…"` picker (`Enter` adds Blank Sampler), `h`/`l` selects active zone synced to Panel/Patchbay, `x` deletes zone.
        app.press_keys("LL").unwrap();
        assert_eq!(app.rack_ui.tab, RackTab::Zones);
        app.press_keys("a").unwrap();
        assert_eq!(app.mode_name(), "PICK");
        app.press_keys("<Enter>").unwrap();
        assert_eq!(app.instrument(inst_id).unwrap().rack.zones.len(), 2);
        let z2_id = app.instrument(inst_id).unwrap().rack.zones[1].id;
        assert_eq!(instrument_rack_view(&app).active_zone_id, z2_id);
        app.press_keys("h").unwrap();
        let z1_id = app.instrument(inst_id).unwrap().rack.zones[0].id;
        assert_eq!(instrument_rack_view(&app).active_zone_id, z1_id);
        app.press_keys("l").unwrap();
        assert_eq!(instrument_rack_view(&app).active_zone_id, z2_id);
        app.press_keys("x").unwrap();
        assert_eq!(app.instrument(inst_id).unwrap().rack.zones.len(), 1);
    }

    #[test]
    fn sample_browser_loads_wav_with_riff_metadata_and_auto_adds_sampler_module() {
        let dir = tempfile::tempdir().unwrap();
        let samples_dir = dir.path().join("samples");
        std::fs::create_dir_all(&samples_dir).unwrap();
        let wav_path = samples_dir.join("pad_c3.wav");
        write_synthetic_wav_with_riff_meta(&wav_path, 1024, 48, 44, 52, 100, 400);

        let proj_path = dir.path().join("demo.json");
        let mut app = App::new(Project::demo(), proj_path).unwrap();
        // Start with a fresh Synth Rack (zero Sampler modules!).
        app.execute_command("new synth SynthToSample").unwrap();
        let inst_id = app.active_instrument().unwrap();
        assert!(app.instrument(inst_id).unwrap().rack.zones[0]
            .modules
            .iter()
            .all(|m| !matches!(m.kind, ModuleKind::Sampler(_))));

        // Assign via `:sample` (shared SPOT with Sample Browser `B`!)
        app.execute_command("sample samples/pad_c3.wav").unwrap();

        let inst = app.instrument(inst_id).unwrap();
        let zone = &inst.rack.zones[0];
        let smp_mod = zone
            .modules
            .iter()
            .find(|m| matches!(m.kind, ModuleKind::Sampler(_)))
            .expect("Sampler module must be auto-added");
        let ModuleKind::Sampler(s) = &smp_mod.kind else {
            unreachable!()
        };
        assert_eq!(s.path.as_deref(), Some("samples/pad_c3.wav"));
        assert_eq!(zone.root_midi, 48);
        assert_eq!(zone.low_midi, 44);
        assert_eq!(zone.high_midi, 52);
        assert_eq!(s.loop_mode, SampleLoopMode::Forward);
        assert_eq!(s.loop_start, 100);
        // RIFF `smpl` dwEnd is inclusive (400 -> exclusive frame 401).
        assert_eq!(s.loop_end, 401);
        assert!(app
            .assets
            .samples
            .contains_key(&module_sample_key(inst_id, zone.id, smp_mod.id)));
        // Auto-wired Sampler -> Out.In
        let out_id = zone.out_module().unwrap().id;
        assert!(zone.cables.iter().any(|c| c.from_module == smp_mod.id
            && c.to_module == out_id
            && c.to_port == InPort::In));

        // Focus Sampler module in Panel -> ModuleVisualView::Sampler has non-empty peaks & loop!
        app.focus_rack_module_panel(inst_id, smp_mod.id);
        let rv = instrument_rack_view(&app);
        match rv.visual {
            ModuleVisualView::Sampler {
                peaks,
                loop_mode,
                frames,
                ..
            } => {
                assert_eq!(frames, 1024);
                assert!(!peaks.is_empty());
                assert_eq!(loop_mode, SampleLoopMode::Forward);
            }
            other => panic!("expected Sampler visual, got {other:?}"),
        }
    }

    #[test]
    fn preset_browser_unified_rack_categories_bare_defaults_and_zero_undo_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let presets_dir = dir.path().join("presets");
        std::fs::create_dir_all(&presets_dir).unwrap();

        InstrumentPreset::new(
            "Acid Modular",
            Some("Modular".into()),
            RackPatch::acid_303(),
        )
        .save(&presets_dir.join("acid-modular.preset.json"))
        .unwrap();
        InstrumentPreset::new("Test Kit", Some("Kit".into()), RackPatch::default_kit())
            .save(&presets_dir.join("test-kit.preset.json"))
            .unwrap();

        let proj_path = dir.path().join("demo.json");
        let mut app = App::new(Project::demo(), proj_path).unwrap();
        let inst_id = app.project().instruments[0].id;
        let orig_rack = app.instrument(inst_id).unwrap().rack.clone();
        app.open_instrument(inst_id).unwrap();
        assert!(!app.dirty());
        let orig_rev = app.doc.revision();

        // Open `P` Preset Browser.
        app.press_keys("P").unwrap();
        let pb = app.preset_browser_view().expect("PresetBrowserView open");
        assert_eq!(pb.engine_label, "RACK");
        assert_eq!(pb.active_category, "All");
        assert_eq!(pb.categories.len(), 6);
        for bare_name in ["Init Synth", "Blank Sampler", "Blank Kit", "Blank Modular"] {
            assert!(
                pb.items.iter().any(|i| i.name == bare_name),
                "missing bare default '{bare_name}'"
            );
        }

        // Cycle category right with `l` -> Synth -> Sampler -> Drums -> Kit -> Modular.
        app.press_keys("llll").unwrap();
        let pb_kit = app.preset_browser_view().unwrap();
        assert_eq!(pb_kit.active_category, "Kit");
        assert!(pb_kit.items.iter().any(|i| i.name == "Test Kit"));

        app.press_keys("l").unwrap();
        let pb_mod = app.preset_browser_view().unwrap();
        assert_eq!(pb_mod.active_category, "Modular");
        assert!(pb_mod.items.iter().any(|i| i.name == "Acid Modular"));

        // Move to Acid Modular (live preview updates active instrument patch) then Esc cancels cleanly!
        let target_idx = pb_mod
            .items
            .iter()
            .position(|i| i.name == "Acid Modular")
            .unwrap();
        app.press_keys("g").unwrap();
        for _ in 0..target_idx {
            app.press_keys("j").unwrap();
        }
        assert_eq!(app.instrument(inst_id).unwrap().rack, RackPatch::acid_303());

        // Esc restores exact original patch with ZERO undo entries and clean dirty flag!
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.input.mode, Mode::Normal);
        assert_eq!(app.instrument(inst_id).unwrap().rack, orig_rack);
        assert!(!app.dirty());
        assert_eq!(app.doc.revision(), orig_rev);

        // Re-open `P`, filter to Modular, commit Acid Modular with Enter -> ONE undo restores orig!
        app.press_keys("Plllll").unwrap();
        let pb2 = app.preset_browser_view().unwrap();
        let idx2 = pb2
            .items
            .iter()
            .position(|i| i.name == "Acid Modular")
            .unwrap();
        app.press_keys("g").unwrap();
        for _ in 0..idx2 {
            app.press_keys("j").unwrap();
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.instrument(inst_id).unwrap().rack, RackPatch::acid_303());
        app.apply(Action::Undo).unwrap();
        assert_eq!(app.instrument(inst_id).unwrap().rack, orig_rack);
    }

    #[test]
    fn bounce_into_rack_zone_and_new_sampler_rack() {
        let dir = tempfile::tempdir().unwrap();
        let proj_path = dir.path().join("bounce.json");
        let mut app = App::new(Project::demo(), proj_path).unwrap();
        let inst_id = app.project().instruments[0].id;

        // 1. Bounce into existing Rack zone (auto-adds Sampler module!)
        let out = app
            .apply(Action::Bounce {
                scope: Some(BounceScope::Range {
                    start_bar: 1,
                    end_bar: 1,
                }),
                name: Some("Lead Resample".into()),
                create_part: false,
                target: Some(BounceTarget::RackZone {
                    instrument: inst_id,
                    zone_index: Some(0),
                    midi: None,
                    zone_name: None,
                }),
            })
            .unwrap();
        let req = out.bounce.expect("BounceRequest emitted");
        std::fs::create_dir_all(req.output_path.parent().unwrap()).unwrap();
        write_synthetic_wav_with_riff_meta(&req.output_path, 512, 60, 0, 127, 0, 0);
        app.complete_bounce(&req, 0.25).unwrap();
        let z0 = &app.instrument(inst_id).unwrap().rack.zones[0];
        let smp_mod = z0
            .modules
            .iter()
            .find(|m| matches!(m.kind, ModuleKind::Sampler(_)))
            .expect("Sampler added to target Rack zone");
        assert!(app
            .assets
            .samples
            .contains_key(&module_sample_key(inst_id, z0.id, smp_mod.id)));

        // 2. Bounce into brand-new Sampler Rack (`BounceTarget::NewSampler`).
        let before_count = app.project().instruments.len();
        let out2 = app
            .apply(Action::Bounce {
                scope: Some(BounceScope::Range {
                    start_bar: 1,
                    end_bar: 1,
                }),
                name: Some("Fresh Loop".into()),
                create_part: false,
                target: Some(BounceTarget::NewSampler),
            })
            .unwrap();
        let req2 = out2.bounce.expect("BounceRequest for NewSampler");
        write_synthetic_wav_with_riff_meta(&req2.output_path, 512, 60, 0, 127, 0, 0);
        app.complete_bounce(&req2, 0.25).unwrap();
        assert_eq!(app.project().instruments.len(), before_count + 1);
        let new_inst = app.project().instruments.last().unwrap();
        let new_z = &new_inst.rack.zones[0];
        let new_smp = new_z
            .modules
            .iter()
            .find(|m| matches!(m.kind, ModuleKind::Sampler(_)))
            .unwrap();
        assert!(app.assets.samples.contains_key(&module_sample_key(
            new_inst.id,
            new_z.id,
            new_smp.id
        )));
    }

    #[test]
    fn all_twelve_module_visual_views_produce_finite_dsp_curves() {
        let mut rack = RackPatch::default_synth();
        let z = &mut rack.zones[0];
        z.modules = vec![
            RackModule::new(1, "VCO", ModuleKind::Vco(VcoModule::default())),
            RackModule::new(2, "Sampler", ModuleKind::Sampler(SamplerModule::default())),
            RackModule::new(3, "Noise", ModuleKind::Noise(NoiseModule::default())),
            RackModule::new(4, "Env", ModuleKind::Env(EnvModule::default())),
            RackModule::new(5, "LFO", ModuleKind::Lfo(LfoModule::default())),
            RackModule::new(6, "Filter", ModuleKind::Filter(FilterModule::default())),
            RackModule::new(7, "VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(8, "Drive", ModuleKind::Drive(DriveModule::default())),
            RackModule::new(9, "LoFi", ModuleKind::LoFi(LoFiModule::default())),
            RackModule::new(
                10,
                "Resonator",
                ModuleKind::Resonator(ResonatorModule::default()),
            ),
            RackModule::new(11, "Out", ModuleKind::Out(OutModule::default())),
        ];
        z.cables = vec![
            PatchCable::new(1, 1, 6, InPort::In, 1.0),
            PatchCable::new(2, 6, 7, InPort::In, 1.0),
            PatchCable::new(3, 1, 7, InPort::Cv, 0.8), // Audio-rate RingMod CV!
            PatchCable::new(4, 7, 11, InPort::In, 1.0),
        ];
        rack.panel_pins = vec![PanelPin::module(1, 6, ModuleParam::Cutoff, "Cutoff")];

        let mut proj = Project::demo();
        let inst_id = proj.instruments[0].id;
        proj.apply(Edit::UpdateRackPatch { id: inst_id, rack })
            .unwrap();
        let mut app = App::new(proj, "all-vis.json".into()).unwrap();
        app.open_instrument(inst_id).unwrap();

        let mut seen = [false; 12];
        for slot in 0..12 {
            app.rack_ui.panel_slot = slot;
            let rv = instrument_rack_view(&app);
            match rv.visual {
                ModuleVisualView::PinnedOverview {
                    osc_curve,
                    filter_curve,
                    env_curve,
                    ..
                } => {
                    assert!(
                        !osc_curve.is_empty() && !filter_curve.is_empty() && !env_curve.is_empty()
                    );
                    seen[0] = true;
                }
                ModuleVisualView::Vco { curve, .. } => {
                    assert!(curve.iter().all(|v| v.is_finite()));
                    seen[1] = true;
                }
                ModuleVisualView::Sampler { .. } => {
                    seen[2] = true;
                }
                ModuleVisualView::Noise { band_curve, .. } => {
                    assert!(!band_curve.is_empty() && band_curve.iter().all(|v| v.is_finite()));
                    seen[3] = true;
                }
                ModuleVisualView::Env { curve, .. } => {
                    assert!(!curve.is_empty());
                    seen[4] = true;
                }
                ModuleVisualView::Lfo { curve, .. } => {
                    assert!(!curve.is_empty() && curve.iter().all(|v| v.is_finite()));
                    seen[5] = true;
                }
                ModuleVisualView::Filter { curve, .. } => {
                    assert!(!curve.is_empty() && curve.iter().all(|v| v.is_finite()));
                    seen[6] = true;
                }
                ModuleVisualView::Vca {
                    ring_mod,
                    transfer_curve,
                    ..
                } => {
                    assert!(ring_mod);
                    assert!(
                        !transfer_curve.is_empty() && transfer_curve.iter().all(|v| v.is_finite())
                    );
                    seen[7] = true;
                }
                ModuleVisualView::Drive { transfer_curve, .. } => {
                    assert!(
                        !transfer_curve.is_empty() && transfer_curve.iter().all(|v| v.is_finite())
                    );
                    seen[8] = true;
                }
                ModuleVisualView::LoFi { transfer_curve, .. } => {
                    assert!(
                        !transfer_curve.is_empty() && transfer_curve.iter().all(|v| v.is_finite())
                    );
                    seen[9] = true;
                }
                ModuleVisualView::Resonator { modes, .. } => {
                    assert!(
                        !modes.is_empty()
                            && modes
                                .iter()
                                .all(|(x, h, d)| x.is_finite() && h.is_finite() && d.is_finite())
                    );
                    seen[10] = true;
                }
                ModuleVisualView::Out { voice_summary, .. } => {
                    assert!(!voice_summary.is_empty());
                    seen[11] = true;
                }
            }
        }
        assert!(
            seen.iter().all(|&s| s),
            "all 12 ModuleVisualView variants must be covered: {seen:?}"
        );
    }

    #[test]
    fn zones_tab_enter_opens_sample_name_output_and_l_switches_to_panel() {
        let mut app = App::new(Project::demo(), "zones-enter.json".into()).unwrap();
        let inst_id = app.project().instruments[0].id;
        let expected_ch = app.instrument(inst_id).unwrap().channel;
        app.open_instrument(inst_id).unwrap();
        app.rack_ui.tab = RackTab::Zones;

        // 1. Enter on ZoneControl::SamplePath opens Sample Browser (`<add Sampler> (Enter)`).
        let sample_idx = app
            .control_descriptors()
            .iter()
            .position(|c| {
                matches!(
                    c.target,
                    ControlTarget::Zone {
                        field: ZoneControl::SamplePath,
                        ..
                    }
                )
            })
            .expect("SamplePath control present on Zones tab");
        app.control_cursor = sample_idx;
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(
            app.input.mode,
            Mode::Browser,
            "Enter on SamplePath must open Sample Browser"
        );
        assert_eq!(app.rack_ui.tab, RackTab::Zones);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.input.mode, Mode::Normal);

        // 2. Enter on ZoneControl::Name opens inline ValueInput rename and commits cleanly.
        let name_idx = app
            .control_descriptors()
            .iter()
            .position(|c| {
                matches!(
                    c.target,
                    ControlTarget::Zone {
                        field: ZoneControl::Name,
                        ..
                    }
                )
            })
            .expect("Name control present on Zones tab");
        app.control_cursor = name_idx;
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(
            app.input.mode,
            Mode::ValueInput,
            "Enter on ZoneControl::Name must open inline rename"
        );
        app.press_keys("LeadZone").unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.input.mode, Mode::Normal);
        assert_eq!(app.rack_ui.tab, RackTab::Zones);
        assert_eq!(
            app.instrument(inst_id).unwrap().rack.zones[0].name,
            "LeadZone"
        );

        // 3. Enter on ControlTarget::Output opens the routed mixer channel detail.
        let out_idx = app
            .control_descriptors()
            .iter()
            .position(|c| matches!(c.target, ControlTarget::Output(_)))
            .expect("Output routing control present on Zones tab");
        app.control_cursor = out_idx;
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(
            app.detail,
            Detail::Channel(ChannelRef::Channel(expected_ch)),
            "Enter on Output must open routed channel detail"
        );

        // 4. Zones tab opens directly on Zone Name (zone selection lives on the horizontal pill
        //    strip via h/l), and L switches cleanly from Zones back to Panel.
        app.open_instrument(inst_id).unwrap();
        app.rack_ui.tab = RackTab::Zones;
        let first_ctrl = app.control_descriptors();
        assert!(
            matches!(
                first_ctrl.first().map(|c| &c.target),
                Some(ControlTarget::Zone {
                    field: ZoneControl::Name,
                    ..
                })
            ),
            "Zones tab must start directly on Zone Name without a redundant Zone selector row"
        );
        app.handle_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT))
            .unwrap();
        assert_eq!(
            app.rack_ui.tab,
            RackTab::Panel,
            "L from Zones tab must switch to Panel tab"
        );

        // 5. Invalid bounce zone error advertises 1-based z1 (never stale z0).
        let err = app
            .execute_command("bounce zone 1 bogus_zone_xyz")
            .expect_err("bogus bounce zone must fail");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("use z1") && !msg.contains("z0"),
            "bounce zone error must advertise 1-based z1, got: {msg}"
        );
    }

    #[test]
    fn rack_zones_crud_add_picker_voice_params_root_param_and_reorder() {
        let dir = tempfile::tempdir().unwrap();
        let samples_dir = dir.path().join("samples");
        std::fs::create_dir_all(&samples_dir).unwrap();
        let kick_wav = samples_dir.join("kick_c2.wav");
        let hat_wav = samples_dir.join("hat_f#2.wav");
        write_synthetic_wav_with_riff_meta(&kick_wav, 512, 36, 36, 36, 0, 0);
        write_synthetic_wav_with_riff_meta(&hat_wav, 512, 42, 42, 42, 0, 0);

        let proj_path = dir.path().join("zones.json");
        let lib = cadence_core::sampler::SampleLibrary::from_roots(
            std::slice::from_ref(&samples_dir),
            Some(&proj_path),
        );
        let mut app = App::new_with_library(Project::demo(), proj_path, lib).unwrap();
        let inst_id = app.doc.project().instruments[0].id;
        app.open_instrument(inst_id).unwrap();
        app.press_keys("LL").unwrap();
        assert_eq!(app.rack_ui.tab, RackTab::Zones);

        // 1. VOICE params + Root Note are exposed directly as first-class ControlTarget::Param!
        let ctrls = app.control_descriptors();
        for expected_param in [
            ZoneParam::RootMidi,
            ZoneParam::VoiceMode,
            ZoneParam::Legato,
            ZoneParam::Glide,
            ZoneParam::PitchTracking,
            ZoneParam::PhaseReset,
        ] {
            assert!(
                ctrls.iter().any(|c| matches!(
                    c.target,
                    ControlTarget::Param(ParamAddress::Zone { param, .. }) if param == expected_param
                )),
                "Zones tab must expose ZoneParam::{expected_param:?} as ControlTarget::Param"
            );
        }
        // Nudge RootMidi via ControlTarget::Param and reset via Alt+Backspace.
        let root_idx = ctrls
            .iter()
            .position(|c| {
                matches!(
                    c.target,
                    ControlTarget::Param(ParamAddress::Zone {
                        param: ZoneParam::RootMidi,
                        ..
                    })
                )
            })
            .unwrap();
        app.control_cursor = root_idx;
        let orig_root = app.instrument(inst_id).unwrap().rack.zones[0].root_midi;
        app.press_keys(".").unwrap();
        assert_eq!(
            app.instrument(inst_id).unwrap().rack.zones[0].root_midi,
            orig_root + 1
        );
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT))
            .unwrap();
        assert_eq!(app.instrument(inst_id).unwrap().rack.zones[0].root_midi, 60);

        // 2. `a` opens `"Add Zone…"` picker supporting Blank Sampler (1), Synth Pad (2), Blank Modular (3),
        //    Sample Browser (4), From Preset (5), Load Preset into Active Zone (6), Import Folder (7).
        app.press_keys("a").unwrap();
        assert_eq!(app.mode_name(), "PICK");
        assert_eq!(app.picker_view().unwrap().items.len(), 7);
        app.press_keys("2").unwrap(); // Synth Pad
        assert_eq!(app.instrument(inst_id).unwrap().rack.zones.len(), 2);
        app.press_keys("a3").unwrap(); // Blank Modular
        assert_eq!(app.instrument(inst_id).unwrap().rack.zones.len(), 3);
        assert_eq!(
            app.instrument(inst_id).unwrap().rack.zones[2].modules.len(),
            1,
            "Blank Modular zone starts with only Out module"
        );

        // From Preset (New Zone) via option 5
        app.press_keys("a5<Enter>").unwrap();
        assert_eq!(app.instrument(inst_id).unwrap().rack.zones.len(), 4);

        // Sample Browser via option 4 adds a sample-loaded zone
        app.press_keys("a4").unwrap();
        assert_eq!(app.mode_name(), "SAMPLES");
        app.press_keys("<Enter>").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while app.mode_name() == "SAMPLES" && std::time::Instant::now() < deadline {
            app.poll_browser_updates().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(app.mode_name(), "NORMAL");
        assert_eq!(app.instrument(inst_id).unwrap().rack.zones.len(), 5);

        // 3. `d` duplicates active zone right next to it, and `Alt+k` / `Alt+j` reorders zones.
        let z_before_dup = app.instrument(inst_id).unwrap().rack.zones[app.rack_ui.zone_cursor].id;
        app.press_keys("d").unwrap();
        assert_eq!(app.instrument(inst_id).unwrap().rack.zones.len(), 6);
        let dup_idx = app.rack_ui.zone_cursor;
        let dup_id = app.instrument(inst_id).unwrap().rack.zones[dup_idx].id;
        assert_ne!(dup_id, z_before_dup);
        app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::ALT))
            .unwrap();
        assert_eq!(app.rack_ui.zone_cursor, dup_idx - 1);
        assert_eq!(
            app.instrument(inst_id).unwrap().rack.zones[dup_idx - 1].id,
            dup_id
        );

        // 4. Import Sample Folder via option 7 + `apply_workflow_sample_folder` + `F` shortcut!
        app.press_keys("a7").unwrap();
        assert_eq!(app.mode_name(), "WORKFLOW");
        // Pressing `F` when empty triggers BrowseFolder cleanly without inserting 'F'.
        app.press_keys("F").unwrap();
        assert_eq!(app.authoring_view().unwrap().text, "");
        app.apply_workflow_sample_folder(samples_dir.clone())
            .unwrap();
        assert_eq!(
            app.authoring_view().unwrap().text,
            samples_dir.to_string_lossy()
        );
        app.press_keys("<Enter>").unwrap();
        assert_eq!(app.mode_name(), "NORMAL");
        assert_eq!(
            app.instrument(inst_id).unwrap().rack.zones.len(),
            2,
            "Importing folder of 2 synthetic WAVs replaces zones with 2 key-mapped zones"
        );
    }

    #[test]
    fn rack_panel_empty_pins_renames_pin_reorder_cable_repatch_resonator_hotkey_route_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let samples_dir = dir.path().join("samples");
        std::fs::create_dir_all(&samples_dir).unwrap();
        write_synthetic_wav_with_riff_meta(&samples_dir.join("tone_a.wav"), 512, 60, 0, 127, 0, 0);
        write_synthetic_wav_with_riff_meta(&samples_dir.join("tone_b.wav"), 512, 60, 0, 127, 0, 0);

        let proj_path = dir.path().join("rack_lifecycle.json");
        let lib =
            cadence_core::sampler::SampleLibrary::from_roots(&[samples_dir], Some(&proj_path));
        let mut app = App::new_with_library(Project::demo(), proj_path, lib).unwrap();

        // 1. Blank Modular has empty panel_pins, yet pill 0 ("Rack") keeps all 5 RackParam controls accessible!
        app.execute_command("instrument ModBlank modular").unwrap();
        let mod_id = app.selected_instrument.unwrap();
        app.open_instrument(mod_id).unwrap();
        assert!(app.instrument(mod_id).unwrap().rack.panel_pins.is_empty());
        let rv = instrument_rack_view(&app);
        assert_eq!(rv.modules[0].module_id, None);
        assert_eq!(rv.modules[0].label, "Rack");
        assert_eq!(rv.modules[0].tag, "RACK");
        let ctrls = app.control_descriptors();
        for p in RackParam::ALL {
            assert!(
                ctrls.iter().any(|c| matches!(
                    c.target,
                    ControlTarget::Param(ParamAddress::Rack { param, .. }) if param == p
                )),
                "Empty panel_pins must still expose RackParam::{p:?} on pill 0"
            );
        }

        // 2. Add Module picker hotkey `0` selects #10 (`Resonator`) directly!
        app.press_keys("a0").unwrap();
        assert!(
            app.instrument(mod_id).unwrap().rack.zones[0]
                .modules
                .iter()
                .any(|m| m.kind.tag() == ModuleKindTag::Resonator),
            "Pressing `a` then `0` must add Resonator (#10)"
        );

        // 3. `B` targets the currently focused Sampler module on Panel (when 2 Sampler modules exist)!
        app.execute_command("mod add sampler SampA").unwrap();
        app.execute_command("mod add sampler SampB").unwrap();
        let samp_b_id = app.instrument(mod_id).unwrap().rack.zones[0]
            .modules
            .iter()
            .find(|m| m.name == "SampB")
            .unwrap()
            .id;
        app.focus_rack_module_panel(mod_id, samp_b_id);
        app.press_keys("B").unwrap();
        assert_eq!(app.mode_name(), "SAMPLES");
        assert!(
            app.browser_view().unwrap().destination.contains("SampB"),
            "B on Panel must target the focused Sampler module (SampB), got: {}",
            app.browser_view().unwrap().destination
        );
        app.press_keys("<Esc>").unwrap();

        // 4. Contextual `r` rename on Panel module pill, Quick Pin label, `Alt+j/k` pin reorder, and pinned badge!
        let lead_id = app.doc.project().instruments[0].id;
        app.open_instrument(lead_id).unwrap();
        app.rack_ui.panel_slot = 0;
        app.control_cursor = 0;
        assert!(!app.instrument(lead_id).unwrap().rack.panel_pins.is_empty());
        // Row 0 on Quick pill is a QUICK pin → `r` renames the pin label!
        let pin0_target = app.instrument(lead_id).unwrap().rack.panel_pins[0].target;
        app.press_keys("rMagicCut<Enter>").unwrap();
        assert_eq!(
            app.instrument(lead_id).unwrap().rack.panel_pins[0].label,
            "MagicCut"
        );
        assert_eq!(
            app.instrument(lead_id).unwrap().rack.panel_pins[0].target,
            pin0_target
        );
        // `Alt+j` reorders Quick pin 0 -> index 1 and tracks control_cursor!
        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT))
            .unwrap();
        assert_eq!(
            app.instrument(lead_id).unwrap().rack.panel_pins[1].label,
            "MagicCut"
        );
        assert_eq!(app.control_cursor, 1);

        // Step to a module pill (`l`): pinned controls carry `ControlView.pinned == true`, and `r` renames module!
        app.press_keys("l").unwrap();
        let rv_mod = instrument_rack_view(&app);
        let focused_mid = rv_mod
            .modules
            .iter()
            .find(|p| p.active)
            .unwrap()
            .module_id
            .unwrap();
        app.press_keys("rMainOsc<Enter>").unwrap();
        assert_eq!(
            app.instrument(lead_id).unwrap().rack.zones[0]
                .module(focused_mid)
                .unwrap()
                .name,
            "MainOsc"
        );
        // Check ControlView.pinned marks pinned params cleanly on a module that has a pinned param.
        let pin_mod = app
            .instrument(lead_id)
            .unwrap()
            .rack
            .panel_pins
            .iter()
            .find_map(|p| match p.target {
                InstrumentParam::Module { module, .. } => Some(module),
                _ => None,
            });
        if let Some(pm) = pin_mod {
            app.focus_rack_module_panel(lead_id, pm);
            let fv = app.frame_view(&AudioSnapshot::default(), "test");
            let iv = fv
                .decks
                .iter()
                .find_map(|d| match &d.view {
                    crate::view::DeckView::Instrument(iv) => Some(iv),
                    _ => None,
                })
                .expect("instrument deck view");
            assert!(
                iv.controls.iter().any(|c| c.pinned),
                "Module pill with a pinned parameter must set ControlView.pinned = true"
            );
        }

        // 5. Patchbay: duplicate cables filtered out of `c` connect picker, AND `c` on a selected cable
        //    re-patches destination preserving exact cable ID, Quick Panel pins, and Macro bindings!
        app.press_keys("L").unwrap();
        assert_eq!(app.rack_ui.tab, RackTab::Patchbay);
        let z0 = app.instrument(lead_id).unwrap().rack.zones[0].clone();
        let existing_cable = z0.cables[0];
        // Focus cable's source module and open connect picker: existing (from -> to.port) must be filtered out!
        app.focus_patchbay_module(&z0, existing_cable.from_module);
        app.rack_ui.patchbay_cable_id = None;
        app.press_keys("c").unwrap();
        assert_eq!(app.mode_name(), "PICK");
        let existing_dst_name = format!(
            "{}.{}",
            z0.module(existing_cable.to_module).unwrap().name,
            existing_cable.to_port.short_label()
        );
        assert!(
            !app.picker_view()
                .unwrap()
                .items
                .iter()
                .any(|(name, _, _)| name == &existing_dst_name),
            "Connect-cable picker must filter out already-connected target '{existing_dst_name}'"
        );
        app.press_keys("<Esc>").unwrap();

        // Pin `existing_cable` and bind it to Macro 1, then select it and press `c` to re-patch!
        let cable_param = InstrumentParam::Cable {
            zone: z0.id,
            cable: existing_cable.id,
        };
        app.apply(Action::RackPinControl {
            instrument: lead_id,
            pin: PanelPin {
                target: cable_param,
                label: "SendDepth".into(),
            },
        })
        .unwrap();
        app.rack_ui.patchbay_cable_id = Some(existing_cable.id);
        app.press_keys("c").unwrap();
        assert_eq!(app.mode_name(), "PICK");
        assert!(app.picker_view().unwrap().title.starts_with("RE-PATCH"));
        // Pick item #2 (a different destination port than current item #1).
        app.press_keys("2").unwrap();
        let z0_after = &app.instrument(lead_id).unwrap().rack.zones[0];
        let repatched = z0_after
            .cable(existing_cable.id)
            .expect("Cable ID must be preserved after re-patch");
        assert!(
            (repatched.to_module, repatched.to_port)
                != (existing_cable.to_module, existing_cable.to_port),
            "Re-patched cable must point to the newly chosen destination"
        );
        assert!(
            app.instrument(lead_id)
                .unwrap()
                .rack
                .panel_pins
                .iter()
                .any(|p| p.target == cable_param && p.label == "SendDepth"),
            "Re-patching cable must preserve Quick Panel pin targeting cable ID"
        );

        // 6. `o` on `gi` opens audio-channel route picker directly!
        app.execute_command("channel Audio AltBus").unwrap();
        let alt_bus_id = app.channel_id("AltBus").unwrap();
        app.open_instrument(lead_id).unwrap();
        app.press_keys("o").unwrap();
        assert_eq!(app.mode_name(), "PICK");
        let alt_pos = app
            .picker_view()
            .unwrap()
            .items
            .iter()
            .position(|(n, _, _)| n == "AltBus")
            .unwrap();
        for _ in 0..alt_pos {
            app.press_keys("j").unwrap();
        }
        app.press_keys("<Enter>").unwrap();
        assert_eq!(app.instrument(lead_id).unwrap().channel, alt_bus_id);

        // 7. `X` on `gi` opens Mode::Confirm with dependent summary and deletes instrument + dependents atomically!
        let part_count_before = app.doc.project().parts.len();
        app.press_keys("X").unwrap();
        assert_eq!(app.mode_name(), "CONFIRM");
        let detail = app
            .confirm_detail()
            .expect("Confirm detail for DeleteInstrument");
        assert!(
            detail.contains("Delete instrument"),
            "Confirm detail must describe instrument deletion: {detail}"
        );
        app.press_keys("<Esc>").unwrap();
        assert_eq!(app.mode_name(), "NORMAL");
        assert!(app.instrument(lead_id).is_some());

        app.press_keys("Xy").unwrap();
        assert_eq!(app.mode_name(), "NORMAL");
        assert!(app.instrument(lead_id).is_none());
        assert!(app.doc.project().parts.len() <= part_count_before);
        // Single `u` restores the deleted instrument and all dependent parts cleanly!
        app.press_keys("u").unwrap();
        assert!(app.instrument(lead_id).is_some());
        assert_eq!(app.doc.project().parts.len(), part_count_before);
    }
}
