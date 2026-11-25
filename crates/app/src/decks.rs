//! The session's side of decks: opening, pinning and focusing them, keeping
//! the focused deck's view state in the session where every key handler reads
//! it, and letting an unpinned working deck follow the selection.
//!
//! `workspace()`, `focus()`, and focused-canvas queries are computed directly
//! from `self.layout`, with zero mutable shadow fields to drift out of sync.
use super::layout::{Deck, DeckKind, DeckState, Direction, Layout, Spot, Target};
use super::*;

impl App {
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// The currently focused deck's kind (single source of truth for key dispatch).
    pub fn focused_deck_kind(&self) -> Option<DeckKind> {
        self.layout.focused().map(|d| d.kind)
    }

    /// The focused main workspace ONLY when Arrange, Pattern canvas, or Mixer is focused.
    /// Returns `None` for Take, Graph, Midi, Macros, Bank, Output, or object decks.
    pub fn focused_canvas(&self) -> Option<Workspace> {
        match self.focused_deck_kind() {
            Some(DeckKind::Arrange) => Some(Workspace::Arrange),
            Some(DeckKind::Pattern) => Some(Workspace::Pattern),
            Some(DeckKind::Mixer) => Some(Workspace::Mixer),
            _ => None,
        }
    }

    /// Active main workspace derived purely from the layout (focused -> working -> pinned -> Arrange).
    /// Used for status/serialization/context queries; NEVER for gating canvas key dispatch.
    pub fn workspace(&self) -> Workspace {
        let main = |deck: &Deck| match deck.kind {
            DeckKind::Arrange => Some(Workspace::Arrange),
            DeckKind::Pattern => Some(Workspace::Pattern),
            DeckKind::Mixer => Some(Workspace::Mixer),
            _ => None,
        };
        self.layout
            .focused()
            .and_then(main)
            .or_else(|| self.layout.working().and_then(main))
            .or_else(|| self.layout.pinned().iter().find_map(main))
            .unwrap_or(Workspace::Arrange)
    }

    /// Coarse focus region derived purely from the focused deck.
    pub fn focus(&self) -> Focus {
        match self.focused_deck_kind() {
            Some(DeckKind::Bank) => Focus::Library,
            Some(kind) if kind.is_object() => Focus::Inspector,
            _ => Focus::Canvas,
        }
    }

    pub fn arrange_canvas_focused(&self) -> bool {
        self.focused_deck_kind() == Some(DeckKind::Arrange)
    }

    pub fn pattern_deck_focused(&self) -> bool {
        self.focused_deck_kind() == Some(DeckKind::Pattern)
    }

    pub fn pattern_canvas_focused(&self) -> bool {
        self.focused_deck_kind() == Some(DeckKind::Pattern)
    }

    pub fn sample_deck_focused(&self) -> bool {
        self.focused_deck_kind() == Some(DeckKind::Sample)
    }

    pub fn mixer_canvas_focused(&self) -> bool {
        self.focused_deck_kind() == Some(DeckKind::Mixer)
    }

    pub fn bank_focused(&self) -> bool {
        self.focused_deck_kind() == Some(DeckKind::Bank)
    }

    /// Arrange across the top, the Bank docked on the right and Output along
    /// the bottom; anything opened goes below Arrange.
    pub(super) fn default_layout() -> Layout {
        Layout::new(
            vec![
                Deck::song(DeckKind::Arrange),
                Deck::song(DeckKind::Bank),
                Deck::song(DeckKind::Output),
            ],
            None,
        )
    }

    pub(super) fn reset_layout(&mut self) {
        self.layout = Self::default_layout();
        self.sync_view();
    }

    /// The deck for a workspace: the song for Arrange, Mixer and Live, the
    /// selected pattern for Pattern.
    pub(super) fn workspace_deck(&self, workspace: Workspace) -> Deck {
        match workspace {
            Workspace::Arrange => Deck::song(DeckKind::Arrange),
            Workspace::Mixer => Deck::song(DeckKind::Mixer),
            Workspace::Pattern => Deck::new(
                DeckKind::Pattern,
                self.selected_pattern.map_or(Target::Song, Target::Pattern),
            ),
        }
    }

    pub(super) fn object_deck(detail: Detail) -> Option<Deck> {
        let (kind, target) = match detail {
            Detail::None => return None,
            Detail::Instrument(id) => (DeckKind::Instrument, Target::Instrument(id)),
            Detail::Placement(id) => (DeckKind::Placement, Target::Placement(id)),
            Detail::Part(id) => (DeckKind::Part, Target::Part(id)),
            Detail::Channel(channel) => (DeckKind::Channel, Target::Channel(channel)),
            Detail::Effect(id) => (DeckKind::Effect, Target::Effect(id)),
        };
        Some(Deck::new(kind, target))
    }

    fn deck_detail(deck: &Deck) -> Detail {
        match deck.target {
            Target::Instrument(id) => Detail::Instrument(id),
            Target::Placement(id) => Detail::Placement(id),
            Target::Part(id) => Detail::Part(id),
            Target::Channel(channel) => Detail::Channel(channel),
            Target::Effect(id) => Detail::Effect(id),
            Target::Song | Target::Pattern(_) | Target::Sample { .. } => Detail::None,
        }
    }

    /// Show a deck and focus it: a visible deck with the same target takes
    /// focus, anything else opens in the working spot. Callers record the trail
    /// with `push_back` first.
    pub(super) fn show_deck(&mut self, deck: Deck) {
        self.stash_focused();
        self.layout.show(deck);
        self.load_focused();
        self.reset_midi_pickup();
        self.sync_view();
    }

    pub(super) fn show_workspace(&mut self, workspace: Workspace) {
        self.show_deck(self.workspace_deck(workspace));
    }

    /// `g` and a deck letter: like `show_deck`, but replacing the working deck
    /// adds a step to the trail, so Esc returns to what was there.
    pub(super) fn go_to_deck(&mut self, deck: Deck) {
        if self.layout.find(deck.kind, deck.target).is_none() && self.layout.working().is_some() {
            self.push_back();
        }
        self.show_deck(deck);
    }

    /// Tab and Shift+Tab cycle decks consistently everywhere.
    pub(super) fn cycle_decks(&mut self, forward: bool) {
        self.stash_focused();
        self.layout.cycle(forward);
        self.load_focused();
        self.sync_view();
    }

    pub(super) fn move_deck_focus(&mut self, direction: Direction) {
        self.stash_focused();
        let moved = self.layout.move_focus(direction);
        self.load_focused();
        self.sync_view();
        if !moved {
            self.status = "No deck that way".into();
        }
    }

    pub(super) fn toggle_deck_pin(&mut self) -> Result<()> {
        self.stash_focused();
        let spot = self.layout.toggle_pin().context("Nothing to pin")?;
        let label = self.deck_label(self.layout.deck(spot).expect("pinned deck"));
        self.status = if self.layout.is_pinned(spot) {
            format!("Pinned {label} · it keeps this target")
        } else {
            format!("Unpinned {label} · it follows the selection")
        };
        self.sync_view();
        Ok(())
    }

    pub(super) fn close_deck(&mut self) -> Result<()> {
        self.stash_focused();
        let closed = self.layout.close_focused().context("Nothing to close")?;
        self.status = format!("Closed {}", self.deck_label(&closed));
        self.load_focused();
        self.sync_view();
        Ok(())
    }

    pub(super) fn only_deck(&mut self) {
        self.stash_focused();
        self.layout.only();
        self.sync_view();
        self.status = "Closed the other decks".into();
    }

    pub(super) fn toggle_deck_zoom(&mut self) {
        self.layout.toggle_zoom();
        self.sync_view();
    }

    /// 1-key workspace layout presets (`z1`..`z4`) built directly on `Layout`.
    pub(super) fn apply_layout_preset(&mut self, preset: u8) {
        self.stash_focused();
        let pat_deck = self.workspace_deck(Workspace::Pattern);
        let inst_deck = self
            .selected_instrument
            .map(|id| Deck::new(DeckKind::Instrument, Target::Instrument(id)));
        let (layout, label) = match preset {
            1 => (
                Layout::new(vec![Deck::song(DeckKind::Arrange)], Some(pat_deck)),
                "Compose (Arrange + Pattern)",
            ),
            2 => {
                self.view_state.bank_mode = crate::BankMode::Parts;
                (
                    Layout::new(
                        vec![
                            Deck::song(DeckKind::Arrange),
                            Deck::song(DeckKind::Bank),
                            Deck::song(DeckKind::Output),
                        ],
                        None,
                    ),
                    "Perform (Arrange + Bank + Output)",
                )
            }
            3 => (
                Layout::new(
                    vec![pat_deck, Deck::song(DeckKind::Output)],
                    inst_deck.or_else(|| Some(Deck::song(DeckKind::Bank))),
                ),
                "Sound Design (Pattern + Instrument + Output)",
            ),
            4 => (
                Layout::new(
                    vec![Deck::song(DeckKind::Arrange), Deck::song(DeckKind::Output)],
                    Some(Deck::song(DeckKind::Mixer)),
                ),
                "Mix (Arrange + Mixer + Output)",
            ),
            _ => return,
        };
        self.layout = layout;
        self.load_focused();
        self.sync_view();
        self.status = format!("Layout preset {preset}: {label}");
    }

    /// Show or hide the Output deck.
    pub(super) fn set_output_visible(&mut self, visible: bool) {
        if visible {
            self.layout.pin(Deck::song(DeckKind::Output));
        } else {
            self.layout.unpin(DeckKind::Output);
        }
        self.sync_view();
    }

    /// A short name for a deck, for the trail and status messages.
    pub fn deck_label(&self, deck: &Deck) -> String {
        let name = match deck.target {
            Target::Song => None,
            Target::Pattern(id) => self.pattern(id).map(|p| p.name.clone()),
            Target::Sample {
                instrument, zone, ..
            } => self.instrument(instrument).map(|inst| {
                if inst.rack.zones.len() > 1 {
                    let zn = inst.rack.zone(zone).map_or("Zone", |z| z.name.as_str());
                    format!("{} · {zn}", inst.name)
                } else {
                    inst.name.clone()
                }
            }),
            Target::Instrument(id) => self.instrument(id).map(|i| i.name.clone()),
            Target::Part(id) => self.part(id).map(|b| b.name.clone()),
            Target::Placement(id) => self.placement(id).and_then(|(lane, placement)| {
                self.part(placement.part)
                    .map(|b| format!("{} on {}", b.name, lane.name))
            }),
            Target::Channel(channel) => Some(self.channel_name(channel).to_string()),
            Target::Effect(id) => self.effect(id).map(|(_, d)| d.name.clone()),
        };
        match name {
            Some(name) => format!("{} {name}", deck.kind.label()),
            None => deck.kind.label().into(),
        }
    }

    /// The trail: the working decks back navigation returns through, then the
    /// current one.
    pub fn trail(&self) -> Vec<String> {
        let mut crumbs: Vec<String> = self
            .back
            .iter()
            .filter_map(|n| n.layout.working().or(n.layout.focused()))
            .map(|d| self.deck_label(d))
            .collect();
        if let Some(deck) = self.layout.working().or(self.layout.focused()) {
            crumbs.push(self.deck_label(deck));
        }
        crumbs.dedup();
        crumbs
    }

    /// Save the session's view state into the focused deck before focus moves.
    fn stash_focused(&mut self) {
        let state = DeckState {
            cursor: self.cursor,
            source_cursor: self.source_cursor,
            control_cursor: self.control_cursor,
            local: match self.scope {
                EditScope::Placement(id) => Some(id),
                EditScope::Definition => None,
            },
        };
        let selected = self.selected_pattern;
        // Only the working deck takes the selection as its target; a pinned
        // deck keeps its own.
        let working = self.layout.focus() == Spot::Working;
        if let Some(deck) = self.layout.focused_mut() {
            match deck.kind {
                DeckKind::Pattern => {
                    if let (true, Some(id)) = (working, selected) {
                        deck.target = Target::Pattern(id);
                    }
                    deck.state = state;
                }
                kind if kind.is_object() => deck.state.control_cursor = state.control_cursor,
                _ => {}
            }
        }
    }

    /// Load the focused deck's view state into the session.
    fn load_focused(&mut self) {
        let Some(deck) = self.layout.focused().copied() else {
            return;
        };
        match (deck.kind, deck.target) {
            (DeckKind::Pattern, Target::Pattern(id)) => {
                self.selected_pattern = Some(id);
                self.cursor = deck.state.cursor;
                self.source_cursor = deck.state.source_cursor;
                self.scope = deck
                    .state
                    .local
                    .filter(|p| self.placement(*p).is_some())
                    .map_or(EditScope::Definition, EditScope::Placement);
            }
            (kind, _) if kind.is_object() => self.control_cursor = deck.state.control_cursor,
            _ => {}
        }
    }

    /// The working deck follows the selection while another deck has focus,
    /// unless it is pinned.
    fn follow_selection(&mut self) {
        if self.layout.focus() == Spot::Working {
            return;
        }
        let Some(deck) = self.layout.working().copied() else {
            return;
        };
        let target = match deck.kind {
            DeckKind::Pattern => self.selected_pattern.map(Target::Pattern),
            DeckKind::Sample => self
                .selected_instrument
                .and_then(|id| self.sampler_target_for_instrument(id))
                .map(|(instrument, zone, module)| Target::Sample {
                    instrument,
                    zone,
                    module,
                }),
            DeckKind::Instrument => self.selected_instrument.map(Target::Instrument),
            DeckKind::Part => self.selected_part.map(Target::Part),
            DeckKind::Placement => self.selected_placement.map(Target::Placement),
            DeckKind::Channel => {
                let mixer = self
                    .layout
                    .focused()
                    .is_some_and(|d| d.kind == DeckKind::Mixer);
                if mixer {
                    Some(Target::Channel(self.selected_channel))
                } else {
                    self.selected_instrument
                        .and_then(|id| self.instrument(id))
                        .map(|i| Target::Channel(ChannelRef::Channel(i.channel)))
                }
            }
            _ => None,
        };
        if let Some(target) = target {
            self.layout.retarget_working(target);
        }
    }

    /// Derive the context-deck fields from the layout.
    pub(super) fn sync_view(&mut self) {
        self.follow_selection();
        let focused = self.layout.focused().copied();
        // The context deck is the focused object deck, else the working one,
        // else a pinned one.
        let object = |spot: Spot| {
            self.layout
                .deck(spot)
                .filter(|d| d.kind.is_object())
                .map(|d| (spot, *d))
        };
        let context = object(self.layout.focus())
            .or_else(|| object(Spot::Working))
            .or_else(|| (0..self.layout.pinned().len()).find_map(|i| object(Spot::Pinned(i))));
        self.detail = context.map_or(Detail::None, |(_, deck)| Self::deck_detail(&deck));
        self.view_state.deck_visible = context.is_some();
        self.view_state.deck_pinned = context.is_some_and(|(spot, _)| self.layout.is_pinned(spot));
        let zoomed = self.layout.zoomed();
        let object_focused = focused.is_some_and(|d| d.kind.is_object());
        self.view_state.deck_zoomed = zoomed && object_focused;
        self.view_state.zoomed = zoomed && !object_focused;
        self.view_state.monitor_visible = self.layout.find_kind(DeckKind::Output).is_some();
    }

    /// Drop decks whose objects were deleted.
    pub(super) fn retain_decks(&mut self) {
        let project = self.doc.project();
        let exists = |deck: &Deck| match deck.target {
            Target::Song => true,
            Target::Pattern(id) => project.pattern(id).is_some(),
            Target::Sample {
                instrument,
                zone,
                module,
            } => project
                .instrument(instrument)
                .and_then(|i| i.rack.zone(zone))
                .and_then(|z| z.module(module))
                .is_some_and(|m| matches!(m.kind, ModuleKind::Sampler(_))),
            Target::Instrument(id) => project.instrument(id).is_some(),
            Target::Part(id) => project.part(id).is_some(),
            Target::Placement(id) => project
                .lanes
                .iter()
                .any(|l| l.placements.iter().any(|p| p.id == id)),
            Target::Channel(ChannelRef::Channel(id)) => {
                project.mixer.channels.iter().any(|c| c.id == id)
            }
            Target::Channel(_) => true,
            Target::Effect(id) => project
                .mixer
                .channels
                .iter()
                .flat_map(|c| &c.inserts)
                .chain(&project.mixer.master.inserts)
                .any(|d| d.id == id),
        };
        let layout = &mut self.layout;
        layout.retain(exists);
    }

    /// Whether a deck kind is shown, for views that adapt to their neighbours.
    pub fn deck_visible_kind(&self, kind: DeckKind) -> bool {
        self.layout.find_kind(kind).is_some()
    }
}
