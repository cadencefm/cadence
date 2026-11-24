//! Song-time event inspection and navigation; no authored material is created.
use super::*;
use cadence_core::arrangement_view::{
    self, ArrangementInspection, ArrangementNote, ArrangementProjection, ArrangementRequest,
};

pub(super) struct CachedArrangementInspection {
    revision: u64,
    key: EventKey,
    result: std::result::Result<ArrangementInspection, String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArrangeView {
    #[default]
    Placements,
    Overview,
    Roll,
}
impl ArrangeView {
    pub fn label(self) -> &'static str {
        match self {
            Self::Placements => "Placements",
            Self::Overview => "Evaluated overview",
            Self::Roll => "Lane roll",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EventSourceView {
    pub pattern: Id,
    pub name: String,
    pub kind: String,
    pub step: usize,
    pub period: usize,
    pub role: String,
    pub value: String,
    pub depth: usize,
}
#[derive(Clone, Debug, PartialEq)]
pub struct EventInspectionView {
    pub event: Option<ArrangementNote>,
    pub sources: Vec<EventSourceView>,
    pub selected: usize,
    pub summary: Vec<String>,
    pub error: Option<String>,
}

impl App {
    pub fn evaluated_arrange_canvas(&self) -> bool {
        self.arrange_canvas_focused() && self.arrange_ui.view != ArrangeView::Placements
    }
    pub fn is_arrangement_inspection(&self) -> bool {
        self.input.mode == Mode::ArrangementInspect
    }
    pub fn arrange_page_steps(&self) -> usize {
        self.arrange_ui
            .page_steps
            .unwrap_or_else(|| self.arrange_visible_bars() * 16)
            .clamp(1, arrangement_view::MAX_VISIBLE_STEPS)
    }
    pub fn set_arrange_page_size(&mut self, steps: usize) {
        self.arrange_ui.page_steps = Some(steps.clamp(1, arrangement_view::MAX_VISIBLE_STEPS));
    }
    pub fn arrangement_projection(
        &self,
        start: u32,
        count: usize,
        lane: Option<Id>,
    ) -> Result<ArrangementProjection> {
        arrangement_view::arrangement_notes(
            self.doc.project(),
            self.doc.compiled(),
            self.doc.flags(),
            ArrangementRequest {
                start_step: start,
                step_count: count,
                lane,
            },
        )
    }
    /// Resolve against the current compiled revision, never a cached note payload.
    pub fn selected_arrangement_event(&self) -> Option<ArrangementNote> {
        let lane = self.selected_lane?;
        if let Some(note) = self.arrange_ui.event.and_then(|key| {
            arrangement_view::event_by_key(
                self.doc.project(),
                self.doc.compiled(),
                self.doc.flags(),
                key,
            )
            .ok()
            .flatten()
        }) {
            if note.lane == lane {
                return Some(note);
            }
        }
        arrangement_view::event_at(
            self.doc.project(),
            self.doc.compiled(),
            self.doc.flags(),
            lane,
            self.arrange_ui.step,
            None,
        )
        .ok()
        .flatten()
    }
    pub(super) fn normalize_arrangement_event(&mut self) {
        if self.arrange_ui.pitch_lane != self.selected_lane {
            self.arrange_ui.pitch_lane = self.selected_lane;
            self.arrange_ui.pitch_anchor = None;
        }
        if self.arrange_ui.event.is_some_and(|key| {
            arrangement_view::event_by_key(
                self.doc.project(),
                self.doc.compiled(),
                self.doc.flags(),
                key,
            )
            .ok()
            .flatten()
            .is_none_or(|note| Some(note.lane) != self.selected_lane)
        }) {
            self.arrange_ui.event = None;
            self.arrange_ui.source_cursor = 0;
        }
        if let Some(note) = self.selected_arrangement_event() {
            self.arrange_ui.pitch_anchor = Some(note.midi);
        }
    }
    pub(super) fn set_arrange_view(&mut self, view: ArrangeView) -> Result<Outcome> {
        let selected = self.arrange_ui.event;
        self.show_workspace(Workspace::Arrange);
        self.input.mode = Mode::Normal;
        self.arrange_ui.view = view;
        self.select_at_cursor();
        self.arrange_ui.event = selected;
        self.normalize_arrangement_event();
        self.status = format!("Arrange · {} · v chooses view", view.label());
        Ok(Outcome::default())
    }
    pub(super) fn select_arrangement_event(&mut self, key: EventKey) -> Result<Outcome> {
        let note = arrangement_view::event_by_key(
            self.doc.project(),
            self.doc.compiled(),
            self.doc.flags(),
            key,
        )?
        .context("Scheduled event no longer exists")?;
        self.suspend_follow();
        self.show_workspace(Workspace::Arrange);
        if self.arrange_ui.view == ArrangeView::Placements {
            self.arrange_ui.view = ArrangeView::Overview;
        }
        self.arrange_ui.step = key.song_step;
        self.set_arrangement_event_context(&note);
        self.arrange_ui.source_cursor = 0;
        self.status = format!(
            "{} at bar {} · step {} · Enter provenance",
            cadence_core::music::note_name(note.midi),
            key.song_step / 16 + 1,
            key.song_step % 16 + 1
        );
        Ok(Outcome::default())
    }
    /// An incoming gate may belong to a different placement than the cursor.
    fn set_arrangement_event_context(&mut self, note: &ArrangementNote) {
        self.arrange_ui.event = Some(note.key);
        self.arrange_ui.pitch_anchor = Some(note.midi);
        self.arrange_ui.pitch_lane = Some(note.lane);
        self.selected_lane = Some(note.lane);
        self.selected_placement = Some(note.key.placement);
        self.selected_part = Some(note.part);
        self.selected_pattern = Some(note.pattern);
        self.selected_instrument = Some(note.instrument);
        self.follow_deck();
    }
    pub(super) fn move_arrangement_event(
        &mut self,
        forward: bool,
        count: usize,
    ) -> Result<Outcome> {
        let lane = self.selected_lane.context("Create a lane first")?;
        self.suspend_follow();
        let mut moved = false;
        for _ in 0..count {
            let selected = self.selected_arrangement_event();
            let at_cursor = selected
                .as_ref()
                .filter(|note| note.key.song_step == self.arrange_ui.step);
            let cursor = arrangement_view::EventCursor {
                lane,
                song_step: self.arrange_ui.step,
                sub_step: at_cursor.map(|note| note.key.sub_step),
                tone: at_cursor.map(|note| note.key.tone),
            };
            let Some(note) = arrangement_view::adjacent_event(
                self.doc.project(),
                self.doc.compiled(),
                self.doc.flags(),
                cursor,
                forward,
            )?
            else {
                break;
            };
            self.arrange_ui.step = note.key.song_step;
            self.set_arrangement_event_context(&note);
            self.arrange_ui.source_cursor = 0;
            moved = true;
        }
        if !moved {
            self.status = if forward {
                "No later note event in this lane"
            } else {
                "No earlier note event in this lane"
            }
            .into();
        }
        Ok(Outcome::default())
    }
    pub(super) fn move_arrangement_page(&mut self, forward: bool, count: usize) -> Result<Outcome> {
        self.suspend_follow();
        let stride = self
            .arrange_page_steps()
            .saturating_mul(count)
            .min(u32::MAX as usize) as u32;
        self.arrange_ui.step = if forward {
            self.arrange_ui
                .step
                .saturating_add(stride)
                .min(self.doc.project().arrangement_steps().saturating_sub(1) as u32)
        } else {
            self.arrange_ui.step.saturating_sub(stride)
        };
        self.select_at_cursor();
        self.normalize_arrangement_event();
        Ok(Outcome::default())
    }
    pub(super) fn handle_arrange_views(&mut self, key: KeyEvent) -> Result<Outcome> {
        if key.kind == KeyEventKind::Repeat {
            return Ok(Outcome::default());
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return Ok(Outcome::default());
        }
        self.input.mode = Mode::Normal;
        let view = match key.code {
            KeyCode::Char('p') => Some(ArrangeView::Placements),
            KeyCode::Char('o') => Some(ArrangeView::Overview),
            KeyCode::Char('r') => Some(ArrangeView::Roll),
            KeyCode::Enter => Some(self.arrange_ui.view),
            _ => None,
        };
        match view {
            Some(view) => self.apply(Action::SetArrangeView { view }),
            None => Ok(Outcome::default()),
        }
    }
    pub(super) fn open_arrangement_inspection(&mut self) -> Result<Outcome> {
        let note = self
            .selected_arrangement_event()
            .context("No scheduled note at this song position · w/b finds note events")?;
        self.arrange_ui.event = Some(note.key);
        self.arrange_ui.source_cursor = 0;
        self.input.mode = Mode::ArrangementInspect;
        Ok(self.release_held())
    }
    pub(super) fn inspect_arrangement_key(&self, key: EventKey) -> Result<ArrangementInspection> {
        let mut cached = self.arrange_ui.inspection_cache.borrow_mut();
        if cached
            .as_ref()
            .is_none_or(|entry| entry.revision != self.doc.revision() || entry.key != key)
        {
            *cached = Some(CachedArrangementInspection {
                revision: self.doc.revision(),
                key,
                result: arrangement_view::inspect_event(
                    self.doc.project(),
                    self.doc.compiled(),
                    self.doc.flags(),
                    key,
                )
                .map_err(|error| format!("{error:#}")),
            });
        }
        cached
            .as_ref()
            .unwrap()
            .result
            .clone()
            .map_err(anyhow::Error::msg)
    }
    pub fn arrangement_inspection(&self) -> Option<EventInspectionView> {
        if !self.is_arrangement_inspection() {
            return None;
        }
        let inspection = self
            .arrange_ui
            .event
            .context("Selected event is no longer present in this revision")
            .and_then(|key| self.inspect_arrangement_key(key));
        let inspection = match inspection {
            Ok(value) => value,
            Err(error) => {
                return Some(EventInspectionView {
                    event: None,
                    sources: Vec::new(),
                    selected: 0,
                    summary: Vec::new(),
                    error: Some(format!("{error:#} · Esc returns to Arrange")),
                })
            }
        };
        let note = &inspection.note;
        let part = self.part(note.part)?;
        let instrument = self.instrument(note.instrument)?;
        let (lane, placement) = self.placement(note.key.placement)?;
        let sources: Vec<_> = inspection
            .sources
            .into_iter()
            .map(|source| {
                let pattern = self
                    .pattern(source.pattern)
                    .expect("validated source inspection");
                let role = format!(
                    "{}{}{}",
                    source.role,
                    if source.displaced {
                        " · underlying / replaced locally"
                    } else {
                        ""
                    },
                    if source.reference_only {
                        " · shared reference"
                    } else {
                        ""
                    }
                );
                EventSourceView {
                    pattern: source.pattern,
                    name: pattern.name.clone(),
                    kind: kind_label(&pattern.kind).into(),
                    step: source.step,
                    period: source.period,
                    role,
                    depth: source.depth,
                    value: match source.value {
                        PatternValue::Notes(step) if !step.active => "rest".into(),
                        PatternValue::Notes(step) => format!(
                            "degree {} · {:.0}% · gate {:.2}{}",
                            step.degree - 1,
                            step.velocity * 100.0,
                            step.gate,
                            if step.chord.is_empty() {
                                String::new()
                            } else {
                                format!(" · +{} chord tones", step.chord.len())
                            }
                        ),
                        PatternValue::Rhythm(hit) => if hit { "hit" } else { "rest" }.into(),
                        PatternValue::Value(value) => format!("{value:.3}"),
                    },
                }
            })
            .collect();
        let summary = vec![
            format!(
                "{} · MIDI {} · velocity {:.0}% · gate {:.2} steps · chord tone {}",
                cadence_core::music::note_name(note.midi),
                note.midi,
                note.velocity * 100.0,
                note.gate,
                note.key.tone + 1
            ),
            format!(
                "Song {:03}.{}.{} · offset {} · lane {}",
                note.key.song_step / 16 + 1,
                (note.key.song_step % 16) / 4 + 1,
                note.key.song_step % 4 + 1,
                note.key.song_step,
                lane.name
            ),
            format!(
                "Placement #{} · local offset {} · {:?} phase {:+} steps",
                placement.id, note.local_step, placement.phase, placement.offset_steps
            ),
            format!(
                "{} → {} → {} · part {:+} st / {:.0}%",
                part.name,
                instrument.name,
                self.channel_name(ChannelRef::Channel(note.channel)),
                part.transpose,
                part.velocity_scale * 100.0
            ),
            format!(
                "Source clock {} · {}{}",
                note.pattern_time,
                if note.local_exception {
                    "LOCAL EXCEPTION · underlying sources are displaced"
                } else {
                    "retained source result"
                },
                if note.channel_muted {
                    " · Audio channel muted"
                } else {
                    ""
                }
            ),
            "Scheduled note/gate; audio envelopes and effect tails can last beyond this range"
                .into(),
        ];
        Some(EventInspectionView {
            event: Some(inspection.note),
            selected: self
                .arrange_ui
                .source_cursor
                .min(sources.len().saturating_sub(1)),
            sources,
            summary,
            error: None,
        })
    }
    pub(super) fn handle_arrangement_inspection(&mut self, key: KeyEvent) -> Result<Outcome> {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return Ok(Outcome::default());
        }
        if key.kind == KeyEventKind::Repeat
            && matches!(key.code, KeyCode::Enter | KeyCode::Char('s' | 'o' | 'i'))
        {
            return Ok(Outcome::default());
        }
        match key.code {
            KeyCode::Esc | KeyCode::Backspace => self.input.mode = Mode::Normal,
            KeyCode::Char('j') | KeyCode::Down => {
                let count = self
                    .arrangement_inspection()
                    .map_or(0, |view| view.sources.len());
                self.arrange_ui.source_cursor = self
                    .arrange_ui
                    .source_cursor
                    .saturating_add(1)
                    .min(count.saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.arrange_ui.source_cursor = self.arrange_ui.source_cursor.saturating_sub(1)
            }
            KeyCode::Enter | KeyCode::Char('s') => return self.follow_arrangement_source(),
            KeyCode::Char('o') => return self.follow_arrangement_local(),
            KeyCode::Char('i') => return self.follow_arrangement_instrument(),
            KeyCode::Char(' ') => {
                return self.apply(Action::Play {
                    playing: !self.transport.playing,
                })
            }
            _ => {}
        }
        Ok(Outcome::default())
    }
    fn inspected_arrangement_event(&self) -> Result<ArrangementNote> {
        let key = self
            .arrange_ui
            .event
            .context("Selected event is no longer present")?;
        arrangement_view::event_by_key(
            self.doc.project(),
            self.doc.compiled(),
            self.doc.flags(),
            key,
        )?
        .context("Scheduled event no longer exists")
    }
    fn follow_arrangement_source(&mut self) -> Result<Outcome> {
        let view = self
            .arrangement_inspection()
            .context("Open event provenance first")?;
        let source = view
            .sources
            .get(view.selected)
            .context("No source available for this event")?
            .clone();
        let note = self.inspected_arrangement_event()?;
        self.push_back();
        self.suspend_follow();
        self.input.mode = Mode::Normal;
        self.set_arrangement_event_context(&note);
        self.selected_pattern = Some(source.pattern);
        self.show_workspace(Workspace::Pattern);
        self.scope = EditScope::Definition;
        self.cursor = source.step;
        self.query.clear();
        self.status = format!(
            "{} · source offset {} / step {} · Esc returns to event",
            source.name,
            source.step,
            source.step + 1
        );
        Ok(Outcome::default())
    }
    fn follow_arrangement_local(&mut self) -> Result<Outcome> {
        let note = self.inspected_arrangement_event()?;
        self.push_back();
        self.suspend_follow();
        self.input.mode = Mode::Normal;
        self.set_arrangement_event_context(&note);
        self.show_workspace(Workspace::Pattern);
        self.scope = EditScope::Placement(note.key.placement);
        self.cursor = note.local_step as usize;
        self.status = "Placement-local occurrence · source and part transforms stay separate · Esc returns to event".into();
        Ok(Outcome::default())
    }
    fn follow_arrangement_instrument(&mut self) -> Result<Outcome> {
        let note = self.inspected_arrangement_event()?;
        self.push_back();
        self.input.mode = Mode::Normal;
        self.set_arrangement_event_context(&note);
        self.show_detail(Detail::Instrument(note.instrument));
        Ok(Outcome::default())
    }
}
