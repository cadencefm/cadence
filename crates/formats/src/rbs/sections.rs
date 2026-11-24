//! Musical form section detection and loop-region pinning for imported `.rbs` songs.

use super::model::RbsSong;
use cadence_core::material::{
    eval_value_points, Modulation, ModulationTarget, Pattern, PatternKind, Section, STEPS_PER_BAR,
};
use cadence_core::mixer::{Channel, ChannelRef};
use std::collections::HashMap;

/// Whether `channel` is audible during bar `bar_idx` after accounting for both static
/// `strip.gain` and any Song-Mode `ModulationTarget::ChannelGain` fade-to-0 / silence lane.
fn bar_channel_audible(
    channel: &Channel,
    bar_idx: usize,
    modulations: &[Modulation],
    patterns: &[Pattern],
) -> bool {
    const AUDIBLE_GAIN_THRESHOLD: f32 = 0.015;
    if channel.strip.gain <= AUDIBLE_GAIN_THRESHOLD {
        return false;
    }
    let cref = ChannelRef::Channel(channel.id);
    let Some(gain_mod) = modulations
        .iter()
        .rev()
        .find(|m| m.target == ModulationTarget::ChannelGain { channel: cref })
    else {
        return true;
    };
    let Some(PatternKind::Values {
        period,
        mode,
        points,
        ..
    }) = patterns
        .iter()
        .find(|p| p.id == gain_mod.pattern)
        .map(|p| &p.kind)
    else {
        return true;
    };
    let start = (bar_idx * STEPS_PER_BAR) as u32;
    let end = start + STEPS_PER_BAR as u32;
    (start..end).any(|step| {
        channel.strip.gain
            * eval_value_points(*period, *mode, points, f64::from(step)).unwrap_or(1.0)
            > AUDIBLE_GAIN_THRESHOLD
    })
}

impl RbsSong {
    /// Detect musical form sections from `loop_start_bar..loop_end_bar` and real bar-by-bar
    /// arrangement transitions (active device mask changes — including `ChannelGain` fade-to-0 /
    /// silence lanes — & multi-device pattern switches), collapsing 1–3 bar micro-blips, keeping
    /// sections in clean musical lengths (4–32 bars whenever possible), capping at `<= 32`
    /// sections, and assigning descriptive musical labels.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn detect_musical_sections(
        &self,
        bars: usize,
        dev_count: usize,
        bar_states: &[Vec<Option<(usize, u8)>>; 4],
        pat_non_empty: &[Vec<bool>; 4],
        channels: &[Channel],
        modulations: &[Modulation],
        patterns: &[Pattern],
        fresh: &mut dyn FnMut() -> u64,
    ) -> Vec<Section> {
        const MAX_RBS_SECTIONS: usize = 32;
        const MIN_SECTION_BARS: usize = 4;
        const MAX_SECTION_BARS: usize = 32;

        let mut active_mask = vec![0u8; bars];
        let mut pat_sig: Vec<[Option<(usize, u8)>; 4]> = vec![[None; 4]; bars];
        for b in 0..bars {
            let mut mask = 0u8;
            for d in 0..dev_count {
                if let Some(ch) = channels.get(d) {
                    if !bar_channel_audible(ch, b, modulations, patterns) {
                        continue;
                    }
                } else if !self.mixer.channels[d].enabled {
                    continue;
                }
                if let Some((p_idx, var)) = bar_states[d].get(b).copied().flatten() {
                    if pat_non_empty[d].get(p_idx).copied().unwrap_or(false) {
                        mask |= 1 << d;
                        pat_sig[b][d] = Some((p_idx, var));
                    }
                }
            }
            active_mask[b] = mask;
        }

        let loop_region = if self.loop_on
            && self.loop_start_bar < self.loop_end_bar
            && (self.loop_start_bar as usize) < bars
        {
            let ls = self.loop_start_bar as usize;
            let le = (self.loop_end_bar as usize).min(bars);
            (ls < le).then_some((ls, le))
        } else {
            None
        };

        #[derive(Clone, Copy)]
        struct Boundary {
            bar: usize,
            priority: u32,
            pinned: bool,
        }

        let mut bounds = Vec::with_capacity(64);
        bounds.push(Boundary {
            bar: 0,
            priority: u32::MAX,
            pinned: true,
        });

        for b in 1..bars {
            let is_loop_edge = loop_region.is_some_and(|(ls, le)| b == ls || b == le);
            let prev_mask = active_mask[b - 1];
            let cur_mask = active_mask[b];
            let mask_changed = cur_mask != prev_mask;
            let switches = (0..4)
                .filter(|&d| pat_sig[b][d] != pat_sig[b - 1][d])
                .count();

            if is_loop_edge || mask_changed || switches >= 2 {
                let prev_drums = (prev_mask & 0x0C) != 0;
                let cur_drums = (cur_mask & 0x0C) != 0;
                let grid_bonus = if b % 16 == 0 {
                    8
                } else if b % 8 == 0 {
                    5
                } else if b % 4 == 0 {
                    3
                } else {
                    0
                };
                let mut priority = grid_bonus;
                if is_loop_edge {
                    priority += 100;
                }
                if cur_drums != prev_drums {
                    priority += 40;
                } else if mask_changed {
                    priority += 25;
                } else {
                    priority += 10 + (switches as u32) * 3;
                }
                bounds.push(Boundary {
                    bar: b,
                    priority,
                    pinned: is_loop_edge,
                });
            }
        }

        bounds.push(Boundary {
            bar: bars,
            priority: u32::MAX,
            pinned: true,
        });

        // 1. Collapse 1–3 bar micro-blips unless both endpoints are pinned (start/end/loop).
        loop {
            let mut best_gap: Option<(usize, usize)> = None;
            for i in 0..bounds.len().saturating_sub(1) {
                let gap = bounds[i + 1].bar - bounds[i].bar;
                if gap < MIN_SECTION_BARS
                    && (!bounds[i].pinned || !bounds[i + 1].pinned)
                    && best_gap.is_none_or(|(_, g)| gap < g)
                {
                    best_gap = Some((i, gap));
                }
            }
            let Some((i, _)) = best_gap else {
                break;
            };
            let remove_idx = if bounds[i].pinned {
                i + 1
            } else if bounds[i + 1].pinned || bounds[i].priority <= bounds[i + 1].priority {
                i
            } else {
                i + 1
            };
            bounds.remove(remove_idx);
        }

        // 2. Cap total section count at `<= 32` by dropping lowest-priority removable boundaries.
        while bounds.len().saturating_sub(1) > MAX_RBS_SECTIONS {
            let mut best: Option<(usize, (u8, u32, usize))> = None;
            for i in 1..bounds.len() - 1 {
                if bounds[i].pinned {
                    continue;
                }
                let merged_len = bounds[i + 1].bar - bounds[i - 1].bar;
                let key = (
                    u8::from(merged_len > MAX_SECTION_BARS),
                    bounds[i].priority,
                    merged_len,
                );
                if best.is_none_or(|(_, best_key)| key < best_key) {
                    best = Some((i, key));
                }
            }
            let Some((remove_idx, _)) = best else {
                break;
            };
            bounds.remove(remove_idx);
        }

        // 3. Subdivide any section longer than 32 bars cleanly on phrase grid boundaries.
        while bounds.len().saturating_sub(1) < MAX_RBS_SECTIONS {
            let Some((long_idx, long_len)) = (0..bounds.len().saturating_sub(1))
                .map(|i| (i, bounds[i + 1].bar - bounds[i].bar))
                .max_by_key(|&(_, len)| len)
            else {
                break;
            };
            if long_len <= MAX_SECTION_BARS {
                break;
            }
            let left = bounds[long_idx].bar;
            let right = bounds[long_idx + 1].bar;
            let mid = (left + right) / 2;
            let lo = left + MIN_SECTION_BARS;
            let hi = right - MIN_SECTION_BARS;
            let split = [16usize, 8, 4, 1]
                .into_iter()
                .find_map(|grid| {
                    (lo..=hi).filter(|&b| b % grid == 0).min_by_key(|&b| {
                        (b.abs_diff(mid), usize::from(pat_sig[b] == pat_sig[b - 1]))
                    })
                })
                .unwrap_or(mid);
            bounds.insert(
                long_idx + 1,
                Boundary {
                    bar: split,
                    priority: 1,
                    pinned: false,
                },
            );
        }

        // 4. Classify each section by active musical instrumentation & form role.
        struct SpanStats {
            start: usize,
            end: usize,
            has_synths: bool,
            has_drums: bool,
            avg_devs: f32,
        }

        let mut stats = Vec::with_capacity(bounds.len().saturating_sub(1));
        for i in 0..bounds.len().saturating_sub(1) {
            let start = bounds[i].bar;
            let end = bounds[i + 1].bar;
            let len = (end - start).max(1);
            let synths_bars = (start..end)
                .filter(|&b| (active_mask[b] & 0x03) != 0)
                .count();
            let drums_bars = (start..end)
                .filter(|&b| (active_mask[b] & 0x0C) != 0)
                .count();
            let total_devs: usize = (start..end)
                .map(|b| active_mask[b].count_ones() as usize)
                .sum();
            stats.push(SpanStats {
                start,
                end,
                has_synths: synths_bars * 2 >= len || (synths_bars > 0 && drums_bars == 0),
                has_drums: drums_bars * 2 >= len || (drums_bars > 0 && synths_bars == 0),
                avg_devs: total_devs as f32 / len as f32,
            });
        }

        let song_has_drums = active_mask.iter().any(|&m| (m & 0x0C) != 0);
        let song_max_devs = stats.iter().map(|s| s.avg_devs).fold(0.0_f32, f32::max);
        let explicit_subloop = loop_region.filter(|&(ls, le)| ls > 0 || le < bars);

        let mut base_labels: Vec<&'static str> = Vec::with_capacity(stats.len());
        for (idx, st) in stats.iter().enumerate() {
            let prev = idx.checked_sub(1).map(|p| &stats[p]);
            let label = if idx == 0 {
                "Intro"
            } else if explicit_subloop == Some((st.start, st.end)) {
                "Loop"
            } else if idx + 1 == stats.len()
                && stats.len() >= 3
                && ((!st.has_drums && song_has_drums)
                    || st.avg_devs + 0.25 < song_max_devs
                    || prev.is_some_and(|p| st.avg_devs + 0.25 < p.avg_devs))
            {
                "Outro"
            } else if !st.has_drums
                && ((song_has_drums && prev.is_some_and(|p| p.has_drums)) || !st.has_synths)
            {
                "Break"
            } else if st.has_drums && prev.is_some_and(|p| !p.has_drums) && st.has_synths {
                "Drop"
            } else if st.has_drums && !st.has_synths {
                "Drums"
            } else if st.has_synths && !st.has_drums {
                "Acid"
            } else {
                "Main"
            };
            base_labels.push(label);
        }

        let mut total_counts: HashMap<&'static str, usize> = HashMap::new();
        for &lbl in &base_labels {
            *total_counts.entry(lbl).or_insert(0) += 1;
        }
        let mut seen_counts: HashMap<&'static str, usize> = HashMap::new();

        let mut sections = Vec::with_capacity(stats.len());
        for (st, lbl) in stats.iter().zip(base_labels) {
            let count = total_counts[lbl];
            let name = if count <= 1 {
                lbl.to_string()
            } else {
                let n = seen_counts.entry(lbl).or_insert(0);
                *n += 1;
                format!("{lbl} {n}")
            };
            sections.push(Section {
                id: fresh(),
                name,
                start_step: (st.start * STEPS_PER_BAR) as u32,
                length_steps: ((st.end - st.start) * STEPS_PER_BAR) as u32,
            });
        }
        sections
    }
}
