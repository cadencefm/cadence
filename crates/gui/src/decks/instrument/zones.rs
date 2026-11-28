//! Instrument `Zones` tab: scrollable horizontal Zone pill strip (`h`/`l` zone, `Alt+j`/`k`
//! reorder), 2D Key × Velocity zone map with integrated chromatic piano strip + live dry
//! oscilloscope, and zone/voice/rack controls below.
use super::panel::pill_scroll_window;
use super::{
    draw_badge_header, draw_card_panel, draw_live_scope_card, draw_simple_card_header,
    split_main_and_scope,
};
use crate::decks::object::draw_control_list;
use crate::display::{trim_label, DisplayList, QuadStyle, Rect, TextAlign};
use crate::theme::{Color, Theme};
use cadence_app::view::{InstrumentDeckView, RackDeckView, RackZoneView};
use cadence_core::music::note_name;

pub(super) fn draw_zones_tab(
    dl: &mut DisplayList,
    body: Rect,
    view: &InstrumentDeckView,
    rack: &RackDeckView,
    theme: &Theme,
) {
    let (pill_row, rest) = if !rack.zones.is_empty() && body.h >= 120.0 {
        let (p, r) = body.split_top(28.0);
        (Some(p), r)
    } else {
        (None, body)
    };

    if let Some(pr) = pill_row {
        draw_zone_pill_strip(dl, pr, &rack.zones, theme);
    }

    let max_vis = (rest.h - 76.0).max(88.0);
    let vis_h = (rest.h * 0.36).clamp(112.0, 176.0).min(max_vis);
    let (vis_outer, ctrl_area) = rest.split_top(vis_h);
    let vis_area = Rect::new(
        vis_outer.x,
        vis_outer.y,
        vis_outer.w,
        (vis_outer.h - 6.0).max(0.0),
    );

    let (overview_card, scope_card) = split_main_and_scope(vis_area);
    draw_active_zone_overview_card(dl, overview_card, &rack.kit_name, &rack.zones, theme);
    if let Some(sc) = scope_card {
        draw_live_scope_card(dl, sc, "DRY SCOPE", None, &rack.live_waveform, theme);
    }

    draw_control_list(dl, ctrl_area, &view.controls, view.control_cursor, theme);
}

fn zone_source_accent(z: &RackZoneView, theme: &Theme) -> Color {
    if z.has_sampler {
        theme.accent.routing
    } else if z.source_tag == "SYNTH" {
        theme.accent.focus
    } else if z.source_tag == "AERO" {
        theme.accent.aero
    } else if z.source_tag == "NOISE" {
        theme.accent.warning_key
    } else {
        theme.accent.modulation
    }
}

fn compact_pill_note_tag(z: &RackZoneView) -> String {
    if z.low_midi == 0 && z.high_midi == 127 {
        "ALL".into()
    } else {
        z.note_range.clone()
    }
}

fn draw_zone_pill_strip(dl: &mut DisplayList, row: Rect, zones: &[RackZoneView], theme: &Theme) {
    if row.is_empty() || zones.is_empty() {
        return;
    }
    dl.push_clip(row);

    let font = theme.font.small;
    let tag_font = theme.font.small - 2.0;
    let cw = theme.char_width(font);
    let tcw = theme.char_width(tag_font);
    let gap = 5.0;
    let pill_h = (row.h - 6.0).clamp(17.0, 22.0);
    let pill_y = row.y + 2.0;

    let tags: Vec<String> = zones.iter().map(compact_pill_note_tag).collect();
    let widths: Vec<f32> = zones
        .iter()
        .enumerate()
        .map(|(idx, z)| {
            let tag_w = tags[idx].chars().count() as f32 * tcw + 8.0;
            let lbl_w = z.name.chars().count() as f32 * cw;
            let choke_w = if z.choke_group.is_some() { 22.0 } else { 0.0 };
            (tag_w + lbl_w + choke_w + 18.0).clamp(62.0, 196.0)
        })
        .collect();
    let active_idx = zones.iter().position(|z| z.selected).unwrap_or(0);
    let (start, end) = pill_scroll_window(&widths, active_idx, row.w, gap);

    let mut x = row.x;
    if start > 0 {
        let r = Rect::new(x, pill_y, 14.0, pill_h);
        dl.text(r, "◀", tag_font, true, theme.text.muted, TextAlign::Center);
        x += 16.0;
    }

    let right_limit = if end < zones.len() {
        row.right() - 18.0
    } else {
        row.right()
    };

    for idx in start..end {
        let z = &zones[idx];
        let accent = zone_source_accent(z, theme);
        let pw = widths[idx].min((right_limit - x).max(0.0));
        if pw < 36.0 {
            break;
        }
        let pr = Rect::new(x, pill_y, pw, pill_h);
        if z.selected {
            dl.panel(
                pr,
                accent.with_alpha(0.20),
                accent,
                theme.radii.badge,
                1.3,
                0.18,
            );
        } else {
            dl.panel(
                pr,
                theme.surface.recessed,
                accent.with_alpha(0.32),
                theme.radii.badge,
                1.0,
                0.0,
            );
            dl.rounded(
                Rect::new(pr.x + 1.0, pr.y + 3.0, 2.5, (pr.h - 6.0).max(4.0)),
                accent.with_alpha(0.75),
                1.0,
            );
        }

        let tag_str = &tags[idx];
        let tag_w = (tag_str.chars().count() as f32 * tcw + 6.0).min(pw * 0.45);
        let tag_rect = Rect::new(pr.x + 6.0, pr.y + 3.0, tag_w, (pr.h - 6.0).max(10.0));
        dl.rounded(
            tag_rect,
            accent.with_alpha(if z.selected { 0.30 } else { 0.14 }),
            theme.radii.badge,
        );
        dl.text(
            tag_rect,
            tag_str.as_str(),
            tag_font,
            true,
            if z.selected {
                accent
            } else {
                accent.with_alpha(0.90)
            },
            TextAlign::Center,
        );

        let choke_w = if z.choke_group.is_some() && pw >= 92.0 {
            22.0
        } else {
            0.0
        };
        if let Some(group) = z.choke_group {
            if choke_w > 0.0 {
                let cr = Rect::new(
                    pr.right() - choke_w - 4.0,
                    pr.y + 3.0,
                    choke_w,
                    (pr.h - 6.0).max(10.0),
                );
                dl.rounded(
                    cr,
                    theme.accent.warning_key.with_alpha(0.18),
                    theme.radii.badge,
                );
                dl.text(
                    cr,
                    format!("C{group}"),
                    tag_font,
                    true,
                    theme.accent.warning_key,
                    TextAlign::Center,
                );
            }
        }

        let lbl_x = tag_rect.right() + 5.0;
        let lbl_w = (pr.right() - lbl_x - choke_w - 6.0).max(0.0);
        if lbl_w >= 14.0 {
            let max_c = (lbl_w / cw).floor().max(1.0) as usize;
            dl.text(
                Rect::new(lbl_x, pr.y, lbl_w, pr.h),
                trim_label(&z.name, max_c),
                font,
                z.selected,
                if z.selected {
                    theme.text.primary
                } else {
                    theme.text.secondary
                },
                TextAlign::Left,
            );
        }

        x += pw + gap;
    }

    if end < zones.len() {
        let r = Rect::new(row.right() - 14.0, pill_y, 14.0, pill_h);
        dl.text(r, "▶", tag_font, true, theme.text.muted, TextAlign::Center);
    }

    dl.pop_clip();
}

fn draw_active_zone_overview_card(
    dl: &mut DisplayList,
    card: Rect,
    kit_name: &str,
    zones: &[RackZoneView],
    theme: &Theme,
) {
    let inner = draw_card_panel(dl, card, theme);
    let compact = card.h < 120.0;
    let (hdr, body) = inner.split_top(if compact { 17.0 } else { 19.0 });

    let Some(selected_idx) = zones
        .iter()
        .position(|z| z.selected)
        .or_else(|| (!zones.is_empty()).then_some(0))
    else {
        draw_simple_card_header(
            dl,
            hdr,
            &format!("RACK ZONES  ·  {kit_name}"),
            Some("0 ZONES"),
            theme.accent.routing,
            theme,
        );
        dl.text(
            body,
            "No zones · press a to add a zone",
            theme.font.small,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
        return;
    };

    let active = &zones[selected_idx];
    let accent = zone_source_accent(active, theme);
    let (view_lo, view_hi) = zone_map_midi_window(zones);

    let key_badge = format!("Key {}", active.note_range);
    let vel_badge = format!("Vel {}", active.vel_range);
    let choke_badge = active.choke_group.map(|g| format!("C{g}"));
    let mut badges: Vec<(&str, Color)> = vec![
        (active.source_tag, accent),
        (key_badge.as_str(), accent),
        (vel_badge.as_str(), theme.accent.modulation),
    ];
    if let Some(ref cb) = choke_badge {
        badges.push((cb.as_str(), theme.accent.warning_key));
    }

    let title = if let Some(sample) = &active.sample_label {
        format!(
            "z{} · {}  ·  Root {}  ·  {sample}",
            active.id,
            active.name,
            note_name(active.root_midi),
        )
    } else {
        format!(
            "z{} · {}  ·  Root {}  ·  {}",
            active.id,
            active.name,
            note_name(active.root_midi),
            active.voice_summary,
        )
    };
    let range_tag = if view_lo == 0 && view_hi == 127 {
        "FULL".to_string()
    } else {
        format!("{}–{}", note_name(view_lo), note_name(view_hi))
    };
    let right_summary = format!(
        "{}/{} · {}m/{}c · {range_tag}",
        selected_idx + 1,
        zones.len(),
        active.module_count,
        active.cable_count,
    );
    draw_badge_header(
        dl,
        hdr,
        &badges,
        &title,
        Some(&right_summary),
        theme.text.primary,
        theme,
    );

    let map_area = Rect::new(body.x, body.y + 3.0, body.w, (body.h - 3.0).max(0.0));
    draw_zone_key_velocity_map(dl, map_area, zones, selected_idx, view_lo, view_hi, theme);
}

pub(super) fn zone_map_midi_window(zones: &[RackZoneView]) -> (u8, u8) {
    if zones.is_empty() {
        return (0, 127);
    }
    if zones.iter().any(|z| z.low_midi == 0 && z.high_midi == 127) {
        return (0, 127);
    }
    let mut min_lo = 127u8;
    let mut max_hi = 0u8;
    for z in zones {
        min_lo = min_lo.min(z.low_midi).min(z.root_midi);
        max_hi = max_hi.max(z.high_midi).max(z.root_midi);
    }
    if max_hi.saturating_sub(min_lo) >= 72 {
        return (0, 127);
    }

    let mut lo = (min_lo.saturating_sub(2) / 12) * 12;
    let mut hi = ((((max_hi.saturating_add(2)) / 12) + 1) * 12)
        .saturating_sub(1)
        .min(127);
    while hi.saturating_sub(lo) + 1 < 24 {
        if lo >= 12 {
            lo -= 12;
        } else if hi < 127 {
            hi = hi.saturating_add(12).min(127);
        } else {
            break;
        }
    }
    (lo, hi)
}

#[inline]
fn is_black_key(midi: u8) -> bool {
    matches!(midi % 12, 1 | 3 | 6 | 8 | 10)
}

#[inline]
fn vel_to_y(grid: Rect, vel: f32) -> f32 {
    grid.bottom() - vel.clamp(0.0, 1.0) * grid.h
}

pub(super) fn zone_block_rect(
    grid: Rect,
    z: &RackZoneView,
    view_lo: u8,
    view_hi: u8,
    key_w: f32,
) -> Option<Rect> {
    if z.high_midi < view_lo || z.low_midi > view_hi {
        return None;
    }
    let lo = z.low_midi.max(view_lo);
    let hi = z.high_midi.min(view_hi);
    let total = f32::from(view_hi - view_lo) + 1.0;
    let raw_x0 = grid.x + (f32::from(lo - view_lo) / total) * grid.w;
    let raw_x1 = grid.x + (f32::from(hi - view_lo + 1) / total) * grid.w;
    let gap = if key_w >= 11.0 { 0.75 } else { 0.0 };
    let x0 = (raw_x0 + gap).min(grid.right());
    let x1 = (raw_x1 - gap).max(x0 + 2.5).min(grid.right());

    let mut y0 = vel_to_y(grid, z.high_vel);
    let mut y1 = vel_to_y(grid, z.low_vel);
    if y1 - y0 < 4.0 {
        let mid = (y0 + y1) * 0.5;
        y0 = (mid - 2.0).max(grid.y);
        y1 = (y0 + 4.0).min(grid.bottom());
        y0 = (y1 - 4.0).max(grid.y);
    }
    Some(Rect::new(x0, y0, (x1 - x0).max(2.0), (y1 - y0).max(2.0)))
}

fn draw_zone_key_velocity_map(
    dl: &mut DisplayList,
    area: Rect,
    zones: &[RackZoneView],
    selected_idx: usize,
    view_lo: u8,
    view_hi: u8,
    theme: &Theme,
) {
    if area.w < 64.0 || area.h < 34.0 {
        return;
    }
    dl.push_clip(area);

    let show_vel_axis = area.w >= 240.0;
    let axis_w = if show_vel_axis { 28.0 } else { 0.0 };
    let (vel_gutter, plot_outer) = area.split_left(axis_w);

    dl.panel(
        plot_outer,
        theme.surface.recessed,
        theme.border.subtle,
        theme.radii.badge,
        1.0,
        0.0,
    );
    let plot_inner = plot_outer.inset(1.0, 1.0);
    if plot_inner.w < 24.0 || plot_inner.h < 24.0 {
        dl.pop_clip();
        return;
    }

    let piano_h = if plot_inner.h >= 78.0 { 20.0 } else { 15.0 };
    let (grid, piano_rect) = plot_inner.split_bottom(piano_h);
    let total_keys = f32::from(view_hi - view_lo) + 1.0;
    let key_w = grid.w / total_keys;

    // Velocity Y-axis labels aligned strictly to the velocity matrix above the piano strip.
    if show_vel_axis && grid.h >= 24.0 {
        let af = (theme.font.small - 2.5).max(8.0);
        let lh = 11.0;
        let gx = vel_gutter.x;
        let gw = (vel_gutter.w - 4.0).max(16.0);
        dl.text(
            Rect::new(gx, grid.y - 1.0, gw, lh),
            "100%",
            af,
            false,
            theme.text.muted,
            TextAlign::Right,
        );
        if grid.h >= 54.0 {
            dl.text(
                Rect::new(gx, grid.y + grid.h * 0.5 - lh * 0.5, gw, lh),
                "50%",
                af,
                false,
                theme.text.muted.with_alpha(0.75),
                TextAlign::Right,
            );
        }
        dl.text(
            Rect::new(gx, (grid.bottom() - lh + 1.0).max(grid.y + lh), gw, lh),
            "0%",
            af,
            false,
            theme.text.muted,
            TextAlign::Right,
        );
    }

    // 1. Chromatic column backdrop + octave/velocity gridlines.
    dl.push_clip(grid);
    for midi in view_lo..=view_hi {
        let x0 = grid.x + (f32::from(midi - view_lo) / total_keys) * grid.w;
        let x1 = grid.x + (f32::from(midi - view_lo + 1) / total_keys) * grid.w;
        let bg = if is_black_key(midi) {
            theme.surface.key_black
        } else {
            theme.surface.key_white
        };
        dl.fill(Rect::new(x0, grid.y, (x1 - x0).max(0.5), grid.h), bg);
        if midi > view_lo {
            if midi % 12 == 0 {
                dl.fill(
                    Rect::new(x0, grid.y, 1.0, grid.h),
                    theme.border.grid_bar.with_alpha(0.75),
                );
            } else if key_w >= 8.0 {
                dl.fill(
                    Rect::new(x0, grid.y, 1.0, grid.h),
                    theme.border.hairline.with_alpha(0.38),
                );
            }
        }
    }

    // Subtle velocity reference lines (25%, 50%, 75%).
    let y50 = vel_to_y(grid, 0.5);
    dl.fill(
        Rect::new(grid.x, y50, grid.w, 1.0),
        theme.border.grid_beat.with_alpha(0.55),
    );
    if grid.h >= 62.0 {
        for v in [0.25_f32, 0.75_f32] {
            let yv = vel_to_y(grid, v);
            dl.fill(
                Rect::new(grid.x, yv, grid.w, 1.0),
                theme.border.hairline.with_alpha(0.38),
            );
        }
    }

    // 2. Sibling (unselected) 2D Key × Velocity blocks — widest spans first so narrow drum pads stay legible.
    let mut sibling_indices: Vec<usize> = (0..zones.len()).filter(|&i| i != selected_idx).collect();
    sibling_indices.sort_by(|&a, &b| {
        let span_a = zones[a].high_midi.saturating_sub(zones[a].low_midi);
        let span_b = zones[b].high_midi.saturating_sub(zones[b].low_midi);
        span_b.cmp(&span_a).then_with(|| a.cmp(&b))
    });

    let block_font = (theme.font.small - 2.0).max(8.5);
    let bcw = theme.char_width(block_font);

    for idx in sibling_indices {
        let z = &zones[idx];
        let Some(br) = zone_block_rect(grid, z, view_lo, view_hi, key_w) else {
            continue;
        };
        let accent = zone_source_accent(z, theme);
        dl.quad_full(
            br,
            QuadStyle {
                color_top: accent.with_alpha(0.32),
                color_bottom: accent.with_alpha(0.10),
                radius: 2.0,
                border_color: accent.with_alpha(0.50),
                border_width: 1.0,
                emissive: 0.0,
                hatch_period: 0.0,
            },
        );
        // Subtle top/bottom velocity cap lines so velocity thresholds read clearly.
        dl.fill(
            Rect::new(br.x + 1.0, br.y, (br.w - 2.0).max(1.0), 1.5),
            accent.with_alpha(0.75),
        );
        if z.low_vel > 0.01 {
            dl.fill(
                Rect::new(br.x + 1.0, br.bottom() - 1.5, (br.w - 2.0).max(1.0), 1.5),
                accent.with_alpha(0.55),
            );
        }
        if br.w >= 17.0 && br.h >= 15.0 {
            if z.high_midi > z.low_midi {
                draw_zone_corner_label(
                    dl, br, &z.name, None, accent, false, block_font, bcw, theme,
                );
            } else {
                let max_c = ((br.w - 3.0) / bcw).floor().max(1.0) as usize;
                dl.text(
                    Rect::new(br.x + 1.5, br.y + 1.0, (br.w - 3.0).max(4.0), br.h - 2.0),
                    trim_label(&z.name, max_c),
                    block_font,
                    false,
                    theme.text.secondary,
                    TextAlign::Center,
                );
            }
        }
    }

    // 3. Active zone 2D Key × Velocity block (always crisp and glowing on top).
    let active = &zones[selected_idx];
    let active_accent = zone_source_accent(active, theme);
    if let Some(br) = zone_block_rect(grid, active, view_lo, view_hi, key_w) {
        // When velocity-cropped, softly frame the active zone's full key column behind it
        // so the unfilled velocity headroom/floor stands out immediately.
        if active.low_vel > 0.01 || active.high_vel < 0.99 {
            let col_frame = Rect::new(br.x, grid.y, br.w, grid.h);
            dl.panel(
                col_frame,
                active_accent.with_alpha(0.05),
                active_accent.with_alpha(0.24),
                2.0,
                1.0,
                0.0,
            );
        }

        dl.quad_full(
            br,
            QuadStyle {
                color_top: active_accent.with_alpha(0.52),
                color_bottom: active_accent.with_alpha(0.18),
                radius: 2.5,
                border_color: active_accent,
                border_width: 1.6,
                emissive: 0.24,
                hatch_period: 0.0,
            },
        );

        // Distinct top & bottom velocity boundary rails.
        dl.fill(
            Rect::new(br.x + 1.5, br.y + 0.5, (br.w - 3.0).max(1.0), 2.0),
            active_accent.mix(theme.text.primary, 0.30),
        );
        dl.fill(
            Rect::new(br.x + 1.5, br.bottom() - 2.0, (br.w - 3.0).max(1.0), 1.5),
            active_accent.with_alpha(0.90),
        );

        // For multi-key spans, give the root-note key column a soft warm amber fill wash
        // (matching the piano root marker below) instead of a center line that slices text.
        if active.low_midi != active.high_midi
            && active.root_midi >= view_lo
            && active.root_midi <= view_hi
        {
            let rx0 =
                (grid.x + (f32::from(active.root_midi - view_lo) / total_keys) * grid.w).max(br.x);
            let rx1 = (grid.x + (f32::from(active.root_midi - view_lo + 1) / total_keys) * grid.w)
                .min(br.right());
            let rw = (rx1 - rx0).max(2.0);
            let rh = (br.h - 3.0).max(2.0);
            dl.quad_full(
                Rect::new(rx0, br.y + 1.5, rw, rh),
                QuadStyle {
                    color_top: theme.accent.warning_key.with_alpha(0.28),
                    color_bottom: theme.accent.warning_key.with_alpha(0.12),
                    radius: 1.5,
                    border_color: theme.accent.warning_key.with_alpha(0.0),
                    border_width: 0.0,
                    emissive: 0.0,
                    hatch_period: 0.0,
                },
            );
            dl.fill(
                Rect::new(rx0, br.y + 0.5, rw, 2.0),
                theme.accent.warning_key.with_alpha(0.90),
            );
            dl.fill(
                Rect::new(rx0, br.bottom() - 2.0, rw, 1.5),
                theme.accent.warning_key.with_alpha(0.85),
            );
        }

        if br.w >= 17.0 && br.h >= 15.0 {
            if active.high_midi > active.low_midi {
                let vel_tag = (active.low_vel > 0.01 || active.high_vel < 0.99)
                    .then_some(active.vel_range.as_str());
                draw_zone_corner_label(
                    dl,
                    br,
                    &active.name,
                    vel_tag,
                    active_accent,
                    true,
                    block_font,
                    bcw,
                    theme,
                );
            } else {
                let max_c = ((br.w - 4.0) / bcw).floor().max(1.0) as usize;
                dl.text(
                    Rect::new(br.x + 2.0, br.y + 1.0, (br.w - 4.0).max(4.0), br.h - 2.0),
                    trim_label(&active.name, max_c),
                    block_font,
                    true,
                    theme.text.primary,
                    TextAlign::Center,
                );
            }
        }
    }
    dl.pop_clip();

    // 4. Integrated 1:1 chromatic piano key strip directly beneath the velocity matrix.
    dl.push_clip(piano_rect);
    let white_key_fill = theme.surface.overlay;
    let black_key_fill = theme.surface.key_black.scale_rgb(0.80);
    dl.fill(piano_rect, white_key_fill);

    for midi in view_lo..=view_hi {
        let x0 = piano_rect.x + (f32::from(midi - view_lo) / total_keys) * piano_rect.w;
        let x1 = piano_rect.x + (f32::from(midi - view_lo + 1) / total_keys) * piano_rect.w;
        let kw = (x1 - x0).max(0.5);
        let in_active = midi >= active.low_midi && midi <= active.high_midi;
        let black = is_black_key(midi);

        if black {
            let cap_h = if key_w >= 7.0 {
                (piano_rect.h * 0.60).max(7.0)
            } else {
                piano_rect.h
            };
            dl.fill(Rect::new(x0, piano_rect.y, kw, cap_h), black_key_fill);
        }

        if in_active {
            dl.fill(
                Rect::new(x0, piano_rect.y, kw, piano_rect.h),
                active_accent.with_alpha(if black { 0.34 } else { 0.24 }),
            );
            dl.fill(Rect::new(x0, piano_rect.y, kw, 2.5), active_accent);
        }

        if midi == active.root_midi {
            let dot_w = kw.clamp(4.0, 9.0);
            let dot_h = 3.5;
            let dx = x0 + (kw - dot_w) * 0.5;
            let dy = piano_rect.bottom() - dot_h - 1.0;
            dl.rounded_glow(
                Rect::new(dx, dy, dot_w, dot_h),
                theme.accent.warning_key,
                1.75,
                0.35,
            );
        }

        if midi > view_lo {
            let sep_col = if midi % 12 == 0 {
                theme.border.grid_bar.with_alpha(0.85)
            } else {
                theme.border.hairline.with_alpha(0.55)
            };
            if midi % 12 == 0 || key_w >= 6.5 {
                dl.fill(Rect::new(x0, piano_rect.y, 1.0, piano_rect.h), sep_col);
            }
        }
    }

    // Crisp octave labels (`C1`, `C2`, `C3`...) on C keys positioned cleanly above any root pip.
    let oct_px = key_w * 12.0;
    let oct_stride: u8 = if oct_px >= 28.0 { 1 } else { 2 };
    let key_font = (theme.font.small - 2.5).max(8.0);
    let label_h = (piano_rect.h - 5.5).max(9.0);
    for midi in view_lo..=view_hi {
        if midi % 12 != 0 {
            continue;
        }
        let oct_idx = midi / 12;
        if oct_idx % oct_stride != 0 {
            continue;
        }
        let x0 = piano_rect.x + (f32::from(midi - view_lo) / total_keys) * piano_rect.w;
        let lbl_w = (oct_px * f32::from(oct_stride) - 4.0).clamp(18.0, 34.0);
        if x0 + 18.0 <= piano_rect.right() {
            let in_active = midi >= active.low_midi && midi <= active.high_midi;
            dl.text(
                Rect::new(x0 + 2.0, piano_rect.y + 2.0, lbl_w, label_h),
                note_name(midi),
                key_font,
                in_active,
                if in_active {
                    theme.text.primary
                } else {
                    theme.text.muted
                },
                TextAlign::Left,
            );
        }
    }

    // Top divider between 2D velocity grid and piano keys.
    dl.fill(
        Rect::new(piano_rect.x, piano_rect.y, piano_rect.w, 1.0),
        theme.border.subtle,
    );
    dl.pop_clip();

    dl.pop_clip();
}

#[allow(clippy::too_many_arguments)]
fn draw_zone_corner_label(
    dl: &mut DisplayList,
    br: Rect,
    name: &str,
    vel_tag: Option<&str>,
    accent: Color,
    active: bool,
    font: f32,
    cw: f32,
    theme: &Theme,
) {
    let raw = match vel_tag {
        Some(v) if br.w >= 90.0 => format!("{name} · {v}"),
        _ => name.to_string(),
    };
    let avail_w = (br.w - 8.0).max(0.0);
    if avail_w < 16.0 || br.h < 14.0 {
        return;
    }
    let max_c = ((avail_w - 6.0) / cw).floor().max(1.0) as usize;
    let text = trim_label(&raw, max_c);
    let pill_w = (text.chars().count() as f32 * cw + 8.0).min(avail_w);
    let pill_h = (font + 4.0).min(br.h - 4.0).max(11.0);
    let pr = Rect::new(br.x + 4.0, br.y + 3.5, pill_w, pill_h);
    dl.panel(
        pr,
        theme
            .surface
            .recessed
            .with_alpha(if active { 0.82 } else { 0.64 }),
        accent.with_alpha(if active { 0.80 } else { 0.40 }),
        theme.radii.badge,
        1.0,
        0.0,
    );
    dl.text(
        pr,
        text,
        font,
        active,
        if active {
            theme.text.primary
        } else {
            theme.text.secondary
        },
        TextAlign::Center,
    );
}
