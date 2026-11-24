//! Semantic Take deck view: bar-by-bar strip of heard performance, override/edit
//! markers, loop indicators, and range selection for `:keep A B` / `Alt+c`.
use crate::App;
use cadence_core::take::TakeChange;

#[derive(Clone, Debug)]
pub struct TakeDeckView {
    pub take_armed: bool,
    pub active_recording: bool,
    pub take_capped: bool,
    pub visual_selecting: bool,
    pub empty: bool,
    pub title: String,
    pub total_bars: usize,
    pub cursor_bar: usize,
    pub mark_range: Option<(usize, usize)>,
    pub bars: Vec<TakeBarSummary>,
    pub events: Vec<TakeEventMarker>,
}

#[derive(Clone, Debug)]
pub struct TakeBarSummary {
    /// 1-based bar index inside the take.
    pub take_bar: usize,
    /// 1-based song bar heard during this take bar (if within arrangement).
    pub song_bar: Option<usize>,
    pub in_loop: bool,
    /// Active live overrides `(lane_index, part_name)` audible in this bar.
    pub overrides: Vec<(usize, String)>,
    pub edited: bool,
    pub muted_lanes: usize,
    pub selected: bool,
    pub in_mark: bool,
}

#[derive(Clone, Debug)]
pub struct TakeEventMarker {
    pub bar_offset: f32,
    pub label: String,
    pub is_edit: bool,
}

impl App {
    pub(super) fn build_take_view(&self) -> TakeDeckView {
        let take_armed = self.takes.armed;
        let active_recording = self.take_recording().is_some();
        let take_capped = self.take_capped();
        let visual_selecting = self.takes.visual_anchor.is_some();
        let Some(take) = self.current_take() else {
            return TakeDeckView {
                take_armed,
                active_recording,
                take_capped,
                visual_selecting: false,
                empty: true,
                title: self.project().title.clone(),
                total_bars: 0,
                cursor_bar: 0,
                mark_range: None,
                bars: Vec::new(),
                events: Vec::new(),
            };
        };
        let total_bars = take.steps().div_ceil(16).max(1) as usize;
        let cursor_bar = self.takes.cursor_bar.min(total_bars.saturating_sub(1));
        let mark_range = match (self.takes.mark_start, self.takes.mark_end) {
            (Some(a), Some(b)) => {
                Some((a.min(b).min(total_bars - 1), a.max(b).min(total_bars - 1)))
            }
            (Some(a), None) => Some((
                a.min(cursor_bar).min(total_bars - 1),
                a.max(cursor_bar).min(total_bars - 1),
            )),
            (None, Some(b)) => Some((
                b.min(cursor_bar).min(total_bars - 1),
                b.max(cursor_bar).min(total_bars - 1),
            )),
            (None, None) => None,
        };

        let states = take.states().unwrap_or_default();
        let mut bars = Vec::with_capacity(total_bars);
        for bar_idx in 0..total_bars {
            let step = take.header.start_step + bar_idx as u64 * 16;
            let bar_end = step + 16;
            let st_idx = states
                .partition_point(|(from, _)| *from <= step)
                .saturating_sub(1);
            let (song_bar, in_loop, overrides, muted_lanes) =
                if let Some((_, state)) = states.get(st_idx) {
                    let seg = state.timeline.unwrap_or_else(|| {
                        cadence_core::timeline::Segment::new(0, 0, None, state.project.end)
                    });
                    let song_len = state.project.arrangement_steps().max(1) as u64;
                    let sb = seg.song_step(step, song_len).map(|s| s as usize / 16 + 1);
                    let ovs: Vec<(usize, String)> = state
                        .active
                        .iter()
                        .map(|ov| {
                            let lane_idx = state
                                .project
                                .lanes
                                .iter()
                                .position(|l| l.id == ov.lane)
                                .unwrap_or(0);
                            let name = state
                                .project
                                .part(ov.part.part)
                                .map_or_else(|| "?".into(), |p| p.name.clone());
                            (lane_idx, name)
                        })
                        .collect();
                    let muted = state
                        .project
                        .lanes
                        .iter()
                        .filter(|l| state.flags.lane_muted(l.id))
                        .count();
                    (sb, seg.region.is_some(), ovs, muted)
                } else {
                    (Some(bar_idx + 1), false, Vec::new(), 0)
                };
            let edited = take.events.iter().any(|ev| {
                ev.at >= step
                    && ev.at < bar_end
                    && matches!(
                        ev.change,
                        TakeChange::Edits { .. } | TakeChange::Replace { .. }
                    )
            });
            let in_mark = mark_range.is_some_and(|(a, b)| (a..=b).contains(&bar_idx));
            bars.push(TakeBarSummary {
                take_bar: bar_idx + 1,
                song_bar,
                in_loop,
                overrides,
                edited,
                muted_lanes,
                selected: bar_idx == cursor_bar,
                in_mark,
            });
        }

        let events = take
            .events
            .iter()
            .map(|ev| TakeEventMarker {
                bar_offset: ev.at.saturating_sub(take.header.start_step) as f32 / 16.0,
                label: ev.label.clone(),
                is_edit: matches!(
                    ev.change,
                    TakeChange::Edits { .. } | TakeChange::Replace { .. }
                ),
            })
            .collect();

        TakeDeckView {
            take_armed,
            active_recording,
            take_capped,
            visual_selecting,
            empty: false,
            title: take.header.title,
            total_bars,
            cursor_bar,
            mark_range,
            bars,
            events,
        }
    }
}
