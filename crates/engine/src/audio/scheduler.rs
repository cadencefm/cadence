//! Sample-accurate step clock and zero-allocation sub-step event scheduler.
use super::*;
use cadence_core::dsp::mix_seed;
use cadence_core::params::{InstrumentParam, RackParam};
use cadence_core::score::{ParamLock, ScoreEvent, ScoreWindow, MAX_INSTRUMENTS};
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) const SUB_EVENT_QUEUE: usize = 256;
pub(super) const MAX_INLINE_LOCKS: usize = 16;

const EMPTY_LOCK: ParamLock = ParamLock {
    target: InstrumentParam::Rack {
        param: RackParam::Gain,
    },
    value: 0.0,
};

#[derive(Clone)]
struct PendingSubEvent {
    fire_frame: u64,
    abs_step: u64,
    sub_ord: u32,
    seed: u32,
    event: ScoreEvent,
    locks: [ParamLock; MAX_INLINE_LOCKS],
    lock_count: usize,
}

/// Action decided at each step boundary before advancing the transport cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StepAction {
    /// Stop transport and park the step cursor at the returned step.
    StopAt(u64),
    /// Trigger the score events for the returned absolute step and advance.
    Trigger(u64),
}

/// Real-time step-clock and fixed-capacity sub-step delay queue.
///
/// The sub-event queue is heap-boxed once in [`StepScheduler::new`] to keep
/// debug stack frames compact and all scheduling/firing strictly zero-alloc.
pub(super) struct StepScheduler {
    frame: u64,
    /// Fractional progress within the current 16th-note step (`[0.0, 1.0)` while
    /// advancing; `>= 1.0` triggers the next step on the next playing frame).
    step_phase: f64,
    last_abs_step: u64,
    pending_sub: Box<[Option<PendingSubEvent>]>,
    pending_count: usize,
    earliest_fire_frame: u64,
}

impl StepScheduler {
    pub(super) fn new() -> Self {
        let pending_sub = (0..SUB_EVENT_QUEUE)
            .map(|_| None)
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            frame: 0,
            step_phase: 1.0,
            last_abs_step: 0,
            pending_sub,
            pending_count: 0,
            earliest_fire_frame: u64::MAX,
        }
    }

    pub(super) fn clear_pending(&mut self) {
        if self.pending_count > 0 {
            for slot in self.pending_sub.iter_mut() {
                *slot = None;
            }
            self.pending_count = 0;
        }
        self.earliest_fire_frame = u64::MAX;
    }

    #[cfg(test)]
    #[inline]
    pub(super) fn pending_count(&self) -> usize {
        self.pending_count
    }

    #[cfg(test)]
    #[inline]
    pub(super) fn earliest_fire_frame(&self) -> u64 {
        self.earliest_fire_frame
    }

    /// Reset frame clock, step phase, and pending sub-events to `start_step`.
    pub(super) fn reset(&mut self, start_step: u64) {
        self.frame = 0;
        self.step_phase = 1.0;
        self.last_abs_step = start_step;
        self.clear_pending();
    }

    /// Seek step position and discard queued sub-events without resetting frame count.
    pub(super) fn seek(&mut self, step: u64) {
        self.last_abs_step = step;
        self.step_phase = 1.0;
        self.clear_pending();
    }

    /// Return `(last_abs_step, frac)` for control-rate automation and snapshot reporting.
    #[inline]
    pub(super) fn step_position(&self, playing: bool) -> (u64, f64) {
        let frac = if playing && self.step_phase < 1.0 {
            self.step_phase.clamp(0.0, 0.9999)
        } else {
            0.0
        };
        (self.last_abs_step, frac)
    }

    #[inline]
    pub(super) fn is_step_due(&self) -> bool {
        self.step_phase + 1e-9 >= 1.0
    }

    #[inline]
    pub(super) fn consume_step_phase(&mut self) {
        self.step_phase = (self.step_phase - 1.0).max(0.0);
    }

    #[inline]
    pub(super) fn advance_frame(&mut self, step_seconds: f64, sample_rate: f64) {
        let step_frames = (step_seconds * sample_rate).max(1.0);
        self.frame += 1;
        self.step_phase += 1.0 / step_frames;
    }

    /// Determine whether `play_step` triggers normally or hits a stop boundary.
    pub(super) fn resolve_step_action(project: &ScoreWindow, play_step: u64) -> StepAction {
        if let Some(window) = project.live_window {
            if let Some(stop) = window.stop_at.filter(|&stop| play_step >= stop) {
                StepAction::StopAt(stop)
            } else {
                StepAction::Trigger(play_step)
            }
        } else {
            let total = project.arrangement_steps().max(1) as u64;
            if project.end_rule == EndRule::Stop && play_step >= total {
                StepAction::StopAt(0)
            } else {
                StepAction::Trigger(play_step)
            }
        }
    }

    /// Fire downbeat events immediately via `on_immediate` and enqueue sub-step
    /// offset events into the fixed-capacity delay queue.
    pub(super) fn schedule_step(
        &mut self,
        project: &ScoreWindow,
        abs_step: u64,
        sample_rate: f64,
        event_overflows: &AtomicU64,
        mut on_immediate: impl FnMut(&ScoreEvent, &[ParamLock], u32, f64),
    ) {
        self.last_abs_step = abs_step;
        let Some(slot) = project.step_slot(abs_step) else {
            return;
        };
        let (start, end) = project.step_ranges[slot];
        if start == end {
            return;
        }
        let step_frames = (project.step_seconds() * sample_rate).max(1.0);
        // Clamp sub-step delay strictly inside the current step so offset=0.9999
        // always fires before the next downbeat.
        let max_delay = (step_frames.floor() as u64).saturating_sub(1).max(1);
        let mut sub_counts = [0_u32; MAX_INSTRUMENTS];
        for idx in start as usize..end as usize {
            let Some(ev) = project.events.get(idx).cloned() else {
                continue;
            };
            let inst_idx = usize::from(ev.inst_idx);
            let Some(inst) = project.instruments.get(inst_idx) else {
                continue;
            };
            let inst_id = inst.id as u32;
            let sub_ord = sub_counts[inst_idx];
            sub_counts[inst_idx] = sub_ord.wrapping_add(1);
            let seed = mix_seed(0, abs_step as u32)
                ^ inst_id
                ^ sub_ord.wrapping_mul(cadence_core::dsp::SECONDARY_WEYL_SALT_32);
            let mut inline_locks = [EMPTY_LOCK; MAX_INLINE_LOCKS];
            let lock_slice = project.event_locks(&ev);
            let lock_count = lock_slice.len().min(MAX_INLINE_LOCKS);
            inline_locks[..lock_count].copy_from_slice(&lock_slice[..lock_count]);

            if ev.offset <= 1e-4 {
                on_immediate(&ev, &inline_locks[..lock_count], seed, abs_step as f64);
            } else {
                let delay =
                    ((f64::from(ev.offset) * step_frames).round() as u64).clamp(1, max_delay);
                let fire_frame = self.frame.saturating_add(delay);
                if let Some(queue_slot) = self.pending_sub.iter_mut().find(|s| s.is_none()) {
                    *queue_slot = Some(PendingSubEvent {
                        fire_frame,
                        abs_step,
                        sub_ord,
                        seed,
                        event: ev,
                        locks: inline_locks,
                        lock_count,
                    });
                    self.pending_count += 1;
                    self.earliest_fire_frame = self.earliest_fire_frame.min(fire_frame);
                } else {
                    event_overflows.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    /// Drain and fire every pending sub-step event due on or before the current
    /// frame in deterministic `(fire_frame, offset, inst_idx, sub_ord)` order.
    pub(super) fn fire_due(&mut self, mut on_due: impl FnMut(&ScoreEvent, &[ParamLock], u32, f64)) {
        if self.pending_count == 0 || self.frame < self.earliest_fire_frame {
            return;
        }
        let mut due_indices = [0usize; SUB_EVENT_QUEUE];
        let mut due_count = 0usize;
        let mut next_earliest = u64::MAX;
        let mut visited = 0usize;
        for (idx, slot) in self.pending_sub.iter().enumerate() {
            if let Some(p) = slot.as_ref() {
                visited += 1;
                if p.fire_frame <= self.frame {
                    due_indices[due_count] = idx;
                    due_count += 1;
                } else {
                    next_earliest = next_earliest.min(p.fire_frame);
                }
                if visited == self.pending_count {
                    break;
                }
            }
        }
        if due_count == 0 {
            self.earliest_fire_frame = next_earliest;
            return;
        }
        if due_count > 1 {
            let pending_ref = &self.pending_sub;
            due_indices[..due_count].sort_unstable_by(|&a, &b| {
                let key = |idx: usize| {
                    pending_ref[idx]
                        .as_ref()
                        .map_or((u64::MAX, u32::MAX, u8::MAX, u32::MAX), |p| {
                            (
                                p.fire_frame,
                                p.event.offset.max(0.0).to_bits(),
                                p.event.inst_idx,
                                p.sub_ord,
                            )
                        })
                };
                key(a).cmp(&key(b))
            });
        }
        for &idx in &due_indices[..due_count] {
            if let Some(pending) = self.pending_sub[idx].take() {
                self.pending_count = self.pending_count.saturating_sub(1);
                let mod_pos = pending.abs_step as f64 + f64::from(pending.event.offset);
                let count = pending.lock_count.min(MAX_INLINE_LOCKS);
                on_due(
                    &pending.event,
                    &pending.locks[..count],
                    pending.seed,
                    mod_pos,
                );
            }
        }
        self.earliest_fire_frame = if self.pending_count == 0 {
            u64::MAX
        } else {
            next_earliest
        };
    }
}
