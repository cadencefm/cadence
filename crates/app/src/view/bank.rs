//! Semantic Bank deck view: sections strip (1..0), lane part launchers (q..p × 1..9), and material library.
use crate::{kind_label, App, BankMode, LANE_KEYS, SECTION_KEYS};
use cadence_core::live::Change;
use cadence_core::material::{Id, PhaseMode};

#[derive(Clone, Debug)]
pub struct BankDeckView {
    pub mode: BankMode,
    pub jam_active: bool,
    pub zoomed: bool,
    pub staging_count: Option<usize>,
    pub selected_lane_idx: usize,
    pub selected_part_idx: usize,
    pub search_query: Option<String>,
    pub sections: Vec<BankSectionItem>,
    pub lanes: Vec<BankLaneGroup>,
    pub patterns: Vec<BankPatternItem>,
    pub total_patterns: usize,
}

#[derive(Clone, Debug)]
pub struct BankSectionItem {
    pub key_char: Option<char>,
    pub name: String,
    pub start_bar: usize,
    pub bars: usize,
    pub active: bool,
    pub queued: bool,
    pub staged: bool,
}

/// Number of part slots displayed per lane window in the Bank deck (scrolling across up to
/// `MAX_LANE_PARTS = 32` slots as `bank_part_idx` moves).
pub const BANK_VISIBLE_PART_SLOTS: usize = 9;

#[derive(Clone, Debug)]
pub struct BankLaneGroup {
    pub lane_id: Id,
    pub lane_name: String,
    pub key_char: Option<char>,
    pub muted: bool,
    pub solo: bool,
    pub staged_mute: Option<bool>,
    pub staged_solo: Option<bool>,
    pub staged_release: bool,
    pub total_parts: usize,
    pub part_window_start: usize,
    pub parts: Vec<BankPartEntry>,
}

#[derive(Clone, Debug)]
pub struct BankPartEntry {
    pub slot_num: usize,
    pub part_id: Id,
    pub part_name: String,
    pub phase: PhaseMode,
    pub offset_steps: i32,
    pub status: BankPartStatus,
    pub staged_launch: bool,
    pub staged_release: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BankPartStatus {
    Idle,
    Arranged,
    Overriding,
    PendingLaunch { bar: u64 },
}

#[derive(Clone, Debug)]
pub struct BankPatternItem {
    pub id: Id,
    pub name: String,
    pub kind_label: &'static str,
    pub period: usize,
    pub selected: bool,
}

impl App {
    pub(super) fn build_bank_view(&self) -> BankDeckView {
        let project = self.project();
        let heard = self.song_step_heard();
        let queued_section_start: Option<u64> =
            self.live.pending.iter().find_map(|p| match p.change {
                Change::Timeline { segment } => Some(segment.song),
                _ => None,
            });
        let staged_section = self.staged_section();
        let sections: Vec<BankSectionItem> = self
            .effective_bank_sections()
            .into_iter()
            .enumerate()
            .filter_map(|(i, id)| {
                let s = project.sections.iter().find(|s| s.id == id)?;
                let start = u64::from(s.start_step);
                let len = u64::from(s.length_steps.max(16));
                Some(BankSectionItem {
                    key_char: SECTION_KEYS.get(i).copied(),
                    name: s.name.clone(),
                    start_bar: s.start_step as usize / 16 + 1,
                    bars: (s.length_steps as usize).div_ceil(16).max(1),
                    active: (start..start + len).contains(&heard),
                    queued: queued_section_start == Some(start),
                    staged: staged_section == Some(s.id),
                })
            })
            .collect();

        let bank_lanes = self.play_bank_lanes();
        let selected_lane_idx = self
            .view_state
            .bank_lane_idx
            .min(project.lanes.len().saturating_sub(1));
        let mut lanes = Vec::with_capacity(project.lanes.len());
        for (index, lane) in project.lanes.iter().enumerate() {
            let lane_id = lane.id;
            let key_char = bank_lanes
                .iter()
                .position(|&id| id == lane_id)
                .and_then(|pos| LANE_KEYS.get(pos).copied());
            let overriding = self.live.lane_state(lane_id).map(|s| s.part);
            let staged_launch_part = self.staged_launch_on_lane(lane_id);
            let staged_release = self.staged_release_on_lane(lane_id);
            let arranged = lane
                .placements
                .iter()
                .find(|p| {
                    (u64::from(p.start_step)..u64::from(p.start_step + p.length_steps))
                        .contains(&heard)
                })
                .map(|p| p.part)
                .filter(|_| overriding.is_none() && self.playing() && !self.is_solo_auditioning());
            let all_parts = self.lane_parts(lane_id);
            let total_parts = all_parts.len();
            let part_window_start = if total_parts <= BANK_VISIBLE_PART_SLOTS {
                0
            } else if index == selected_lane_idx {
                let sel = self
                    .view_state
                    .bank_part_idx
                    .min(total_parts.saturating_sub(1));
                sel.saturating_sub(BANK_VISIBLE_PART_SLOTS / 2)
                    .min(total_parts - BANK_VISIBLE_PART_SLOTS)
            } else {
                let pending_idx = all_parts.iter().position(|entry| {
                    self.live.pending.iter().any(|p| {
                        matches!(p.change, Change::Launch { lane, part } if lane == lane_id && part == *entry)
                    })
                });
                let focus_idx = pending_idx
                    .or_else(|| overriding.and_then(|ov| all_parts.iter().position(|e| *e == ov)))
                    .or_else(|| {
                        staged_launch_part.and_then(|st| all_parts.iter().position(|e| *e == st))
                    })
                    .or_else(|| {
                        arranged.and_then(|arr| all_parts.iter().position(|e| e.part == arr))
                    });
                focus_idx.map_or(0, |idx| {
                    idx.saturating_sub(BANK_VISIBLE_PART_SLOTS / 2)
                        .min(total_parts - BANK_VISIBLE_PART_SLOTS)
                })
            };
            let parts = all_parts
                .iter()
                .enumerate()
                .skip(part_window_start)
                .take(BANK_VISIBLE_PART_SLOTS)
                .map(|(n, entry)| {
                    let pending = self.live.pending.iter().find(|p| {
                        matches!(p.change, Change::Launch { lane, part } if lane == lane_id && part == *entry)
                    });
                    let status = if let Some(p) = pending {
                        BankPartStatus::PendingLaunch {
                            bar: self.song_position(p.at) / 16 + 1,
                        }
                    } else if overriding == Some(*entry) {
                        BankPartStatus::Overriding
                    } else if arranged == Some(entry.part) {
                        BankPartStatus::Arranged
                    } else {
                        BankPartStatus::Idle
                    };
                    BankPartEntry {
                        slot_num: n + 1,
                        part_id: entry.part,
                        part_name: self
                            .part(entry.part)
                            .map_or_else(|| "?".into(), |p| p.name.clone()),
                        phase: entry.phase,
                        offset_steps: entry.offset_steps,
                        status,
                        staged_launch: staged_launch_part == Some(*entry),
                        staged_release: staged_release && overriding == Some(*entry),
                    }
                })
                .collect();
            lanes.push(BankLaneGroup {
                lane_id,
                lane_name: lane.name.clone(),
                key_char,
                muted: self.flags().lane_muted(lane_id),
                solo: self.flags().lane_solo(lane_id),
                staged_mute: self.staged_lane_mute(lane_id),
                staged_solo: self.staged_lane_solo(lane_id),
                staged_release,
                total_parts,
                part_window_start,
                parts,
            });
        }

        let patterns = self
            .visible_patterns()
            .into_iter()
            .map(|p| BankPatternItem {
                id: p.id,
                name: p.name.clone(),
                kind_label: kind_label(&p.kind),
                period: self.pattern_period(p.id),
                selected: Some(p.id) == self.selected_pattern,
            })
            .collect();

        BankDeckView {
            mode: self.view_state.bank_mode,
            jam_active: self.is_jam(),
            zoomed: self.layout.zoomed(),
            staging_count: self.staging().map(<[_]>::len),
            selected_lane_idx,
            selected_part_idx: self.view_state.bank_part_idx,
            search_query: self.search_line().map(str::to_owned),
            sections,
            lanes,
            patterns,
            total_patterns: project.patterns.len(),
        }
    }
}
