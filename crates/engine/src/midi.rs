//! Bi-directional MIDI input & feedback output onto Cadence's live actions.
//!
//! Parsing (`MidiMessage`) and hardware feedback encoding (`encode_feedback`) are
//! pure and work without hardware dependencies; OS device access (`midir`) is gated
//! behind the `midi` feature.
use cadence_core::midi_map::{FeedbackProtocol, MidiFeedback, TransportLedKind};
pub use cadence_core::runtime::MidiMessage;

#[cfg(any(feature = "midi", test))]
mod ring {
    use super::MidiMessage;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    pub(super) const MIDI_RING_CAP: usize = 512; // power of two

    #[inline]
    fn pack(msg: MidiMessage) -> u64 {
        match msg {
            MidiMessage::NoteOn {
                channel,
                note,
                velocity,
            } => {
                1 | (u64::from(channel) << 8)
                    | (u64::from(note) << 16)
                    | (u64::from(velocity) << 24)
            }
            MidiMessage::NoteOff { channel, note } => {
                2 | (u64::from(channel) << 8) | (u64::from(note) << 16)
            }
            MidiMessage::ProgramChange { channel, program } => {
                3 | (u64::from(channel) << 8) | (u64::from(program) << 16)
            }
            MidiMessage::Control {
                channel,
                controller,
                value,
            } => {
                4 | (u64::from(channel) << 8)
                    | (u64::from(controller) << 16)
                    | (u64::from(value) << 24)
            }
            MidiMessage::PitchBend { channel, value } => {
                let raw = (value as u16) as u64;
                5 | (u64::from(channel) << 8) | (raw << 16)
            }
            MidiMessage::Mmc { command } => 6 | (u64::from(command) << 8),
        }
    }

    #[inline]
    fn unpack(bits: u64) -> Option<MidiMessage> {
        match bits & 0xFF {
            1 => Some(MidiMessage::NoteOn {
                channel: ((bits >> 8) & 0xFF) as u8,
                note: ((bits >> 16) & 0xFF) as u8,
                velocity: ((bits >> 24) & 0xFF) as u8,
            }),
            2 => Some(MidiMessage::NoteOff {
                channel: ((bits >> 8) & 0xFF) as u8,
                note: ((bits >> 16) & 0xFF) as u8,
            }),
            3 => Some(MidiMessage::ProgramChange {
                channel: ((bits >> 8) & 0xFF) as u8,
                program: ((bits >> 16) & 0xFF) as u8,
            }),
            4 => Some(MidiMessage::Control {
                channel: ((bits >> 8) & 0xFF) as u8,
                controller: ((bits >> 16) & 0xFF) as u8,
                value: ((bits >> 24) & 0xFF) as u8,
            }),
            5 => Some(MidiMessage::PitchBend {
                channel: ((bits >> 8) & 0xFF) as u8,
                value: ((bits >> 16) & 0xFFFF) as u16 as i16,
            }),
            6 => Some(MidiMessage::Mmc {
                command: ((bits >> 8) & 0xFF) as u8,
            }),
            _ => None,
        }
    }

    /// Bounded lock-free SPSC MIDI ring buffer (zero heap allocation
    /// in the real-time OS MIDI callback).
    pub(super) struct MidiRing {
        slots: Box<[AtomicU64]>,
        head: AtomicUsize,
        tail: AtomicUsize,
        dropped: AtomicU64,
    }

    impl MidiRing {
        pub(super) fn new() -> Self {
            let slots = (0..MIDI_RING_CAP)
                .map(|_| AtomicU64::new(0))
                .collect::<Vec<_>>()
                .into_boxed_slice();
            Self {
                slots,
                head: AtomicUsize::new(0),
                tail: AtomicUsize::new(0),
                dropped: AtomicU64::new(0),
            }
        }

        /// Producer-only push from the OS MIDI callback. Never allocates or blocks.
        #[inline]
        pub(super) fn push(&self, msg: MidiMessage) -> bool {
            let tail = self.tail.load(Ordering::Relaxed);
            let head = self.head.load(Ordering::Acquire);
            if tail.wrapping_sub(head) >= MIDI_RING_CAP {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            self.slots[tail & (MIDI_RING_CAP - 1)].store(pack(msg), Ordering::Relaxed);
            self.tail.store(tail.wrapping_add(1), Ordering::Release);
            true
        }

        /// Consumer-only bounded drain on the UI/host thread.
        pub(super) fn drain(&self, max: usize) -> Vec<MidiMessage> {
            let mut head = self.head.load(Ordering::Relaxed);
            let tail = self.tail.load(Ordering::Acquire);
            let available = tail.wrapping_sub(head).min(max);
            let mut out = Vec::with_capacity(available);
            for _ in 0..available {
                let packed = self.slots[head & (MIDI_RING_CAP - 1)].load(Ordering::Relaxed);
                if let Some(msg) = unpack(packed) {
                    out.push(msg);
                }
                head = head.wrapping_add(1);
            }
            self.head.store(head, Ordering::Release);
            out
        }

        pub(super) fn dropped_count(&self) -> u64 {
            self.dropped.load(Ordering::Relaxed)
        }
    }
}

fn sanitize_ascii(text: &str, max_len: usize) -> Vec<u8> {
    text.chars()
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c as u8
            } else {
                match c {
                    '·' | '•' => b'-',
                    '→' => b'>',
                    '–' | '—' => b'-',
                    _ => b' ',
                }
            }
        })
        .take(max_len)
        .collect()
}

/// Pure wire-protocol serializer for semantic `MidiFeedback` messages.
pub fn encode_feedback(protocol: FeedbackProtocol, feedback: &MidiFeedback) -> Vec<Vec<u8>> {
    match protocol {
        FeedbackProtocol::None => Vec::new(),
        FeedbackProtocol::StandardCcNote => match feedback {
            MidiFeedback::CcValue { channel, cc, value } => {
                vec![vec![0xB0 | (channel & 0x0F), cc & 0x7F, value & 0x7F]]
            }
            MidiFeedback::PadRgb { index, r, g, b } => {
                let lit = (*r).max(*g).max(*b);
                let vel = if lit > 64 {
                    127
                } else if lit > 8 {
                    64
                } else {
                    0
                };
                vec![vec![0x99, 36 + (index & 7), vel]]
            }
            MidiFeedback::InitDawMode(_)
            | MidiFeedback::Display { .. }
            | MidiFeedback::TransportLed { .. } => Vec::new(),
        },
        FeedbackProtocol::ArturiaDaw => match feedback {
            MidiFeedback::InitDawMode(on) => vec![vec![
                0xF0,
                0x00,
                0x20,
                0x6B,
                0x7F,
                0x42,
                0x02,
                0x02,
                0x40,
                0x6A,
                if *on { 0x21 } else { 0x20 },
                0xF7,
            ]],
            MidiFeedback::PadRgb { index, r, g, b } => vec![vec![
                0xF0,
                0x00,
                0x20,
                0x6B,
                0x7F,
                0x42,
                0x02,
                0x02,
                0x16,
                0x04 + (index & 7),
                r & 0x7F,
                g & 0x7F,
                b & 0x7F,
                0xF7,
            ]],
            MidiFeedback::TransportLed { kind, active } => {
                let (id, on_rgb, dim_rgb) = match kind {
                    TransportLedKind::Loop => (0x57, (0, 110, 127), (0, 20, 28)),
                    TransportLedKind::Stop => (0x58, (127, 84, 0), (24, 16, 0)),
                    TransportLedKind::Play => (0x59, (0, 127, 42), (0, 26, 10)),
                    TransportLedKind::Record => (0x5A, (127, 10, 10), (28, 4, 4)),
                    TransportLedKind::Tap => (0x5B, (120, 120, 120), (20, 20, 20)),
                };
                let (r, g, b) = if *active { on_rgb } else { dim_rgb };
                vec![vec![
                    0xF0,
                    0x00,
                    0x20,
                    0x6B,
                    0x7F,
                    0x42,
                    0x02,
                    0x02,
                    0x16,
                    id,
                    r & 0x7F,
                    g & 0x7F,
                    b & 0x7F,
                    0xF7,
                ]]
            }
            MidiFeedback::Display {
                line1,
                line2,
                value_bar,
            } => {
                let l1 = sanitize_ascii(line1, 19);
                let l2 = sanitize_ascii(line2, 15);
                let mut msg = vec![0xF0, 0x00, 0x20, 0x6B, 0x7F, 0x42, 0x04, 0x02, 0x60];
                if let Some(val) = value_bar {
                    msg.extend_from_slice(&[0x1F, 0x03, 0x01, val & 0x7F, 0x00, 0x00]);
                } else {
                    msg.extend_from_slice(&[0x1F, 0x02, 0x01, 0x00]);
                }
                msg.push(0x01);
                msg.extend_from_slice(&l1);
                msg.push(0x00);
                msg.push(0x02);
                msg.extend_from_slice(&l2);
                msg.push(0x00);
                msg.push(0xF7);
                vec![msg]
            }
            MidiFeedback::CcValue { channel, cc, value } => {
                vec![vec![0xB0 | (channel & 0x0F), cc & 0x7F, value & 0x7F]]
            }
        },
    }
}

#[cfg(feature = "midi")]
mod device {
    use super::ring::MidiRing;
    use super::{encode_feedback, MidiMessage};
    use anyhow::{anyhow, Context, Result};
    use cadence_core::midi_map::{FeedbackProtocol, MidiFeedback};
    use std::sync::Arc;

    pub struct MidiInput {
        _connection: midir::MidiInputConnection<()>,
        ring: Arc<MidiRing>,
        pub port: String,
    }

    pub struct MidiOutput {
        connection: midir::MidiOutputConnection,
        last_error: Option<String>,
        pub port: String,
    }

    pub fn list_ports() -> Result<Vec<String>> {
        let input = midir::MidiInput::new("cadence").context("Opening MIDI input")?;
        Ok(input
            .ports()
            .iter()
            .filter_map(|port| input.port_name(port).ok())
            .collect())
    }

    pub fn list_output_ports() -> Result<Vec<String>> {
        let output = midir::MidiOutput::new("cadence").context("Opening MIDI output")?;
        Ok(output
            .ports()
            .iter()
            .filter_map(|port| output.port_name(port).ok())
            .collect())
    }

    /// Connect to the first input port whose name contains `filter` (any port when empty).
    pub fn connect(filter: &str) -> Result<MidiInput> {
        let mut input = midir::MidiInput::new("cadence").context("Opening MIDI input")?;
        // Keep SysEx enabled so universal MMC transport messages arrive cleanly.
        input.ignore(midir::Ignore::Time | midir::Ignore::ActiveSense);
        let wanted = filter.to_ascii_lowercase();
        let ports = input.ports();
        let port = ports
            .iter()
            .find(|port| {
                input
                    .port_name(port)
                    .is_ok_and(|name| name.to_ascii_lowercase().contains(&wanted))
            })
            .ok_or_else(|| {
                if ports.is_empty() {
                    anyhow!("No MIDI input ports are available")
                } else {
                    anyhow!("No MIDI input port matches {filter:?}; see --list-midi")
                }
            })?
            .clone();
        let name = input.port_name(&port)?;
        let ring = Arc::new(MidiRing::new());
        let cb_ring = Arc::clone(&ring);
        let connection = input
            .connect(
                &port,
                "cadence-in",
                move |_, bytes, _| {
                    if let Some(message) = MidiMessage::parse(bytes) {
                        let _ = cb_ring.push(message);
                    }
                },
                (),
            )
            .map_err(|error| anyhow!("Connecting to MIDI input {name}: {error}"))?;
        Ok(MidiInput {
            _connection: connection,
            ring,
            port: name,
        })
    }

    /// Connect to the first output port whose name contains `filter` (any port when empty).
    pub fn connect_output(filter: &str) -> Result<MidiOutput> {
        let output = midir::MidiOutput::new("cadence").context("Opening MIDI output")?;
        let wanted = filter.to_ascii_lowercase();
        let ports = output.ports();
        let port = ports
            .iter()
            .find(|port| {
                output
                    .port_name(port)
                    .is_ok_and(|name| name.to_ascii_lowercase().contains(&wanted))
            })
            .ok_or_else(|| {
                if ports.is_empty() {
                    anyhow!("No MIDI output ports are available")
                } else {
                    anyhow!("No MIDI output port matches {filter:?}")
                }
            })?
            .clone();
        let name = output.port_name(&port)?;
        let connection = output
            .connect(&port, "cadence-out")
            .map_err(|error| anyhow!("Connecting to MIDI output {name}: {error}"))?;
        Ok(MidiOutput {
            connection,
            last_error: None,
            port: name,
        })
    }

    impl MidiInput {
        pub fn drain(&self) -> Vec<MidiMessage> {
            self.ring.drain(256)
        }

        pub fn dropped_count(&self) -> u64 {
            self.ring.dropped_count()
        }
    }

    impl MidiOutput {
        /// Send feedback packets, surfacing any OS MIDI output failure cleanly.
        pub fn try_send_feedback(
            &mut self,
            protocol: FeedbackProtocol,
            events: &[MidiFeedback],
        ) -> Result<()> {
            for ev in events {
                for packet in encode_feedback(protocol, ev) {
                    if let Err(err) = self.connection.send(&packet) {
                        let msg = format!("MIDI OUT send to {} failed: {err}", self.port);
                        self.last_error = Some(msg.clone());
                        return Err(anyhow!(msg));
                    }
                }
            }
            self.last_error = None;
            Ok(())
        }

        /// Send feedback packets while capturing any failure into [`Self::last_error`].
        pub fn send_feedback(&mut self, protocol: FeedbackProtocol, events: &[MidiFeedback]) {
            let _ = self.try_send_feedback(protocol, events);
        }

        pub fn last_error(&self) -> Option<&str> {
            self.last_error.as_deref()
        }
    }
}

#[cfg(feature = "midi")]
pub use device::{connect, connect_output, list_output_ports, list_ports, MidiInput, MidiOutput};

#[cfg(test)]
mod tests {
    use super::ring::{MidiRing, MIDI_RING_CAP};
    use super::*;

    #[test]
    fn channel_voice_pitch_bend_and_mmc_messages_parse_cleanly() {
        assert_eq!(
            MidiMessage::parse(&[0x92, 60, 100]),
            Some(MidiMessage::NoteOn {
                channel: 2,
                note: 60,
                velocity: 100
            })
        );
        assert_eq!(
            MidiMessage::parse(&[0x90, 60, 0]),
            Some(MidiMessage::NoteOff {
                channel: 0,
                note: 60
            })
        );
        assert_eq!(
            MidiMessage::parse(&[0xc9, 4]),
            Some(MidiMessage::ProgramChange {
                channel: 9,
                program: 4
            })
        );
        // Pitch bend center (LSB=0, MSB=64 -> 8192 - 8192 = 0)
        assert_eq!(
            MidiMessage::parse(&[0xe0, 0x00, 0x40]),
            Some(MidiMessage::PitchBend {
                channel: 0,
                value: 0
            })
        );
        // Universal MMC Play SysEx (strict 6-byte F0 7F 7F 06 02 F7)
        assert_eq!(
            MidiMessage::parse(&[0xf0, 0x7f, 0x7f, 0x06, 0x02, 0xf7]),
            Some(MidiMessage::Mmc { command: 0x02 })
        );
        // Reject non-6-byte SysEx payloads (e.g. Locate / trailing data).
        assert_eq!(
            MidiMessage::parse(&[0xf0, 0x7f, 0x7f, 0x06, 0x44, 0x06, 0x01, 0xf7]),
            None
        );
        assert_eq!(MidiMessage::parse(&[0x90, 200, 1]), None);
        assert_eq!(MidiMessage::parse(&[0xf8]), None);
        assert_eq!(MidiMessage::parse(&[]), None);
    }

    #[test]
    fn lock_free_midi_ring_roundtrips_all_variants_and_bounds_overflow() {
        let ring = MidiRing::new();
        let samples = [
            MidiMessage::NoteOn {
                channel: 15,
                note: 127,
                velocity: 127,
            },
            MidiMessage::NoteOff {
                channel: 3,
                note: 42,
            },
            MidiMessage::ProgramChange {
                channel: 9,
                program: 12,
            },
            MidiMessage::Control {
                channel: 1,
                controller: 74,
                value: 99,
            },
            MidiMessage::PitchBend {
                channel: 0,
                value: -8192,
            },
            MidiMessage::PitchBend {
                channel: 4,
                value: 8191,
            },
            MidiMessage::Mmc { command: 0x02 },
        ];
        for msg in samples {
            assert!(ring.push(msg));
        }
        assert_eq!(ring.drain(64), samples);
        assert_eq!(ring.dropped_count(), 0);

        // Overflow drops cleanly with accurate counter and zero panic/alloc.
        for i in 0..(MIDI_RING_CAP + 17) {
            let ok = ring.push(MidiMessage::Control {
                channel: 0,
                controller: 1,
                value: (i & 0x7F) as u8,
            });
            assert_eq!(ok, i < MIDI_RING_CAP);
        }
        assert_eq!(ring.dropped_count(), 17);
        let first_half = ring.drain(256);
        let second_half = ring.drain(512);
        assert_eq!(first_half.len() + second_half.len(), MIDI_RING_CAP);
    }

    #[test]
    fn arturia_daw_and_standard_feedback_encode_valid_frames() {
        let init = encode_feedback(
            FeedbackProtocol::ArturiaDaw,
            &MidiFeedback::InitDawMode(true),
        );
        assert_eq!(
            init[0],
            vec![0xF0, 0x00, 0x20, 0x6B, 0x7F, 0x42, 0x02, 0x02, 0x40, 0x6A, 0x21, 0xF7]
        );
        let pad = encode_feedback(
            FeedbackProtocol::ArturiaDaw,
            &MidiFeedback::PadRgb {
                index: 2,
                r: 127,
                g: 40,
                b: 10,
            },
        );
        assert_eq!(
            pad[0],
            vec![0xF0, 0x00, 0x20, 0x6B, 0x7F, 0x42, 0x02, 0x02, 0x16, 0x06, 127, 40, 10, 0xF7]
        );
        let oled = encode_feedback(
            FeedbackProtocol::ArturiaDaw,
            &MidiFeedback::Display {
                line1: "Cutoff".into(),
                line2: "1.20 kHz".into(),
                value_bar: Some(90),
            },
        );
        assert_eq!(oled[0][0], 0xF0);
        assert_eq!(*oled[0].last().unwrap(), 0xF7);
        assert!(oled[0].windows(6).any(|w| w == b"Cutoff"));

        let std_cc = encode_feedback(
            FeedbackProtocol::StandardCcNote,
            &MidiFeedback::CcValue {
                channel: 1,
                cc: 74,
                value: 100,
            },
        );
        assert_eq!(std_cc[0], vec![0xB1, 74, 100]);
    }
}
