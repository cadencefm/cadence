//! Live timing and lane part entries.
//!
//! Each lane stores the `LanePart` entries it can launch, and the project holds
//! one launch-timing configuration. What is playing right now lives in
//! `crate::live`, never in the song.
use super::{Edit, Id, PhaseMode, Project, MAX_DERIVED_OFFSET};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_PARTS_ON_LANES: usize = 256;
pub const MAX_LANE_PARTS: usize = 32;
pub const MAX_BOUNDARY_BARS: u32 = 64;
pub const MAX_LATE_TOLERANCE: u32 = 4;

/// A launchable part on a lane: a part ID with its phase and offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanePart {
    pub part: Id,
    pub phase: PhaseMode,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub offset_steps: i32,
}

impl LanePart {
    pub fn new(part: Id) -> Self {
        Self {
            part,
            phase: PhaseMode::Song,
            offset_steps: 0,
        }
    }
}

/// Where a live change lands on the playing clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "at", rename_all = "snake_case", deny_unknown_fields)]
pub enum Boundary {
    Now,
    Beat,
    Bar,
    Bars {
        count: u32,
    },
    /// The next step where this pattern's repeat period begins.
    Cycle {
        pattern: Id,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditTiming {
    /// Song edits made while playing are heard on the next step.
    #[default]
    Now,
    /// Song edits made while playing follow launch timing.
    Launch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveTiming {
    pub launch: Boundary,
    pub edits: EditTiming,
    /// A change made within this many steps after a boundary lands on it.
    pub late_tolerance_steps: u32,
}

impl Default for LiveTiming {
    fn default() -> Self {
        Self {
            launch: Boundary::Bar,
            edits: EditTiming::Now,
            late_tolerance_steps: 1,
        }
    }
}

impl LiveTiming {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

fn is_zero(value: &i32) -> bool {
    *value == 0
}

impl Boundary {
    /// Steps between boundaries, or `None` for `now`.
    pub fn quantum(self, project: &Project) -> Result<Option<u64>> {
        Ok(match self {
            Self::Now => None,
            Self::Beat => Some(4),
            Self::Bar => Some(16),
            Self::Bars { count } => Some(u64::from(count.max(1)) * 16),
            Self::Cycle { pattern } => Some(project.pattern_period(pattern)?.max(1)),
        })
    }
    pub fn label(self, project: &Project) -> String {
        match self {
            Self::Now => "now".into(),
            Self::Beat => "beat".into(),
            Self::Bar => "bar".into(),
            Self::Bars { count } => format!("{count} bars"),
            Self::Cycle { pattern } => format!(
                "cycle {}",
                project.pattern(pattern).map_or("?", |p| p.name.as_str())
            ),
        }
    }
    /// Parses `now`, `beat`, `bar`, `4`, `4bars`, `4 bars` or `cycle NAME`.
    pub fn parse(text: &str, project: &Project) -> Result<Self> {
        let text = text.trim();
        let lower = text.to_ascii_lowercase();
        if let Some(name) = lower.strip_prefix("cycle") {
            let name = text[text.len() - name.len()..].trim().trim_matches('"');
            let pattern = project
                .patterns
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(name))
                .with_context(|| format!("No pattern named {name}"))?;
            return Ok(Self::Cycle {
                pattern: pattern.id,
            });
        }
        let boundary = match lower.as_str() {
            "now" | "immediate" => Self::Now,
            "beat" => Self::Beat,
            "bar" | "1" | "1bar" | "1 bar" => Self::Bar,
            other => {
                let digits = other
                    .trim_end_matches("bars")
                    .trim_end_matches("bar")
                    .trim();
                let count: u32 = digits.parse().with_context(|| {
                    format!("Unknown boundary {text}; use now, beat, bar, N bars or cycle PATTERN")
                })?;
                if count == 1 {
                    Self::Bar
                } else {
                    Self::Bars { count }
                }
            }
        };
        Ok(boundary)
    }
}

pub fn edit_label(edit: &Edit) -> &'static str {
    match edit {
        Edit::PutPattern { .. } => "pattern",
        Edit::SetNote { .. } | Edit::SetRhythm { .. } | Edit::SetValue { .. } => "step",
        Edit::PutPart { .. } => "part",
        Edit::CreateInstrument { .. }
        | Edit::UpdateInstrumentMeta { .. }
        | Edit::UpdateRackPatch { .. }
        | Edit::AddRackZone { .. }
        | Edit::RemoveRackZone { .. }
        | Edit::ReorderRackZone { .. }
        | Edit::AddRackModule { .. }
        | Edit::RemoveRackModule { .. }
        | Edit::RenameRackModule { .. }
        | Edit::ConnectCable { .. }
        | Edit::DisconnectCable { .. }
        | Edit::SetCableAmount { .. }
        | Edit::PinPanelControl { .. }
        | Edit::UnpinPanelControl { .. } => "instrument",
        Edit::CreateChannel { .. }
        | Edit::UpdateChannelMeta { .. }
        | Edit::RemoveChannel { .. } => "channel",
        Edit::AddEffect { .. }
        | Edit::RemoveEffect { .. }
        | Edit::MoveEffect { .. }
        | Edit::UpdateEffect { .. } => "effect",
        Edit::SetTempo { .. } => "tempo",
        Edit::SetKey { .. } => "key",
        _ => "song",
    }
}

impl Project {
    pub(super) fn validate_live_setup(&self) -> Result<()> {
        let mut total_lane_parts = 0usize;
        for lane in &self.lanes {
            ensure!(
                lane.parts.len() <= MAX_LANE_PARTS,
                "Lane {} has more than {MAX_LANE_PARTS} parts",
                lane.name
            );
            let mut seen = HashSet::new();
            for entry in &lane.parts {
                ensure!(
                    seen.insert(entry.part),
                    "Lane {} lists part {} more than once",
                    lane.name,
                    entry.part
                );
                let part = self.part(entry.part).with_context(|| {
                    format!("Lane {} lists a missing part {}", lane.name, entry.part)
                })?;
                ensure!(
                    entry.offset_steps.unsigned_abs() <= MAX_DERIVED_OFFSET,
                    "Part {} offset exceeds bounds",
                    part.name
                );
                total_lane_parts += 1;
            }
        }
        ensure!(
            total_lane_parts <= MAX_PARTS_ON_LANES,
            "Too many lane part entries"
        );
        self.validate_boundary(self.timing.launch)?;
        ensure!(
            self.timing.late_tolerance_steps <= MAX_LATE_TOLERANCE,
            "Late tolerance must be 0..{MAX_LATE_TOLERANCE} steps"
        );
        Ok(())
    }

    fn validate_boundary(&self, boundary: Boundary) -> Result<()> {
        match boundary {
            Boundary::Bars { count } => ensure!(
                (1..=MAX_BOUNDARY_BARS).contains(&count),
                "Boundary must be 1..{MAX_BOUNDARY_BARS} bars"
            ),
            Boundary::Cycle { pattern } => {
                self.pattern(pattern)
                    .context("Boundary names a missing pattern")?;
            }
            _ => {}
        }
        Ok(())
    }
}
