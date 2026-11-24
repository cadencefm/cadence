//! Takes: the score of a performance.
//!
//! A take stores the project and live flags it started from, each lane's
//! override at that moment, the initial timeline segment, and every change with
//! the transport step it landed on. Because every change lands on a step and
//! folds through the same [`PerformanceState`] transition as live playback, a
//! take replays exactly and renders offline deterministically.
use crate::live::{Change, Landed, PerformanceState};
use crate::material::{
    Edit, Id, LanePart, LiveFlags, LiveSegment, Override, Project, LIVE_RING_STEPS, STEPS_PER_BAR,
};
use crate::mixer::{ChannelParam, ChannelRef};
use crate::params::ParamAddress;
use crate::score::ScoreWindow;
use crate::store;
use crate::timeline::Segment;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

pub const TAKE_VERSION: u32 = 1;
pub const MAX_TAKE_BYTES: u64 = 32 * 1024 * 1024;
/// Reserved tail budget so `{"end":<u64>}\n` always fits within [`MAX_TAKE_BYTES`].
const TAKE_END_RESERVE_BYTES: u64 = 64;
/// Bounded command queue depth between host and background disk writer thread.
const JOURNAL_CHANNEL_CAPACITY: usize = 256;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TakeHeader {
    pub take: u32,
    pub title: String,
    pub start_step: u64,
    pub active: Vec<Override>,
    pub project: Box<Project>,
    #[serde(default, skip_serializing_if = "LiveFlags::is_empty")]
    pub flags: LiveFlags,
    /// The timeline segment active when the take started, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment: Option<Segment>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum TakeChange {
    Launch {
        lane: Id,
        part: LanePart,
    },
    Release {
        lane: Id,
    },
    ReleaseAll,
    /// Live lane/channel mute and solo flags.
    Flags {
        flags: LiveFlags,
    },
    /// Song edits applied to the document as it stood.
    Edits {
        edits: Vec<Edit>,
    },
    /// The whole document, after undo, redo or a live edit.
    Replace {
        project: Box<Project>,
    },
    /// Arrangement timeline segment heard from this step.
    Segment {
        segment: Segment,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TakeEvent {
    pub at: u64,
    pub label: String,
    pub change: TakeChange,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TakeMark {
    mark: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TakeEnd {
    end: u64,
}

#[derive(Clone, Debug)]
pub struct Take {
    pub header: TakeHeader,
    pub events: Vec<TakeEvent>,
    pub end_step: u64,
}

enum JournalCmd {
    Header(Box<TakeHeader>),
    Event(Box<TakeEvent>),
    Mark(u64),
    Seal(u64),
    Discard,
}

struct JournalWriter {
    tx: Option<SyncSender<JournalCmd>>,
    capped: Arc<AtomicBool>,
    handle: Option<JoinHandle<Result<()>>>,
}

impl JournalWriter {
    fn spawn(path: PathBuf) -> Result<Self> {
        Self::spawn_with_limit(path, MAX_TAKE_BYTES)
    }

    fn spawn_with_limit(path: PathBuf, max_bytes: u64) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .with_context(|| format!("Creating take directory {}", parent.display()))?;
        }
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .with_context(|| format!("Opening take journal {}", path.display()))?;
        let (tx, rx) = mpsc::sync_channel::<JournalCmd>(JOURNAL_CHANNEL_CAPACITY);
        let capped = Arc::new(AtomicBool::new(false));
        let worker_capped = Arc::clone(&capped);
        let worker_path = path.clone();
        let handle = thread::Builder::new()
            .name("cadence-take-journal".into())
            .spawn(move || -> Result<()> {
                let mut writer = BufWriter::new(file);
                let mut discarded = false;
                let mut bytes_written = 0u64;
                let mut is_capped = false;
                let body_limit = max_bytes.saturating_sub(TAKE_END_RESERVE_BYTES);
                let mut line_buf = Vec::with_capacity(4096);
                let mut write_bounded =
                    |buf: &[u8], limit: u64, bytes_written: &mut u64| -> Result<bool> {
                        let len = buf.len() as u64;
                        if bytes_written.saturating_add(len) > limit {
                            return Ok(false);
                        }
                        writer.write_all(buf)?;
                        writer.flush()?;
                        *bytes_written = bytes_written.saturating_add(len);
                        Ok(true)
                    };
                while let Ok(cmd) = rx.recv() {
                    match cmd {
                        JournalCmd::Header(header) => {
                            if !is_capped {
                                line_buf.clear();
                                serde_json::to_writer(&mut line_buf, &header)?;
                                line_buf.push(b'\n');
                                if !write_bounded(&line_buf, body_limit, &mut bytes_written)? {
                                    is_capped = true;
                                    worker_capped.store(true, Ordering::Release);
                                }
                            }
                        }
                        JournalCmd::Event(event) => {
                            if !is_capped {
                                line_buf.clear();
                                serde_json::to_writer(&mut line_buf, &event)?;
                                line_buf.push(b'\n');
                                if !write_bounded(&line_buf, body_limit, &mut bytes_written)? {
                                    is_capped = true;
                                    worker_capped.store(true, Ordering::Release);
                                }
                            }
                        }
                        JournalCmd::Mark(mark) => {
                            if !is_capped {
                                line_buf.clear();
                                serde_json::to_writer(&mut line_buf, &TakeMark { mark })?;
                                line_buf.push(b'\n');
                                if !write_bounded(&line_buf, body_limit, &mut bytes_written)? {
                                    is_capped = true;
                                    worker_capped.store(true, Ordering::Release);
                                }
                            }
                        }
                        JournalCmd::Seal(end) => {
                            if bytes_written > 0 {
                                line_buf.clear();
                                serde_json::to_writer(&mut line_buf, &TakeEnd { end })?;
                                line_buf.push(b'\n');
                                let _ = write_bounded(&line_buf, max_bytes, &mut bytes_written)?;
                                writer.get_ref().sync_data()?;
                            }
                            break;
                        }
                        JournalCmd::Discard => {
                            discarded = true;
                            break;
                        }
                    }
                }
                drop(writer);
                if discarded {
                    let _ = fs::remove_file(&worker_path);
                }
                Ok(())
            })
            .context("Spawning take journal writer thread")?;
        Ok(Self {
            tx: Some(tx),
            capped,
            handle: Some(handle),
        })
    }

    fn send(&self, cmd: JournalCmd) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(cmd);
        }
    }

    fn send_if_uncapped(&self, cmd: JournalCmd) {
        if self.capped.load(Ordering::Acquire) {
            return;
        }
        self.send(cmd);
    }

    fn try_send_mark(&self, step: u64) {
        if self.capped.load(Ordering::Acquire) {
            return;
        }
        if let Some(tx) = &self.tx {
            let _ = tx.try_send(JournalCmd::Mark(step));
        }
    }

    fn finish(mut self, cmd: JournalCmd) -> Result<()> {
        self.send(cmd);
        self.tx.take();
        if let Some(handle) = self.handle.take() {
            match handle.join() {
                Ok(res) => res?,
                Err(_) => anyhow::bail!("Take journal writer thread panicked"),
            }
        }
        Ok(())
    }
}

impl Drop for JournalWriter {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Persistence sink for [`TakeJournal`] streaming writes.
///
/// The default disk-backed sink ([`open_disk_journal_sink`]) streams JSONL lines
/// on a dedicated background thread so UI/audio paths never block on disk I/O.
/// Pure tests and in-memory hosts can supply [`MemoryJournalSink`] or a custom
/// sink with zero OS threads and zero disk I/O.
pub trait TakeJournalSink: Send {
    fn write_header(&mut self, header: &TakeHeader);
    fn write_event(&mut self, event: &TakeEvent);
    fn write_mark(&mut self, step: u64);
    /// True once the sink has hit its byte cap and stopped appending further records.
    fn is_capped(&self) -> bool {
        false
    }
    fn seal(self: Box<Self>, end_step: u64) -> Result<()>;
    fn discard(self: Box<Self>) -> Result<()>;
}

type SealCallback = Box<dyn FnOnce(Vec<u8>) -> Result<()> + Send>;

/// Pure in-memory [`TakeJournalSink`] that encodes the exact JSONL take wire
/// format without spawning OS threads or touching the filesystem.
pub struct MemoryJournalSink {
    buf: Vec<u8>,
    max_bytes: u64,
    capped: bool,
    on_seal: Option<SealCallback>,
}

impl Default for MemoryJournalSink {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryJournalSink {
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            max_bytes: MAX_TAKE_BYTES,
            capped: false,
            on_seal: None,
        }
    }

    pub fn with_on_seal<F>(on_seal: F) -> Self
    where
        F: FnOnce(Vec<u8>) -> Result<()> + Send + 'static,
    {
        Self {
            buf: Vec::new(),
            max_bytes: MAX_TAKE_BYTES,
            capped: false,
            on_seal: Some(Box::new(on_seal)),
        }
    }

    pub fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    fn push_bounded_line<T: Serialize>(&mut self, value: &T, limit: u64) -> bool {
        let mut line = Vec::new();
        if serde_json::to_writer(&mut line, value).is_err() {
            return false;
        }
        line.push(b'\n');
        if (self.buf.len() as u64).saturating_add(line.len() as u64) > limit {
            return false;
        }
        self.buf.extend_from_slice(&line);
        true
    }

    fn push_body_line<T: Serialize>(&mut self, value: &T) {
        if self.capped {
            return;
        }
        let body_limit = self.max_bytes.saturating_sub(TAKE_END_RESERVE_BYTES);
        if !self.push_bounded_line(value, body_limit) {
            self.capped = true;
        }
    }
}

impl TakeJournalSink for MemoryJournalSink {
    fn write_header(&mut self, header: &TakeHeader) {
        self.push_body_line(header);
    }

    fn write_event(&mut self, event: &TakeEvent) {
        self.push_body_line(event);
    }

    fn write_mark(&mut self, step: u64) {
        self.push_body_line(&TakeMark { mark: step });
    }

    fn is_capped(&self) -> bool {
        self.capped
    }

    fn seal(mut self: Box<Self>, end_step: u64) -> Result<()> {
        if !self.buf.is_empty() {
            let max_bytes = self.max_bytes;
            let _ = self.push_bounded_line(&TakeEnd { end: end_step }, max_bytes);
        }
        if let Some(cb) = self.on_seal.take() {
            cb(self.buf)?;
        }
        Ok(())
    }

    fn discard(self: Box<Self>) -> Result<()> {
        Ok(())
    }
}

/// Open the default background-thread disk journal writer for `path`.
pub fn open_disk_journal_sink(path: PathBuf) -> Result<Box<dyn TakeJournalSink>> {
    Ok(Box::new(JournalWriter::spawn(path)?))
}

impl TakeJournalSink for JournalWriter {
    fn write_header(&mut self, header: &TakeHeader) {
        self.send(JournalCmd::Header(Box::new(header.clone())));
    }

    fn write_event(&mut self, event: &TakeEvent) {
        self.send_if_uncapped(JournalCmd::Event(Box::new(event.clone())));
    }

    fn write_mark(&mut self, step: u64) {
        self.try_send_mark(step);
    }

    fn is_capped(&self) -> bool {
        self.capped.load(Ordering::Acquire)
    }

    fn seal(self: Box<Self>, end_step: u64) -> Result<()> {
        (*self).finish(JournalCmd::Seal(end_step))
    }

    fn discard(self: Box<Self>) -> Result<()> {
        (*self).finish(JournalCmd::Discard)
    }
}

/// Always-running performance journal. Streams minimal typed JSONL deltas and
/// periodic bar marks through an optional [`TakeJournalSink`] (disk-backed on a
/// background thread, or pure in-memory), and keeps an instant in-memory view
/// for the Take deck and Range Keep.
pub struct TakeJournal {
    pub path: Option<PathBuf>,
    header: TakeHeader,
    events: Vec<TakeEvent>,
    end_step: u64,
    last_mark_bar: u64,
    capped: bool,
    sink: Option<Box<dyn TakeJournalSink>>,
}

impl TakeJournal {
    pub fn open_with_sink(
        path: Option<PathBuf>,
        mut sink: Option<Box<dyn TakeJournalSink>>,
        project: &Project,
        flags: &LiveFlags,
        active: &[Override],
        start_step: u64,
        segment: Option<Segment>,
    ) -> Result<Self> {
        let header = TakeHeader {
            take: TAKE_VERSION,
            title: project.title.clone(),
            start_step,
            active: active.to_vec(),
            project: Box::new(project.clone()),
            flags: flags.clone(),
            segment,
        };
        let mut capped = false;
        if let Some(sink) = sink.as_mut() {
            sink.write_header(&header);
            capped = sink.is_capped();
        }
        Ok(Self {
            path,
            header,
            events: Vec::new(),
            end_step: start_step,
            last_mark_bar: start_step / STEPS_PER_BAR as u64,
            capped,
            sink,
        })
    }

    pub fn open(
        path: Option<PathBuf>,
        project: &Project,
        flags: &LiveFlags,
        active: &[Override],
        start_step: u64,
        segment: Option<Segment>,
    ) -> Result<Self> {
        let sink = match &path {
            Some(p) => Some(open_disk_journal_sink(p.clone())?),
            None => None,
        };
        Self::open_with_sink(path, sink, project, flags, active, start_step, segment)
    }

    pub fn start_step(&self) -> u64 {
        self.header.start_step
    }

    pub fn end_step(&self) -> u64 {
        self.end_step
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// True once the underlying journal sink has reached [`MAX_TAKE_BYTES`] and
    /// stopped appending further records so the sealed take stays readable.
    pub fn is_capped(&self) -> bool {
        self.capped || self.sink.as_deref().is_some_and(TakeJournalSink::is_capped)
    }

    pub fn header(&self) -> &TakeHeader {
        &self.header
    }

    pub fn events(&self) -> &[TakeEvent] {
        &self.events
    }

    /// Advance the journal clock and emit a periodic bar mark whenever crossing a bar.
    pub fn advance_to(&mut self, next_step: u64) {
        if next_step > self.end_step {
            self.end_step = next_step;
        }
        if self.is_capped() {
            self.capped = true;
            return;
        }
        let bar = self.end_step / STEPS_PER_BAR as u64;
        if bar > self.last_mark_bar {
            self.last_mark_bar = bar;
            if let Some(sink) = self.sink.as_mut() {
                sink.write_mark(self.end_step);
                if sink.is_capped() {
                    self.capped = true;
                }
            }
        }
    }

    pub fn record(&mut self, landed: &[Landed]) {
        for landed in landed {
            let change = match &landed.change {
                Change::Launch { lane, part } => TakeChange::Launch {
                    lane: *lane,
                    part: *part,
                },
                Change::Release { lane } => TakeChange::Release { lane: *lane },
                Change::ReleaseAll => TakeChange::ReleaseAll,
                Change::Flags { flags } => TakeChange::Flags {
                    flags: flags.clone(),
                },
                Change::Timeline { segment } => TakeChange::Segment { segment: *segment },
                Change::Song {
                    edits: Some(edits), ..
                } => TakeChange::Edits {
                    edits: edits.clone(),
                },
                Change::Song { project, .. } => TakeChange::Replace {
                    project: Box::new((**project).clone()),
                },
            };
            self.push(landed.at, landed.label.clone(), change);
        }
    }

    /// A change heard immediately rather than on the timeline.
    pub fn record_song(
        &mut self,
        at: u64,
        edits: Option<Vec<Edit>>,
        project: &Project,
        label: &str,
    ) {
        let change = match edits {
            Some(edits) if edits.is_empty() => return,
            Some(edits) => TakeChange::Edits { edits },
            None => TakeChange::Replace {
                project: Box::new(project.clone()),
            },
        };
        self.push(at, label.to_string(), change);
    }

    pub fn record_flags(&mut self, at: u64, flags: LiveFlags, label: &str) {
        self.push(at, label.to_string(), TakeChange::Flags { flags });
    }

    pub fn record_segment(&mut self, at: u64, segment: Segment, label: &str) {
        self.push(at, label.to_string(), TakeChange::Segment { segment });
    }

    fn push(&mut self, at: u64, label: String, change: TakeChange) {
        if self.is_capped() {
            self.capped = true;
            return;
        }
        let min_at = self
            .events
            .last()
            .map_or(self.header.start_step, |e| e.at)
            .max(self.header.start_step);
        let at = at.max(min_at);
        if at > self.end_step {
            self.end_step = at;
        }
        let event = TakeEvent { at, label, change };
        if let Some(sink) = self.sink.as_mut() {
            sink.write_event(&event);
            if sink.is_capped() {
                self.capped = true;
                return;
            }
        }
        self.events.push(event);
    }

    /// Instant in-memory snapshot of everything heard in this take so far.
    pub fn snapshot(&self, current_step: u64) -> Take {
        let end_step = self
            .end_step
            .max(current_step)
            .max(self.header.start_step + 1);
        Take {
            header: self.header.clone(),
            events: self.events.clone(),
            end_step,
        }
    }

    /// Seal the journal through its sink and return the finished [`Take`], or
    /// discard it if zero steps elapsed and no changes occurred.
    pub fn seal(mut self, end_step: u64) -> Result<Option<Take>> {
        let final_end = self.end_step.max(end_step);
        if final_end <= self.header.start_step && self.events.is_empty() {
            if let Some(sink) = self.sink.take() {
                sink.discard()?;
            }
            return Ok(None);
        }
        let end_step = final_end.max(self.header.start_step + 1);
        if let Some(sink) = self.sink.take() {
            sink.seal(end_step)?;
        }
        Ok(Some(Take {
            header: self.header,
            events: self.events,
            end_step,
        }))
    }
}

/// Compute minimal changed-object [`Edit`] deltas transforming `before` into `after`,
/// so undo/redo journal entries store compact edits instead of full-project snapshots.
pub fn diff_project_edits(before: &Project, after: &Project) -> Vec<Edit> {
    let mut edits = Vec::new();
    if before.title != after.title {
        edits.push(Edit::RenameProject {
            title: after.title.clone(),
        });
    }
    if (before.bpm - after.bpm).abs() > f64::EPSILON {
        edits.push(Edit::SetTempo { bpm: after.bpm });
    }
    if before.root_midi != after.root_midi || before.scale != after.scale {
        edits.push(Edit::SetKey {
            root_midi: after.root_midi,
            scale: after.scale.clone(),
        });
    }
    if before.bars != after.bars {
        edits.push(Edit::SetBars { bars: after.bars });
    }
    if before.end != after.end {
        edits.push(Edit::SetEndRule { end: after.end });
    }
    if before.timing != after.timing {
        edits.push(Edit::SetLiveTiming {
            timing: after.timing,
        });
    }

    // Additions & modifications first (in reference order).
    // 1. Channels: create new channels (with empty inserts so moved effects settle cleanly),
    //    update existing channel metadata, and diff strip gain/pan via `Edit::SetParam`.
    for ch in &after.mixer.channels {
        match before.channel(ch.id) {
            None => {
                let mut created = ch.clone();
                created.inserts.clear();
                edits.push(Edit::CreateChannel { channel: created });
            }
            Some(prev) => {
                let name = (prev.name != ch.name).then(|| ch.name.clone());
                let output = (prev.output != ch.output).then_some(ch.output);
                let sends = (prev.sends != ch.sends).then(|| ch.sends.clone());
                if name.is_some() || output.is_some() || sends.is_some() {
                    edits.push(Edit::UpdateChannelMeta {
                        id: ch.id,
                        name,
                        output,
                        sends,
                    });
                }
                if prev.strip.gain != ch.strip.gain {
                    edits.push(Edit::SetParam {
                        address: ParamAddress::Channel {
                            channel: ChannelRef::Channel(ch.id),
                            param: ChannelParam::Gain,
                        },
                        value: ch.strip.gain,
                    });
                }
                if prev.strip.pan != ch.strip.pan {
                    edits.push(Edit::SetParam {
                        address: ParamAddress::Channel {
                            channel: ChannelRef::Channel(ch.id),
                            param: ChannelParam::Pan,
                        },
                        value: ch.strip.pan,
                    });
                }
            }
        }
    }
    if before.mixer.master.strip.gain != after.mixer.master.strip.gain {
        edits.push(Edit::SetParam {
            address: ParamAddress::Channel {
                channel: ChannelRef::Master,
                param: ChannelParam::Gain,
            },
            value: after.mixer.master.strip.gain,
        });
    }
    if before.mixer.master.strip.pan != after.mixer.master.strip.pan {
        edits.push(Edit::SetParam {
            address: ParamAddress::Channel {
                channel: ChannelRef::Master,
                param: ChannelParam::Pan,
            },
            value: after.mixer.master.strip.pan,
        });
    }

    // 2. Effects across all channels + Master: remove deleted devices, then add/move/update
    //    inserts in exact target slot order.
    let mut sim_buses: std::collections::HashMap<ChannelRef, Vec<Id>> =
        std::collections::HashMap::new();
    for ch in &before.mixer.channels {
        let mut ids = Vec::with_capacity(ch.inserts.len());
        for dev in &ch.inserts {
            if after.device(dev.id).is_none() {
                edits.push(Edit::RemoveEffect { device: dev.id });
            } else {
                ids.push(dev.id);
            }
        }
        sim_buses.insert(ChannelRef::Channel(ch.id), ids);
    }
    let mut master_ids = Vec::with_capacity(before.mixer.master.inserts.len());
    for dev in &before.mixer.master.inserts {
        if after.device(dev.id).is_none() {
            edits.push(Edit::RemoveEffect { device: dev.id });
        } else {
            master_ids.push(dev.id);
        }
    }
    sim_buses.insert(ChannelRef::Master, master_ids);
    for ch in &after.mixer.channels {
        sim_buses.entry(ChannelRef::Channel(ch.id)).or_default();
    }

    let target_buses = after
        .mixer
        .channels
        .iter()
        .map(|ch| (ChannelRef::Channel(ch.id), ch.inserts.as_slice()))
        .chain(std::iter::once((
            ChannelRef::Master,
            after.mixer.master.inserts.as_slice(),
        )));
    for (bus, inserts) in target_buses {
        for (slot, dev) in inserts.iter().enumerate() {
            match before.device(dev.id) {
                None => {
                    let cur_len = sim_buses.get(&bus).map_or(0, Vec::len);
                    let index = if slot == cur_len { None } else { Some(slot) };
                    edits.push(Edit::AddEffect {
                        channel: bus,
                        device: dev.clone(),
                        index,
                    });
                    sim_buses.entry(bus).or_default().insert(slot, dev.id);
                }
                Some((_, prev_dev)) => {
                    let already_in_place = sim_buses
                        .get(&bus)
                        .and_then(|v| v.get(slot))
                        .is_some_and(|&id| id == dev.id);
                    if !already_in_place {
                        edits.push(Edit::MoveEffect {
                            device: dev.id,
                            target_channel: bus,
                            target_slot: Some(slot),
                        });
                        for list in sim_buses.values_mut() {
                            list.retain(|&id| id != dev.id);
                        }
                        sim_buses.entry(bus).or_default().insert(slot, dev.id);
                    }
                    let name = (prev_dev.name != dev.name).then(|| dev.name.clone());
                    let bypassed = (prev_dev.bypassed != dev.bypassed).then_some(dev.bypassed);
                    let kind = (prev_dev.kind != dev.kind).then_some(dev.kind);
                    if name.is_some() || bypassed.is_some() || kind.is_some() {
                        edits.push(Edit::UpdateEffect {
                            device: dev.id,
                            name,
                            bypassed,
                            kind,
                        });
                    }
                }
            }
        }
    }
    for pat in &after.patterns {
        if before.patterns.iter().find(|b| b.id == pat.id) != Some(pat) {
            edits.push(Edit::PutPattern {
                pattern: pat.clone(),
            });
        }
    }
    for inst in &after.instruments {
        match before.instruments.iter().find(|b| b.id == inst.id) {
            None => {
                edits.push(Edit::CreateInstrument {
                    instrument: inst.clone(),
                });
            }
            Some(prev) => {
                let name = (prev.name != inst.name).then(|| inst.name.clone());
                let channel = (prev.channel != inst.channel).then_some(inst.channel);
                if name.is_some() || channel.is_some() {
                    edits.push(Edit::UpdateInstrumentMeta {
                        id: inst.id,
                        name,
                        channel,
                    });
                }
                if prev.rack != inst.rack {
                    edits.push(Edit::UpdateRackPatch {
                        id: inst.id,
                        rack: inst.rack.clone(),
                    });
                }
            }
        }
    }
    for part in &after.parts {
        if before.parts.iter().find(|b| b.id == part.id) != Some(part) {
            edits.push(Edit::PutPart { part: part.clone() });
        }
    }
    for lane in &after.lanes {
        if before.lanes.iter().find(|b| b.id == lane.id) != Some(lane) {
            edits.push(Edit::PutLane { lane: lane.clone() });
        }
    }
    for sec in &after.sections {
        if before.sections.iter().find(|b| b.id == sec.id) != Some(sec) {
            edits.push(Edit::PutSection {
                section: sec.clone(),
            });
        }
    }
    for m in &after.modulations {
        if before.modulations.iter().find(|b| b.id == m.id) != Some(m) {
            edits.push(Edit::PutModulation {
                modulation: m.clone(),
            });
        }
    }
    for d in &after.ducks {
        if before.ducks.iter().find(|b| b.id == d.id) != Some(d) {
            edits.push(Edit::PutDuck { duck: d.clone() });
        }
    }
    for mc in &after.macros {
        if before.macros.iter().find(|b| b.index == mc.index) != Some(mc) {
            edits.push(Edit::PutMacro {
                macro_control: mc.clone(),
            });
        }
    }

    // Removals second (in reverse dependency order: modulations before macros/targets).
    for d in &before.ducks {
        if !after.ducks.iter().any(|a| a.id == d.id) {
            edits.push(Edit::RemoveDuck { id: d.id });
        }
    }
    for m in &before.modulations {
        if !after.modulations.iter().any(|a| a.id == m.id) {
            edits.push(Edit::RemoveModulation { id: m.id });
        }
    }
    for mc in &before.macros {
        if !after.macros.iter().any(|a| a.index == mc.index) {
            edits.push(Edit::RemoveMacro { index: mc.index });
        }
    }
    for sec in &before.sections {
        if !after.sections.iter().any(|a| a.id == sec.id) {
            edits.push(Edit::RemoveSection { id: sec.id });
        }
    }
    for lane in &before.lanes {
        if !after.lanes.iter().any(|a| a.id == lane.id) {
            edits.push(Edit::RemoveLane { id: lane.id });
        }
    }
    let mut cur_lane_ids: Vec<Id> = before
        .lanes
        .iter()
        .filter(|l| after.lanes.iter().any(|a| a.id == l.id))
        .map(|l| l.id)
        .collect();
    for lane in &after.lanes {
        if !cur_lane_ids.contains(&lane.id) {
            cur_lane_ids.push(lane.id);
        }
    }
    for (target_index, lane) in after.lanes.iter().enumerate() {
        if cur_lane_ids.get(target_index) != Some(&lane.id) {
            if let Some(pos) = cur_lane_ids.iter().position(|&id| id == lane.id) {
                cur_lane_ids.remove(pos);
                cur_lane_ids.insert(target_index, lane.id);
                edits.push(Edit::ReorderLane {
                    lane: lane.id,
                    target_index,
                });
            }
        }
    }
    for part in &before.parts {
        if !after.parts.iter().any(|a| a.id == part.id) {
            edits.push(Edit::RemovePart { id: part.id });
        }
    }
    for inst in &before.instruments {
        if !after.instruments.iter().any(|a| a.id == inst.id) {
            edits.push(Edit::RemoveInstrument { id: inst.id });
        }
    }
    for pat in &before.patterns {
        if !after.patterns.iter().any(|a| a.id == pat.id) {
            edits.push(Edit::RemovePattern { id: pat.id });
        }
    }
    for ch in &before.mixer.channels {
        if !after.mixer.channels.iter().any(|a| a.id == ch.id) {
            edits.push(Edit::RemoveChannel { id: ch.id });
        }
    }
    edits
}

impl Take {
    pub fn to_jsonl_bytes(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        serde_json::to_writer(&mut buf, &self.header)?;
        buf.push(b'\n');
        for event in &self.events {
            serde_json::to_writer(&mut buf, event)?;
            buf.push(b'\n');
        }
        serde_json::to_writer(&mut buf, &TakeEnd { end: self.end_step })?;
        buf.push(b'\n');
        Ok(buf)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let bytes = self.to_jsonl_bytes()?;
        store::write_atomic_bytes(path, MAX_TAKE_BYTES, &bytes)
    }

    pub fn load(path: &Path) -> Result<Self> {
        Self::load_inner(path, false)
    }

    /// Crash-tolerant loader for unsealed session journals: truncates any partial
    /// or corrupted trailing line and derives `end_step` from the end record, the
    /// last periodic bar mark, or the last valid event.
    pub fn load_recover(path: &Path) -> Result<Self> {
        Self::load_inner(path, true)
    }

    fn load_inner(path: &Path, recover: bool) -> Result<Self> {
        let bytes = store::read_bounded(path, MAX_TAKE_BYTES)?;
        Self::from_jsonl_bytes(&bytes, recover)
    }

    pub fn from_jsonl_bytes(bytes: &[u8], recover: bool) -> Result<Self> {
        let text = std::str::from_utf8(bytes).context("Take is not valid UTF-8")?;
        let mut lines = text.lines();
        let header: TakeHeader = serde_json::from_str(lines.next().context("Take is empty")?)
            .context("Invalid take header")?;
        ensure!(
            header.take == TAKE_VERSION,
            "Unsupported take version {}",
            header.take
        );
        header.project.validate()?;
        header.project.validate_flags(&header.flags)?;
        let mut events = Vec::new();
        let mut end = None;
        let mut last_mark = header.start_step;
        let remaining: Vec<&str> = lines.collect();
        for (idx, line) in remaining.iter().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            if end.is_some() {
                if recover {
                    break;
                }
                anyhow::bail!("Take line {}: trailing data after end record", idx + 2);
            }
            if let Ok(done) = serde_json::from_str::<TakeEnd>(line) {
                ensure!(
                    done.end >= header.start_step,
                    "Take end {} precedes start {}",
                    done.end,
                    header.start_step
                );
                ensure!(
                    events.last().is_none_or(|e: &TakeEvent| e.at <= done.end),
                    "Take end precedes its last event"
                );
                end = Some(done.end);
                continue;
            }
            if let Ok(mark) = serde_json::from_str::<TakeMark>(line) {
                if mark.mark >= last_mark {
                    last_mark = mark.mark;
                }
                continue;
            }
            match serde_json::from_str::<TakeEvent>(line) {
                Ok(event) => {
                    ensure!(
                        events.last().is_none_or(|e: &TakeEvent| e.at <= event.at)
                            && event.at >= header.start_step,
                        "Take events must be in order and after the take starts"
                    );
                    events.push(event);
                }
                Err(err) => {
                    if recover && idx + 1 == remaining.len() {
                        // Partial trailing line from a mid-write crash: stop cleanly.
                        break;
                    }
                    return Err(err).with_context(|| format!("Take line {}", idx + 2));
                }
            }
        }
        let end_step = match end {
            Some(step) => step,
            None if recover => {
                let last_event = events.last().map_or(header.start_step, |e| e.at);
                last_mark.max(last_event).max(header.start_step + 1)
            }
            None => anyhow::bail!("Take has no end; it was not stopped cleanly"),
        };
        Ok(Self {
            header,
            events,
            end_step,
        })
    }

    pub fn steps(&self) -> u64 {
        self.end_step.saturating_sub(self.header.start_step)
    }

    /// Replay the take through the same [`PerformanceState`] fold used live.
    pub fn states(&self) -> Result<Vec<(u64, PerformanceState)>> {
        let mut state = PerformanceState {
            project: Arc::new((*self.header.project).clone()),
            flags: self.header.flags.clone(),
            active: self.header.active.clone(),
            timeline: self.header.segment,
        };
        let mut states = vec![(0, state.clone())];
        for event in &self.events {
            let change = match &event.change {
                TakeChange::Launch { lane, part } => {
                    ensure!(
                        state.project.lane(*lane).is_some(),
                        "Take launches into missing lane {lane}"
                    );
                    ensure!(
                        state.project.part(part.part).is_some(),
                        "Take launches missing part {}",
                        part.part
                    );
                    Change::Launch {
                        lane: *lane,
                        part: *part,
                    }
                }
                TakeChange::Release { lane } => Change::Release { lane: *lane },
                TakeChange::ReleaseAll => Change::ReleaseAll,
                TakeChange::Flags { flags } => Change::Flags {
                    flags: flags.clone(),
                },
                TakeChange::Segment { segment } => Change::Timeline { segment: *segment },
                TakeChange::Edits { edits } => {
                    let mut next = (*state.project).clone();
                    next.apply_transaction_validated(edits)
                        .with_context(|| format!("Take edit at step {}", event.at))?;
                    Change::Song {
                        project: Arc::new(next),
                        edits: None,
                    }
                }
                TakeChange::Replace { project } => {
                    let next = (**project).clone();
                    next.validate()?;
                    Change::Song {
                        project: Arc::new(next),
                        edits: None,
                    }
                }
            };
            state.apply(event.at, &change);
            match states.last_mut() {
                Some(last) if last.0 == event.at => last.1 = state.clone(),
                _ => states.push((event.at, state.clone())),
            }
        }
        Ok(states)
    }

    /// The take's timeline with the engine scores to adopt at each step where
    /// instruments, the mixer, channel flags, tempo, key, or the ring window
    /// roll forward. Supports takes of arbitrary length.
    pub fn compile(&self, tail_bars: u32) -> Result<Vec<(u64, ScoreWindow)>> {
        let tail_steps = u64::from(tail_bars).saturating_mul(STEPS_PER_BAR as u64);
        let total = self
            .steps()
            .checked_add(tail_steps)
            .context("Take length overflowed")?;
        ensure!(total > 0, "Take has no steps to compile");
        let start = self.header.start_step;
        let end = start
            .checked_add(total)
            .context("Take end step overflowed")?;
        let states = self.states()?;
        let segments: Vec<LiveSegment<'_>> = states
            .iter()
            .map(|(from, state)| LiveSegment {
                from: *from,
                song: &state.project,
                flags: &state.flags,
                active: &state.active,
                timeline: state.timeline,
                audition: None,
            })
            .collect();
        let mut scores = Vec::new();
        let mut win_start = start;
        while win_start < end {
            let remaining = end - win_start;
            let win_steps = remaining.min(LIVE_RING_STEPS);
            let win_bars = win_steps.div_ceil(STEPS_PER_BAR as u64) as usize;
            let win_end = win_start + (win_bars * STEPS_PER_BAR) as u64;

            let start_idx = states
                .partition_point(|(from, _)| *from <= win_start)
                .saturating_sub(1);
            let mut statics = (&states[start_idx].1.project, &states[start_idx].1.flags);
            scores.push((
                win_start,
                statics
                    .0
                    .compile_live_window(statics.1, win_start, win_bars, &segments)?,
            ));

            for (from, state) in &states[start_idx + 1..] {
                if *from >= win_end || *from >= end {
                    break;
                }
                if statics.0.timeline_compatible(&state.project)
                    && statics.1.mixer_compatible(&state.flags)
                {
                    continue;
                }
                statics = (&state.project, &state.flags);
                scores.push((
                    *from,
                    state.project.compile_live_window(
                        &state.flags,
                        win_start,
                        win_bars,
                        &segments,
                    )?,
                ));
            }

            win_start = win_end;
        }
        Ok(scores)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::EndRule;

    #[test]
    fn diff_project_edits_reconstructs_modified_project() {
        let base = Project::demo();
        let mut modified = base.clone();
        modified.title = "Edited Demo".into();
        modified.bpm = 142.5;
        modified.end = EndRule::Continue;
        modified.patterns[0].name = "Motif Prime".into();
        modified.lanes[0].placements.pop();
        let edits = diff_project_edits(&base, &modified);
        assert!(!edits.is_empty());

        let mut rebuilt = base.clone();
        rebuilt
            .apply_transaction_with_flags(&edits, &LiveFlags::default())
            .unwrap();
        assert_eq!(rebuilt.title, modified.title);
        assert_eq!(rebuilt.bpm, modified.bpm);
        assert_eq!(rebuilt.end, modified.end);
        assert_eq!(rebuilt.patterns[0].name, "Motif Prime");
        assert_eq!(
            rebuilt.lanes[0].placements.len(),
            modified.lanes[0].placements.len()
        );
    }

    #[test]
    fn diff_project_edits_emits_typed_instrument_creation_meta_and_rack_edits() {
        use crate::material::InstrumentInstance;
        use crate::rack::{ModuleParam, RackPatch};

        let base = Project::demo();
        let mut modified = base.clone();
        // 1. Metadata-only change (name + channel) on existing instrument 0.
        let inst0_id = modified.instruments[0].id;
        let alt_ch = modified
            .mixer
            .channels
            .iter()
            .find(|c| {
                c.id != modified.instruments[0].channel
                    && c.kind == crate::mixer::ChannelKind::Audio
            })
            .unwrap()
            .id;
        modified.instruments[0].name = "Lead Prime".into();
        modified.instruments[0].channel = alt_ch;
        // 2. Patch-only change on existing Rack instrument 1.
        let inst1_id = modified.instruments[1].id;
        let filter_target = crate::params::InstrumentParam::Module {
            zone: 1,
            module: 3,
            param: ModuleParam::Cutoff,
        };
        modified.instruments[1]
            .rack
            .write_param_native(filter_target, 1_800.0)
            .unwrap();
        // 3. Brand-new Sampler Rack instrument.
        let new_id = modified.max_id() + 1;
        modified.instruments.push(InstrumentInstance {
            id: new_id,
            name: "OneShot".into(),
            channel: alt_ch,
            rack: RackPatch::default_sampler(),
        });

        let edits = diff_project_edits(&base, &modified);
        assert!(edits.iter().any(|e| matches!(
            e,
            Edit::UpdateInstrumentMeta {
                id,
                name: Some(name),
                channel: Some(ch),
            } if *id == inst0_id && name == "Lead Prime" && *ch == alt_ch
        )));
        assert!(edits.iter().any(|e| matches!(
            e,
            Edit::UpdateRackPatch { id, rack }
                if *id == inst1_id
                    && (rack.read_param_native(filter_target).unwrap() - 1_800.0).abs() < 1e-4
        )));
        assert!(edits.iter().any(|e| matches!(
            e,
            Edit::CreateInstrument { instrument } if instrument.id == new_id
        )));

        let mut rebuilt = base.clone();
        rebuilt.apply_transaction_validated(&edits).unwrap();
        assert_eq!(rebuilt.instruments, modified.instruments);
    }

    #[test]
    fn journal_streams_marks_seals_discards_empty_and_recovers_truncated_crash() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::demo();
        let flags = LiveFlags::default();

        // Empty journal with zero steps elapsed is discarded on seal.
        let empty_path = dir.path().join("empty.take.jsonl");
        let empty =
            TakeJournal::open(Some(empty_path.clone()), &project, &flags, &[], 0, None).unwrap();
        assert!(empty.seal(0).unwrap().is_none());
        assert!(!empty_path.exists());

        // Active journal writes header, events, periodic bar marks, and seals cleanly.
        let path = dir.path().join("live.take.jsonl");
        let mut journal =
            TakeJournal::open(Some(path.clone()), &project, &flags, &[], 0, None).unwrap();
        journal.advance_to(16);
        journal.record_flags(16, flags.clone(), "flags");
        journal.advance_to(32);
        let sealed = journal.seal(32).unwrap().unwrap();
        assert_eq!(sealed.end_step, 32);
        let loaded = Take::load(&path).unwrap();
        assert_eq!(loaded.end_step, 32);
        assert_eq!(loaded.events.len(), 1);

        // Simulate a mid-write crash: strip the end line and append a partial JSON fragment.
        let text = fs::read_to_string(&path).unwrap();
        let mut crashed: Vec<&str> = text.lines().collect();
        crashed.pop(); // remove TakeEnd
        let mut payload = crashed.join("\n");
        payload.push_str("\n{\"at\":40,\"label\":\"half-written");
        let crash_path = dir.path().join("crashed.take.jsonl");
        fs::write(&crash_path, payload).unwrap();

        // Strict Take::load refuses unsealed/truncated files; load_recover truncates cleanly.
        assert!(Take::load(&crash_path).is_err());
        let recovered = Take::load_recover(&crash_path).unwrap();
        assert_eq!(recovered.end_step, 32);
        assert_eq!(recovered.events.len(), 1);
    }

    #[test]
    fn journal_clamps_monotonic_timestamps_and_take_compile_skips_removed_instruments() {
        use crate::material::{Modulation, ModulationTarget};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("monotonic.take.jsonl");
        let base = Project::demo();
        let flags = LiveFlags::default();
        let seg = Segment::new(0, 0, None, EndRule::Loop);

        // Out-of-order late-snapped timestamp after a later event must be clamped monotonically.
        let mut journal =
            TakeJournal::open(Some(path.clone()), &base, &flags, &[], 0, Some(seg)).unwrap();
        journal.record_flags(20, flags.clone(), "immediate");
        journal.record_flags(16, flags.clone(), "late-snapped");
        assert_eq!(journal.events()[0].at, 20);
        assert_eq!(journal.events()[1].at, 20);

        // Remove instrument 3 (Kick) mid-take via diff_project_edits and verify Take::compile()
        // gracefully skips arrangement steps referencing removed instruments across segments!
        let mut trimmed = base.clone();
        trimmed.ducks.clear();
        trimmed.lanes.retain(|l| l.id != 3);
        trimmed.parts.retain(|p| p.instrument != 3);
        trimmed.instruments.retain(|i| i.id != 3);
        let edits = diff_project_edits(&base, &trimmed);
        journal.record_song(80, Some(edits), &trimmed, "remove kick");
        let take = journal.seal(128).unwrap().unwrap();
        let reloaded = Take::load(&path).unwrap();
        assert_eq!(reloaded.events.len(), 3);
        let compiled = take.compile(0).unwrap();
        assert!(compiled.len() >= 2);

        // diff_project_edits removing both a Macro and its Modulation applies cleanly.
        let mut with_macro = base.clone();
        let mac = crate::macros::MacroControl::new(0, "M1");
        with_macro.macros.push(mac);
        let pat_id = with_macro.patterns[6].id;
        with_macro.modulations.push(Modulation {
            id: with_macro.max_id() + 1,
            name: "Auto M1".into(),
            target: ModulationTarget::Macro { index: 0 },
            pattern: pat_id,
        });
        let remove_edits = diff_project_edits(&with_macro, &base);
        let mut rebuilt = with_macro.clone();
        rebuilt.apply_transaction_validated(&remove_edits).unwrap();
        assert!(rebuilt.macros.is_empty());
        assert_eq!(rebuilt.modulations.len(), base.modulations.len());
    }

    #[test]
    fn memory_journal_sink_streams_and_seals_without_disk_or_threads() {
        use std::sync::{Arc, Mutex};

        let project = Project::demo();
        let flags = LiveFlags::default();
        let captured: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
        let sink_slot = Arc::clone(&captured);
        let sink = Box::new(MemoryJournalSink::with_on_seal(move |bytes| {
            *sink_slot.lock().unwrap() = Some(bytes);
            Ok(())
        }));

        let mut journal = TakeJournal::open_with_sink(
            Some(PathBuf::from("/virtual/take-1.take.jsonl")),
            Some(sink),
            &project,
            &flags,
            &[],
            0,
            None,
        )
        .unwrap();
        journal.advance_to(16);
        journal.record_flags(16, flags.clone(), "flags");
        let sealed = journal.seal(32).unwrap().unwrap();
        assert_eq!(sealed.end_step, 32);

        let bytes = captured
            .lock()
            .unwrap()
            .clone()
            .expect("seal callback invoked");
        let parsed = Take::from_jsonl_bytes(&bytes, false).unwrap();
        assert_eq!(parsed.end_step, 32);
        assert_eq!(parsed.events.len(), 1);

        // Discarded empty take never fires on_seal.
        let discard_called = Arc::new(Mutex::new(false));
        let flag = Arc::clone(&discard_called);
        let empty = TakeJournal::open_with_sink(
            Some(PathBuf::from("/virtual/empty.take.jsonl")),
            Some(Box::new(MemoryJournalSink::with_on_seal(move |_| {
                *flag.lock().unwrap() = true;
                Ok(())
            }))),
            &project,
            &flags,
            &[],
            0,
            None,
        )
        .unwrap();
        assert!(empty.seal(0).unwrap().is_none());
        assert!(!*discard_called.lock().unwrap());
    }

    #[test]
    fn journal_writer_and_memory_sink_enforce_byte_cap_and_seal_readably() {
        use std::sync::{Arc, Mutex};

        let dir = tempfile::tempdir().unwrap();
        let project = Project::demo();
        let flags = LiveFlags::default();
        let header = TakeHeader {
            take: TAKE_VERSION,
            title: project.title.clone(),
            start_step: 0,
            active: Vec::new(),
            project: Box::new(project.clone()),
            flags: flags.clone(),
            segment: None,
        };
        let header_bytes = serde_json::to_vec(&header).unwrap().len() as u64 + 1;
        // Budget enough room for header + a few events + end reserve, but well below 200 events.
        let cap_bytes = header_bytes + 512 + TAKE_END_RESERVE_BYTES;

        // 1. Disk-backed JournalWriter enforces byte cap, marks capped, and seals a loadable file.
        let disk_path = dir.path().join("capped.take.jsonl");
        let writer = JournalWriter::spawn_with_limit(disk_path.clone(), cap_bytes).unwrap();
        let capped_flag = Arc::clone(&writer.capped);
        let mut journal = TakeJournal::open_with_sink(
            Some(disk_path.clone()),
            Some(Box::new(writer)),
            &project,
            &flags,
            &[],
            0,
            None,
        )
        .unwrap();
        for i in 0..200u64 {
            let step = (i + 1) * 4;
            journal.advance_to(step);
            journal.record_flags(step, flags.clone(), &format!("event-{i:04}"));
        }
        let sealed = journal.seal(900).unwrap().unwrap();
        assert_eq!(sealed.end_step, 900);
        assert!(
            capped_flag.load(Ordering::Acquire),
            "JournalWriter must mark capped once byte budget is reached"
        );
        let disk_len = fs::metadata(&disk_path).unwrap().len();
        assert!(
            disk_len <= cap_bytes,
            "capped take file ({disk_len} bytes) must stay <= cap ({cap_bytes} bytes)"
        );
        let reloaded = Take::load(&disk_path).unwrap();
        assert_eq!(reloaded.end_step, 900);
        assert!(
            !reloaded.events.is_empty() && reloaded.events.len() < 200,
            "capped take must retain events written before cap and skip overflow (got {})",
            reloaded.events.len()
        );

        // 2. MemoryJournalSink enforces the exact same cap synchronously.
        let captured: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&captured);
        let mem_sink = MemoryJournalSink::with_on_seal(move |bytes| {
            *slot.lock().unwrap() = Some(bytes);
            Ok(())
        })
        .with_max_bytes(cap_bytes);
        let mut mem_journal = TakeJournal::open_with_sink(
            None,
            Some(Box::new(mem_sink)),
            &project,
            &flags,
            &[],
            0,
            None,
        )
        .unwrap();
        for i in 0..200u64 {
            let step = (i + 1) * 4;
            mem_journal.advance_to(step);
            mem_journal.record_flags(step, flags.clone(), &format!("mem-{i:04}"));
        }
        assert!(mem_journal.is_capped());
        mem_journal.seal(900).unwrap().unwrap();
        let mem_bytes = captured.lock().unwrap().clone().unwrap();
        assert!(mem_bytes.len() as u64 <= cap_bytes);
        let mem_loaded = Take::from_jsonl_bytes(&mem_bytes, false).unwrap();
        assert_eq!(mem_loaded.end_step, 900);
        assert!(!mem_loaded.events.is_empty() && mem_loaded.events.len() < 200);
    }

    #[test]
    fn take_steps_uses_saturating_subtraction_without_underflow() {
        let project = Project::demo();
        let take = Take {
            header: TakeHeader {
                take: TAKE_VERSION,
                title: project.title.clone(),
                start_step: 64,
                active: Vec::new(),
                project: Box::new(project),
                flags: LiveFlags::default(),
                segment: None,
            },
            events: Vec::new(),
            end_step: 16,
        };
        assert_eq!(take.steps(), 0);
    }
}
