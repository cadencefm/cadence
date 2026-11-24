//! Pure musical timeline mapping transport steps onto song steps, loop passes,
//! and end-of-song rules.
use crate::music::EndRule;
use serde::{Deserialize, Serialize};

/// One contiguous mapping from transport steps onto the song.
///
/// Starting at transport step `from`, the song is at `song`. If `region` is
/// `Some((start, length))`, song steps at or past `start` repeat `[start, start + length)`.
/// Otherwise `end` decides what happens once the song reaches its length.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Segment {
    pub from: u64,
    pub song: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<(u64, u64)>,
    pub end: EndRule,
}

impl Segment {
    pub const fn new(from: u64, song: u64, region: Option<(u64, u64)>, end: EndRule) -> Self {
        Self {
            from,
            song,
            region,
            end,
        }
    }

    /// The song step heard at `transport`, or `None` past a continuing/stopping end
    /// or before `self.from`.
    pub fn song_step(&self, transport: u64, song_length: u64) -> Option<u64> {
        let delta = transport.checked_sub(self.from)?;
        let mut step = self.song.checked_add(delta)?;
        if let Some((start, len)) = self.region {
            if len > 0 && step >= start {
                step = start + (step - start) % len;
            }
        }
        if song_length == 0 {
            return None;
        }
        if step >= song_length {
            return match self.end {
                EndRule::Loop => Some(step % song_length),
                EndRule::Stop | EndRule::Continue => None,
            };
        }
        Some(step)
    }

    /// The song step at `transport` without wrapping or stopping at the song's
    /// current length (used when extending a song past its end, e.g. Keep).
    pub fn song_step_unbounded(&self, transport: u64) -> Option<u64> {
        let delta = transport.checked_sub(self.from)?;
        let mut step = self.song.checked_add(delta)?;
        if let Some((start, len)) = self.region {
            if len > 0 && step >= start {
                step = start + (step - start) % len;
            }
        }
        Some(step)
    }

    /// Transport step at which this segment stops automatically, if `EndRule::Stop`
    /// is active and no loop region keeps it cycling.
    pub fn stop_at(&self, song_length: u64) -> Option<u64> {
        if self.end != EndRule::Stop || self.region.is_some_and(|(_, len)| len > 0) {
            return None;
        }
        if self.song >= song_length {
            Some(self.from)
        } else {
            self.from.checked_add(song_length - self.song)
        }
    }

    /// Transport step at which the current loop pass (or full-song loop pass)
    /// finishes after `transport`.
    pub fn pass_end(&self, transport: u64, song_length: u64) -> Option<u64> {
        let delta = transport.checked_sub(self.from)?;
        let raw = self.song.checked_add(delta)?;
        if let Some((start, len)) = self.region {
            if len == 0 {
                return None;
            }
            if raw < start {
                return transport.checked_add((start - raw).checked_add(len)?);
            }
            let offset = (raw - start) % len;
            return transport.checked_add(len - offset);
        }
        if self.end == EndRule::Loop && song_length > 0 {
            let offset = raw % song_length;
            return transport.checked_add(song_length - offset);
        }
        None
    }
}

/// Ordered, non-empty sequence of [`Segment`] transitions over transport time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timeline {
    segments: Vec<Segment>,
}

impl Timeline {
    pub fn new(initial: Segment) -> Self {
        Self {
            segments: vec![initial],
        }
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// The active segment at `transport`.
    pub fn segment_at(&self, transport: u64) -> Segment {
        let index = self.segments.partition_point(|s| s.from <= transport);
        self.segments[index.saturating_sub(1)]
    }

    /// The song step heard at `transport`, or `None` past a continuing/stopping end.
    pub fn song_step(&self, transport: u64, song_length: u64) -> Option<u64> {
        self.segment_at(transport).song_step(transport, song_length)
    }

    /// Active loop region `(start, length)` at `transport`, if looping.
    pub fn looping_at(&self, transport: u64) -> Option<(u64, u64)> {
        self.segment_at(transport).region
    }

    /// Transport step where the current loop or song pass ends after `transport`.
    pub fn pass_end(&self, transport: u64, song_length: u64) -> Option<u64> {
        self.segment_at(transport).pass_end(transport, song_length)
    }

    /// Transport step where playback stops automatically if the segment active at
    /// `transport` reaches a stopping end before any later remap.
    pub fn stop_at(&self, transport: u64, song_length: u64) -> Option<u64> {
        let index = self.segments.partition_point(|s| s.from <= transport);
        let seg = self.segments[index.saturating_sub(1)];
        let stop = seg.stop_at(song_length)?;
        let next_from = self.segments.get(index).map(|s| s.from);
        if next_from.is_some_and(|next| next <= stop) {
            return None;
        }
        Some(stop)
    }

    /// Schedule or apply a new segment starting at `segment.from`, replacing any
    /// segments at or after `segment.from`.
    pub fn remap(&mut self, segment: Segment) {
        self.segments.retain(|s| s.from < segment.from);
        self.segments.push(segment);
    }

    /// Cancel any scheduled remaps starting strictly after `transport` (keeping at
    /// least the initial segment).
    pub fn cancel_after(&mut self, transport: u64) {
        if self.segments.len() <= 1 {
            return;
        }
        let keep = self
            .segments
            .partition_point(|s| s.from <= transport)
            .max(1);
        self.segments.truncate(keep);
    }

    /// The last landed segment at or before `transport`.
    pub fn landed(&self, transport: u64) -> Segment {
        self.segment_at(transport)
    }

    /// Drop historical segments strictly superseded at `transport` so live memory
    /// stays bounded.
    pub fn prune_before(&mut self, transport: u64) {
        let index = self.segments.partition_point(|s| s.from <= transport);
        let first = index.saturating_sub(1);
        if first > 0 {
            self.segments.drain(0..first);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn song_wrap_continue_and_stop_rules() {
        let len = 64;
        let looping = Timeline::new(Segment::new(0, 0, None, EndRule::Loop));
        assert_eq!(looping.song_step(0, len), Some(0));
        assert_eq!(looping.song_step(63, len), Some(63));
        assert_eq!(looping.song_step(64, len), Some(0));
        assert_eq!(looping.song_step(130, len), Some(2));
        assert_eq!(looping.pass_end(0, len), Some(64));
        assert_eq!(looping.pass_end(65, len), Some(128));
        assert_eq!(looping.stop_at(0, len), None);

        let continuing = Timeline::new(Segment::new(0, 0, None, EndRule::Continue));
        assert_eq!(continuing.song_step(63, len), Some(63));
        assert_eq!(continuing.song_step(64, len), None);
        assert_eq!(continuing.pass_end(0, len), None);
        assert_eq!(continuing.stop_at(0, len), None);

        let stopping = Timeline::new(Segment::new(0, 0, None, EndRule::Stop));
        assert_eq!(stopping.song_step(63, len), Some(63));
        assert_eq!(stopping.song_step(64, len), None);
        assert_eq!(stopping.stop_at(0, len), Some(64));
        assert_eq!(stopping.stop_at(16, len), Some(64));

        // Zero-length region does not loop, so EndRule::Stop still fires at song end.
        let zero_region_stop = Segment::new(0, 0, Some((16, 0)), EndRule::Stop);
        assert_eq!(zero_region_stop.stop_at(len), Some(64));
    }

    #[test]
    fn exact_loop_passes_and_pass_end() {
        let len = 256;
        let tl = Timeline::new(Segment::new(16, 64, Some((64, 32)), EndRule::Stop));
        assert_eq!(tl.looping_at(16), Some((64, 32)));
        assert_eq!(tl.song_step(16, len), Some(64));
        assert_eq!(tl.song_step(47, len), Some(95));
        assert_eq!(tl.song_step(48, len), Some(64));
        assert_eq!(tl.song_step(80, len), Some(64));
        assert_eq!(tl.pass_end(16, len), Some(48));
        assert_eq!(tl.pass_end(30, len), Some(48));
        assert_eq!(tl.pass_end(48, len), Some(80));
        assert_eq!(tl.stop_at(16, len), None, "looping prevents auto-stop");

        // Entering a loop region from before its start plays up to start + len on pass 1.
        let lead_in = Timeline::new(Segment::new(0, 48, Some((64, 32)), EndRule::Loop));
        assert_eq!(lead_in.song_step(0, len), Some(48));
        assert_eq!(lead_in.song_step(16, len), Some(64));
        assert_eq!(lead_in.song_step(48, len), Some(64));
        assert_eq!(lead_in.pass_end(0, len), Some(48));
        assert_eq!(lead_in.pass_end(16, len), Some(48));
    }

    #[test]
    fn remap_lands_on_its_step_cancel_and_prune_work() {
        let len = 256;
        let mut tl = Timeline::new(Segment::new(0, 0, None, EndRule::Loop));
        tl.remap(Segment::new(64, 128, Some((128, 64)), EndRule::Loop));
        assert_eq!(tl.song_step(63, len), Some(63));
        assert_eq!(tl.song_step(64, len), Some(128));
        assert_eq!(tl.song_step(127, len), Some(191));
        assert_eq!(tl.song_step(128, len), Some(128));
        assert_eq!(tl.landed(63).from, 0);
        assert_eq!(tl.landed(64).from, 64);

        // Replacing at same step replaces the segment; cancelling drops future ones.
        tl.remap(Segment::new(64, 192, None, EndRule::Stop));
        assert_eq!(tl.song_step(64, len), Some(192));
        assert_eq!(tl.stop_at(64, len), Some(128));
        tl.cancel_after(32);
        assert_eq!(tl.segments().len(), 1);
        assert_eq!(tl.song_step(64, len), Some(64));

        // Pruning drops historical segments strictly before active.
        tl.remap(Segment::new(64, 128, None, EndRule::Loop));
        tl.remap(Segment::new(128, 0, None, EndRule::Loop));
        tl.prune_before(100);
        assert_eq!(tl.segments().len(), 2);
        assert_eq!(tl.segments()[0].from, 64);
        assert_eq!(tl.segments()[1].from, 128);
    }
}
