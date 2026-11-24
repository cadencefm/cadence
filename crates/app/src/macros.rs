//! Assignable Project Macros (`M1..M8`) actions, Ex commands (`:macro ...`),
//! keyboard controls inside the Macros deck (`gk`), and target formatting helpers.
use super::*;
use cadence_core::macros::{MacroBinding, MacroControl, MAX_MACRO_BINDINGS, MAX_PROJECT_MACROS};

impl App {
    pub(super) fn apply_set_macro(&mut self, index: u8, value: f32) -> Result<Outcome> {
        self.commit_macro_value(index, value, false)
    }

    pub(super) fn commit_macro_value(
        &mut self,
        index: u8,
        value: f32,
        coalesce: bool,
    ) -> Result<Outcome> {
        ensure!(
            usize::from(index) < MAX_PROJECT_MACROS,
            "Macro slot must be 1..={MAX_PROJECT_MACROS}"
        );
        ensure!(
            value.is_finite() && (0.0..=1.0).contains(&value),
            "Macro value must be within 0.0..=1.0 (or 0%..=100%)"
        );
        let name = self
            .doc
            .project()
            .macro_control(index)
            .map_or_else(|| MacroControl::default_name(index), |m| m.name.clone());
        let label = format!("M{} {name} → {:.0}%", usize::from(index) + 1, value * 100.0);
        self.commit_coalesced(
            vec![Edit::SetMacroValue { index, value }],
            label,
            document::CoalesceKey::Macro(index),
            coalesce,
        )
    }

    pub(super) fn apply_rename_macro(&mut self, index: u8, name: String) -> Result<Outcome> {
        ensure!(
            usize::from(index) < MAX_PROJECT_MACROS,
            "Macro slot must be 1..={MAX_PROJECT_MACROS}"
        );
        let trimmed = name.trim().to_string();
        let mut macro_control = self
            .doc
            .project()
            .macro_control(index)
            .cloned()
            .unwrap_or_else(|| MacroControl::new(index, trimmed.clone()));
        macro_control.name = trimmed.clone();
        macro_control.validate_self()?;
        let label = format!("Renamed M{} → {trimmed}", usize::from(index) + 1);
        self.commit(vec![Edit::PutMacro { macro_control }], label)
    }

    pub(super) fn apply_bind_macro(
        &mut self,
        index: u8,
        target: ParamAddress,
        min: f32,
        max: f32,
    ) -> Result<Outcome> {
        ensure!(
            usize::from(index) < MAX_PROJECT_MACROS,
            "Macro slot must be 1..={MAX_PROJECT_MACROS}"
        );
        let binding = MacroBinding::new(target, min, max);
        binding.validate()?;
        self.doc.project().validate_macro_target(target)?;

        let mut macro_control = self
            .doc
            .project()
            .macro_control(index)
            .cloned()
            .unwrap_or_else(|| MacroControl::new(index, MacroControl::default_name(index)));
        if let Some(existing) = macro_control
            .bindings
            .iter_mut()
            .find(|b| b.target == target)
        {
            existing.min = min;
            existing.max = max;
        } else {
            ensure!(
                macro_control.bindings.len() < MAX_MACRO_BINDINGS,
                "Macro {} already has {MAX_MACRO_BINDINGS} bindings",
                usize::from(index) + 1
            );
            macro_control.bindings.push(binding);
        }
        let (_, target_label) = self.format_macro_target(target);
        let arrow = if binding.is_inverted() { "↓" } else { "→" };
        let label = format!(
            "Bound M{} {} → {target_label} ({:.0}% {arrow} {:.0}%)",
            usize::from(index) + 1,
            macro_control.name,
            min * 100.0,
            max * 100.0
        );
        self.commit(vec![Edit::PutMacro { macro_control }], label)
    }

    pub(super) fn apply_unbind_macro(
        &mut self,
        index: u8,
        binding: Option<usize>,
    ) -> Result<Outcome> {
        ensure!(
            usize::from(index) < MAX_PROJECT_MACROS,
            "Macro slot must be 1..={MAX_PROJECT_MACROS}"
        );
        let mut macro_control = self
            .doc
            .project()
            .macro_control(index)
            .cloned()
            .with_context(|| format!("Macro {} is not configured", usize::from(index) + 1))?;
        let label = match binding {
            Some(b_idx) => {
                ensure!(
                    b_idx < macro_control.bindings.len(),
                    "Macro {} has no binding #{}",
                    usize::from(index) + 1,
                    b_idx + 1
                );
                let removed = macro_control.bindings.remove(b_idx);
                let (_, tname) = self.format_macro_target(removed.target);
                format!("Unbound {tname} from M{}", usize::from(index) + 1)
            }
            None => {
                if macro_control.bindings.is_empty() {
                    self.status = format!("M{} has no bindings", usize::from(index) + 1);
                    return Ok(Outcome::default());
                }
                macro_control.bindings.clear();
                format!(
                    "Cleared all bindings on M{} {}",
                    usize::from(index) + 1,
                    macro_control.name
                )
            }
        };
        self.commit(vec![Edit::PutMacro { macro_control }], label)
    }

    pub(super) fn apply_clear_macro(&mut self, index: u8) -> Result<Outcome> {
        ensure!(
            usize::from(index) < MAX_PROJECT_MACROS,
            "Macro slot must be 1..={MAX_PROJECT_MACROS}"
        );
        if self.doc.project().macro_control(index).is_none() {
            self.status = format!("Macro {} is already empty", usize::from(index) + 1);
            return Ok(Outcome::default());
        }
        let mut edits: Vec<Edit> = self
            .doc
            .project()
            .modulations
            .iter()
            .filter(|m| m.target == ModulationTarget::Macro { index })
            .map(|m| Edit::RemoveModulation { id: m.id })
            .collect();
        let removed_mods = edits.len();
        edits.push(Edit::RemoveMacro { index });
        let label = if removed_mods > 0 {
            format!(
                "Cleared M{} (unlinked {removed_mods} modulation{})",
                usize::from(index) + 1,
                if removed_mods == 1 { "" } else { "s" }
            )
        } else {
            format!("Cleared M{}", usize::from(index) + 1)
        };
        self.commit(edits, label)
    }

    /// Whether any Project Macro currently binds `address`.
    pub fn param_is_macro_bound(&self, address: ParamAddress) -> bool {
        self.doc
            .project()
            .macros
            .iter()
            .any(|m| m.bindings.iter().any(|b| b.target == address))
    }

    pub(super) fn parse_macro_slot(raw: &str) -> Result<u8> {
        let trimmed = raw.trim().trim_start_matches(['m', 'M']);
        let one_based = trimmed
            .parse::<u8>()
            .with_context(|| format!("Macro slot '{raw}' must be 1..={MAX_PROJECT_MACROS}"))?;
        ensure!(
            (1..=MAX_PROJECT_MACROS as u8).contains(&one_based),
            "Macro slot must be 1..={MAX_PROJECT_MACROS}"
        );
        Ok(one_based - 1)
    }

    /// `:macro ...` Ex command handler.
    pub(super) fn execute_macro_command(&mut self, args: &[String]) -> Result<Outcome> {
        if args.is_empty() {
            self.go_to_deck(layout::Deck::song(layout::DeckKind::Macros));
            self.status =
                "Project Macros (M1–M8) · 1–8/hl select · a bind · r rename · e mod · Alt+h/l ,/. adjust · M learn · R rec"
                    .into();
            return Ok(Outcome::default());
        }
        let index = Self::parse_macro_slot(&args[0])?;
        ensure!(
            args.len() >= 2,
            "Usage: :macro 1..8 <VALUE> | name <NAME> | bind <rack|zone|module|cable|effect|channel|send> ... | unbind <INDEX|all> | clear"
        );
        match args[1].to_ascii_lowercase().as_str() {
            "name" | "rename" => {
                ensure!(args.len() >= 3, "Usage: :macro 1..8 name <NAME>");
                let name = args[2..].join(" ");
                self.view_state.macro_cursor = usize::from(index);
                self.apply(Action::RenameMacro { index, name })
            }
            "clear" | "delete" | "remove" => {
                ensure!(args.len() == 2, "Usage: :macro 1..8 clear");
                self.view_state.macro_cursor = usize::from(index);
                self.apply(Action::ClearMacro { index })
            }
            "unbind" => {
                ensure!(args.len() == 3, "Usage: :macro 1..8 unbind <1..N|all>");
                let binding = if args[2].eq_ignore_ascii_case("all") {
                    None
                } else {
                    let b = args[2]
                        .parse::<usize>()
                        .context("Binding index must be a positive integer or 'all'")?
                        .checked_sub(1)
                        .context("Binding indices start at 1")?;
                    Some(b)
                };
                self.view_state.macro_cursor = usize::from(index);
                self.apply(Action::UnbindMacro { index, binding })
            }
            "bind" => {
                let (target, min, max) = if args.len() == 5 {
                    let target = self
                        .parse_dotted_zone_param(&args[2], true)
                        .context("Usage: :macro 1..8 bind <rack|zone|module|cable|effect|channel|send> <TARGET> <PARAM> <MIN> <MAX> | :macro 1..8 bind <KIT.ZONE.PARAM> <MIN> <MAX>")?;
                    let min = param_ops::parse_normalized(&args[3])?;
                    let max = param_ops::parse_normalized(&args[4])?;
                    (target, min, max)
                } else {
                    ensure!(
                        args.len() == 7,
                        "Usage: :macro 1..8 bind <rack|zone|module|cable|effect|channel|send> <TARGET> <PARAM> <MIN> <MAX> | :macro 1..8 bind <KIT.ZONE.PARAM> <MIN> <MAX>"
                    );
                    let min = param_ops::parse_normalized(&args[5])?;
                    let max = param_ops::parse_normalized(&args[6])?;
                    let target = self.parse_param_address(&args[2], &args[3], &args[4])?;
                    (target, min, max)
                };
                self.view_state.macro_cursor = usize::from(index);
                self.apply(Action::BindMacro {
                    index,
                    target,
                    min,
                    max,
                })
            }
            value_raw => {
                ensure!(
                    args.len() == 2,
                    "Usage: :macro 1..8 <0.0..=1.0 | 0%..=100%>"
                );
                let value = parse_amount(value_raw)?;
                self.view_state.macro_cursor = usize::from(index);
                self.apply(Action::SetMacro { index, value })
            }
        }
    }

    /// Clamp `self.view_state.macro_binding_cursor` cleanly to the selected macro's bindings.
    pub(crate) fn clamp_macro_binding_cursor(&mut self) {
        self.view_state.macro_cursor = self.view_state.macro_cursor.min(MAX_PROJECT_MACROS - 1);
        let count = self
            .doc
            .project()
            .macro_control(self.view_state.macro_cursor as u8)
            .map_or(0, |m| m.bindings.len());
        self.view_state.macro_binding_cursor =
            self.view_state.macro_binding_cursor.and_then(|idx| {
                if count == 0 {
                    None
                } else {
                    Some(idx.min(count - 1))
                }
            });
    }

    pub(crate) fn step_macro_vertical(&mut self, delta: isize, wrap: bool) {
        self.view_state.macro_cursor = self.view_state.macro_cursor.min(MAX_PROJECT_MACROS - 1);
        let count = self
            .doc
            .project()
            .macro_control(self.view_state.macro_cursor as u8)
            .map_or(0, |m| m.bindings.len());
        if count == 0 {
            self.view_state.macro_binding_cursor = None;
            self.status = self.macro_slot_status(self.view_state.macro_cursor as u8);
            return;
        }
        // Positions: 0 = Dial (`None`), 1..=count = Binding(`i - 1`).
        let total = (count + 1) as isize;
        let cur = match self.view_state.macro_binding_cursor {
            None => 0isize,
            Some(i) => (i + 1).min(count) as isize,
        };
        let next = if wrap {
            (cur + delta).rem_euclid(total) as usize
        } else {
            (cur + delta).clamp(0, total - 1) as usize
        };
        self.view_state.macro_binding_cursor = if next == 0 { None } else { Some(next - 1) };
        self.status = self.macro_slot_status(self.view_state.macro_cursor as u8);
    }

    /// `NORMAL`-mode keyboard controls when the Project Macros Deck (`gk`) has focus.
    /// Strict invariant: `h/j/k/l` ONLY navigate slots & dial/binding rows — never mutate!
    pub(crate) fn handle_macros_deck_key(
        &mut self,
        key: KeyEvent,
        count: usize,
    ) -> Option<Result<Outcome>> {
        self.clamp_macro_binding_cursor();
        let slot = self.view_state.macro_cursor as u8;

        match key.code {
            KeyCode::Char('h') | KeyCode::Left => {
                self.view_state.macro_cursor = self.view_state.macro_cursor.saturating_sub(count);
                self.view_state.macro_binding_cursor = None;
                self.status = self.macro_slot_status(self.view_state.macro_cursor as u8);
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.view_state.macro_cursor =
                    (self.view_state.macro_cursor + count).min(MAX_PROJECT_MACROS - 1);
                self.view_state.macro_binding_cursor = None;
                self.status = self.macro_slot_status(self.view_state.macro_cursor as u8);
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.step_macro_vertical(count.min(isize::MAX as usize) as isize, false);
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.step_macro_vertical(-(count.min(isize::MAX as usize) as isize), false);
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char(c @ '1'..='8') => {
                self.view_state.macro_cursor = (c as u8 - b'1') as usize;
                self.view_state.macro_binding_cursor = None;
                self.status = self.macro_slot_status(self.view_state.macro_cursor as u8);
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('a') => Some(self.open_macro_bind_picker(slot)),
            KeyCode::Char('r') => Some(self.begin_workflow(WorkflowKind::RenameMacro(slot))),
            KeyCode::Char('e') => Some(self.open_focused_links_menu()),
            KeyCode::Enter => {
                if let Some(b_idx) = self.view_state.macro_binding_cursor {
                    Some(self.jump_to_macro_binding_target(slot, b_idx))
                } else {
                    Some(self.begin_macro_value_input(slot, String::new()))
                }
            }
            KeyCode::Char('v') => self
                .view_state
                .macro_binding_cursor
                .map(|b_idx| self.invert_macro_binding(slot, b_idx)),
            KeyCode::Char('x') => {
                if let Some(b_idx) = self.view_state.macro_binding_cursor {
                    let res = self.apply(Action::UnbindMacro {
                        index: slot,
                        binding: Some(b_idx),
                    });
                    self.clamp_macro_binding_cursor();
                    return Some(res);
                }
                if let Some(mac) = self.doc.project().macro_control(slot) {
                    if !mac.bindings.is_empty() {
                        Some(self.apply(Action::UnbindMacro {
                            index: slot,
                            binding: None,
                        }))
                    } else {
                        Some(self.apply(Action::ClearMacro { index: slot }))
                    }
                } else {
                    self.status = format!("M{} is already empty", usize::from(slot) + 1);
                    Some(Ok(Outcome::default()))
                }
            }
            KeyCode::Char('X') => Some(self.apply(Action::ClearMacro { index: slot })),
            _ => None,
        }
    }

    fn jump_to_macro_binding_target(&mut self, index: u8, binding_idx: usize) -> Result<Outcome> {
        let Some(target) = self
            .doc
            .project()
            .macro_control(index)
            .and_then(|m| m.bindings.get(binding_idx))
            .map(|b| b.target)
        else {
            return Ok(Outcome::default());
        };
        self.jump_to_param_address(target)?;
        Ok(Outcome::default())
    }

    fn invert_macro_binding(&mut self, index: u8, binding_idx: usize) -> Result<Outcome> {
        let mut mac = self
            .doc
            .project()
            .macro_control(index)
            .cloned()
            .context("Macro not configured")?;
        let binding = mac
            .bindings
            .get_mut(binding_idx)
            .context("Macro binding missing")?;
        std::mem::swap(&mut binding.min, &mut binding.max);
        let (_, tname) = self.format_macro_target(binding.target);
        let label = format!(
            "Inverted M{} {tname} ({:.0}% → {:.0}%)",
            usize::from(index) + 1,
            binding.min * 100.0,
            binding.max * 100.0
        );
        self.commit(vec![Edit::PutMacro { macro_control: mac }], label)
    }

    /// Direct-manipulate the focused Macro dial (`±2%` fine, `±10%` coarse) or binding range (`min`/`max`).
    pub(crate) fn nudge_macro_focused(
        &mut self,
        direction: i8,
        coarse: bool,
        coalesce: bool,
    ) -> Result<Outcome> {
        self.clamp_macro_binding_cursor();
        let slot = self.view_state.macro_cursor as u8;
        match self.view_state.macro_binding_cursor {
            None => {
                let current = self
                    .doc
                    .project()
                    .macro_control(slot)
                    .map_or(0.0, |m| m.value);
                let step = if coarse { 0.10 } else { 0.02 };
                let next = (((current + f32::from(direction) * step) * 100.0).round() / 100.0)
                    .clamp(0.0, 1.0);
                self.apply_macro_nudge(slot, next, coalesce)
            }
            Some(b_idx) => {
                // Fine (`Alt+h/l` / `,/.`) adjusts `min`; coarse (`Alt+H/L` / `</>`) adjusts `max`.
                let delta = f32::from(direction) * 0.02;
                self.nudge_macro_binding_bound(slot, b_idx, coarse, delta, coalesce)
            }
        }
    }

    /// Reorder the focused Macro binding row (`Alt+j/k`).
    pub(crate) fn reorder_macro_binding(&mut self, delta: isize) -> Result<Outcome> {
        self.clamp_macro_binding_cursor();
        let Some(b_idx) = self.view_state.macro_binding_cursor else {
            return Ok(Outcome::default());
        };
        let slot = self.view_state.macro_cursor as u8;
        let mut mac = self
            .doc
            .project()
            .macro_control(slot)
            .cloned()
            .context("Macro not configured")?;
        let len = mac.bindings.len();
        if len <= 1 {
            return Ok(Outcome::default());
        }
        let next = (b_idx as isize + delta).clamp(0, len.saturating_sub(1) as isize) as usize;
        if next == b_idx {
            return Ok(Outcome::default());
        }
        let item = mac.bindings.remove(b_idx);
        mac.bindings.insert(next, item);
        let label = format!("Moved M{} binding → #{}", usize::from(slot) + 1, next + 1);
        let out = self.commit(vec![Edit::PutMacro { macro_control: mac }], label)?;
        self.view_state.macro_binding_cursor = Some(next);
        Ok(out)
    }

    /// Reset the focused Macro dial (`0%`) or binding range (`0% → 100%`) (`Alt+Backspace`).
    pub(crate) fn reset_macro_focused(&mut self) -> Result<Outcome> {
        self.clamp_macro_binding_cursor();
        let slot = self.view_state.macro_cursor as u8;
        match self.view_state.macro_binding_cursor {
            None => {
                if self.doc.project().macro_control(slot).is_none() {
                    self.status = format!("M{} is already at 0%", usize::from(slot) + 1);
                    return Ok(Outcome::default());
                }
                self.apply_macro_nudge(slot, 0.0, false)
            }
            Some(b_idx) => {
                let mut mac = self
                    .doc
                    .project()
                    .macro_control(slot)
                    .cloned()
                    .context("Macro not configured")?;
                let binding = mac
                    .bindings
                    .get_mut(b_idx)
                    .context("Macro binding missing")?;
                binding.min = 0.0;
                binding.max = 1.0;
                let (_, tname) = self.format_macro_target(binding.target);
                let label = format!("Reset M{} {tname} range → 0% → 100%", usize::from(slot) + 1);
                self.commit(vec![Edit::PutMacro { macro_control: mac }], label)
            }
        }
    }

    fn nudge_macro_binding_bound(
        &mut self,
        index: u8,
        binding_idx: usize,
        is_max: bool,
        delta: f32,
        coalesce: bool,
    ) -> Result<Outcome> {
        let mut mac = self
            .doc
            .project()
            .macro_control(index)
            .cloned()
            .context("Macro not configured")?;
        let binding = mac
            .bindings
            .get_mut(binding_idx)
            .context("Macro binding missing")?;
        if is_max {
            binding.max = (((binding.max + delta) * 100.0).round() / 100.0).clamp(0.0, 1.0);
        } else {
            binding.min = (((binding.min + delta) * 100.0).round() / 100.0).clamp(0.0, 1.0);
        }
        let min = binding.min;
        let max = binding.max;
        let target = binding.target;
        let (_, tname) = self.format_macro_target(target);
        let arrow = if binding.is_inverted() { "↓" } else { "→" };
        let label = format!(
            "M{} {tname} range {:.0}% {arrow} {:.0}%",
            usize::from(index) + 1,
            min * 100.0,
            max * 100.0
        );
        self.commit_coalesced(
            vec![Edit::PutMacro { macro_control: mac }],
            label,
            document::CoalesceKey::MacroBinding {
                index,
                binding: binding_idx,
            },
            coalesce,
        )
    }

    fn apply_macro_nudge(&mut self, index: u8, next: f32, repeat: bool) -> Result<Outcome> {
        let rounded = (next * 1000.0).round() / 1000.0;
        self.commit_macro_value(index, rounded, repeat)
    }

    pub(crate) fn macro_slot_status(&self, index: u8) -> String {
        if let Some(mac) = self.doc.project().macro_control(index) {
            if let Some(b_idx) = self.view_state.macro_binding_cursor {
                if let Some(b) = mac.bindings.get(b_idx) {
                    let (_, tname) = self.format_macro_target(b.target);
                    return format!(
                        "M{} binding #{}/{} · {tname} ({:.0}% → {:.0}%)",
                        usize::from(index) + 1,
                        b_idx + 1,
                        mac.bindings.len(),
                        b.min * 100.0,
                        b.max * 100.0
                    );
                }
            }
            format!(
                "M{} {} · {:.0}% · {} binding{}",
                usize::from(index) + 1,
                mac.name,
                mac.value * 100.0,
                mac.bindings.len(),
                if mac.bindings.len() == 1 { "" } else { "s" }
            )
        } else {
            format!(
                "M{} (unassigned) · a bind · r rename · e mod · Alt+h/l ,/. adjust · Enter exact",
                usize::from(index) + 1
            )
        }
    }
}
