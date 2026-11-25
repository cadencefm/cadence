//! GPU Mixer console deck: vertical channel strips with 32-bit float `-60..+6 dBFS`
//! stereo gradient VU meters, `0 dB` hairline, pre-fader input bars & hold ticks, `HOT`/`!CLIP`
//! status badges, live top-down Compressor/Limiter GR micro-meters, glowing fader thumbs, pan
//! bars, M/S pills, and a scrollable vertical selected-channel detail stack aligned 1:1 with `j`/`k`.
use crate::chrome::{draw_deck_frame, DeckFrameSpec};
use crate::decks::output::{
    db_norm, db_to_meter_norm, fmt_db_fixed, METER_GRATICULE_DB, METER_ZERO_DB_NORM,
};
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::{Color, Theme};
use cadence_app::view::{
    mixer_bank_scroll_window, mixer_detail_scroll_window, MixerChannelView, MixerDeckView,
    MixerRole, MixerSlot, UiModeKind,
};
use cadence_core::dsp::math::lin_to_db;

/// Minimum logical width of a single vertical mixer channel strip.
pub const MIXER_MIN_STRIP_W: f32 = 88.0;
/// Maximum logical width of a single vertical mixer channel strip so strips stay
/// consistent console widths regardless of channel count, scroll offset, or zoom.
pub const MIXER_MAX_STRIP_W: f32 = 112.0;

const STRIP_GAP: f32 = 4.0;
const MASTER_DIVIDER_W: f32 = 10.0;
const OVERFLOW_RAIL_W: f32 = 20.0;
const OVERFLOW_RAIL_NARROW_W: f32 = 16.0;
const OVERFLOW_RAIL_GAP: f32 = 4.0;
const MAX_CONSOLE_BANK_SLOTS: usize = 24;

const DETAIL_ROW_H: f32 = 20.0;
const DETAIL_ROW_GAP: f32 = 2.0;
const DETAIL_ROW_PITCH: f32 = DETAIL_ROW_H + DETAIL_ROW_GAP;
const DETAIL_HEADER_H: f32 = 22.0;
const DETAIL_FOOTER_H: f32 = 17.0;
const DETAIL_CHROME_H: f32 = DETAIL_HEADER_H + DETAIL_FOOTER_H + 8.0;

/// Fixed visible row count for the bottom channel detail stack, determined strictly from
/// total Mixer deck inner height (never per-channel slot count) so faders and `0 dB`
/// hairlines stay locked vertically when switching channels.
pub fn mixer_detail_visible_rows(inner_h: f32) -> usize {
    if inner_h < 220.0 {
        0
    } else if inner_h < 300.0 {
        2
    } else if inner_h < 500.0 {
        3
    } else if inner_h < 680.0 {
        4
    } else if inner_h < 840.0 {
        5
    } else {
        6
    }
}

/// Deterministic horizontal console strip geometry computed solely from viewport width
/// and total channel count (independent of scroll position so strips never shift or resize).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MixerStripLayout {
    pub master_pinned: bool,
    pub bank_overflows: bool,
    pub bank_slots: usize,
    pub strip_w: f32,
    pub rail_w: f32,
    pub left_rail_x: f32,
    pub bank_x: f32,
    pub right_rail_x: f32,
    pub divider_x: f32,
    pub master_x: f32,
}

/// Compute uniform horizontal console layout for the Mixer strip bay.
///
/// - Pins `Master` on the right behind a console divider whenever `has_master && total_channels >= 2`
///   and at least two strips fit (`area_w >= 2 * MIXER_MIN_STRIP_W + MASTER_DIVIDER_W`).
/// - Caps strip width at `MIXER_MAX_STRIP_W` (`112px`) so strips keep consistent console geometry
///   whether a project has 3 channels or 24.
/// - When the scrollable bank overflows, reserves both left `<` and right `>` overflow rails for
///   every scroll offset so strips never shift horizontally when scrolling off index 0.
pub fn compute_strip_layout(
    area_x: f32,
    area_w: f32,
    total_channels: usize,
    has_master: bool,
) -> MixerStripLayout {
    let can_pin_master =
        has_master && total_channels >= 2 && area_w >= 2.0 * MIXER_MIN_STRIP_W + MASTER_DIVIDER_W;

    if can_pin_master {
        let bank_len = total_channels - 1;
        let unscrolled_w = (area_w - MASTER_DIVIDER_W).max(0.0);
        let max_unscrolled_bank = (((unscrolled_w - MIXER_MIN_STRIP_W + STRIP_GAP)
            / (MIXER_MIN_STRIP_W + STRIP_GAP))
            .floor() as usize)
            .clamp(1, MAX_CONSOLE_BANK_SLOTS);
        let bank_overflows = bank_len > max_unscrolled_bank;

        if !bank_overflows {
            let gaps = bank_len.saturating_sub(1) as f32 * STRIP_GAP;
            let fit_w = (unscrolled_w - gaps) / total_channels as f32;
            let strip_w = fit_w.clamp(MIXER_MIN_STRIP_W, MIXER_MAX_STRIP_W);
            let master_x = area_x + area_w - strip_w;
            let divider_x = master_x - MASTER_DIVIDER_W;
            return MixerStripLayout {
                master_pinned: true,
                bank_overflows: false,
                bank_slots: bank_len,
                strip_w,
                rail_w: 0.0,
                left_rail_x: area_x,
                bank_x: area_x,
                right_rail_x: divider_x,
                divider_x,
                master_x,
            };
        }

        let rail_w = if area_w < 360.0 {
            OVERFLOW_RAIL_NARROW_W
        } else {
            OVERFLOW_RAIL_W
        };
        let chrome_w = 2.0 * (rail_w + OVERFLOW_RAIL_GAP) + MASTER_DIVIDER_W;
        let avail = (area_w - chrome_w).max(MIXER_MIN_STRIP_W);
        let bank_slots = (((avail - MIXER_MIN_STRIP_W + STRIP_GAP)
            / (MIXER_MIN_STRIP_W + STRIP_GAP))
            .floor() as usize)
            .clamp(1, MAX_CONSOLE_BANK_SLOTS.min(bank_len));
        let gaps = bank_slots.saturating_sub(1) as f32 * STRIP_GAP;
        let fit_w = (avail - gaps) / (bank_slots + 1) as f32;
        let strip_w = fit_w.clamp(64.0, MIXER_MAX_STRIP_W);
        let left_rail_x = area_x;
        let bank_x = left_rail_x + rail_w + OVERFLOW_RAIL_GAP;
        let master_x = area_x + area_w - strip_w;
        let divider_x = master_x - MASTER_DIVIDER_W;
        let right_rail_x = divider_x - rail_w;
        return MixerStripLayout {
            master_pinned: true,
            bank_overflows: true,
            bank_slots,
            strip_w,
            rail_w,
            left_rail_x,
            bank_x,
            right_rail_x,
            divider_x,
            master_x,
        };
    }

    // Unpinned mode: either no Master strip in view, only 1 total channel, or ultra-narrow pane.
    let bank_len = total_channels.max(1);
    let max_unscrolled = (((area_w + STRIP_GAP) / (MIXER_MIN_STRIP_W + STRIP_GAP)).floor()
        as usize)
        .clamp(1, MAX_CONSOLE_BANK_SLOTS);
    let bank_overflows = bank_len > max_unscrolled;
    if !bank_overflows {
        let gaps = bank_len.saturating_sub(1) as f32 * STRIP_GAP;
        let fit_w = (area_w - gaps) / bank_len as f32;
        let strip_w = fit_w.clamp(64.0, MIXER_MAX_STRIP_W).min(area_w);
        return MixerStripLayout {
            master_pinned: false,
            bank_overflows: false,
            bank_slots: bank_len,
            strip_w,
            rail_w: 0.0,
            left_rail_x: area_x,
            bank_x: area_x,
            right_rail_x: area_x + area_w,
            divider_x: area_x + area_w,
            master_x: area_x + area_w,
        };
    }

    let rail_w = if area_w < 360.0 {
        OVERFLOW_RAIL_NARROW_W
    } else {
        OVERFLOW_RAIL_W
    };
    let chrome_w = 2.0 * (rail_w + OVERFLOW_RAIL_GAP);
    let avail = (area_w - chrome_w).max(64.0);
    let bank_slots = (((avail + STRIP_GAP) / (MIXER_MIN_STRIP_W + STRIP_GAP)).floor() as usize)
        .clamp(1, MAX_CONSOLE_BANK_SLOTS.min(bank_len));
    let gaps = bank_slots.saturating_sub(1) as f32 * STRIP_GAP;
    let fit_w = (avail - gaps) / bank_slots as f32;
    let strip_w = fit_w.clamp(64.0, MIXER_MAX_STRIP_W).min(avail);
    let left_rail_x = area_x;
    let bank_x = left_rail_x + rail_w + OVERFLOW_RAIL_GAP;
    let right_rail_x = area_x + area_w - rail_w;
    MixerStripLayout {
        master_pinned: false,
        bank_overflows: true,
        bank_slots,
        strip_w,
        rail_w,
        left_rail_x,
        bank_x,
        right_rail_x,
        divider_x: right_rail_x,
        master_x: area_x + area_w,
    }
}

/// Number of scrollable bank strips visible in a Mixer pane of width `inner_w`.
/// Shared by `cadence-gui`'s viewport metrics sync (`H`/`L`, `PgUp`/`PgDn`, smooth scroll).
pub fn mixer_bank_capacity(inner_w: f32, total_channels: usize, has_master: bool) -> usize {
    compute_strip_layout(0.0, inner_w.max(0.0), total_channels, has_master).bank_slots
}

struct DetailRow {
    slot: MixerSlot,
    tag: String,
    title: String,
    value: String,
    bar: Option<f32>,
    controlled: bool,
    accent: Color,
    dimmed: bool,
}

fn build_detail_rows(ch: &MixerChannelView, theme: &Theme) -> Vec<DetailRow> {
    let mut rows = Vec::with_capacity(ch.inserts.len() + ch.sends.len() + ch.ducks.len() + 1);
    for (i, (fx_name, bypassed)) in ch.inserts.iter().enumerate() {
        rows.push(DetailRow {
            slot: MixerSlot::Insert(i),
            tag: format!("FX {}", i + 1),
            title: fx_name.clone(),
            value: if *bypassed {
                "BYPASSED".into()
            } else {
                "ACTIVE".into()
            },
            bar: None,
            controlled: false,
            accent: if *bypassed {
                theme.text.muted
            } else {
                theme.accent.modulation
            },
            dimmed: *bypassed,
        });
    }
    for (i, (send_name, amt)) in ch.sends.iter().enumerate() {
        let controlled = ch.send_controlled.get(i).copied().unwrap_or(false);
        rows.push(DetailRow {
            slot: MixerSlot::Send(i),
            tag: "SND".into(),
            title: format!("→ {send_name}"),
            value: format!("{:.0}%", amt * 100.0),
            bar: Some(amt.clamp(0.0, 1.0)),
            controlled,
            accent: if controlled {
                theme.accent.modulation
            } else {
                theme.accent.warning_key
            },
            dimmed: false,
        });
    }
    if !matches!(ch.role, MixerRole::Master | MixerRole::Return) {
        rows.push(DetailRow {
            slot: MixerSlot::Output,
            tag: "OUT".into(),
            title: format!("Output bus → {}", ch.route_name),
            value: ch.route_name.clone(),
            bar: None,
            controlled: false,
            accent: theme.accent.routing,
            dimmed: false,
        });
    }
    for (i, duck_name) in ch.ducks.iter().enumerate() {
        let (title, value, bar) = match ch.duck_details.get(i) {
            Some((depth, recovery_ms, src)) => (
                format!("{duck_name} ← {src}"),
                format!("{:.0}% · {:.0}ms", depth * 100.0, recovery_ms),
                Some(depth.clamp(0.0, 1.0)),
            ),
            None => (duck_name.clone(), "DUCK".into(), None),
        };
        rows.push(DetailRow {
            slot: MixerSlot::Duck(i),
            tag: "DUCK".into(),
            title,
            value,
            bar,
            controlled: false,
            accent: theme.accent.alert,
            dimmed: false,
        });
    }
    rows
}

fn draw_overflow_rail(
    dl: &mut DisplayList,
    rect: Rect,
    arrow: &str,
    hidden_count: usize,
    theme: &Theme,
) {
    let active = hidden_count > 0;
    dl.panel(
        rect,
        if active {
            theme.accent.focus.with_alpha(0.11)
        } else {
            theme.surface.recessed.with_alpha(0.55)
        },
        if active {
            theme.accent.focus.with_alpha(0.55)
        } else {
            theme.border.subtle.with_alpha(0.45)
        },
        theme.radii.badge,
        1.0,
        if active { 0.08 } else { 0.0 },
    );
    let cy = rect.y + rect.h * 0.5;
    let arrow_col = if active {
        theme.accent.focus
    } else {
        theme.text.muted.with_alpha(0.30)
    };
    let arrow_y = if active && rect.h >= 54.0 {
        cy - 15.0
    } else {
        cy - 8.0
    };
    dl.text(
        Rect::new(rect.x, arrow_y, rect.w, 16.0),
        arrow,
        theme.font.body,
        true,
        arrow_col,
        TextAlign::Center,
    );
    if active && rect.h >= 54.0 {
        dl.text(
            Rect::new(rect.x, cy + 2.0, rect.w, 14.0),
            format!("{hidden_count}"),
            theme.font.small - 1.5,
            true,
            theme.accent.focus,
            TextAlign::Center,
        );
    }
}

fn draw_master_divider(
    dl: &mut DisplayList,
    divider_x: f32,
    y: f32,
    h: f32,
    master_selected: bool,
    theme: &Theme,
) {
    let line_col = if master_selected {
        theme.accent.focus.with_alpha(0.55)
    } else {
        theme.border.hairline
    };
    dl.rounded(
        Rect::new(
            divider_x + (MASTER_DIVIDER_W - 2.0) * 0.5,
            y + 4.0,
            2.0,
            (h - 8.0).max(0.0),
        ),
        line_col,
        1.0,
    );
}

fn resolve_bank_window(view: &MixerDeckView, layout: &MixerStripLayout) -> (usize, usize, usize) {
    let bank_len = if layout.master_pinned {
        view.channels.len().saturating_sub(1)
    } else {
        view.channels.len()
    };
    let selected_bank_idx = if layout.master_pinned && view.selected_index >= bank_len {
        None
    } else if bank_len > 0 {
        Some(view.selected_index.min(bank_len - 1))
    } else {
        None
    };
    let first = mixer_bank_scroll_window(
        selected_bank_idx,
        bank_len,
        layout.bank_slots,
        view.bank_scroll,
    );
    let shown = bank_len.saturating_sub(first).min(layout.bank_slots);
    (bank_len, first, shown)
}

pub fn draw_mixer(
    dl: &mut DisplayList,
    area: Rect,
    view: &MixerDeckView,
    focused: bool,
    mode: UiModeKind,
    theme: &Theme,
) {
    let est_inner_w = (area.w - crate::chrome::DECK_FRAME_INSET_X).max(0.0);
    let has_master = view
        .channels
        .last()
        .is_some_and(|c| c.role == MixerRole::Master);
    let pre_layout = compute_strip_layout(0.0, est_inner_w, view.channels.len(), has_master);
    let (pre_bank_len, pre_first, pre_shown) = resolve_bank_window(view, &pre_layout);
    let badge = if pre_layout.bank_overflows && pre_shown > 0 {
        if pre_layout.master_pinned {
            format!(
                "{}-{} / {} + MST",
                pre_first + 1,
                pre_first + pre_shown,
                pre_bank_len
            )
        } else {
            format!(
                "{}-{} / {}",
                pre_first + 1,
                pre_first + pre_shown,
                pre_bank_len
            )
        }
    } else {
        format!("{} channels", view.channels.len())
    };
    let inner = draw_deck_frame(
        dl,
        area,
        &DeckFrameSpec {
            title: "MIXER",
            right_badge: Some(&badge),
            accent: theme.accent.focus,
            focused,
            mode,
        },
        theme,
    );
    if inner.w < 120.0 || inner.h < 90.0 {
        return;
    }
    dl.push_clip(inner);

    let selected_ch = view.channels.get(view.selected_index);
    let detail_rows = selected_ch
        .map(|ch| build_detail_rows(ch, theme))
        .unwrap_or_default();
    let visible_detail_rows = mixer_detail_visible_rows(inner.h);
    let detail_h = if visible_detail_rows == 0 {
        24.0
    } else {
        let fixed_h = DETAIL_CHROME_H + visible_detail_rows as f32 * DETAIL_ROW_PITCH;
        fixed_h.min((inner.h * 0.44).max(70.0))
    };
    let (strips_area, detail_area) = inner.split_bottom(detail_h);
    let strip_h = (strips_area.h - 4.0).max(0.0);

    let layout = compute_strip_layout(
        strips_area.x,
        strips_area.w,
        view.channels.len(),
        has_master,
    );
    let (bank_len, first, shown_bank) = resolve_bank_window(view, &layout);
    let left_hidden = first;
    let right_hidden = bank_len.saturating_sub(first + shown_bank);

    if layout.bank_overflows {
        draw_overflow_rail(
            dl,
            Rect::new(layout.left_rail_x, strips_area.y, layout.rail_w, strip_h),
            "<",
            left_hidden,
            theme,
        );
    }

    for (slot, ch) in view.channels[..bank_len]
        .iter()
        .skip(first)
        .take(shown_bank)
        .enumerate()
    {
        let x = layout.bank_x + slot as f32 * (layout.strip_w + STRIP_GAP);
        let rect = Rect::new(x, strips_area.y, layout.strip_w, strip_h);
        let active_slot = ch.selected.then_some(view.focused_slot);
        draw_strip(dl, rect, ch, focused, active_slot, theme);
    }

    if layout.bank_overflows {
        draw_overflow_rail(
            dl,
            Rect::new(layout.right_rail_x, strips_area.y, layout.rail_w, strip_h),
            ">",
            right_hidden,
            theme,
        );
    }

    if layout.master_pinned {
        if let Some(master) = view.channels.last() {
            draw_master_divider(
                dl,
                layout.divider_x,
                strips_area.y,
                strip_h,
                master.selected,
                theme,
            );
            let mrect = Rect::new(layout.master_x, strips_area.y, layout.strip_w, strip_h);
            let active_slot = master.selected.then_some(view.focused_slot);
            draw_strip(dl, mrect, master, focused, active_slot, theme);
        }
    }

    // Selected channel inspector: telemetry header + fixed-height scrollable slot list + footer.
    if let Some(ch) = selected_ch {
        draw_detail_stack(
            dl,
            detail_area,
            ch,
            &detail_rows,
            view.focused_slot,
            view.detail_scroll,
            focused,
            theme,
        );
    }

    dl.pop_clip();
}

fn draw_empty_detail_banner(
    dl: &mut DisplayList,
    rect: Rect,
    role: MixerRole,
    cw: f32,
    theme: &Theme,
) {
    dl.panel(
        rect,
        theme.surface.recessed,
        theme.border.subtle.with_alpha(0.55),
        theme.radii.badge,
        1.0,
        0.0,
    );
    let empty_msg = match role {
        MixerRole::Return => "Return → Master · No inserts or ducks (f add fx · d add duck)",
        MixerRole::Master => "Master → DAC / WAV · No inserts or ducks (f add fx · d add duck)",
        MixerRole::Audio | MixerRole::Group => {
            "No inserts, sends, or ducks (f add fx · w add send · d add duck)"
        }
    };
    let max_c = ((rect.w - 14.0) / cw).max(12.0) as usize;
    dl.text(
        rect.inset(8.0, 0.0),
        trim_label(empty_msg, max_c),
        theme.font.small - 0.5,
        false,
        theme.text.muted,
        TextAlign::Left,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_detail_stack(
    dl: &mut DisplayList,
    detail_area: Rect,
    ch: &MixerChannelView,
    rows: &[DetailRow],
    focused_slot: MixerSlot,
    prev_detail_scroll: usize,
    focused: bool,
    theme: &Theme,
) {
    let slot_accent = theme.accent.focus;
    let slot_glow = theme.glow.focus_border;
    dl.fill(
        Rect::new(detail_area.x, detail_area.y, detail_area.w, 1.0),
        theme.border.subtle,
    );
    let meter_str = ch.meter.map_or_else(
        || "awaiting engine".into(),
        |m| {
            let in_hold = m.input_peak_hold[0].max(m.input_peak_hold[1]);
            let out_hold = m.output_peak_hold[0].max(m.output_peak_hold[1]);
            let flag = if m.clipped {
                " · !CLIP"
            } else if m.hot {
                " · HOT (32f)"
            } else {
                ""
            };
            let gr_str = match ch.gr_tag {
                Some(tag) if ch.gr_db > 0.05 || ch.gr_hold_db > 0.05 => {
                    format!(" · {tag} -{:4.1} (pk -{:4.1}) dB", ch.gr_db, ch.gr_hold_db)
                }
                _ => String::new(),
            };
            format!(
                "pre {} · L {} / R {} · hold {} dBFS{flag}{gr_str}",
                fmt_db_fixed(in_hold),
                fmt_db_fixed(m.output_peak[0]),
                fmt_db_fixed(m.output_peak[1]),
                fmt_db_fixed(out_hold),
            )
        },
    );
    let live_gain_str = match (&ch.effective_gain_db, ch.automated_gain_scale) {
        (Some(eff_db), Some(scale)) => {
            format!(" · live {eff_db} dB ({:.0}%)", scale * 100.0)
        }
        (Some(eff_db), None) => format!(" · live {eff_db} dB"),
        _ => String::new(),
    };
    let live_pan_str = match &ch.effective_pan_label {
        Some(eff_pan) => format!(" (live {eff_pan})"),
        None => String::new(),
    };
    let slot_badge = match focused_slot {
        MixerSlot::Fader => format!("ROW: FADER ({} dB)", ch.gain_db),
        MixerSlot::Pan => format!("ROW: PAN ({})", ch.pan_label),
        MixerSlot::Insert(i) => format!(
            "ROW: FX #{} ({})",
            i + 1,
            ch.inserts.get(i).map_or("—", |(n, _)| n.as_str())
        ),
        MixerSlot::Send(i) => ch.sends.get(i).map_or_else(
            || "ROW: SEND".into(),
            |(n, a)| format!("ROW: SEND {n} ({:.0}%)", a * 100.0),
        ),
        MixerSlot::Output => format!("ROW: OUT → {}", ch.route_name),
        MixerSlot::Duck(i) => format!(
            "ROW: DUCK ({})",
            ch.ducks.get(i).map_or("—", String::as_str)
        ),
    };
    let list_top = detail_area.y + DETAIL_HEADER_H + 3.0;
    let footer_y = detail_area.bottom() - DETAIL_FOOTER_H;
    let list_h = (footer_y - list_top - 3.0).max(0.0);
    let visible_rows = if detail_area.h >= 64.0 && list_h >= DETAIL_ROW_H {
        ((list_h + DETAIL_ROW_GAP) / DETAIL_ROW_PITCH)
            .floor()
            .max(1.0) as usize
    } else {
        0
    };
    let focused_idx = rows.iter().position(|r| r.slot == focused_slot);
    let scroll = if visible_rows > 0 {
        mixer_detail_scroll_window(focused_idx, rows.len(), visible_rows, prev_detail_scroll)
    } else {
        0
    };
    let scroll_badge = (visible_rows > 0 && rows.len() > visible_rows).then(|| {
        let start = scroll + 1;
        let end = (scroll + visible_rows).min(rows.len());
        let arrow = match (scroll > 0, end < rows.len()) {
            (true, true) => "↕",
            (true, false) => "↑",
            (false, true) => "↓",
            (false, false) => "",
        };
        format!("{arrow}{start}-{end}/{}", rows.len())
    });

    let cw = theme.char_width(theme.font.small - 0.5);
    let badge_w = (slot_badge.chars().count() as f32 * cw + 14.0).clamp(90.0, 220.0);
    let scroll_badge_w = scroll_badge
        .as_ref()
        .map_or(0.0, |s| s.chars().count() as f32 * cw + 12.0);
    let scroll_badge_gap = if scroll_badge.is_some() { 6.0 } else { 0.0 };
    let header_line = format!(
        "{} · trim {} dB{live_gain_str} · pan {}{live_pan_str} · {meter_str}",
        ch.name, ch.gain_db, ch.pan_label
    );
    let hdr_w = (detail_area.w - badge_w - scroll_badge_w - scroll_badge_gap - 12.0).max(40.0);
    let max_h_chars = (hdr_w / theme.char_width(theme.font.small)).max(12.0) as usize;
    dl.text(
        Rect::new(detail_area.x + 2.0, detail_area.y + 3.0, hdr_w, 18.0),
        trim_label(&header_line, max_h_chars),
        theme.font.small,
        true,
        theme.text.primary,
        TextAlign::Left,
    );
    let sb_rect = Rect::new(
        detail_area.right() - badge_w,
        detail_area.y + 3.0,
        badge_w,
        17.0,
    );
    if let Some(ref sb_text) = scroll_badge {
        let sc_rect = Rect::new(
            sb_rect.x - scroll_badge_gap - scroll_badge_w,
            detail_area.y + 3.0,
            scroll_badge_w,
            17.0,
        );
        dl.panel(
            sc_rect,
            theme.surface.recessed,
            theme.border.subtle,
            theme.radii.badge,
            1.0,
            0.0,
        );
        dl.text(
            sc_rect.inset(4.0, 0.0),
            sb_text,
            theme.font.small - 1.0,
            true,
            theme.text.secondary,
            TextAlign::Center,
        );
    }
    dl.panel(
        sb_rect,
        slot_accent.with_alpha(0.14),
        slot_accent,
        theme.radii.badge,
        1.0,
        if focused { slot_glow * 0.7 } else { 0.0 },
    );
    dl.text(
        sb_rect.inset(4.0, 0.0),
        trim_label(&slot_badge, ((badge_w - 8.0) / cw).max(6.0) as usize),
        theme.font.small - 1.0,
        true,
        slot_accent,
        TextAlign::Center,
    );

    if detail_area.h < 64.0 {
        return;
    }

    let list_area = Rect::new(
        detail_area.x + 2.0,
        list_top,
        (detail_area.w - 4.0).max(0.0),
        list_h,
    );

    if list_area.h >= DETAIL_ROW_H && list_area.w >= 80.0 {
        dl.push_clip(list_area);
        if rows.is_empty() {
            let empty_rect = Rect::new(list_area.x, list_area.y, list_area.w, DETAIL_ROW_H);
            draw_empty_detail_banner(dl, empty_rect, ch.role, cw, theme);
        } else {
            let needs_scroll = rows.len() > visible_rows;
            let row_w = if needs_scroll {
                (list_area.w - 9.0).max(40.0)
            } else {
                list_area.w
            };
            let tag_w = 44.0_f32;
            let val_w = (row_w * 0.24).clamp(72.0, 126.0);
            let show_bar = row_w >= 320.0;
            let bar_w = if show_bar {
                (row_w * 0.20).clamp(64.0, 118.0)
            } else {
                0.0
            };

            for (vis_idx, row) in rows.iter().skip(scroll).take(visible_rows).enumerate() {
                let y = list_area.y + vis_idx as f32 * DETAIL_ROW_PITCH;
                let rr = Rect::new(list_area.x, y, row_w, DETAIL_ROW_H);
                let is_active = row.slot == focused_slot;
                let border_col = if is_active {
                    slot_accent
                } else {
                    theme.border.subtle.with_alpha(0.65)
                };
                dl.panel(
                    rr,
                    if is_active {
                        slot_accent.with_alpha(0.15)
                    } else {
                        theme.surface.recessed
                    },
                    border_col,
                    theme.radii.badge,
                    if is_active { 1.3 } else { 1.0 },
                    if is_active && focused {
                        slot_glow * 0.7
                    } else {
                        0.0
                    },
                );
                // Left focus/category rail.
                let rail_col = if is_active {
                    slot_accent
                } else {
                    row.accent.with_alpha(0.60)
                };
                dl.rounded(
                    Rect::new(
                        rr.x + 2.0,
                        rr.y + 3.0,
                        if is_active { 3.0 } else { 2.0 },
                        rr.h - 6.0,
                    ),
                    rail_col,
                    1.5,
                );
                // Category tag pill (FX 1 / SND / OUT / DUCK).
                let tag_rect = Rect::new(rr.x + 8.0, rr.y + 3.0, tag_w, rr.h - 6.0);
                let tag_col = if is_active { slot_accent } else { row.accent };
                dl.rounded(tag_rect, tag_col.with_alpha(0.18), theme.radii.badge);
                dl.text(
                    tag_rect,
                    &row.tag,
                    theme.font.small - 1.5,
                    true,
                    tag_col,
                    TextAlign::Center,
                );
                // Right value column.
                let val_x = rr.right() - val_w - 8.0;
                let val_rect = Rect::new(val_x, rr.y, val_w, rr.h);
                let val_col = if is_active {
                    slot_accent
                } else if row.dimmed {
                    theme.text.muted
                } else if row.controlled {
                    theme.accent.modulation
                } else {
                    theme.text.secondary
                };
                let max_v = (val_w / cw).max(5.0) as usize;
                dl.text(
                    val_rect,
                    trim_label(&row.value, max_v),
                    theme.font.small - 1.0,
                    is_active || row.controlled,
                    val_col,
                    TextAlign::Right,
                );
                // Optional mini horizontal level bar (SND / DUCK).
                let title_right = if let (true, Some(norm)) = (show_bar, row.bar) {
                    let bx = val_x - bar_w - 10.0;
                    let bar_rect = Rect::new(bx, rr.y + (rr.h - 5.0) * 0.5, bar_w, 5.0);
                    dl.rounded(bar_rect, theme.meter.track_bg, 2.5);
                    let fill_w = (bar_rect.w * norm.clamp(0.0, 1.0)).max(if norm > 0.005 {
                        2.0
                    } else {
                        0.0
                    });
                    if fill_w > 0.0 {
                        let fill_col = if is_active { slot_accent } else { row.accent };
                        dl.rounded_glow(
                            Rect::new(bar_rect.x, bar_rect.y, fill_w, bar_rect.h),
                            fill_col,
                            2.5,
                            if is_active || row.controlled {
                                0.25
                            } else {
                                0.0
                            },
                        );
                    }
                    if row.controlled {
                        dl.rounded_glow(
                            Rect::new(bx - 9.0, rr.y + rr.h * 0.5 - 2.5, 5.0, 5.0),
                            theme.accent.modulation,
                            2.5,
                            0.65,
                        );
                        bx - 14.0
                    } else {
                        bx - 8.0
                    }
                } else if row.controlled {
                    let dx = val_x - 10.0;
                    dl.rounded_glow(
                        Rect::new(dx, rr.y + rr.h * 0.5 - 2.5, 5.0, 5.0),
                        theme.accent.modulation,
                        2.5,
                        0.65,
                    );
                    dx - 6.0
                } else {
                    val_x - 8.0
                };
                // Title text.
                let title_x = tag_rect.right() + 8.0;
                let title_w = (title_right - title_x).max(24.0);
                let title_col = if is_active {
                    theme.text.primary
                } else if row.dimmed {
                    theme.text.muted
                } else {
                    theme.text.primary
                };
                let max_t = (title_w / theme.char_width(theme.font.small - 0.5)).max(6.0) as usize;
                dl.text(
                    Rect::new(title_x, rr.y, title_w, rr.h),
                    trim_label(&row.title, max_t),
                    theme.font.small - 0.5,
                    is_active,
                    title_col,
                    TextAlign::Left,
                );
            }

            let show_audio_empty_hint = matches!(ch.role, MixerRole::Audio | MixerRole::Group)
                && ch.inserts.is_empty()
                && ch.sends.is_empty()
                && ch.ducks.is_empty();
            if show_audio_empty_hint && !needs_scroll {
                let hint_y = list_area.y + rows.len() as f32 * DETAIL_ROW_PITCH;
                if hint_y + DETAIL_ROW_H <= list_area.bottom() + 1.0 {
                    let hint_rect = Rect::new(list_area.x, hint_y, row_w, DETAIL_ROW_H);
                    draw_empty_detail_banner(dl, hint_rect, ch.role, cw, theme);
                }
            }

            if needs_scroll {
                let track = Rect::new(
                    list_area.right() - 4.0,
                    list_area.y + 1.0,
                    4.0,
                    (list_area.h - 2.0).max(8.0),
                );
                dl.rounded(track, theme.meter.track_bg, 2.0);
                let frac = (visible_rows as f32 / rows.len() as f32).clamp(0.15, 1.0);
                let thumb_h = (track.h * frac).max(12.0).min(track.h);
                let max_scroll = rows.len().saturating_sub(visible_rows).max(1) as f32;
                let thumb_y = track.y + (scroll as f32 / max_scroll) * (track.h - thumb_h);
                dl.rounded(
                    Rect::new(track.x, thumb_y, track.w, thumb_h),
                    if focused {
                        slot_accent.with_alpha(0.75)
                    } else {
                        theme.border.hairline
                    },
                    2.0,
                );
            }
        }
        dl.pop_clip();
    }

    // Footer: modulation summary on left + focused-row quick action cue on right.
    let mods = if ch.level_modulators.is_empty() {
        "none".into()
    } else {
        ch.level_modulators.join(", ")
    };
    let pan_mods = if ch.pan_modulators.is_empty() {
        "none".into()
    } else {
        ch.pan_modulators.join(", ")
    };
    let footer_left = format!("Level: {mods} · Pan: {pan_mods}");
    let row_hint = match focused_slot {
        MixerSlot::Fader | MixerSlot::Pan => {
            "j/k row · Alt+h/l adjust · Enter exact · C clear clip"
        }
        MixerSlot::Insert(_) => "Enter open · Alt+j/k reorder · b byp · r ren · v move · x del",
        MixerSlot::Send(_) => "Alt+h/l adjust · Enter exact · M learn · x del",
        MixerSlot::Output => "Enter route bus · Alt+h/l cycle",
        MixerSlot::Duck(_) => "Alt+h/l depth · Enter exact · r edit duck · x del",
    };
    let footer_cw = theme.char_width(theme.font.small - 0.5);
    let show_hint = detail_area.w >= 540.0;
    if show_hint {
        let hint_w = (row_hint.chars().count() as f32 * footer_cw + 8.0).min(detail_area.w * 0.56);
        let left_w = (detail_area.w - hint_w - 12.0).max(80.0);
        dl.text(
            Rect::new(detail_area.x + 2.0, footer_y, left_w, DETAIL_FOOTER_H),
            trim_label(&footer_left, (left_w / footer_cw).max(10.0) as usize),
            theme.font.small - 0.5,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
        dl.text(
            Rect::new(
                detail_area.right() - hint_w - 2.0,
                footer_y,
                hint_w,
                DETAIL_FOOTER_H,
            ),
            trim_label(row_hint, (hint_w / footer_cw).max(10.0) as usize),
            theme.font.small - 0.5,
            false,
            theme.text.secondary,
            TextAlign::Right,
        );
    } else {
        let max_f = ((detail_area.w - 4.0) / footer_cw).max(12.0) as usize;
        dl.text(
            Rect::new(
                detail_area.x + 2.0,
                footer_y,
                detail_area.w - 4.0,
                DETAIL_FOOTER_H,
            ),
            trim_label(&footer_left, max_f),
            theme.font.small - 0.5,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
    }
}

fn role_color(role: MixerRole, theme: &Theme) -> Color {
    match role {
        MixerRole::Master => theme.accent.focus,
        MixerRole::Audio => theme.accent.routing,
        MixerRole::Group => theme.accent.modulation,
        MixerRole::Return => theme.accent.warning_key,
    }
}

fn draw_strip(
    dl: &mut DisplayList,
    rect: Rect,
    ch: &MixerChannelView,
    deck_focused: bool,
    focused_slot: Option<MixerSlot>,
    theme: &Theme,
) {
    let slot_accent = theme.accent.focus;
    let slot_glow = theme.glow.focus_border.max(0.24);
    let fader_focused = deck_focused && focused_slot == Some(MixerSlot::Fader);
    let pan_focused = deck_focused && focused_slot == Some(MixerSlot::Pan);
    let clipped = ch.meter.is_some_and(|m| m.clipped);
    let hot = ch.meter.is_some_and(|m| m.hot && !m.clipped);
    let is_master = ch.role == MixerRole::Master;
    let boosted = ch.gain > 1.001;
    let boost_col = if is_master {
        theme.meter.clip
    } else {
        theme.meter.hot
    };
    let rcol = role_color(ch.role, theme);
    let border = if clipped {
        theme.meter.clip
    } else if ch.selected && deck_focused {
        theme.accent.focus
    } else if hot {
        theme.meter.hot.with_alpha(0.75)
    } else if ch.selected {
        rcol.with_alpha(0.75)
    } else {
        theme.border.subtle
    };
    dl.panel(
        rect,
        if ch.selected {
            theme.surface.active_row
        } else {
            theme.surface.recessed
        },
        border,
        theme.radii.pill,
        if ch.selected || clipped || hot {
            1.5
        } else {
            1.0
        },
        if clipped {
            theme.glow.clip * 0.5
        } else if ch.selected && deck_focused {
            0.14
        } else if hot {
            0.10
        } else {
            0.0
        },
    );

    let inner = rect.inset(6.0, 5.0);
    if inner.h < 64.0 {
        return;
    }
    let (hdr, rest) = inner.split_top(35.0);
    let (meter_fader_area, bottom) = rest.split_bottom(58.0);

    // Channel name.
    let max_c = (hdr.w / theme.char_width(theme.font.small)) as usize;
    dl.text(
        Rect::new(hdr.x, hdr.y, hdr.w, 16.0),
        trim_label(&ch.name, max_c),
        theme.font.small,
        ch.selected,
        if ch.selected {
            theme.accent.focus
        } else {
            theme.text.primary
        },
        TextAlign::Center,
    );

    // Role pill + non-overlapping HOT / !CLIP indicator.
    let automated = ch.effective_gain.is_some();
    let pan_automated = ch.effective_pan.is_some();
    let any_automated = automated || pan_automated;
    let pill_row = Rect::new(hdr.x + 2.0, hdr.y + 18.0, (hdr.w - 4.0).max(20.0), 15.0);
    let badge_font = theme.font.small - 2.0;

    if clipped || hot {
        let gap = 3.0;
        let role_w = (pill_row.w * 0.38).clamp(24.0, 42.0);
        let status_w = (pill_row.w - role_w - gap).max(20.0);
        let role_rect = Rect::new(pill_row.x, pill_row.y, role_w, pill_row.h);
        let status_rect = Rect::new(role_rect.right() + gap, pill_row.y, status_w, pill_row.h);

        if any_automated {
            dl.panel(
                role_rect,
                rcol.with_alpha(0.18),
                theme.accent.modulation.with_alpha(0.75),
                theme.radii.badge,
                1.0,
                0.12,
            );
        } else {
            dl.rounded(role_rect, rcol.with_alpha(0.18), theme.radii.badge);
        }
        dl.text(
            role_rect,
            ch.role.short_label(),
            badge_font,
            true,
            rcol,
            TextAlign::Center,
        );

        let (status_label, status_col, status_glow) = if clipped {
            (
                "!CLIP".to_string(),
                theme.meter.clip,
                theme.glow.clip * 0.45,
            )
        } else {
            let hot_peak = ch.meter.map_or(0.0, |m| {
                m.input_peak_hold[0]
                    .max(m.input_peak_hold[1])
                    .max(m.output_peak_hold[0])
                    .max(m.output_peak_hold[1])
            });
            let label = if status_w >= 58.0 && hot_peak > 1.005 {
                format!("HOT +{:.1}", lin_to_db(hot_peak).clamp(0.1, 99.9))
            } else {
                "HOT".into()
            };
            (label, theme.meter.hot, 0.16)
        };
        dl.panel(
            status_rect,
            status_col.with_alpha(0.22),
            status_col,
            theme.radii.badge,
            1.0,
            status_glow,
        );
        dl.text(
            status_rect,
            status_label,
            badge_font,
            true,
            status_col,
            TextAlign::Center,
        );
    } else {
        let role_pill = Rect::new(hdr.x + 4.0, hdr.y + 18.0, (hdr.w - 8.0).max(20.0), 15.0);
        if any_automated {
            dl.panel(
                role_pill,
                rcol.with_alpha(0.18),
                theme.accent.modulation.with_alpha(0.60),
                theme.radii.badge,
                1.0,
                0.12,
            );
        } else {
            dl.rounded(role_pill, rcol.with_alpha(0.18), theme.radii.badge);
        }
        dl.text(
            role_pill,
            ch.role.label(),
            badge_font,
            true,
            rcol,
            TextAlign::Center,
        );
        if any_automated {
            dl.rounded_glow(
                Rect::new(
                    role_pill.right() - 8.0,
                    role_pill.y + role_pill.h * 0.5 - 2.5,
                    5.0,
                    5.0,
                ),
                theme.accent.modulation,
                2.5,
                0.65,
            );
        }
    }

    // Reserve a dedicated non-overlapping row for the live Compressor/Limiter GR badge when present.
    let has_gr = ch.gr_tag.is_some();
    let (meters_region, gr_badge_rect) = if has_gr && meter_fader_area.h >= 68.0 {
        let (top, bot) = meter_fader_area.split_bottom(17.0);
        (top, Some(bot))
    } else {
        (meter_fader_area, None)
    };

    // Stereo vertical -60..+6 dBFS VU meters + pre-fader input bar + Fader + optional GR micro-bar.
    if meters_region.h >= 36.0 {
        let mf = meters_region.inset(3.0, 4.0);
        let pre_w = 3.0;
        let pre_gap = 2.0;
        let meter_col_w = 8.0;
        let lr_gap = 2.0;
        let meter_to_fader = 9.0;
        let fader_w = 10.0;
        let gr_gap = 8.0;
        let gr_col_w = 5.0;
        let show_gr_bar = has_gr && mf.w >= 56.0;

        let core_w = pre_w
            + pre_gap
            + meter_col_w * 2.0
            + lr_gap
            + meter_to_fader
            + fader_w
            + if show_gr_bar { gr_gap + gr_col_w } else { 0.0 };
        let show_scale = mf.w >= core_w + 26.0 && mf.h >= 90.0;
        let scale_w = if show_scale { 22.0 } else { 0.0 };
        let scale_gap = if show_scale { 3.0 } else { 0.0 };
        let total_w = scale_w + scale_gap + core_w;
        let start_x = mf.x + ((mf.w - total_w) * 0.5).max(0.0);

        let pre_x = start_x + scale_w + scale_gap;
        let l_x = pre_x + pre_w + pre_gap;
        let r_x = l_x + meter_col_w + lr_gap;
        let fx = r_x + meter_col_w + meter_to_fader;
        let gr_x = fx + fader_w + gr_gap;

        let y_zero = mf.bottom() - mf.h * METER_ZERO_DB_NORM;
        let headroom_h = (y_zero - mf.y).max(0.0);
        let headroom_tint = if is_master {
            theme.meter.clip.with_alpha(0.11)
        } else {
            theme.meter.hot.with_alpha(0.10)
        };

        // Optional dB scale labels on wider console strips, spaced >= 12 px apart so labels never overlap.
        if show_scale {
            let sfont = theme.font.small - 2.5;
            let mut last_y = f32::NEG_INFINITY;
            for (db, label) in [
                (6.0_f32, "+6"),
                (0.0, "0"),
                (-12.0, "-12"),
                (-24.0, "-24"),
                (-48.0, "-48"),
            ] {
                // Skip +6 when headroom band above 0 dB is too compact to keep "+6" and "0" distinct.
                if db > 0.0 && headroom_h < 13.0 {
                    continue;
                }
                let y = mf.bottom() - mf.h * db_to_meter_norm(db);
                let ty = (y - 5.5).clamp(mf.y - 1.0, mf.bottom() - 10.5);
                if ty - last_y < 11.5 {
                    continue;
                }
                last_y = ty;
                let is_zero = db == 0.0;
                dl.text(
                    Rect::new(start_x, ty, scale_w, 11.0),
                    label,
                    sfont,
                    is_zero,
                    if is_zero {
                        theme.text.secondary
                    } else {
                        theme.text.muted
                    },
                    TextAlign::Right,
                );
            }
        }

        // Pre-fader slim input bar + hold tick.
        let pre_rect = Rect::new(pre_x, mf.y, pre_w, mf.h);
        dl.rounded(pre_rect, theme.meter.track_bg, 1.5);
        if headroom_h > 0.5 {
            dl.fill(
                Rect::new(pre_rect.x, pre_rect.y, pre_rect.w, headroom_h),
                headroom_tint,
            );
        }
        if let Some(m) = ch.meter {
            let in_peak = m.input_peak[0].max(m.input_peak[1]);
            let in_norm = db_norm(in_peak);
            let in_h = (pre_rect.h * in_norm).clamp(0.0, pre_rect.h);
            if in_h > 0.5 {
                let col = if in_peak > 1.0 {
                    theme.meter.hot
                } else {
                    theme.meter.pre_fader
                };
                dl.rounded(
                    Rect::new(pre_rect.x, pre_rect.bottom() - in_h, pre_rect.w, in_h),
                    col.with_alpha(0.80),
                    1.0,
                );
            }
            let in_hold = m.input_peak_hold[0].max(m.input_peak_hold[1]);
            let ih_norm = db_norm(in_hold);
            if ih_norm > 0.02 {
                let hy = pre_rect.bottom() - (pre_rect.h * ih_norm).min(pre_rect.h - 2.0);
                dl.fill(
                    Rect::new(pre_rect.x - 0.5, hy, pre_rect.w + 1.0, 2.0),
                    if in_hold > 1.0 {
                        theme.meter.hot
                    } else {
                        theme.meter.pre_fader
                    },
                );
            }
        }

        // Post-fader L & R meters on -60..+6 dBFS scale.
        for side in 0..2 {
            let mx = if side == 0 { l_x } else { r_x };
            let mrect = Rect::new(mx, mf.y, meter_col_w, mf.h);
            dl.rounded(mrect, theme.meter.track_bg, 2.0);
            if headroom_h > 0.5 {
                dl.fill(
                    Rect::new(mrect.x, mrect.y, mrect.w, headroom_h),
                    headroom_tint,
                );
            }
            for db in METER_GRATICULE_DB {
                let gy = mrect.bottom() - mrect.h * db_to_meter_norm(db);
                dl.fill(
                    Rect::new(mrect.x + 1.0, gy, (mrect.w - 2.0).max(1.0), 1.0),
                    theme.border.grid_step,
                );
            }
            if let Some(m) = ch.meter {
                let peak = m.output_peak[side];
                let norm = db_norm(peak);
                let fh = (mrect.h * norm).clamp(0.0, mrect.h);
                if fh > 0.5 {
                    let top_col = if peak > 1.0 {
                        if is_master || m.clipped {
                            theme.meter.clip
                        } else {
                            theme.meter.hot
                        }
                    } else if peak >= 0.5 {
                        theme.meter.high
                    } else {
                        theme.meter.mid
                    };
                    let glow = if peak > 1.0 {
                        if is_master || m.clipped {
                            theme.glow.clip
                        } else {
                            0.32
                        }
                    } else {
                        0.22
                    };
                    dl.gradient_v(
                        Rect::new(mrect.x, mrect.bottom() - fh, mrect.w, fh),
                        top_col,
                        theme.meter.low,
                        2.0,
                        glow,
                    );
                }
                let hold = m.output_peak_hold[side];
                let hnorm = db_norm(hold);
                if hnorm > 0.02 {
                    let hy = mrect.bottom() - (mrect.h * hnorm).min(mrect.h - 2.0);
                    let hold_col = if hold > 1.0 {
                        if is_master || m.clipped {
                            theme.meter.clip
                        } else {
                            theme.meter.hot
                        }
                    } else {
                        theme.meter.hold_tick
                    };
                    dl.fill(Rect::new(mrect.x, hy, mrect.w, 2.0), hold_col);
                }
            }
        }

        // Crisp 0 dB unity hairline spanning Pre-fader + L + R meters.
        let meter_span_w = (r_x + meter_col_w) - pre_x;
        dl.fill(
            Rect::new(pre_x - 1.0, y_zero, meter_span_w + 2.0, 1.0),
            theme.meter.zero_db.with_alpha(0.70),
        );

        // Vertical Fader track + headroom band + 0 dB detent tick + glowing trim thumb + live automated ghost fader needle.
        if fader_focused {
            let fader_ring = Rect::new(fx - 4.0, mf.y - 2.0, fader_w + 8.0, mf.h + 4.0);
            dl.panel(
                fader_ring,
                slot_accent.with_alpha(0.08),
                slot_accent.with_alpha(0.80),
                theme.radii.badge,
                1.2,
                slot_glow * 0.75,
            );
        }
        let track = Rect::new(fx + 3.0, mf.y, 4.0, mf.h);
        dl.rounded(track, theme.meter.track_bg, 2.0);
        if headroom_h > 0.5 {
            dl.fill(
                Rect::new(track.x, track.y, track.w, headroom_h),
                headroom_tint,
            );
        }
        let gnorm = db_norm(ch.gain);
        let fill_h = (track.h * gnorm).clamp(0.0, track.h);
        let fill_top = track.bottom() - fill_h;
        let trim_center_y = fill_top.clamp(mf.y + 4.0, mf.bottom() - 4.0);
        if fill_h > 0.5 {
            let base_alpha = if automated { 0.28 } else { 0.65 };
            if boosted && fill_top < y_zero {
                let below_h = (track.bottom() - y_zero).max(0.0);
                if below_h > 0.5 {
                    dl.rounded(
                        Rect::new(track.x, y_zero, track.w, below_h),
                        rcol.with_alpha(base_alpha),
                        2.0,
                    );
                }
                let above_h = (y_zero - fill_top).max(0.0);
                if above_h > 0.5 {
                    dl.rounded(
                        Rect::new(track.x, fill_top, track.w, above_h),
                        boost_col.with_alpha(if automated { 0.45 } else { 0.88 }),
                        2.0,
                    );
                }
            } else {
                dl.rounded(
                    Rect::new(track.x, fill_top, track.w, fill_h),
                    rcol.with_alpha(base_alpha),
                    2.0,
                );
            }
        }

        if let Some(eff_gain) = ch.effective_gain {
            let eff_norm = db_norm(eff_gain);
            let eff_fill_h = (track.h * eff_norm).clamp(0.0, track.h);
            let eff_y = (track.bottom() - eff_fill_h).clamp(mf.y + 2.0, mf.bottom() - 2.0);
            let ry0 = trim_center_y.min(eff_y);
            let ry1 = trim_center_y.max(eff_y);
            if ry1 - ry0 >= 1.5 {
                dl.rounded(
                    Rect::new(fx, ry0, fader_w, ry1 - ry0),
                    theme.accent.modulation.with_alpha(0.18),
                    2.0,
                );
            }
            if eff_fill_h > 0.5 {
                dl.rounded_glow(
                    Rect::new(track.x, track.bottom() - eff_fill_h, track.w, eff_fill_h),
                    theme.accent.modulation.with_alpha(0.88),
                    2.0,
                    0.28,
                );
            }
        }

        // Crisp 0 dB unity detent tick across the fader track (under thumb, above fill).
        dl.fill(
            Rect::new(fx, y_zero, fader_w, 1.0),
            theme.meter.zero_db.with_alpha(0.65),
        );

        let thumb_y = (track.bottom() - fill_h - 4.0).clamp(mf.y, mf.bottom() - 8.0);
        dl.panel(
            Rect::new(fx - 2.0, thumb_y, fader_w + 4.0, 8.0),
            if fader_focused {
                slot_accent
            } else if ch.selected {
                theme.accent.focus
            } else if automated {
                theme.text.secondary
            } else {
                theme.text.primary
            },
            theme.surface.canvas,
            2.0,
            1.0,
            if ch.selected { 0.35 } else { 0.0 },
        );

        if let Some(eff_gain) = ch.effective_gain {
            let eff_norm = db_norm(eff_gain);
            let eff_fill_h = (track.h * eff_norm).clamp(0.0, track.h);
            let eff_y = (track.bottom() - eff_fill_h).clamp(mf.y + 2.0, mf.bottom() - 2.0);
            dl.rounded_glow(
                Rect::new(fx - 3.0, eff_y - 1.5, fader_w + 6.0, 3.0),
                theme.accent.modulation,
                1.5,
                0.68,
            );
            dl.rounded_glow(
                Rect::new(fx - 6.0, eff_y - 2.5, 4.0, 5.0),
                theme.accent.modulation,
                2.0,
                0.75,
            );
        }

        // Optional live top-down Compressor/Limiter GR micro-bar (0..-24 dB) + peak-hold tick.
        if show_gr_bar {
            let grect = Rect::new(gr_x, mf.y, gr_col_w, mf.h);
            dl.rounded(grect, theme.meter.track_bg, 2.0);
            for gr_tick in [6.0_f32, 12.0, 18.0] {
                let ty = grect.y + grect.h * (gr_tick / 24.0);
                dl.fill(
                    Rect::new(grect.x + 0.5, ty, (grect.w - 1.0).max(1.0), 1.0),
                    theme.border.grid_step,
                );
            }
            let gr_norm = (ch.gr_db / 24.0).clamp(0.0, 1.0);
            let drop_h = (grect.h * gr_norm).clamp(0.0, grect.h);
            if drop_h > 0.5 {
                dl.gradient_v(
                    Rect::new(grect.x, grect.y, grect.w, drop_h),
                    theme.meter.gr,
                    theme.meter.clip,
                    1.5,
                    0.35,
                );
            }
            let hold_norm = (ch.gr_hold_db / 24.0).clamp(0.0, 1.0);
            if hold_norm > 0.01 {
                let hy = (grect.y + grect.h * hold_norm).clamp(grect.y, grect.bottom() - 2.0);
                dl.fill(
                    Rect::new(grect.x - 0.5, hy, grect.w + 1.0, 2.0),
                    if ch.gr_hold_db >= 9.0 {
                        theme.meter.clip
                    } else {
                        theme.meter.hold_tick
                    },
                );
            }
        }
    }

    // Compact live GR readout pill (sits cleanly above trim dB readout, never colliding).
    if let (Some(tag), Some(gr_area)) = (ch.gr_tag, gr_badge_rect) {
        let active_gr = ch.gr_db > 0.05 || ch.gr_hold_db > 0.05;
        let gr_text = if ch.gr_db > 0.05 {
            format!("{tag} -{:.1}", ch.gr_db)
        } else if ch.gr_hold_db > 0.05 {
            format!("{tag} -{:.1}", ch.gr_hold_db)
        } else {
            format!("{tag} 0.0")
        };
        let gr_col = if ch.gr_db >= 6.0 || ch.gr_hold_db >= 9.0 {
            theme.meter.clip
        } else if active_gr {
            theme.meter.gr
        } else {
            theme.text.muted
        };
        let pill_w = (gr_area.w - 4.0).clamp(48.0, 116.0).min(gr_area.w);
        let pill = Rect::new(
            gr_area.x + (gr_area.w - pill_w) * 0.5,
            gr_area.y + 1.0,
            pill_w,
            (gr_area.h - 2.0).max(12.0),
        );
        dl.panel(
            pill,
            if active_gr {
                gr_col.with_alpha(0.15)
            } else {
                theme.surface.elevated
            },
            gr_col.with_alpha(if active_gr { 0.65 } else { 0.30 }),
            theme.radii.badge,
            1.0,
            if active_gr { 0.14 } else { 0.0 },
        );
        dl.text(
            pill,
            gr_text,
            theme.font.small - 2.0,
            active_gr,
            gr_col,
            TextAlign::Center,
        );
    }

    // Bottom: live effective dB (when automated) or trim dB readout, centered pan bar, and M/S pills.
    let (db_text, db_col) = match &ch.effective_gain_db {
        Some(eff_db) => (format!("{eff_db} dB"), theme.accent.modulation),
        None => (
            format!("{} dB", ch.gain_db),
            if boosted {
                boost_col
            } else {
                theme.text.primary
            },
        ),
    };
    let db_rect = Rect::new(bottom.x, bottom.y, bottom.w, 15.0);
    if fader_focused {
        dl.panel(
            db_rect,
            slot_accent.with_alpha(0.12),
            slot_accent.with_alpha(0.85),
            theme.radii.badge,
            1.0,
            0.0,
        );
    }
    dl.text(
        db_rect,
        db_text,
        theme.font.small - 1.0,
        ch.selected || automated || boosted,
        db_col,
        TextAlign::Center,
    );

    // Pan indicator bar + glowing focus ring when MixerSlot::Pan is focused!
    if pan_focused {
        let pan_ring = Rect::new(bottom.x, bottom.y + 16.0, bottom.w, 13.0);
        dl.panel(
            pan_ring,
            slot_accent.with_alpha(0.14),
            slot_accent,
            theme.radii.badge,
            1.2,
            slot_glow * 0.8,
        );
    }
    let pan_track = Rect::new(
        bottom.x + 4.0,
        bottom.y + 20.0,
        (bottom.w - 8.0).max(12.0),
        5.0,
    );
    dl.rounded(pan_track, theme.meter.track_bg, 2.5);
    let mid_x = pan_track.x + pan_track.w * 0.5;
    dl.fill(
        Rect::new(mid_x - 0.5, pan_track.y - 1.0, 1.0, pan_track.h + 2.0),
        theme.border.hairline,
    );
    let pan_x = mid_x + ch.pan.clamp(-1.0, 1.0) * (pan_track.w * 0.5);
    let px0 = mid_x.min(pan_x);
    let pw = (pan_x - mid_x).abs().max(2.0);
    let pan_col = if pan_focused {
        slot_accent
    } else {
        theme.accent.routing
    };
    dl.rounded_glow(
        Rect::new(px0, pan_track.y, pw, pan_track.h),
        pan_col.with_alpha(if pan_automated { 0.32 } else { 1.0 }),
        2.0,
        0.0,
    );
    if pan_focused {
        dl.rounded_glow(
            Rect::new(pan_x - 2.5, pan_track.y - 2.0, 5.0, pan_track.h + 4.0),
            slot_accent,
            2.5,
            slot_glow,
        );
    }
    if let Some(eff_pan) = ch.effective_pan {
        let eff_x = (mid_x + eff_pan.clamp(-1.0, 1.0) * (pan_track.w * 0.5))
            .clamp(pan_track.x + 2.0, pan_track.right() - 2.0);
        let dx0 = pan_x.min(eff_x);
        let dw = (eff_x - pan_x).abs();
        if dw >= 1.5 {
            dl.rounded(
                Rect::new(dx0, pan_track.y - 1.0, dw, pan_track.h + 2.0),
                theme.accent.modulation.with_alpha(0.22),
                2.0,
            );
        }
        let ex0 = mid_x.min(eff_x);
        let ew = (eff_x - mid_x).abs().max(2.0);
        dl.rounded_glow(
            Rect::new(ex0, pan_track.y, ew, pan_track.h),
            theme.accent.modulation.with_alpha(0.86),
            2.0,
            0.35,
        );
        dl.rounded_glow(
            Rect::new(eff_x - 2.0, pan_track.y - 2.5, 4.0, pan_track.h + 5.0),
            theme.accent.modulation,
            2.0,
            0.78,
        );
    }

    // M / S pills.
    let half_w = ((bottom.w - 4.0) * 0.5).max(16.0);
    let m_rect = Rect::new(bottom.x, bottom.y + 32.0, half_w, 20.0);
    let s_rect = Rect::new(bottom.x + half_w + 4.0, bottom.y + 32.0, half_w, 20.0);
    dl.panel(
        m_rect,
        if ch.muted {
            theme.accent.alert.with_alpha(0.30)
        } else {
            theme.surface.elevated
        },
        if ch.muted {
            theme.accent.alert
        } else {
            theme.border.subtle
        },
        theme.radii.badge,
        1.0,
        if ch.muted { 0.35 } else { 0.0 },
    );
    dl.text(
        m_rect,
        "M",
        theme.font.small - 1.0,
        ch.muted,
        if ch.muted {
            theme.accent.alert
        } else {
            theme.text.muted
        },
        TextAlign::Center,
    );
    dl.panel(
        s_rect,
        if ch.solo {
            theme.accent.warning_key.with_alpha(0.30)
        } else {
            theme.surface.elevated
        },
        if ch.solo {
            theme.accent.warning_key
        } else {
            theme.border.subtle
        },
        theme.radii.badge,
        1.0,
        if ch.solo { 0.30 } else { 0.0 },
    );
    dl.text(
        s_rect,
        "S",
        theme.font.small - 1.0,
        ch.solo,
        if ch.solo {
            theme.accent.warning_key
        } else {
            theme.text.muted
        },
        TextAlign::Center,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadence_core::mixer::ChannelRef;

    fn sample_channel(
        role: MixerRole,
        inserts: usize,
        sends: usize,
        ducks: usize,
    ) -> MixerChannelView {
        MixerChannelView {
            channel: ChannelRef::Channel(1),
            index: 0,
            name: "Strip".into(),
            role,
            selected: true,
            gain: 1.0,
            gain_db: "+0.0".into(),
            pan: 0.0,
            pan_label: "C".into(),
            muted: false,
            solo: false,
            route_name: "Master".into(),
            sends: (0..sends).map(|i| (format!("Ret{i}"), 0.25)).collect(),
            send_controlled: vec![false; sends],
            inserts: (0..inserts).map(|i| (format!("FX{i}"), i == 1)).collect(),
            ducks: (0..ducks).map(|i| format!("Duck{i}")).collect(),
            duck_details: (0..ducks).map(|_| (0.6, 150.0, "Kick".into())).collect(),
            level_modulators: Vec::new(),
            automated_gain_scale: None,
            effective_gain: None,
            effective_gain_db: None,
            pan_modulators: Vec::new(),
            effective_pan: None,
            effective_pan_label: None,
            meter: None,
            gr_db: 0.0,
            gr_hold_db: 0.0,
            gr_tag: None,
        }
    }

    #[test]
    fn detail_stack_omits_output_on_return_and_master_and_stacks_in_slot_order() {
        let theme = Theme::cadence_dark();
        let ret = sample_channel(MixerRole::Return, 1, 0, 1);
        let ret_rows = build_detail_rows(&ret, &theme);
        assert!(
            !ret_rows.iter().any(|r| r.slot == MixerSlot::Output),
            "Return channel must omit OUT row from Mixer detail stack"
        );
        assert_eq!(
            ret_rows.iter().map(|r| r.slot).collect::<Vec<_>>(),
            vec![MixerSlot::Insert(0), MixerSlot::Duck(0)]
        );

        let master = sample_channel(MixerRole::Master, 1, 0, 0);
        let master_rows = build_detail_rows(&master, &theme);
        assert!(!master_rows.iter().any(|r| r.slot == MixerSlot::Output));

        let audio = sample_channel(MixerRole::Audio, 2, 2, 1);
        let audio_rows = build_detail_rows(&audio, &theme);
        assert_eq!(
            audio_rows.iter().map(|r| r.slot).collect::<Vec<_>>(),
            vec![
                MixerSlot::Insert(0),
                MixerSlot::Insert(1),
                MixerSlot::Send(0),
                MixerSlot::Send(1),
                MixerSlot::Output,
                MixerSlot::Duck(0),
            ]
        );

        // Render with many rows to verify scrolling, vertical stacking order, and non-empty draw list.
        let crowded = sample_channel(MixerRole::Audio, 6, 3, 2);
        let view = MixerDeckView {
            channels: vec![crowded],
            selected_index: 0,
            focused_slot: MixerSlot::Duck(1),
            bank_scroll: 0,
            detail_scroll: 0,
            pending: false,
            solo_audition: false,
        };
        let mut dl = DisplayList::new(900.0, 420.0, 1.0);
        draw_mixer(
            &mut dl,
            Rect::new(0.0, 0.0, 900.0, 420.0),
            &view,
            true,
            UiModeKind::Normal,
            &theme,
        );
        assert!(
            !dl.base.commands.is_empty(),
            "Mixer draw_mixer must emit display commands for scrollable vertical stack"
        );
        let duck_ys: Vec<f32> = dl
            .base
            .texts
            .iter()
            .filter(|t| t.text == "DUCK")
            .map(|t| t.rect.y)
            .collect();
        assert_eq!(
            duck_ys.len(),
            2,
            "expected both DUCK rows visible when scrolled to bottom"
        );
        assert!(
            duck_ys[1] > duck_ys[0],
            "DUCK rows must stack vertically top-to-bottom: {duck_ys:?}"
        );
    }

    #[test]
    fn smooth_console_scrolling_pins_master_locks_strip_widths_and_keeps_detail_height_stable() {
        let theme = Theme::cadence_dark();
        let make_console = |selected_idx: usize| -> Vec<MixerChannelView> {
            let mut chs: Vec<MixerChannelView> = (0..13)
                .map(|i| {
                    // Alternate empty vs heavily loaded channels to prove detail height never shifts.
                    let (ins, snd, dck) = if i % 2 == 0 { (0, 0, 0) } else { (5, 2, 1) };
                    let mut ch = sample_channel(MixerRole::Audio, ins, snd, dck);
                    ch.channel = ChannelRef::Channel(i as u64 + 1);
                    ch.index = i;
                    ch.name = format!("Ch{i:02}");
                    ch.selected = i == selected_idx;
                    ch
                })
                .collect();
            let mut master = sample_channel(MixerRole::Master, 1, 0, 0);
            master.channel = ChannelRef::Master;
            master.index = 13;
            master.name = "Master".into();
            master.selected = selected_idx == 13;
            chs.push(master);
            chs
        };

        // 1. Standard Mix split width (1060px): 13 bank + pinned Master overflows -> 9 bank + Master + `<` & `>` rails.
        let area = Rect::new(0.0, 0.0, 1075.0, 400.0);
        let layout_a = compute_strip_layout(0.0, 1060.0, 14, true);
        assert!(layout_a.master_pinned);
        assert!(layout_a.bank_overflows);
        assert_eq!(layout_a.bank_slots, 9);
        assert!(layout_a.strip_w >= MIXER_MIN_STRIP_W && layout_a.strip_w <= MIXER_MAX_STRIP_W);

        // Render at scroll 0 (Ch00 selected): Ch00..Ch08 + Master visible, `>` rail shows 4 hidden strips!
        let view0 = MixerDeckView {
            channels: make_console(0),
            selected_index: 0,
            focused_slot: MixerSlot::Fader,
            bank_scroll: 0,
            detail_scroll: 0,
            pending: false,
            solo_audition: false,
        };
        let mut dl0 = DisplayList::new(1075.0, 400.0, 1.0);
        draw_mixer(&mut dl0, area, &view0, true, UiModeKind::Normal, &theme);
        assert_eq!(dl0.current_clip(), dl0.viewport);
        let texts0: Vec<&str> = dl0.base.texts.iter().map(|t| t.text.as_str()).collect();
        assert!(texts0.contains(&"Ch00") && texts0.contains(&"Ch08"));
        assert!(!texts0.contains(&"Ch09"));
        assert!(
            texts0.contains(&"Master"),
            "Master must stay pinned on right"
        );
        assert!(texts0.contains(&">") && texts0.contains(&"4"));

        // Render at scroll 2 (Ch05 selected — crowded channel with 9 detail rows):
        // Ch02..Ch10 + Master visible, `<` shows 2 hidden, `>` shows 2 hidden, and Master X/Y + detail divider Y stay IDENTICAL!
        let view2 = MixerDeckView {
            channels: make_console(5),
            selected_index: 5,
            focused_slot: MixerSlot::Fader,
            bank_scroll: 2,
            detail_scroll: 0,
            pending: false,
            solo_audition: false,
        };
        let mut dl2 = DisplayList::new(1075.0, 400.0, 1.0);
        draw_mixer(&mut dl2, area, &view2, true, UiModeKind::Normal, &theme);
        let texts2: Vec<&str> = dl2.base.texts.iter().map(|t| t.text.as_str()).collect();
        assert!(texts2.contains(&"Ch02") && texts2.contains(&"Ch10"));
        assert!(!texts2.contains(&"Ch01") && !texts2.contains(&"Ch11"));
        assert!(texts2.contains(&"Master"));
        assert!(texts2.contains(&"<") && texts2.contains(&">"));
        assert!(texts2.iter().any(|t| t.contains("1-3/9")));

        // Verify zero horizontal Master drift and zero vertical detail bounce between empty Ch00 and 9-row Ch05!
        let master_rect = |dl: &DisplayList| {
            dl.base
                .texts
                .iter()
                .find(|t| t.text == "Master")
                .expect("Master label")
                .rect
        };
        let detail_hdr_y = |dl: &DisplayList| {
            dl.base
                .texts
                .iter()
                .find(|t| t.text.starts_with("ROW: FADER"))
                .expect("ROW badge")
                .rect
                .y
        };
        assert!((master_rect(&dl0).x - master_rect(&dl2).x).abs() < 0.01);
        assert!((master_rect(&dl0).w - master_rect(&dl2).w).abs() < 0.01);
        assert!(
            (detail_hdr_y(&dl0) - detail_hdr_y(&dl2)).abs() < 0.01,
            "Detail stack height must remain locked across empty vs crowded channels"
        );

        // 2. Small 4-channel project caps strips at MIXER_MAX_STRIP_W (112px) without ballooning.
        let small = compute_strip_layout(0.0, 1060.0, 4, true);
        assert!(small.master_pinned && !small.bank_overflows);
        assert!((small.strip_w - MIXER_MAX_STRIP_W).abs() < 0.01);
        assert!((small.master_x - (1060.0 - MIXER_MAX_STRIP_W)).abs() < 0.01);

        // 3. Height-responsive detail rows: compact (0), short (2), standard (3), tall (4), zoomed (5..=6).
        assert_eq!(mixer_detail_visible_rows(180.0), 0);
        assert_eq!(mixer_detail_visible_rows(260.0), 2);
        assert_eq!(mixer_detail_visible_rows(380.0), 3);
        assert_eq!(mixer_detail_visible_rows(560.0), 4);
        assert_eq!(mixer_detail_visible_rows(740.0), 5);
        assert_eq!(mixer_detail_visible_rows(900.0), 6);
    }

    #[test]
    fn narrow_strip_collapse_omits_scale_and_compact_height_collapses_detail_list() {
        let theme = Theme::cadence_dark();
        let ch = sample_channel(MixerRole::Audio, 0, 0, 0);
        let view = MixerDeckView {
            channels: vec![ch],
            selected_index: 0,
            focused_slot: MixerSlot::Fader,
            bank_scroll: 0,
            detail_scroll: 0,
            pending: false,
            solo_audition: false,
        };

        // 1. Standard 112px strip shows dB scale labels (`-12`, `-24`, `-48`) AND Audio empty-state hint (`w add send`).
        let mut dl_wide = DisplayList::new(620.0, 460.0, 1.0);
        draw_mixer(
            &mut dl_wide,
            Rect::new(0.0, 0.0, 620.0, 460.0),
            &view,
            true,
            UiModeKind::Normal,
            &theme,
        );
        assert_eq!(dl_wide.current_clip(), dl_wide.viewport);
        assert!(dl_wide.base.texts.iter().any(|t| t.text == "-12"));
        assert!(dl_wide
            .base
            .texts
            .iter()
            .any(|t| t.text.contains("f add fx · w add send · d add duck")));

        // 2. Narrow 4-strip bank (~88 px/strip) with GR bar collapses dB scale labels cleanly
        //    (`mf.w < core_w + 26.0` when GR column is shown).
        let mut channels_4 = Vec::new();
        for i in 0..4 {
            let mut c = sample_channel(MixerRole::Audio, 0, 0, 0);
            c.channel = ChannelRef::Channel(i as u64 + 1);
            c.index = i;
            c.name = format!("S{i}");
            c.selected = i == 0;
            c.gr_tag = Some("CMP");
            c.gr_db = 4.5;
            c.gr_hold_db = 6.0;
            channels_4.push(c);
        }
        let view_narrow = MixerDeckView {
            channels: channels_4,
            selected_index: 0,
            focused_slot: MixerSlot::Fader,
            bank_scroll: 0,
            detail_scroll: 0,
            pending: false,
            solo_audition: false,
        };
        let mut dl_narrow = DisplayList::new(380.0, 420.0, 1.0);
        draw_mixer(
            &mut dl_narrow,
            Rect::new(0.0, 0.0, 380.0, 420.0),
            &view_narrow,
            true,
            UiModeKind::Normal,
            &theme,
        );
        assert_eq!(dl_narrow.current_clip(), dl_narrow.viewport);
        assert!(!dl_narrow.base.texts.iter().any(|t| t.text == "-12"));
        assert!(dl_narrow.base.texts.iter().any(|t| t.text == "CMP -4.5"));

        // 3. Compact deck height (< 220 px inner) collapses detail stack to telemetry header only.
        let mut dl_short = DisplayList::new(620.0, 190.0, 1.0);
        draw_mixer(
            &mut dl_short,
            Rect::new(0.0, 0.0, 620.0, 190.0),
            &view,
            true,
            UiModeKind::Normal,
            &theme,
        );
        assert_eq!(dl_short.current_clip(), dl_short.viewport);
        assert!(!dl_short.base.texts.iter().any(|t| t.text == "OUT"));

        // 4. Tiny area early-return keeps clip stack balanced.
        let mut dl_tiny = DisplayList::new(100.0, 80.0, 1.0);
        draw_mixer(
            &mut dl_tiny,
            Rect::new(0.0, 0.0, 100.0, 80.0),
            &view,
            true,
            UiModeKind::Normal,
            &theme,
        );
        assert_eq!(dl_tiny.current_clip(), dl_tiny.viewport);
    }

    #[test]
    fn hot_vs_master_clip_badges_and_effective_gain_pan_ghost_needles() {
        use cadence_core::runtime::ChannelMeter;

        let theme = Theme::cadence_dark();
        let mut hot_audio = sample_channel(MixerRole::Audio, 0, 0, 0);
        hot_audio.name = "Bass".into();
        hot_audio.selected = true;
        hot_audio.gain = 1.0;
        hot_audio.effective_gain = Some(0.5);
        hot_audio.effective_gain_db = Some("-6.0".into());
        hot_audio.automated_gain_scale = Some(0.5);
        hot_audio.pan = -0.25;
        hot_audio.pan_label = "L25".into();
        hot_audio.effective_pan = Some(0.65);
        hot_audio.effective_pan_label = Some("R65".into());
        hot_audio.meter = Some(ChannelMeter {
            channel: ChannelRef::Channel(1),
            input_peak: [1.35, 1.25],
            input_peak_hold: [1.45, 1.38],
            output_peak: [1.20, 1.15],
            output_peak_hold: [1.42, 1.32],
            hot: true,
            clipped: false,
        });

        let mut clipped_master = sample_channel(MixerRole::Master, 0, 0, 0);
        clipped_master.channel = ChannelRef::Master;
        clipped_master.index = 1;
        clipped_master.name = "Master".into();
        clipped_master.selected = false;
        clipped_master.meter = Some(ChannelMeter {
            channel: ChannelRef::Master,
            input_peak: [1.30, 1.28],
            input_peak_hold: [1.40, 1.35],
            output_peak: [1.25, 1.22],
            output_peak_hold: [1.32, 1.29],
            hot: true,
            clipped: true,
        });

        let view = MixerDeckView {
            channels: vec![hot_audio, clipped_master],
            selected_index: 0,
            focused_slot: MixerSlot::Fader,
            bank_scroll: 0,
            detail_scroll: 0,
            pending: false,
            solo_audition: false,
        };
        let mut dl = DisplayList::new(620.0, 480.0, 1.0);
        draw_mixer(
            &mut dl,
            Rect::new(0.0, 0.0, 620.0, 480.0),
            &view,
            true,
            UiModeKind::Normal,
            &theme,
        );
        assert_eq!(dl.current_clip(), dl.viewport);

        // Hot internal audio bus renders `CH` + `HOT` badge (never `!CLIP`), while Master renders `MST` + `!CLIP`.
        assert!(dl.base.texts.iter().any(|t| t.text == "CH"));
        assert!(dl
            .base
            .texts
            .iter()
            .any(|t| t.text.starts_with("HOT") && t.color == theme.meter.hot));
        assert!(dl.base.texts.iter().any(|t| t.text == "MST"));
        assert!(dl
            .base
            .texts
            .iter()
            .any(|t| t.text == "!CLIP" && t.color == theme.meter.clip));

        // Automated gain + pan emit modulation-colored ghost needles/bars and live telemetry readouts.
        let mod_col = theme.accent.modulation;
        let mod_lin = mod_col.to_linear();
        let mod_quads = dl
            .base
            .quads
            .iter()
            .filter(|q| {
                (q.color_top[0] - mod_lin[0]).abs() < 1e-4
                    && (q.color_top[1] - mod_lin[1]).abs() < 1e-4
                    && (q.color_top[2] - mod_lin[2]).abs() < 1e-4
                    && q.params[2] > 0.5
            })
            .count();
        assert!(
            mod_quads >= 3,
            "expected glowing modulation ghost needles for effective gain and pan, got {mod_quads}"
        );
        assert!(dl
            .base
            .texts
            .iter()
            .any(|t| t.text == "-6.0 dB" && t.color == mod_col));
        assert!(dl
            .base
            .texts
            .iter()
            .any(|t| t.text.contains("live -6.0 dB") && t.text.contains("live R65")));
    }
}
