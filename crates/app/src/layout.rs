//! Decks and their layout: which decks are pinned, which one is working, and
//! which one has focus. Pure data with no knowledge of the session, so the
//! rules for showing, pinning, closing and moving focus are tested alone.
use cadence_core::material::Id;
use cadence_core::mixer::ChannelRef;

/// What a deck shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeckKind {
    Arrange,
    Bank,
    Pattern,
    Sample,
    Mixer,
    Output,
    Take,
    Graph,
    Midi,
    Macros,
    Instrument,
    Placement,
    Part,
    Channel,
    Effect,
}

/// Where a pinned deck goes: wide decks stack across the top, tall decks
/// dock on the right, editors can go in either.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Wide,
    Tall,
    Block,
}

impl DeckKind {
    pub fn shape(self) -> Shape {
        match self {
            DeckKind::Arrange
            | DeckKind::Output
            | DeckKind::Take
            | DeckKind::Graph
            | DeckKind::Midi
            | DeckKind::Macros => Shape::Wide,
            DeckKind::Bank | DeckKind::Mixer => Shape::Tall,
            _ => Shape::Block,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DeckKind::Arrange => "ARRANGE",
            DeckKind::Bank => "BANK",
            DeckKind::Pattern => "PATTERN",
            DeckKind::Sample => "SAMPLE",
            DeckKind::Mixer => "MIXER",
            DeckKind::Output => "OUTPUT",
            DeckKind::Take => "TAKE",
            DeckKind::Graph => "GRAPH",
            DeckKind::Midi => "DEVICES",
            DeckKind::Macros => "MACROS",
            DeckKind::Instrument => "INSTRUMENT",
            DeckKind::Placement => "PLACEMENT",
            DeckKind::Part => "PART",
            DeckKind::Channel => "CHANNEL",
            DeckKind::Effect => "EFFECT",
        }
    }

    /// Decks that show one object of the project.
    pub fn is_object(self) -> bool {
        matches!(
            self,
            DeckKind::Instrument
                | DeckKind::Placement
                | DeckKind::Part
                | DeckKind::Channel
                | DeckKind::Effect
        )
    }

    /// Decks that take focus. Output only observes.
    pub fn focusable(self) -> bool {
        self != DeckKind::Output
    }
}

/// The object a deck shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Song,
    Pattern(Id),
    Sample {
        instrument: Id,
        zone: u16,
        module: u16,
    },
    Instrument(Id),
    Placement(Id),
    Part(Id),
    Channel(ChannelRef),
    Effect(Id),
}

/// A deck's own view state while another deck has focus. The focused deck's
/// state lives in the session, where every key handler reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeckState {
    pub cursor: usize,
    pub source_cursor: usize,
    pub control_cursor: usize,
    /// Pattern decks: editing one placement's occurrence instead of the
    /// shared definition.
    pub local: Option<Id>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deck {
    pub kind: DeckKind,
    pub target: Target,
    pub state: DeckState,
}

impl Deck {
    pub fn new(kind: DeckKind, target: Target) -> Self {
        Self {
            kind,
            target,
            state: DeckState::default(),
        }
    }

    pub fn song(kind: DeckKind) -> Self {
        Self::new(kind, Target::Song)
    }

    fn shows(&self, kind: DeckKind, target: Target) -> bool {
        self.kind == kind && self.target == target
    }
}

/// Which deck has focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spot {
    Pinned(usize),
    Working,
}

/// A direction for moving focus between decks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Down,
    Up,
    Right,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pinned: Vec<Deck>,
    working: Option<Deck>,
    focus: Spot,
    zoomed: bool,
}

impl Layout {
    pub fn new(pinned: Vec<Deck>, working: Option<Deck>) -> Self {
        let mut layout = Self {
            pinned,
            working,
            focus: Spot::Working,
            zoomed: false,
        };
        layout.focus = layout.first_focusable();
        layout
    }

    fn first_focusable(&self) -> Spot {
        self.focusable().first().copied().unwrap_or(Spot::Working)
    }

    /// The decks focus can move between, in screen order.
    pub fn focusable(&self) -> Vec<Spot> {
        self.visible()
            .into_iter()
            .filter(|s| self.deck(*s).is_some_and(|d| d.kind.focusable()))
            .collect()
    }

    /// Pin a deck without disturbing the working deck or focus.
    pub fn pin(&mut self, deck: Deck) -> Spot {
        if let Some(i) = self
            .pinned
            .iter()
            .position(|d| d.shows(deck.kind, deck.target))
        {
            return Spot::Pinned(i);
        }
        self.pinned.push(deck);
        Spot::Pinned(self.pinned.len() - 1)
    }

    /// Remove a pinned deck, keeping focus where it was when possible.
    pub fn unpin(&mut self, kind: DeckKind) {
        let Some(i) = self.pinned.iter().position(|d| d.kind == kind) else {
            return;
        };
        let focused = self.focused().copied();
        self.pinned.remove(i);
        self.focus = focused
            .and_then(|d| self.find(d.kind, d.target))
            .unwrap_or_else(|| self.first_focusable());
    }

    pub fn pinned(&self) -> &[Deck] {
        &self.pinned
    }

    pub fn working(&self) -> Option<&Deck> {
        self.working.as_ref()
    }

    pub fn focus(&self) -> Spot {
        self.focus
    }

    pub fn zoomed(&self) -> bool {
        self.zoomed
    }

    pub fn deck(&self, spot: Spot) -> Option<&Deck> {
        match spot {
            Spot::Pinned(i) => self.pinned.get(i),
            Spot::Working => self.working.as_ref(),
        }
    }

    pub fn deck_mut(&mut self, spot: Spot) -> Option<&mut Deck> {
        match spot {
            Spot::Pinned(i) => self.pinned.get_mut(i),
            Spot::Working => self.working.as_mut(),
        }
    }

    pub fn focused(&self) -> Option<&Deck> {
        self.deck(self.focus)
    }

    pub fn focused_mut(&mut self) -> Option<&mut Deck> {
        self.deck_mut(self.focus)
    }

    pub fn is_pinned(&self, spot: Spot) -> bool {
        matches!(spot, Spot::Pinned(i) if i < self.pinned.len())
    }

    /// Every deck on screen, in focus order: wide decks top to bottom, the
    /// working deck, then the decks docked on the right.
    pub fn visible(&self) -> Vec<Spot> {
        let mut spots: Vec<Spot> = (0..self.pinned.len())
            .filter(|&i| self.pinned[i].kind.shape() != Shape::Tall)
            .map(Spot::Pinned)
            .collect();
        if self.working.is_some() {
            spots.push(Spot::Working);
        }
        spots.extend(
            (0..self.pinned.len())
                .filter(|&i| self.pinned[i].kind.shape() == Shape::Tall)
                .map(Spot::Pinned),
        );
        spots
    }

    /// Find a visible deck showing this target.
    pub fn find(&self, kind: DeckKind, target: Target) -> Option<Spot> {
        if let Some(i) = self.pinned.iter().position(|d| d.shows(kind, target)) {
            return Some(Spot::Pinned(i));
        }
        self.working
            .as_ref()
            .filter(|d| d.shows(kind, target))
            .map(|_| Spot::Working)
    }

    /// Any visible deck of this kind, pinned decks first.
    pub fn find_kind(&self, kind: DeckKind) -> Option<Spot> {
        if let Some(i) = self.pinned.iter().position(|d| d.kind == kind) {
            return Some(Spot::Pinned(i));
        }
        self.working
            .as_ref()
            .filter(|d| d.kind == kind)
            .map(|_| Spot::Working)
    }

    /// Show a deck and focus it. A visible deck with the same target takes
    /// focus; otherwise it opens in the working spot. Returns whether the
    /// working deck was replaced, which the trail records.
    pub fn show(&mut self, deck: Deck) -> bool {
        self.zoomed = false;
        if let Some(spot) = self.find(deck.kind, deck.target) {
            self.focus = spot;
            return false;
        }
        self.working = Some(deck);
        self.focus = Spot::Working;
        true
    }

    /// Focus a spot if it is on screen.
    pub fn set_focus(&mut self, spot: Spot) -> bool {
        if self.deck(spot).is_none() {
            return false;
        }
        if self.zoomed && spot != self.focus {
            self.zoomed = false;
        }
        self.focus = spot;
        true
    }

    /// Pin the focused working deck, or return a pinned deck to the working
    /// spot. Returns the deck's new spot.
    pub fn toggle_pin(&mut self) -> Option<Spot> {
        match self.focus {
            Spot::Working => {
                let deck = self.working.take()?;
                self.pinned.push(deck);
                self.focus = Spot::Pinned(self.pinned.len() - 1);
            }
            Spot::Pinned(i) if i < self.pinned.len() => {
                let deck = self.pinned.remove(i);
                self.working = Some(deck);
                self.focus = Spot::Working;
            }
            Spot::Pinned(_) => return None,
        }
        Some(self.focus)
    }

    /// Close the focused deck. Focus moves to the working deck, or the first
    /// one left on screen.
    pub fn close_focused(&mut self) -> Option<Deck> {
        let closed = match self.focus {
            Spot::Working => self.working.take(),
            Spot::Pinned(i) if i < self.pinned.len() => Some(self.pinned.remove(i)),
            Spot::Pinned(_) => None,
        };
        self.zoomed = false;
        self.focus = if self.working.is_some() {
            Spot::Working
        } else {
            self.first_focusable()
        };
        closed
    }

    /// Close every deck except the focused one.
    pub fn only(&mut self) {
        let Some(deck) = self.focused().copied() else {
            return;
        };
        if self.is_pinned(self.focus) {
            self.pinned = vec![deck];
            self.working = None;
            self.focus = Spot::Pinned(0);
        } else {
            self.pinned.clear();
            self.focus = Spot::Working;
        }
    }

    pub fn toggle_zoom(&mut self) -> bool {
        if self.focused().is_some() {
            self.zoomed = !self.zoomed;
        }
        self.zoomed
    }

    pub fn set_zoom(&mut self, zoomed: bool) {
        self.zoomed = zoomed && self.focused().is_some();
    }

    /// Tab and Shift+Tab: the next or previous deck on screen.
    pub fn cycle(&mut self, forward: bool) -> Spot {
        let visible = self.focusable();
        if visible.is_empty() {
            return self.focus;
        }
        let at = visible.iter().position(|s| *s == self.focus).unwrap_or(0);
        let next = if forward {
            (at + 1) % visible.len()
        } else {
            (at + visible.len() - 1) % visible.len()
        };
        self.set_focus(visible[next]);
        self.focus
    }

    /// Move focus to the deck in a direction: wide decks are above the working
    /// deck, docked decks to its right.
    pub fn move_focus(&mut self, direction: Direction) -> bool {
        let wide: Vec<Spot> = self
            .focusable()
            .into_iter()
            .filter(|s| matches!(s, Spot::Pinned(i) if self.pinned[*i].kind.shape() != Shape::Tall))
            .collect();
        let tall: Vec<Spot> = self
            .focusable()
            .into_iter()
            .filter(|s| matches!(s, Spot::Pinned(i) if self.pinned[*i].kind.shape() == Shape::Tall))
            .collect();
        let mut column: Vec<Spot> = wide.clone();
        if self.working.is_some() {
            column.push(Spot::Working);
        }
        let target = match direction {
            Direction::Up | Direction::Down => {
                let list = if tall.contains(&self.focus) {
                    &tall
                } else {
                    &column
                };
                let at = list.iter().position(|s| *s == self.focus);
                match (at, direction) {
                    (Some(i), Direction::Up) if i > 0 => Some(list[i - 1]),
                    (Some(i), Direction::Down) if i + 1 < list.len() => Some(list[i + 1]),
                    _ => None,
                }
            }
            Direction::Right if !tall.contains(&self.focus) => tall.first().copied(),
            Direction::Left if tall.contains(&self.focus) => column.last().copied(),
            _ => None,
        };
        target.is_some_and(|spot| self.set_focus(spot))
    }

    /// Replace the working deck's target, keeping its kind.
    pub fn retarget_working(&mut self, target: Target) {
        if let Some(deck) = &mut self.working {
            if deck.target != target {
                deck.target = target;
                deck.state = DeckState::default();
            }
        }
    }

    /// Drop decks whose objects no longer exist.
    pub fn retain(&mut self, exists: impl Fn(&Deck) -> bool) {
        let focused = self.focused().copied();
        self.pinned.retain(&exists);
        if self.working.as_ref().is_some_and(|d| !exists(d)) {
            self.working = None;
        }
        self.focus = focused
            .and_then(|d| self.find(d.kind, d.target))
            .unwrap_or_else(|| {
                if self.working.is_some() {
                    Spot::Working
                } else {
                    self.first_focusable()
                }
            });
        if self.focused().is_none() {
            self.zoomed = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_layout() -> Layout {
        Layout::new(
            vec![Deck::song(DeckKind::Arrange), Deck::song(DeckKind::Bank)],
            None,
        )
    }

    #[test]
    fn showing_a_visible_deck_focuses_it_and_others_open_in_the_working_spot() {
        let mut layout = default_layout();
        assert_eq!(layout.focus(), Spot::Pinned(0));
        assert!(!layout.show(Deck::song(DeckKind::Bank)));
        assert_eq!(layout.focus(), Spot::Pinned(1));
        assert!(layout.show(Deck::new(DeckKind::Pattern, Target::Pattern(7))));
        assert_eq!(layout.focus(), Spot::Working);
        assert!(layout.show(Deck::new(DeckKind::Pattern, Target::Pattern(8))));
        assert_eq!(layout.working().unwrap().target, Target::Pattern(8));
        assert!(!layout.show(Deck::new(DeckKind::Pattern, Target::Pattern(8))));
    }

    #[test]
    fn pinning_keeps_a_target_while_the_working_spot_moves_on() {
        let mut layout = default_layout();
        layout.show(Deck::new(DeckKind::Pattern, Target::Pattern(7)));
        assert_eq!(layout.toggle_pin(), Some(Spot::Pinned(2)));
        assert!(layout.working().is_none());
        layout.show(Deck::new(DeckKind::Pattern, Target::Pattern(8)));
        assert_eq!(layout.pinned()[2].target, Target::Pattern(7));
        assert_eq!(layout.working().unwrap().target, Target::Pattern(8));
        // Unpinning returns the deck to the working spot.
        layout.set_focus(Spot::Pinned(2));
        assert_eq!(layout.toggle_pin(), Some(Spot::Working));
        assert_eq!(layout.working().unwrap().target, Target::Pattern(7));
        assert_eq!(layout.pinned().len(), 2);
    }

    #[test]
    fn focus_order_and_directions_follow_the_screen() {
        let mut layout = default_layout();
        layout.show(Deck::new(DeckKind::Instrument, Target::Instrument(3)));
        assert_eq!(
            layout.visible(),
            vec![Spot::Pinned(0), Spot::Working, Spot::Pinned(1)]
        );
        assert!(layout.move_focus(Direction::Up));
        assert_eq!(layout.focus(), Spot::Pinned(0));
        assert!(layout.move_focus(Direction::Right));
        assert_eq!(layout.focus(), Spot::Pinned(1));
        assert!(layout.move_focus(Direction::Left));
        assert_eq!(layout.focus(), Spot::Working);
        assert!(!layout.move_focus(Direction::Down));
        layout.cycle(true);
        assert_eq!(layout.focus(), Spot::Pinned(1));
        layout.cycle(true);
        assert_eq!(layout.focus(), Spot::Pinned(0));
    }

    #[test]
    fn closing_zooming_and_only_keep_focus_on_screen() {
        let mut layout = default_layout();
        layout.show(Deck::song(DeckKind::Mixer));
        assert!(layout.toggle_zoom());
        layout.cycle(true);
        assert!(!layout.zoomed(), "moving focus ends a zoom");
        layout.set_focus(Spot::Working);
        assert_eq!(layout.close_focused().unwrap().kind, DeckKind::Mixer);
        assert_eq!(layout.focus(), Spot::Pinned(0));
        layout.only();
        assert_eq!(layout.pinned().len(), 1);
        assert_eq!(layout.pinned()[0].kind, DeckKind::Arrange);
    }

    #[test]
    fn decks_for_deleted_objects_disappear() {
        let mut layout = default_layout();
        layout.show(Deck::new(DeckKind::Placement, Target::Placement(9)));
        layout.retain(|d| d.target != Target::Placement(9));
        assert!(layout.working().is_none());
        assert_eq!(layout.focus(), Spot::Pinned(0));
    }
}
