//! GPU Pattern deck: zoomable Notes piano roll + velocity sub-lane, glowing Rhythm step pads,
//! antialiased GPU Values curves with breakpoint diamonds, Derived source chips, and smooth
//! color-coded multi-use playheads.
use crate::chrome::{draw_deck_frame, DeckFrameSpec};
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::{Color, Theme};
use cadence_app::view::{
    PatternContentView, PatternDeckView, PatternPlayhead, SourcePreviewKind, UiModeKind,
};
use cadence_core::material::ValueUnit;
use cadence_core::music::note_name;
use cadence_core::playback::PlaybackUse;

pub fn draw_pattern(
    dl: &mut DisplayList,
    area: Rect,
    view: &PatternDeckView,
    mode: UiModeKind,
    theme: &Theme,
) {
    let accent = view
        .pattern_id
        .map_or(theme.accent.focus, |id| theme.track_color(id as usize));
    let badge = if view.empty {
        String::new()
    } else {
        format!(
            "{} · {} · {} steps · zoom {:.0}% · {}",
            view.kind_label,
            view.scope_label,
            view.period,
            view.zoom * 100.0,
            view.playback_status
        )
    };
    let inner = draw_deck_frame(
        dl,
        area,
        &DeckFrameSpec {
            title: &view.name,
            right_badge: if badge.is_empty() { None } else { Some(&badge) },
            accent,
            focused: view.focused,
            mode,
        },
        theme,
    );
    if inner.w < 120.0 || inner.h < 48.0 {
        return;
    }
    dl.push_clip(inner);

    if view.empty {
        dl.text(
            inner.inset(12.0, 12.0),
            "No pattern selected  ·  press nn for notes, nr for rhythm, nv for values, or gb to browse Bank",
            theme.font.body,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
        dl.pop_clip();
        return;
    }

    let mut content_area = inner;

    // Derived source chips strip at top when this pattern derives from others.
    if !view.sources.is_empty() {
        let (src_row, rest) = content_area.split_top(26.0);
        content_area = rest;
        dl.text(
            Rect::new(src_row.x, src_row.y, 68.0, src_row.h),
            "SOURCES:",
            theme.font.small,
            true,
            if view.focused {
                theme.accent.focus
            } else {
                theme.text.muted
            },
            TextAlign::Left,
        );
        let mut sx = src_row.x + 70.0;
        let cw = theme.char_width(theme.font.small);
        for chip in &view.sources {
            let lbl = if chip.selected {
                format!("▸ {} ({})", chip.name, chip.kind_label)
            } else {
                format!("{} ({})", chip.name, chip.kind_label)
            };
            let w = (lbl.chars().count() as f32 * cw + 16.0).min(196.0);
            if sx + w > src_row.right() {
                break;
            }
            let r = Rect::new(sx, src_row.y + 2.0, w, src_row.h - 4.0);
            let ccol = theme.track_color(chip.id as usize);
            let (bg, border, bw, glow) = if chip.selected && view.focused {
                (ccol.with_alpha(0.28), ccol, 1.5, 0.22)
            } else if chip.selected {
                (ccol.with_alpha(0.14), ccol.with_alpha(0.72), 1.2, 0.0)
            } else {
                (theme.surface.recessed, theme.border.hairline, 1.0, 0.0)
            };
            dl.panel(r, bg, border, theme.radii.badge, bw, glow);
            let max_c = ((r.w - 10.0) / cw) as usize;
            dl.text(
                r.inset(5.0, 0.0),
                trim_label(&lbl, max_c),
                theme.font.small,
                chip.selected,
                if chip.selected {
                    ccol
                } else {
                    theme.text.primary
                },
                TextAlign::Left,
            );
            sx += w + 6.0;
        }
        // Stacked live mini-previews of source patterns directly above the evaluated result!
        if !view.source_previews.is_empty() && content_area.h >= 180.0 {
            let strip_h = 22.0;
            let total_h =
                (view.source_previews.len() as f32 * strip_h + 4.0).min(content_area.h * 0.28);
            let (preview_area, rest) = content_area.split_top(total_h);
            content_area = rest;
            let label_w = 104.0;
            for (idx, sp) in view.source_previews.iter().enumerate() {
                let sy = preview_area.y + idx as f32 * strip_h;
                if sy + strip_h > preview_area.bottom() {
                    break;
                }
                let row = Rect::new(preview_area.x, sy, preview_area.w, strip_h - 3.0);
                let scol = theme.track_color(sp.id as usize);
                dl.rounded(row, theme.surface.recessed.with_alpha(0.75), 3.0);
                let max_c = ((label_w - 8.0) / theme.char_width(theme.font.small - 1.0)) as usize;
                dl.text(
                    Rect::new(row.x + 6.0, row.y, label_w - 8.0, row.h),
                    trim_label(&format!("↪ {}", sp.name), max_c),
                    theme.font.small - 1.0,
                    false,
                    scol,
                    TextAlign::Left,
                );
                let plot = Rect::new(
                    row.x + label_w,
                    row.y + 2.0,
                    (row.w - label_w - 6.0).max(1.0),
                    (row.h - 4.0).max(1.0),
                );
                match &sp.kind {
                    SourcePreviewKind::Rhythm(hits) if !hits.is_empty() => {
                        let sw = plot.w / hits.len() as f32;
                        for (i, &on) in hits.iter().enumerate() {
                            if on {
                                dl.rounded_glow(
                                    Rect::new(
                                        plot.x + i as f32 * sw + 1.0,
                                        plot.y + 1.0,
                                        (sw - 2.0).max(2.0),
                                        plot.h - 2.0,
                                    ),
                                    scol,
                                    2.0,
                                    0.18,
                                );
                            }
                        }
                    }
                    SourcePreviewKind::Notes(notes) if !notes.is_empty() => {
                        let sw = plot.w / notes.len() as f32;
                        for (i, opt) in notes.iter().enumerate() {
                            if let Some(norm) = opt {
                                let ny = plot.bottom() - 3.0 - norm * (plot.h - 5.0).max(1.0);
                                dl.rounded_glow(
                                    Rect::new(
                                        plot.x + i as f32 * sw + 1.0,
                                        ny,
                                        (sw - 2.0).max(3.0),
                                        3.5,
                                    ),
                                    scol,
                                    1.5,
                                    0.22,
                                );
                            }
                        }
                    }
                    SourcePreviewKind::Values(vals) if vals.len() >= 2 => {
                        let n = vals.len();
                        let pts: Vec<(f32, f32)> = vals
                            .iter()
                            .enumerate()
                            .map(|(i, &v)| {
                                (
                                    plot.x + plot.w * (i as f32 / (n - 1) as f32),
                                    plot.bottom() - 1.0 - v * (plot.h - 2.0).max(1.0),
                                )
                            })
                            .collect();
                        dl.polyline(&pts, 1.6, scol, 0.20, Some((plot.bottom(), 0.16)));
                    }
                    _ => {}
                }
            }
        }
    }

    // Downstream consumer chips strip when viewing a Values pattern driving Macros/Mods/P-locks/Filter Env.
    if !view.consumers.is_empty() && content_area.h >= 96.0 {
        let (cons_row, rest) = content_area.split_top(24.0);
        content_area = rest;
        dl.text(
            Rect::new(cons_row.x, cons_row.y, 58.0, cons_row.h),
            "DRIVES:",
            theme.font.small - 0.5,
            true,
            theme.accent.modulation,
            TextAlign::Left,
        );
        let hint_w = if cons_row.w >= 340.0 { 92.0 } else { 0.0 };
        let chips_right = cons_row.right() - hint_w;
        let mut cx = cons_row.x + 60.0;
        let cw = theme.char_width(theme.font.small - 0.5);
        for (idx, chip) in view.consumers.iter().enumerate() {
            let max_w = (chips_right - cx - 4.0).min(216.0);
            if max_w < 54.0 {
                let rem = view.consumers.len() - idx;
                let more = format!("+{rem}");
                let mw = (more.len() as f32 * cw + 10.0).min((chips_right - cx).max(0.0));
                if mw >= 24.0 {
                    dl.text(
                        Rect::new(cx, cons_row.y, mw, cons_row.h),
                        more,
                        theme.font.small - 1.0,
                        true,
                        theme.text.muted,
                        TextAlign::Left,
                    );
                }
                break;
            }
            let w = (chip.label.chars().count() as f32 * cw + 14.0).min(max_w);
            let r = Rect::new(cx, cons_row.y + 2.0, w, cons_row.h - 4.0);
            let col = theme.accent.modulation;
            dl.panel(
                r,
                col.with_alpha(0.16),
                col.with_alpha(0.75),
                theme.radii.badge,
                1.0,
                0.08,
            );
            let max_c = ((r.w - 10.0) / cw).max(4.0) as usize;
            dl.text(
                r.inset(5.0, 0.0),
                trim_label(&chip.label, max_c),
                theme.font.small - 0.5,
                true,
                col,
                TextAlign::Left,
            );
            cx += w + 6.0;
        }
        if hint_w > 0.0 {
            dl.text(
                Rect::new(chips_right, cons_row.y, hint_w, cons_row.h),
                "e jump/edit",
                theme.font.small - 1.0,
                false,
                theme.text.muted,
                TextAlign::Right,
            );
        }
    }

    // Inline Strudel/Tidal Mini-Notation code bar when viewing a Mini pattern.
    if let Some(code) = &view.mini_code {
        let (code_row, rest) = content_area.split_top(28.0);
        content_area = rest;
        let bar = Rect::new(code_row.x, code_row.y + 1.0, code_row.w, code_row.h - 4.0);
        dl.panel(
            bar,
            theme.surface.recessed,
            accent.with_alpha(0.70),
            theme.radii.badge,
            1.2,
            0.16,
        );
        let badge_w = 54.0;
        let badge_r = Rect::new(bar.x + 4.0, bar.y + 3.0, badge_w, bar.h - 6.0);
        dl.rounded_glow(badge_r, accent.with_alpha(0.24), theme.radii.badge, 0.14);
        dl.text(
            badge_r,
            "MINI ▸",
            theme.font.small,
            true,
            accent,
            TextAlign::Center,
        );
        let hint_w = 124.0;
        let code_x = badge_r.right() + 8.0;
        let code_w = (bar.right() - hint_w - code_x - 6.0).max(20.0);
        let max_c = (code_w / theme.char_width(theme.font.body)) as usize;
        dl.text(
            Rect::new(code_x, bar.y, code_w, bar.h),
            trim_label(code, max_c),
            theme.font.body,
            false,
            theme.text.primary,
            TextAlign::Left,
        );
        if bar.w >= 260.0 {
            dl.text(
                Rect::new(bar.right() - hint_w - 4.0, bar.y, hint_w, bar.h),
                "Enter · edit",
                theme.font.small - 0.5,
                false,
                theme.text.muted,
                TextAlign::Right,
            );
        }
    }

    match &view.content {
        PatternContentView::Empty => {}
        PatternContentView::Notes {
            steps,
            sub_notes,
            pitch_range,
        } => {
            draw_notes_roll(
                dl,
                content_area,
                view,
                &NotesRollData {
                    steps,
                    sub_notes,
                    pitch_range: *pitch_range,
                },
                accent,
                theme,
            );
        }
        PatternContentView::Rhythm { steps } => {
            draw_rhythm_pads(dl, content_area, view, steps, accent, theme);
        }
        PatternContentView::Values {
            unit,
            mode,
            range,
            curve,
            points,
            cursor_value,
            cursor_authored,
        } => {
            draw_value_curve(
                dl,
                content_area,
                view,
                &ValueCurveData {
                    unit: *unit,
                    mode_label: mode.label(),
                    range: *range,
                    curve,
                    points,
                    cursor_value: *cursor_value,
                    cursor_authored: *cursor_authored,
                },
                accent,
                theme,
            );
        }
    }

    dl.pop_clip();
}

fn playhead_color(kind: PlaybackUse, theme: &Theme) -> Color {
    match kind {
        PlaybackUse::Placement => theme.accent.playhead,
        PlaybackUse::Override => theme.accent.override_bar,
        PlaybackUse::Audition => theme.accent.audition,
        PlaybackUse::ParameterLink | PlaybackUse::Modulation => theme.accent.modulation,
    }
}

fn draw_playheads(dl: &mut DisplayList, grid: Rect, view: &PatternDeckView, theme: &Theme) {
    let start = view.page_start as f64;
    let count = view.page_steps.max(1) as f64;
    for ph in &view.playheads {
        if ph.position >= start && ph.position <= start + count {
            let x = grid.x + ((ph.position - start) / count) as f32 * grid.w;
            let col = playhead_color(ph.kind, theme);
            dl.rounded_glow(
                Rect::new(x - 1.0, grid.y, 2.0, grid.h),
                col,
                0.0,
                theme.glow.playhead,
            );
        }
    }
}

struct NotesRollData<'a> {
    steps: &'a [cadence_app::view::PatternNoteStep],
    sub_notes: &'a [cadence_app::view::SubStepNoteView],
    pitch_range: (u8, u8),
}

struct PatternSubHeader<'a> {
    readout: &'a str,
    emphasized: bool,
    pill_label: &'a str,
    pill_w: f32,
    pill_color: Color,
}

fn draw_pattern_sub_header(
    dl: &mut DisplayList,
    bar_rect: Rect,
    spec: &PatternSubHeader<'_>,
    theme: &Theme,
) {
    dl.panel(
        bar_rect,
        theme.surface.recessed,
        theme.border.subtle,
        theme.radii.badge,
        1.0,
        0.0,
    );
    let text_w = (bar_rect.w - spec.pill_w - 18.0).max(20.0);
    let max_c = (text_w / theme.char_width(theme.font.small)).max(8.0) as usize;
    dl.text(
        Rect::new(bar_rect.x + 8.0, bar_rect.y, text_w, bar_rect.h),
        trim_label(spec.readout, max_c),
        theme.font.small,
        spec.emphasized,
        if spec.emphasized {
            theme.text.primary
        } else {
            theme.text.muted
        },
        TextAlign::Left,
    );
    let pill_r = Rect::new(
        bar_rect.right() - spec.pill_w - 4.0,
        bar_rect.y + 2.0,
        spec.pill_w,
        bar_rect.h - 4.0,
    );
    dl.panel(
        pill_r,
        spec.pill_color.with_alpha(0.20),
        spec.pill_color.with_alpha(0.85),
        theme.radii.badge,
        1.0,
        0.0,
    );
    dl.text(
        pill_r,
        spec.pill_label,
        theme.font.small - 1.5,
        true,
        spec.pill_color,
        TextAlign::Center,
    );
}

fn draw_notes_roll(
    dl: &mut DisplayList,
    area: Rect,
    view: &PatternDeckView,
    data: &NotesRollData<'_>,
    accent: Color,
    theme: &Theme,
) {
    let NotesRollData {
        steps,
        sub_notes,
        pitch_range,
    } = *data;
    let gutter_w = 48.0;
    let voice_bar_h = if area.h >= 120.0 { 22.0 } else { 0.0 };
    let (voice_bar, rest_area) = area.split_top(voice_bar_h);
    let vel_h = if rest_area.h >= 145.0 { 42.0 } else { 0.0 };
    let (top, vel_area) = rest_area.split_bottom(vel_h);
    let (step_hdr, roll_body) = top.split_top(20.0);

    // Focused step & polyphonic voice readout bar (`Voice 2/4 · A♭4 · len 1.50 · vel 88%`).
    if voice_bar_h >= 18.0 {
        let literal_editable = view.editable_literal || view.is_placement_scope;
        let mode_col = if view.insert_mode {
            theme.accent.playhead
        } else {
            theme.accent.focus
        };
        let bar_rect = Rect::new(voice_bar.x, voice_bar.y, voice_bar.w, voice_bar.h - 2.0);
        let readout = match &view.voice_readout {
            Some(r) => format!("STEP {:02}/{:02}  ·  {r}", view.cursor + 1, view.period),
            None if literal_editable => format!(
                "STEP {:02}/{:02}  ·  Rest  ·  o add note · i step insert",
                view.cursor + 1,
                view.period
            ),
            None if view.mini_code.is_some() => format!(
                "STEP {:02}/{:02}  ·  Rest  ·  Enter edit mini · F freeze",
                view.cursor + 1,
                view.period
            ),
            None if !view.sources.is_empty() => format!(
                "STEP {:02}/{:02}  ·  Rest  ·  j/k source · Enter/gs open · F freeze",
                view.cursor + 1,
                view.period
            ),
            None => format!(
                "STEP {:02}/{:02}  ·  Rest  ·  r edit · F freeze",
                view.cursor + 1,
                view.period
            ),
        };
        let (pill_lbl, pill_w) = if view.insert_mode {
            ("STEP INSERT", 92.0)
        } else if literal_editable {
            ("SELECT", 68.0)
        } else if view.mini_code.is_some() {
            ("MINI", 56.0)
        } else {
            ("DERIVED", 72.0)
        };
        draw_pattern_sub_header(
            dl,
            bar_rect,
            &PatternSubHeader {
                readout: &readout,
                emphasized: view.voice_readout.is_some(),
                pill_label: pill_lbl,
                pill_w,
                pill_color: mode_col,
            },
            theme,
        );
    }

    let grid = Rect::new(
        roll_body.x + gutter_w,
        roll_body.y,
        (roll_body.w - gutter_w).max(1.0),
        roll_body.h,
    );
    let n_steps = steps.len().max(1);
    let step_w = grid.w / n_steps as f32;

    // Step numbers header.
    dl.fill(step_hdr, theme.surface.recessed);
    dl.text(
        Rect::new(step_hdr.x + 4.0, step_hdr.y, gutter_w - 8.0, step_hdr.h),
        format!("{} {}", view.root_note, view.scale_name),
        theme.font.small - 1.0,
        true,
        theme.text.muted,
        TextAlign::Left,
    );
    let active_cur_col = if view.insert_mode {
        theme.accent.playhead
    } else {
        theme.accent.focus
    };
    for (i, st) in steps.iter().enumerate() {
        let sx = grid.x + i as f32 * step_w;
        if st.selected {
            dl.fill(
                Rect::new(sx, step_hdr.y, step_w, step_hdr.h),
                active_cur_col.with_alpha(0.22),
            );
        }
        if i % 4 == 0 || step_w >= 26.0 || st.selected {
            dl.text(
                Rect::new(sx, step_hdr.y, step_w.max(22.0), step_hdr.h),
                format!("{:02}", st.step_index + 1),
                theme.font.small - 1.0,
                st.selected || i % 4 == 0,
                if st.selected {
                    active_cur_col
                } else if i % 4 == 0 {
                    theme.text.secondary
                } else {
                    theme.text.muted
                },
                TextAlign::Center,
            );
        }
    }

    // Pitch rows with scale-degree highlighting.
    let (lo, hi) = pitch_range;
    let rows = (hi.saturating_sub(lo) as usize + 1).max(1);
    let row_h = (grid.h / rows as f32).max(5.0);
    for r_idx in 0..rows {
        let midi = hi.saturating_sub(r_idx as u8);
        let y = grid.y + r_idx as f32 * row_h;
        if y >= grid.bottom() {
            break;
        }
        let pc = midi % 12;
        let bg = if pc == view.root_pc {
            theme.surface.key_root
        } else if view.scale_pcs[pc as usize] {
            theme.surface.key_scale
        } else if matches!(pc, 1 | 3 | 6 | 8 | 10) {
            theme.surface.key_black
        } else {
            theme.surface.key_white
        };
        dl.fill(Rect::new(grid.x, y, grid.w, row_h), bg);
        dl.fill(
            Rect::new(grid.x, y + row_h - 1.0, grid.w, 1.0),
            theme.border.grid_step.with_alpha(0.6),
        );
        if row_h >= 12.0 || pc == view.root_pc || pc == 0 {
            dl.text(
                Rect::new(roll_body.x + 2.0, y, gutter_w - 6.0, row_h.max(13.0)),
                note_name(midi),
                (theme.font.small - 1.0).min(row_h * 0.85).max(8.5),
                pc == view.root_pc,
                if pc == view.root_pc {
                    theme.accent.focus
                } else {
                    theme.text.muted
                },
                TextAlign::Right,
            );
        }
    }

    // Vertical step/beat grid lines + active cursor column.
    for (i, st) in steps.iter().enumerate() {
        let sx = grid.x + i as f32 * step_w;
        if st.selected {
            dl.fill(
                Rect::new(sx, grid.y, step_w, grid.h),
                active_cur_col.with_alpha(if view.insert_mode { 0.12 } else { 0.07 }),
            );
        }
        let line_col = if i % 16 == 0 {
            theme.border.grid_bar
        } else if i % 4 == 0 {
            theme.border.grid_beat
        } else {
            theme.border.grid_step
        };
        dl.fill(Rect::new(sx, grid.y, 1.0, grid.h), line_col);
    }

    // Note pills: use fractional sub-step bars when algebra/mini-notation produces sub-step timing.
    dl.push_clip(grid);
    if !sub_notes.is_empty() {
        for ev in sub_notes {
            if ev.midi < lo || ev.midi > hi {
                continue;
            }
            let sx = grid.x + (ev.step_rel as f32 + ev.offset) * step_w;
            let r_idx = hi.saturating_sub(ev.midi) as f32;
            let ny = grid.y + r_idx * row_h + 1.0;
            let nw = (step_w * ev.gate.max(0.18) - 1.5).max(4.0);
            let nr = Rect::new(sx + 0.5, ny, nw, (row_h - 2.0).max(4.0));
            let base_col = if ev.offset > 0.001 {
                theme.accent.modulation.mix(accent, 0.55)
            } else {
                accent
            };
            let fill = base_col.scale_rgb(0.48 + 0.52 * ev.velocity.clamp(0.0, 1.0));
            let hit = view.playheads.iter().any(|ph| {
                let start = ev.step_index as f64 + f64::from(ev.offset);
                let end = start + f64::from(ev.gate.max(0.12));
                ph.position >= start && ph.position < end
            });
            dl.panel(
                nr,
                if hit { base_col.scale_rgb(1.25) } else { fill },
                if ev.selected {
                    theme.accent.warning_key
                } else {
                    base_col
                },
                theme.radii.note,
                if ev.selected { 1.4 } else { 0.8 },
                if hit {
                    theme.glow.active_note
                } else if ev.selected {
                    0.22
                } else {
                    0.0
                },
            );
            if ev.primary && nr.w >= 24.0 && nr.h >= 12.0 {
                dl.text(
                    nr.inset(3.0, 0.0),
                    &ev.note_name,
                    theme.font.small - 1.5,
                    true,
                    theme.text.inverted,
                    TextAlign::Left,
                );
            }
        }
    } else {
        for (i, st) in steps.iter().enumerate() {
            if !st.active {
                continue;
            }
            let sx = grid.x + i as f32 * step_w;
            let base_col = if st.overridden {
                theme.accent.modulation
            } else if st.inherited {
                theme.accent.routing
            } else {
                accent
            };
            let multi_voice = st.voices.len() > 1;
            // Draw sibling voices first, then the selected voice last so its halo always sits cleanly on top.
            for pass_selected in [false, true] {
                for v in &st.voices {
                    if v.selected != pass_selected {
                        continue;
                    }
                    if v.midi < lo || v.midi > hi {
                        continue;
                    }
                    let r_idx = hi.saturating_sub(v.midi) as f32;
                    let ny = grid.y + r_idx * row_h + 1.0;
                    let nw = (step_w * v.gate.max(0.25) - 2.0).max(5.0);
                    let nr = Rect::new(sx + 1.0, ny, nw, (row_h - 2.0).max(4.0));
                    let sibling_on_cursor = st.selected && multi_voice && !v.selected;
                    let vel_scale = if sibling_on_cursor {
                        0.36 + 0.34 * v.velocity.clamp(0.0, 1.0)
                    } else {
                        0.48 + 0.52 * v.velocity.clamp(0.0, 1.0)
                    };
                    let mut fill = base_col.scale_rgb(vel_scale);
                    let hit = is_step_sounding(&view.playheads, st.step_index, v.gate);
                    if hit {
                        fill = base_col.scale_rgb(1.25);
                    }
                    let (border_col, border_w, glow) = if v.selected && view.insert_mode {
                        (theme.accent.playhead, 2.0, 0.32)
                    } else if v.selected {
                        (theme.accent.focus, 2.0, theme.glow.focus_border.max(0.28))
                    } else if sibling_on_cursor {
                        (base_col.with_alpha(0.75), 1.0, 0.0)
                    } else if hit {
                        (base_col, 1.0, theme.glow.active_note)
                    } else {
                        (base_col, 0.8, 0.0)
                    };
                    dl.panel(nr, fill, border_col, theme.radii.note, border_w, glow);
                    if nr.w >= 24.0 && nr.h >= 11.0 {
                        let lbl = if v.is_root && st.conditional {
                            format!("{} {}", v.note_name, st.condition_label)
                        } else {
                            v.note_name.clone()
                        };
                        dl.text(
                            nr.inset(3.0, 0.0),
                            lbl,
                            theme.font.small - 1.5,
                            v.selected || v.is_root,
                            if sibling_on_cursor {
                                theme.text.inverted.with_alpha(0.92)
                            } else {
                                theme.text.inverted
                            },
                            TextAlign::Left,
                        );
                    }
                }
            }
        }
    }
    draw_playheads(dl, grid, view, theme);
    dl.pop_clip();

    // Velocity sub-lane underneath piano roll!
    if vel_h >= 24.0 {
        let vgrid = Rect::new(
            vel_area.x + gutter_w,
            vel_area.y + 4.0,
            (vel_area.w - gutter_w).max(1.0),
            (vel_area.h - 4.0).max(1.0),
        );
        dl.fill(vgrid, theme.surface.recessed);
        dl.text(
            Rect::new(vel_area.x + 4.0, vgrid.y, gutter_w - 8.0, vgrid.h),
            "VEL",
            theme.font.small - 1.0,
            true,
            theme.text.muted,
            TextAlign::Right,
        );
        if !sub_notes.is_empty() {
            for ev in sub_notes.iter().filter(|e| e.primary) {
                let sx = vgrid.x + (ev.step_rel as f32 + ev.offset) * step_w;
                let bar_h = (vgrid.h - 4.0) * ev.velocity.clamp(0.05, 1.0);
                let bar_w = (step_w * 0.28).clamp(2.5, 8.0);
                let col = if ev.selected {
                    theme.accent.warning_key
                } else if ev.offset > 0.001 {
                    theme.accent.modulation.mix(accent, 0.55)
                } else {
                    accent
                };
                dl.rounded_glow(
                    Rect::new(sx + 1.0, vgrid.bottom() - bar_h - 2.0, bar_w, bar_h),
                    col,
                    1.5,
                    if ev.selected { 0.30 } else { 0.10 },
                );
            }
        } else {
            for (i, st) in steps.iter().enumerate() {
                if !st.active {
                    continue;
                }
                let sx = vgrid.x + i as f32 * step_w;
                let voice_vel = if st.selected {
                    st.voices
                        .iter()
                        .find(|v| v.selected)
                        .map_or(st.velocity, |v| v.velocity)
                } else {
                    st.velocity
                };
                let bar_h = (vgrid.h - 4.0) * voice_vel.clamp(0.05, 1.0);
                let bar_w = (step_w * 0.60).clamp(3.0, 14.0);
                let bx = sx + (step_w - bar_w) * 0.5;
                let col = if st.selected {
                    theme.accent.focus
                } else {
                    accent
                };
                dl.rounded_glow(
                    Rect::new(bx, vgrid.bottom() - bar_h - 2.0, bar_w, bar_h),
                    col,
                    2.0,
                    if st.selected { 0.30 } else { 0.08 },
                );
            }
        }
        draw_playheads(dl, vgrid, view, theme);
    }
}

fn is_step_sounding(playheads: &[PatternPlayhead], step: usize, gate: f32) -> bool {
    let s = step as f64;
    let e = s + f64::from(gate.max(0.25));
    playheads.iter().any(|p| p.position >= s && p.position < e)
}

fn draw_rhythm_pads(
    dl: &mut DisplayList,
    area: Rect,
    view: &PatternDeckView,
    steps: &[cadence_app::view::PatternRhythmStep],
    accent: Color,
    theme: &Theme,
) {
    let info_bar_h = if area.h >= 96.0 { 22.0 } else { 0.0 };
    let (info_bar, pads_area) = area.split_top(info_bar_h);
    if info_bar_h >= 18.0 {
        let step_active = steps.iter().find(|s| s.selected).is_some_and(|s| s.active);
        let state_lbl = if step_active { "Hit" } else { "Rest" };
        let is_literal_rhythm = view.editable_literal;
        let cue = if is_literal_rhythm {
            "Enter / x toggle hit · r edit"
        } else if !view.sources.is_empty() {
            "j/k source · Enter/gs open · F freeze"
        } else {
            "r edit · F freeze"
        };
        let readout = format!(
            "STEP {:02}/{:02}  ·  {state_lbl}  ·  {cue}",
            view.cursor + 1,
            view.period
        );
        let (pill_lbl, pill_w) = if is_literal_rhythm {
            ("RHYTHM", 68.0)
        } else if !view.sources.is_empty() {
            ("DERIVED", 72.0)
        } else {
            ("EUCLIDEAN", 82.0)
        };
        let pill_col = if is_literal_rhythm {
            accent
        } else {
            theme.accent.focus
        };
        let bar_rect = Rect::new(info_bar.x, info_bar.y, info_bar.w, info_bar.h - 2.0);
        draw_pattern_sub_header(
            dl,
            bar_rect,
            &PatternSubHeader {
                readout: &readout,
                emphasized: step_active,
                pill_label: pill_lbl,
                pill_w,
                pill_color: pill_col,
            },
            theme,
        );
    }

    let grid = pads_area.inset(4.0, 10.0);
    let n = steps.len().max(1);
    let step_w = grid.w / n as f32;
    let pad_h = (grid.h * 0.52).clamp(28.0, 72.0);
    let pad_y = grid.y + (grid.h - pad_h) * 0.45;

    for (i, st) in steps.iter().enumerate() {
        let sx = grid.x + i as f32 * step_w;
        let pad_gap = if i % 4 == 0 && i > 0 { 4.0 } else { 2.0 };
        let r = Rect::new(
            sx + pad_gap,
            pad_y,
            (step_w - pad_gap - 2.0).max(6.0),
            pad_h,
        );
        let hit = is_step_sounding(&view.playheads, st.step_index, 1.0);
        let sel_col = theme.accent.focus;
        let (fill, border, glow) = if st.active {
            (
                if hit {
                    accent.scale_rgb(1.3)
                } else {
                    accent.with_alpha(0.75)
                },
                if st.selected { sel_col } else { accent },
                if hit { theme.glow.active_note } else { 0.18 },
            )
        } else {
            (
                theme.surface.recessed,
                if st.selected {
                    sel_col
                } else if i % 4 == 0 {
                    theme.border.grid_bar
                } else {
                    theme.border.subtle
                },
                0.0,
            )
        };
        dl.panel(
            r,
            fill,
            border,
            theme.radii.pill,
            if st.selected { 2.0 } else { 1.0 },
            glow,
        );
        dl.text(
            Rect::new(r.x, r.bottom() + 4.0, r.w, 16.0),
            format!("{:02}", st.step_index + 1),
            theme.font.small - 1.0,
            st.selected || i % 4 == 0,
            if st.selected {
                sel_col
            } else {
                theme.text.muted
            },
            TextAlign::Center,
        );
    }
    draw_playheads(
        dl,
        Rect::new(grid.x, pad_y - 6.0, grid.w, pad_h + 12.0),
        view,
        theme,
    );
}

struct ValueCurveData<'a> {
    unit: ValueUnit,
    mode_label: &'a str,
    range: (f32, f32),
    curve: &'a [(f64, f32)],
    points: &'a [(f64, f32)],
    cursor_value: f32,
    cursor_authored: bool,
}

fn draw_value_curve(
    dl: &mut DisplayList,
    area: Rect,
    view: &PatternDeckView,
    data: &ValueCurveData<'_>,
    accent: Color,
    theme: &Theme,
) {
    let ValueCurveData {
        unit,
        mode_label,
        range,
        curve,
        points,
        cursor_value,
        cursor_authored,
    } = *data;
    let info_bar_h = if area.h >= 84.0 { 22.0 } else { 0.0 };
    let (info_row, plot_outer) = area.split_top(info_bar_h);
    if info_bar_h >= 18.0 {
        let unit_lbl = if unit == ValueUnit::Degrees {
            "DEGREES"
        } else {
            "NORMALIZED 0–1"
        };
        let val_lbl = if unit == ValueUnit::Degrees {
            format!("{cursor_value:+.0} deg")
        } else {
            format!("{:.1}%", cursor_value * 100.0)
        };
        let (readout, pill_lbl, pill_w, pill_col) = if view.editable_literal {
            let point_state = if cursor_authored {
                "authored"
            } else {
                "interpolated"
            };
            (
                format!(
                    "{unit_lbl} · {mode_label} · step {} = {val_lbl} ({point_state}) · Alt+j/k adjust · Enter exact · m mode",
                    view.cursor + 1
                ),
                "VALUES",
                68.0,
                accent,
            )
        } else if !view.sources.is_empty() {
            (
                format!(
                    "{unit_lbl} · {mode_label} · step {} = {val_lbl} · j/k source · Enter/gs open · F freeze",
                    view.cursor + 1
                ),
                "DERIVED",
                72.0,
                theme.accent.focus,
            )
        } else {
            (
                format!(
                    "{unit_lbl} · {mode_label} · step {} = {val_lbl} · r edit · F freeze",
                    view.cursor + 1
                ),
                "SEEDED",
                68.0,
                theme.accent.focus,
            )
        };
        let bar_rect = Rect::new(info_row.x, info_row.y, info_row.w, info_row.h - 2.0);
        draw_pattern_sub_header(
            dl,
            bar_rect,
            &PatternSubHeader {
                readout: &readout,
                emphasized: view.editable_literal && cursor_authored,
                pill_label: pill_lbl,
                pill_w,
                pill_color: pill_col,
            },
            theme,
        );
    }

    let plot = plot_outer.inset(6.0, 6.0);
    if plot.is_empty() {
        return;
    }
    dl.panel(
        plot,
        theme.surface.recessed,
        theme.border.subtle,
        theme.radii.pill,
        1.0,
        0.0,
    );
    dl.push_clip(plot);

    let (lo, hi) = range;
    let span = (hi - lo).max(1e-4);
    let count = view.page_steps.max(1) as f64;
    let map_x = |rel: f64| plot.x + 6.0 + (rel / count) as f32 * (plot.w - 12.0);
    let map_y = |v: f32| {
        let norm = ((v - lo) / span).clamp(0.0, 1.0);
        plot.bottom() - 8.0 - norm * (plot.h - 16.0)
    };

    // Beat vertical lines & zero line.
    for step in (0..=view.page_steps).step_by(4) {
        let x = map_x(step as f64);
        dl.fill(
            Rect::new(x, plot.y, 1.0, plot.h),
            if step % 16 == 0 {
                theme.border.grid_bar
            } else {
                theme.border.grid_beat
            },
        );
    }
    if lo < 0.0 && hi > 0.0 {
        dl.fill(
            Rect::new(plot.x, map_y(0.0), plot.w, 1.0),
            theme.border.grid_bar,
        );
    }

    // Cursor column marker.
    if view.cursor >= view.page_start && view.cursor < view.page_start + view.page_steps {
        let cx = map_x((view.cursor - view.page_start) as f64);
        dl.rounded_glow(
            Rect::new(cx, plot.y, 1.5, plot.h),
            theme.accent.focus.with_alpha(0.55),
            0.0,
            0.0,
        );
    }

    // Smooth GPU polyline + translucent area fill.
    let pts: Vec<(f32, f32)> = curve.iter().map(|&(rx, v)| (map_x(rx), map_y(v))).collect();
    let base_y = if lo < 0.0 && hi > 0.0 {
        map_y(0.0)
    } else {
        plot.bottom() - 8.0
    };
    dl.polyline(&pts, 2.0, accent, theme.glow.scope, Some((base_y, 0.20)));

    // Authored breakpoint diamonds (or subtler sampled markers on derived/seeded value curves).
    let cursor_rel = view.cursor as f64 - view.page_start as f64;
    for &(rx, v) in points {
        let px = map_x(rx);
        let py = map_y(v);
        let is_cur = (rx - cursor_rel).abs() < 0.01;
        let (sz, fill, border, glow) = if view.editable_literal {
            if is_cur {
                (9.5, theme.accent.focus, theme.text.primary, 0.45)
            } else {
                (8.0, accent, theme.text.primary, 0.40)
            }
        } else if is_cur {
            (7.5, theme.accent.focus, theme.text.primary, 0.30)
        } else {
            (5.5, accent.with_alpha(0.75), theme.border.subtle, 0.12)
        };
        dl.panel(
            Rect::new(px - sz * 0.5, py - sz * 0.5, sz, sz),
            fill,
            border,
            2.0,
            if is_cur { 1.4 } else { 1.0 },
            glow,
        );
    }

    draw_playheads(dl, plot, view, theme);
    dl.pop_clip();
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadence_app::view::{PatternNoteStep, PatternRhythmStep, PatternSourceChip};

    fn base_pattern_view(
        kind_label: &'static str,
        scope_label: &str,
        editable_literal: bool,
        is_placement_scope: bool,
        content: PatternContentView,
    ) -> PatternDeckView {
        PatternDeckView {
            empty: false,
            pattern_id: Some(1),
            name: "Test Pat".into(),
            kind_label,
            scope_label: scope_label.into(),
            editable_literal,
            is_placement_scope,
            zoom: 1.0,
            period: 16,
            page_start: 0,
            page_steps: 16,
            cursor: 0,
            insert_mode: false,
            selected_voice: 0,
            voice_readout: None,
            focused: true,
            root_note: "C4".into(),
            scale_name: "Minor",
            root_pc: 0,
            scale_pcs: [true; 12],
            playback_status: "∥ PAUSED".into(),
            playheads: Vec::new(),
            sources: Vec::new(),
            consumers: Vec::new(),
            source_previews: Vec::new(),
            mini_code: None,
            content,
        }
    }

    fn empty_notes_content() -> PatternContentView {
        let steps = (0..16)
            .map(|i| PatternNoteStep {
                step_index: i,
                active: false,
                midi: 60,
                note_name: "C4".into(),
                velocity: 0.8,
                gate: 0.5,
                chord: Vec::new(),
                voices: Vec::new(),
                overridden: false,
                inherited: false,
                conditional: false,
                condition_label: String::new(),
                selected: i == 0,
            })
            .collect();
        PatternContentView::Notes {
            steps,
            sub_notes: Vec::new(),
            pitch_range: (55, 74),
        }
    }

    fn rhythm_content() -> PatternContentView {
        let steps = (0..16)
            .map(|i| PatternRhythmStep {
                step_index: i,
                active: i % 4 == 0,
                selected: i == 0,
            })
            .collect();
        PatternContentView::Rhythm { steps }
    }

    fn values_content(cursor_authored: bool) -> PatternContentView {
        PatternContentView::Values {
            unit: ValueUnit::Normalized,
            mode: cadence_core::params::ValueMode::Ramp,
            range: (0.0, 1.0),
            curve: vec![(0.0, 0.25), (8.0, 0.75), (16.0, 0.25)],
            points: vec![(0.0, 0.25), (8.0, 0.75)],
            cursor_value: 0.25,
            cursor_authored,
        }
    }

    #[test]
    fn notes_sub_header_shows_add_note_cue_only_on_literal_or_placement_scope() {
        let theme = Theme::cadence_dark();
        let area = Rect::new(0.0, 0.0, 960.0, 360.0);

        // 1. Literal Notes definition shows `o add note · i step insert`.
        let literal = base_pattern_view(
            "Notes",
            "SHARED DEFINITION",
            true,
            false,
            empty_notes_content(),
        );
        let mut dl_lit = DisplayList::new(960.0, 360.0, 1.0);
        draw_pattern(&mut dl_lit, area, &literal, UiModeKind::Normal, &theme);
        assert_eq!(dl_lit.current_clip(), dl_lit.viewport);
        assert!(dl_lit
            .base
            .texts
            .iter()
            .any(|t| t.text.contains("o add note · i step insert")));

        // 2. Mini pattern hides `o add note · i step insert` and shows `Enter edit mini`.
        let mut mini = base_pattern_view(
            "Mini",
            "SHARED DEFINITION",
            false,
            false,
            empty_notes_content(),
        );
        mini.mini_code = Some("c4 ~ e4 g4".into());
        let mut dl_mini = DisplayList::new(960.0, 360.0, 1.0);
        draw_pattern(&mut dl_mini, area, &mini, UiModeKind::Normal, &theme);
        assert_eq!(dl_mini.current_clip(), dl_mini.viewport);
        assert!(!dl_mini
            .base
            .texts
            .iter()
            .any(|t| t.text.contains("o add note")));
        assert!(dl_mini
            .base
            .texts
            .iter()
            .any(|t| t.text.contains("Enter edit mini")));
        assert!(dl_mini.base.texts.iter().any(|t| t.text == "Enter · edit"));

        // 3. Derived Transpose pattern hides `o add note · i step insert` and shows source cue.
        let mut derived = base_pattern_view(
            "Transpose",
            "SHARED DEFINITION",
            false,
            false,
            empty_notes_content(),
        );
        derived.sources.push(PatternSourceChip {
            id: 2,
            name: "Lead".into(),
            kind_label: "Notes",
            selected: true,
        });
        let mut dl_drv = DisplayList::new(960.0, 360.0, 1.0);
        draw_pattern(&mut dl_drv, area, &derived, UiModeKind::Normal, &theme);
        assert_eq!(dl_drv.current_clip(), dl_drv.viewport);
        assert!(!dl_drv
            .base
            .texts
            .iter()
            .any(|t| t.text.contains("o add note")));
        assert!(dl_drv
            .base
            .texts
            .iter()
            .any(|t| t.text.contains("Enter/gs open")));

        // 4. Placement local-occurrence scope retains `o add note · i step insert` even for derived patterns
        //    and relies on typed `is_placement_scope`, not display strings.
        let placed = base_pattern_view(
            "Transpose",
            "CUSTOM LABEL",
            false,
            true,
            empty_notes_content(),
        );
        let mut dl_pl = DisplayList::new(960.0, 360.0, 1.0);
        draw_pattern(&mut dl_pl, area, &placed, UiModeKind::Normal, &theme);
        assert_eq!(dl_pl.current_clip(), dl_pl.viewport);
        assert!(dl_pl
            .base
            .texts
            .iter()
            .any(|t| t.text.contains("o add note · i step insert")));
    }

    #[test]
    fn rhythm_sub_header_shows_toggle_cue_for_literal_rhythm_and_non_literal_cues_otherwise() {
        let theme = Theme::cadence_dark();
        let area = Rect::new(0.0, 0.0, 960.0, 320.0);

        let literal_rhythm =
            base_pattern_view("Rhythm", "SHARED DEFINITION", true, false, rhythm_content());
        let mut dl_r = DisplayList::new(960.0, 320.0, 1.0);
        draw_pattern(&mut dl_r, area, &literal_rhythm, UiModeKind::Normal, &theme);
        assert_eq!(dl_r.current_clip(), dl_r.viewport);
        assert!(dl_r
            .base
            .texts
            .iter()
            .any(|t| t.text.contains("Enter / x toggle hit · r edit")));
        assert!(!dl_r
            .base
            .texts
            .iter()
            .any(|t| t.text.contains("o add note")));

        let euclid = base_pattern_view(
            "Euclidean",
            "SHARED DEFINITION",
            false,
            false,
            rhythm_content(),
        );
        let mut dl_e = DisplayList::new(960.0, 320.0, 1.0);
        draw_pattern(&mut dl_e, area, &euclid, UiModeKind::Normal, &theme);
        assert_eq!(dl_e.current_clip(), dl_e.viewport);
        assert!(dl_e
            .base
            .texts
            .iter()
            .any(|t| t.text.contains("r edit · F freeze")));
        assert!(!dl_e
            .base
            .texts
            .iter()
            .any(|t| t.text.contains("toggle hit")));
    }

    #[test]
    fn values_sub_header_shows_edit_cue_for_literal_and_non_literal_cues_for_derived_and_seeded() {
        let theme = Theme::cadence_dark();
        let area = Rect::new(0.0, 0.0, 960.0, 320.0);

        let literal_vals = base_pattern_view(
            "Values",
            "SHARED DEFINITION",
            true,
            false,
            values_content(true),
        );
        let mut dl_lit = DisplayList::new(960.0, 320.0, 1.0);
        draw_pattern(&mut dl_lit, area, &literal_vals, UiModeKind::Normal, &theme);
        assert_eq!(dl_lit.current_clip(), dl_lit.viewport);
        assert!(dl_lit
            .base
            .texts
            .iter()
            .any(|t| t.text.contains("(authored)") && t.text.contains("Alt+j/k adjust")));
        assert!(dl_lit.base.texts.iter().any(|t| t.text == "VALUES"));

        let mut derived_vals = base_pattern_view(
            "Shift",
            "SHARED DEFINITION",
            false,
            false,
            values_content(false),
        );
        derived_vals.sources.push(PatternSourceChip {
            id: 2,
            name: "Cutoff".into(),
            kind_label: "Values",
            selected: true,
        });
        let mut dl_drv = DisplayList::new(960.0, 320.0, 1.0);
        draw_pattern(&mut dl_drv, area, &derived_vals, UiModeKind::Normal, &theme);
        assert_eq!(dl_drv.current_clip(), dl_drv.viewport);
        assert!(dl_drv
            .base
            .texts
            .iter()
            .any(|t| t.text.contains("j/k source · Enter/gs open · F freeze")));
        assert!(dl_drv.base.texts.iter().any(|t| t.text == "DERIVED"));
        assert!(!dl_drv
            .base
            .texts
            .iter()
            .any(|t| t.text.contains("(authored)")));

        let seeded_vals = base_pattern_view(
            "SeededValues",
            "SHARED DEFINITION",
            false,
            false,
            values_content(false),
        );
        let mut dl_seed = DisplayList::new(960.0, 320.0, 1.0);
        draw_pattern(&mut dl_seed, area, &seeded_vals, UiModeKind::Normal, &theme);
        assert_eq!(dl_seed.current_clip(), dl_seed.viewport);
        assert!(dl_seed
            .base
            .texts
            .iter()
            .any(|t| t.text.contains("r edit · F freeze")));
        assert!(dl_seed.base.texts.iter().any(|t| t.text == "SEEDED"));
    }
}
