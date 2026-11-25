//! Arrangement placement grid with live staging badges, override double-bars, and M/S pills.
use super::{draw_dashed_rect, draw_lane_meter_pill, draw_vertical_time_grid, TimelineGeom};
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::{Color, Theme};
use cadence_app::view::ArrangeDeckView;

pub(super) fn draw_placements(
    dl: &mut DisplayList,
    body: Rect,
    view: &ArrangeDeckView,
    geom: TimelineGeom,
    theme: &Theme,
) {
    if view.lanes.is_empty() {
        dl.text(
            body.inset(12.0, 12.0),
            "No lanes yet  ·  press np for a placed part, or n for creation menu",
            theme.font.body,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
        return;
    }

    // Build visual row table (lane row + optional dimmed `arr.` underlay when overridden).
    let mut rows: Vec<(usize, bool)> = Vec::new();
    for (i, lane) in view.lanes.iter().enumerate() {
        rows.push((i, false));
        if lane.override_info.is_some() {
            rows.push((i, true));
        }
    }
    let row_h = (body.h / rows.len().max(1) as f32).clamp(24.0, 42.0);
    let visible = (body.h / row_h).floor().max(1.0) as usize;
    let sel_row = rows
        .iter()
        .position(|&(i, arr)| !arr && view.lanes[i].selected)
        .unwrap_or(0);
    let first = sel_row.saturating_sub(visible / 2);

    // Edit cursor column highlight.
    let cur_bar_start = (view.cursor_step / 16) * 16;
    let cx0 = geom.step_to_x(f64::from(cur_bar_start));
    let cx1 = geom.step_to_x(f64::from(cur_bar_start + 16));

    for (vis_idx, &(lane_idx, is_arr)) in rows.iter().skip(first).take(visible).enumerate() {
        let lane = &view.lanes[lane_idx];
        let y = body.y + vis_idx as f32 * row_h;
        let row_rect = Rect::new(body.x, y, body.w, row_h);
        let lane_col = theme.track_color(lane.index);

        let has_staged = !is_arr
            && (!lane.staged.is_empty()
                || lane.staged_mute.is_some()
                || lane.staged_solo.is_some());

        if lane.selected && !is_arr {
            dl.fill(row_rect, theme.surface.active_row);
        } else if vis_idx % 2 == 1 {
            dl.fill(row_rect, theme.surface.recessed.with_alpha(0.35));
        }
        if has_staged {
            dl.fill(row_rect, theme.accent.modulation.with_alpha(0.08));
            dl.fill(
                Rect::new(body.x, y + 1.0, 3.0, (row_h - 2.0).max(1.0)),
                theme.accent.modulation,
            );
        }
        dl.fill(
            Rect::new(body.x, row_rect.bottom() - 1.0, body.w, 1.0),
            theme.border.subtle.with_alpha(0.6),
        );

        // Gutter: key + lane name + M/S pills (or dimmed `arr.`).
        if is_arr {
            dl.text(
                Rect::new(body.x + 26.0, y, geom.gutter_w - 30.0, row_h),
                "↪ arr.",
                theme.font.small,
                false,
                theme.text.muted,
                TextAlign::Left,
            );
        } else {
            let mut gx = body.x + 4.0;
            if let Some(k) = lane.key_char {
                let badge_w = if view.jam_active { 18.0 } else { 26.0 };
                let kr = Rect::new(gx, y + (row_h - 18.0) * 0.5, badge_w, 18.0);
                let label = if view.jam_active {
                    k.to_string()
                } else {
                    format!("⌥{k}")
                };
                if view.jam_active {
                    dl.panel(
                        kr,
                        lane_col.with_alpha(0.28),
                        lane_col,
                        theme.radii.badge,
                        1.0,
                        0.28,
                    );
                } else {
                    dl.rounded(kr, lane_col.with_alpha(0.14), theme.radii.badge);
                }
                dl.text(
                    kr,
                    label,
                    theme.font.small - if view.jam_active { 0.0 } else { 1.0 },
                    view.jam_active,
                    if view.jam_active {
                        lane_col
                    } else {
                        lane_col.with_alpha(0.85)
                    },
                    TextAlign::Center,
                );
                gx += badge_w + 4.0;
            }
            let meter_reserve = if view.show_lane_meters { 13.0 } else { 0.0 };
            let ms_w = 40.0 + meter_reserve;
            let name_w = (geom.gutter_w - (gx - body.x) - ms_w - 4.0).max(20.0);
            let max_c = (name_w / theme.char_width(theme.font.small)) as usize;
            dl.text(
                Rect::new(gx, y, name_w, row_h),
                trim_label(&lane.name, max_c),
                theme.font.small,
                lane.selected,
                if lane.selected {
                    theme.accent.focus
                } else if lane.muted {
                    theme.text.muted
                } else {
                    theme.text.primary
                },
                TextAlign::Left,
            );
            if view.show_lane_meters {
                let meter_h = (row_h - 8.0).clamp(14.0, 26.0);
                let meter_rect = Rect::new(
                    body.x + geom.gutter_w - 50.0,
                    y + (row_h - meter_h) * 0.5,
                    9.0,
                    meter_h,
                );
                draw_lane_meter_pill(
                    dl,
                    meter_rect,
                    lane.meter_peak,
                    lane.meter_hot,
                    lane.meter_clipped,
                    lane.muted,
                    theme,
                );
            }
            // M / S pill badges (with dashed modulation outline when mute/solo is staged).
            let m_rect = Rect::new(
                body.x + geom.gutter_w - 38.0,
                y + (row_h - 16.0) * 0.5,
                16.0,
                16.0,
            );
            let s_rect = Rect::new(
                body.x + geom.gutter_w - 20.0,
                y + (row_h - 16.0) * 0.5,
                16.0,
                16.0,
            );
            draw_ms_pill(
                dl,
                m_rect,
                "M",
                lane.muted,
                lane.staged_mute,
                theme.accent.alert,
                theme,
            );
            draw_ms_pill(
                dl,
                s_rect,
                "S",
                lane.solo,
                lane.staged_solo,
                theme.accent.warning_key,
                theme,
            );
        }

        // Gutter divider.
        dl.fill(
            Rect::new(geom.timeline_x - 3.0, y, 1.0, row_h),
            theme.border.hairline,
        );

        // Timeline clip region for blocks.
        let lane_tl = Rect::new(geom.timeline_x, y, geom.timeline_w, row_h);
        dl.push_clip(lane_tl);
        draw_vertical_time_grid(dl, lane_tl, view, geom, theme);

        // Cursor bar cell reticle on the selected lane.
        if lane.selected && !is_arr && cx1 > geom.timeline_x && cx0 < lane_tl.right() {
            dl.panel(
                Rect::new(cx0 + 1.0, y + 2.0, (cx1 - cx0 - 2.0).max(4.0), row_h - 5.0),
                theme.accent.focus.with_alpha(0.08),
                theme.accent.focus.with_alpha(0.75),
                theme.radii.badge,
                1.0,
                0.08,
            );
        }

        // Placement blocks.
        for pl in &lane.placements {
            let x0 = geom.step_to_x(f64::from(pl.start_step));
            let x1 = geom.step_to_x(f64::from(pl.start_step + pl.length_steps));
            if x1 <= geom.timeline_x || x0 >= lane_tl.right() {
                continue;
            }
            let r = Rect::new(
                x0 + 1.5,
                y + 3.0,
                (x1 - x0 - 3.0).max(4.0),
                (row_h - 7.0).max(8.0),
            );
            let pcol = theme.track_color(pl.part_id as usize);
            let (fill, border, text_col) = if is_arr {
                (pcol.with_alpha(0.10), theme.border.subtle, theme.text.muted)
            } else if lane.muted {
                (
                    pcol.with_alpha(0.12),
                    pcol.with_alpha(0.35),
                    theme.text.muted,
                )
            } else {
                (
                    pcol.with_alpha(if pl.selected { 0.34 } else { 0.22 }),
                    if pl.selected {
                        theme.accent.focus
                    } else {
                        pcol.with_alpha(0.80)
                    },
                    theme.text.primary,
                )
            };
            dl.panel(
                r,
                fill,
                border,
                theme.radii.pill,
                if pl.selected && !is_arr { 1.6 } else { 1.0 },
                if pl.selected && !is_arr { 0.18 } else { 0.0 },
            );
            // Crisp bright top edge bar on active placements.
            if !is_arr && !lane.muted {
                dl.fill(
                    Rect::new(r.x + 2.0, r.y + 1.0, (r.w - 4.0).max(0.0), 2.0),
                    pcol,
                );
            }
            let pl_start = u64::from(pl.start_step);
            let pl_end = pl_start + u64::from(pl.length_steps);
            let occluded_by_override = !is_arr
                && lane
                    .override_info
                    .as_ref()
                    .is_some_and(|ov| ov.from_step < pl_end && ov.end_step > pl_start);
            if !occluded_by_override {
                if let Some(lbl_rect) = geom.visible_label_rect(r, 5.0) {
                    let mut lbl = pl.part_name.clone();
                    if pl.has_exceptions {
                        lbl.push_str(" *");
                    }
                    if pl.phase_restart {
                        lbl.push_str(" ↩");
                    }
                    let max_c = (lbl_rect.w / theme.char_width(theme.font.small)) as usize;
                    dl.text(
                        lbl_rect,
                        trim_label(&lbl, max_c),
                        theme.font.small,
                        pl.selected && !is_arr,
                        text_col,
                        TextAlign::Left,
                    );
                }
            }
        }

        // Glowing live override double-bar drawn over the lane when active!
        if !is_arr {
            if let Some(ov) = &lane.override_info {
                let x0 = geom.step_to_x(ov.from_step as f64).max(geom.timeline_x);
                let x1 = geom.step_to_x(ov.end_step as f64).min(lane_tl.right());
                if x1 > x0 + 2.0 {
                    let r = Rect::new(x0, y + 2.0, x1 - x0, (row_h - 5.0).max(8.0));
                    dl.hatched_band(
                        r,
                        theme.accent.override_bar.with_alpha(0.26),
                        theme.accent.override_bar,
                        theme.radii.pill,
                        10.0,
                        theme.glow.override_bar,
                    );
                    dl.rounded_glow(
                        Rect::new(r.x, r.y, r.w, 2.5),
                        theme.accent.override_bar,
                        1.0,
                        theme.glow.override_bar,
                    );
                    dl.rounded_glow(
                        Rect::new(r.x, r.bottom() - 2.5, r.w, 2.5),
                        theme.accent.override_bar,
                        1.0,
                        theme.glow.override_bar,
                    );
                    if r.w >= 34.0 {
                        // Stop the live override label before any pending/staged mark on this lane so text never collides.
                        let next_mark_x = lane
                            .pending
                            .iter()
                            .chain(&lane.staged)
                            .map(|m| geom.step_to_x(m.step as f64))
                            .filter(|&mx| mx > r.x + 28.0 && mx < r.right())
                            .fold(r.right(), f32::min);
                        let lbl_w = (next_mark_x - r.x - 6.0).max(0.0);
                        if lbl_w >= 28.0 {
                            let max_c =
                                ((lbl_w - 6.0) / theme.char_width(theme.font.small)) as usize;
                            let lbl = if max_c >= ov.part_name.chars().count() + 28 {
                                format!("▶ LIVE: {}  (Alt+x rel · Alt+c keep)", ov.part_name)
                            } else {
                                format!("▶ LIVE: {}", ov.part_name)
                            };
                            dl.text(
                                Rect::new(r.x + 6.0, r.y, lbl_w - 6.0, r.h),
                                trim_label(&lbl, max_c),
                                theme.font.small,
                                true,
                                theme.text.primary,
                                TextAlign::Left,
                            );
                        }
                    }
                }
            }

            // Pending (`◆`/`■`) & Staged (`◇`/`□`) launch/release preview bands & beacons.
            for (marks, is_staged) in [(&lane.pending, false), (&lane.staged, true)] {
                for mark in marks {
                    let mx = geom.step_to_x(mark.step as f64);
                    if mx < geom.timeline_x - 4.0 || mx > lane_tl.right() {
                        continue;
                    }
                    let col = if mark.is_release {
                        theme.accent.alert
                    } else if is_staged {
                        theme.accent.modulation
                    } else {
                        theme.accent.warning_key
                    };
                    if is_staged {
                        let span_w = (lane_tl.right() - mx).max(24.0);
                        let preview_band = Rect::new(mx, y + 3.0, span_w, (row_h - 7.0).max(12.0));
                        dl.rounded(preview_band, col.with_alpha(0.13), theme.radii.badge);
                        draw_dashed_rect(dl, preview_band, col.with_alpha(0.85), 6.0, 4.0, 1.2);
                    }
                    let badge_w = (mark.label.chars().count() as f32
                        * theme.char_width(theme.font.small - 1.0)
                        + 12.0)
                        .clamp(48.0, 108.0)
                        .min((lane_tl.right() - mx).max(20.0));
                    let pill = Rect::new(mx + 2.0, y + 5.0, badge_w, (row_h - 11.0).max(14.0));
                    let pill_bg = theme.surface.overlay.mix(col, 0.28);
                    if is_staged {
                        dl.rounded_glow(pill, pill_bg, theme.radii.badge, 0.22);
                        draw_dashed_rect(dl, pill, col, 4.0, 2.5, 1.0);
                    } else {
                        dl.panel(pill, pill_bg, col, theme.radii.badge, 1.0, 0.48);
                    }
                    let max_c =
                        ((pill.w - 6.0) / theme.char_width(theme.font.small - 1.0)) as usize;
                    dl.text(
                        pill.inset(3.0, 0.0),
                        trim_label(&mark.label, max_c),
                        theme.font.small - 1.0,
                        true,
                        col,
                        TextAlign::Left,
                    );
                }
            }
        }

        dl.pop_clip();
    }
}

fn draw_ms_pill(
    dl: &mut DisplayList,
    rect: Rect,
    label: &str,
    active: bool,
    staged: Option<bool>,
    active_col: Color,
    theme: &Theme,
) {
    let fill = if let Some(next_on) = staged {
        if next_on {
            active_col.with_alpha(0.24)
        } else {
            theme.accent.modulation.with_alpha(0.18)
        }
    } else if active {
        active_col.with_alpha(0.28)
    } else {
        theme.surface.recessed
    };
    dl.rounded(rect, fill, theme.radii.badge);
    if staged.is_some() {
        draw_dashed_rect(dl, rect, theme.accent.modulation, 3.0, 2.0, 1.0);
    }
    let text_col = if let Some(next_on) = staged {
        if next_on {
            active_col
        } else {
            theme.accent.modulation
        }
    } else if active {
        active_col
    } else {
        theme.text.muted
    };
    dl.text(
        rect,
        label,
        theme.font.small - 1.0,
        active || staged.is_some(),
        text_col,
        TextAlign::Center,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadence_app::view::{ArrangeLaneRow, ArrangeOverrideRow, ArrangePlacementBlock};
    use cadence_app::ArrangeView;

    fn sample_view(override_info: Option<ArrangeOverrideRow>) -> ArrangeDeckView {
        ArrangeDeckView {
            mode: ArrangeView::Placements,
            zoom: 1.0,
            song_bars: 8,
            page_start_step: 0,
            page_steps: 128,
            cursor_step: 0,
            selected_lane: Some(1),
            playhead_step: Some(32.0),
            solo_audition: false,
            playing: true,
            focused: true,
            jam_active: false,
            show_lane_meters: false,
            playing_loop: None,
            loop_region: None,
            staging_active: false,
            staged_boundary_step: None,
            sections: Vec::new(),
            pending_edit_bars: Vec::new(),
            lanes: vec![ArrangeLaneRow {
                id: 1,
                index: 0,
                name: "Arp".into(),
                key_char: Some('e'),
                selected: true,
                muted: false,
                solo: false,
                staged_mute: None,
                staged_solo: None,
                meter_peak: None,
                meter_hot: false,
                meter_clipped: false,
                placements: vec![
                    ArrangePlacementBlock {
                        id: 10,
                        part_id: 100,
                        part_name: "Intro Hook".into(),
                        start_step: 0,
                        length_steps: 16,
                        phase_restart: false,
                        has_exceptions: false,
                        selected: false,
                    },
                    ArrangePlacementBlock {
                        id: 11,
                        part_id: 101,
                        part_name: "Arp 16th".into(),
                        start_step: 16,
                        length_steps: 48,
                        phase_restart: false,
                        has_exceptions: false,
                        selected: false,
                    },
                ],
                override_info,
                pending: Vec::new(),
                staged: Vec::new(),
            }],
            evaluated: None,
        }
    }

    #[test]
    fn active_live_override_suppresses_occluded_main_row_placement_text_but_keeps_underlay() {
        let theme = Theme::cadence_dark();
        let body = Rect::new(0.0, 0.0, 960.0, 84.0);
        let geom = TimelineGeom {
            gutter_w: 160.0,
            timeline_x: 164.0,
            timeline_w: 796.0,
            start_step: 0.0,
            span_steps: 128.0,
        };
        let row_h = (body.h / 2.0).clamp(24.0, 42.0);

        // Full and partial overlap both suppress main-row label on "Arp 16th" while retaining
        // the dimmed `↪ arr.` underlay label and the non-overlapped "Intro Hook" main-row label.
        for (from_step, end_step) in [(16, 64), (32, 80)] {
            let view = sample_view(Some(ArrangeOverrideRow {
                part_id: 200,
                part_name: "Arp 32nd Lift".into(),
                from_step,
                end_step,
            }));
            let mut dl = DisplayList::new(960.0, 200.0, 1.0);
            draw_placements(&mut dl, body, &view, geom, &theme);

            let main_row_texts: Vec<&str> = dl
                .base
                .texts
                .iter()
                .filter(|t| t.rect.y < body.y + row_h - 0.5)
                .map(|t| t.text.as_str())
                .collect();
            let arr_row_texts: Vec<&str> = dl
                .base
                .texts
                .iter()
                .filter(|t| t.rect.y >= body.y + row_h - 0.5)
                .map(|t| t.text.as_str())
                .collect();

            assert!(
                main_row_texts
                    .iter()
                    .any(|s| s.contains("LIVE: Arp 32nd Lift")),
                "expected LIVE override label on main row, got {main_row_texts:?}"
            );
            assert!(
                !main_row_texts.iter().any(|s| s.contains("Arp 16th")),
                "occluded placement label must be suppressed on main row ({from_step}..{end_step}), got {main_row_texts:?}"
            );
            assert!(
                main_row_texts.iter().any(|s| s.contains("Intro Hook")),
                "non-overlapped placement label must remain visible on main row, got {main_row_texts:?}"
            );
            assert!(
                arr_row_texts.iter().any(|s| s.contains("arr.")),
                "expected arr. underlay gutter label, got {arr_row_texts:?}"
            );
            assert!(
                arr_row_texts.iter().any(|s| s.contains("Arp 16th")),
                "overridden placement label must remain visible on arr. underlay row, got {arr_row_texts:?}"
            );
            assert!(
                arr_row_texts.iter().any(|s| s.contains("Intro Hook")),
                "earlier placement label must remain visible on arr. underlay row, got {arr_row_texts:?}"
            );
        }

        // Without an override, "Arp 16th" renders normally on the main row.
        let view_no_ov = sample_view(None);
        let mut dl_no_ov = DisplayList::new(960.0, 200.0, 1.0);
        draw_placements(&mut dl_no_ov, body, &view_no_ov, geom, &theme);
        assert!(
            dl_no_ov
                .base
                .texts
                .iter()
                .any(|t| t.text.contains("Arp 16th")),
            "placement label must render normally when no live override is active"
        );
    }

    #[test]
    fn left_clipped_placement_label_clamps_to_timeline_left_edge() {
        let theme = Theme::cadence_dark();
        let body = Rect::new(0.0, 0.0, 960.0, 84.0);
        // Viewport starts at step 32, so placement 0..96 is scrolled partially off the left edge
        // (x0 < timeline_x, visible span 32..96), while 0..33 leaves only a tiny < 26 px sliver.
        let geom = TimelineGeom {
            gutter_w: 160.0,
            timeline_x: 164.0,
            timeline_w: 796.0,
            start_step: 32.0,
            span_steps: 128.0,
        };
        let mut view = sample_view(None);
        view.page_start_step = 32;
        view.lanes[0].placements = vec![
            ArrangePlacementBlock {
                id: 20,
                part_id: 200,
                part_name: "Tiny Sliver".into(),
                start_step: 0,
                length_steps: 33,
                phase_restart: false,
                has_exceptions: false,
                selected: false,
            },
            ArrangePlacementBlock {
                id: 21,
                part_id: 201,
                part_name: "Long Verse Pad".into(),
                start_step: 0,
                length_steps: 96,
                phase_restart: false,
                has_exceptions: true,
                selected: true,
            },
        ];

        let mut dl = DisplayList::new(960.0, 200.0, 1.0);
        draw_placements(&mut dl, body, &view, geom, &theme);
        assert_eq!(dl.current_clip(), dl.viewport);

        let long_lbl = dl
            .base
            .texts
            .iter()
            .find(|t| t.text.contains("Long Verse Pad *"))
            .expect("left-clipped placement must keep its label visible inside the timeline");
        assert!(
            long_lbl.rect.x >= geom.timeline_x,
            "clamped label x ({}) must stay >= timeline_x ({})",
            long_lbl.rect.x,
            geom.timeline_x
        );
        assert!(
            (long_lbl.rect.x - (geom.timeline_x + 5.0)).abs() < 0.01,
            "left-clipped placement label should sit cleanly at timeline_x + 5.0, got {}",
            long_lbl.rect.x
        );
        assert!(
            !dl.base.texts.iter().any(|t| t.text.contains("Tiny Sliver")),
            "narrow left-clipped sliver (< 26 px visible) must omit cramped label"
        );
    }

    #[test]
    fn live_override_bar_uses_plus_modifier_labels() {
        let theme = Theme::cadence_dark();
        let body = Rect::new(0.0, 0.0, 1200.0, 84.0);
        let geom = TimelineGeom {
            gutter_w: 160.0,
            timeline_x: 164.0,
            timeline_w: 1036.0,
            start_step: 0.0,
            span_steps: 128.0,
        };
        let view = sample_view(Some(ArrangeOverrideRow {
            part_id: 200,
            part_name: "Arp Lift".into(),
            from_step: 0,
            end_step: 128,
        }));
        let mut dl = DisplayList::new(1200.0, 200.0, 1.0);
        draw_placements(&mut dl, body, &view, geom, &theme);
        assert!(dl
            .base
            .texts
            .iter()
            .any(|t| t.text.contains("Alt+x rel · Alt+c keep")));
        assert!(!dl
            .base
            .texts
            .iter()
            .any(|t| t.text.contains("Alt-x") || t.text.contains("Alt-c")));
    }
}
