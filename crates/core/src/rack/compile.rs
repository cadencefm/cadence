//! Deterministic Kahn topological DAG compiler for [`RackZone`] patchbays.
//!
//! Produces a flat, zero-allocation [`CompiledZoneGraph`] with `z⁻¹` 1-sample
//! feedback edge flags (`feedback: true`), per-module contiguous incoming cable
//! slices, voice-lifetime envelope detection, and SPOT Patchbay column layout.

use crate::rack::modules::ModuleKindTag;
use crate::rack::types::{InPort, OutPort, RackZone, MAX_CABLES_PER_ZONE, MAX_MODULES_PER_ZONE};
use anyhow::{ensure, Context, Result};

/// Human-readable column header names matching `ModuleKindTag::column()` (`0..=3`).
pub const PATCHBAY_COLUMNS: [&str; 4] = ["SOURCES", "MODULATORS", "PROCESSORS", "VCA / OUT"];

/// Compiled execution slot for one [`crate::rack::RackModule`] inside a [`RackZone`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompiledOp {
    /// Stable authored [`crate::rack::RackModule::id`].
    pub module_id: u16,
    /// Original index inside `RackZone::modules`.
    pub module_index: u8,
    /// Module kind tag for fast branch dispatch.
    pub kind: ModuleKindTag,
    /// Semantic Patchbay column (`0=SOURCES`, `1=MODULATORS`, `2=PROCESSORS`, `3=VCA/OUT`).
    pub column: u8,
    /// Start index inside [`CompiledZoneGraph::cables`] for cables targeting this slot.
    pub in_cable_start: u16,
    /// Count of incoming cables targeting this slot.
    pub in_cable_len: u16,
}

/// Compiled patch cable in topological slot coordinates, grouped by destination `to_slot`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompiledCable {
    /// Stable authored [`crate::rack::PatchCable::id`].
    pub cable_id: u16,
    /// Original index inside `RackZone::cables`.
    pub cable_index: u8,
    /// Source execution slot (`0..ops.len()`).
    pub from_slot: u8,
    /// Source output port.
    pub from_port: OutPort,
    /// Destination execution slot (`0..ops.len()`).
    pub to_slot: u8,
    /// Destination input port.
    pub to_port: InPort,
    /// Base authored attenuverter amount (overridable per-voice via P-locks/ModLanes).
    pub amount: f32,
    /// True when this edge closes a cycle or self-loop (`from_slot >= to_slot`) and
    /// MUST read `prev_slots[from_slot]` (`z⁻¹` 1-sample delay).
    pub feedback: bool,
}

/// Flat, zero-allocation compiled DSP program and Patchbay layout for one [`RackZone`].
#[derive(Clone, Debug, PartialEq)]
pub struct CompiledZoneGraph {
    /// Stable authored [`RackZone::id`].
    pub zone_id: u16,
    /// Modules in deterministic topological execution order (`len <= MAX_MODULES_PER_ZONE`).
    pub ops: Box<[CompiledOp]>,
    /// Cables grouped contiguously by `to_slot` so each op scans only its incoming slice.
    pub cables: Box<[CompiledCable]>,
    /// Sorted `(module_id, slot)` map for O(log N) lookup without HashMaps.
    pub module_slot_by_id: Box<[(u16, u8)]>,
    /// Sorted `(cable_id, compiled_cable_index)` map for O(log N) live cable modulation updates.
    pub cable_index_by_id: Box<[(u16, u8)]>,
    /// Execution slot of the zone's mandatory `Out` module.
    pub out_slot: u8,
    /// Execution slots of `Env` modules that gate voice lifetime (patched to `Vca.Cv` or
    /// `Out.Cv` on an audio path reaching `Out`).
    pub lifetime_envs: Box<[u8]>,
    /// Execution slots of frontier `Vca` modules reached from `out_slot` along `InPort::In`
    /// whose `Cv` inputs are non-empty and strictly driven by [`Self::lifetime_envs`]
    /// (empty when `Out.Cv` itself is strictly driven by [`Self::lifetime_envs`]). When all
    /// lifetime envelopes finish and `bias == 0`, these VCAs close their audio path to exact silence.
    pub env_gated_vcas: Box<[u8]>,
    /// Execution slots of stateful tail-bearing modules (`Filter`, `Drive`, `Resonator`, `Mobius`, `Out`)
    /// reachable from `out_slot` along `InPort::In` without crossing any [`Self::env_gated_vcas`]
    /// (or strictly `[out_slot]` when `Out.Cv` itself is strictly driven by [`Self::lifetime_envs`]).
    pub post_gate_tail_slots: Box<[u8]>,
    /// Execution slots of `Sampler` modules in this zone (for voice lifetime when no Env gates audio).
    pub sampler_slots: Box<[u8]>,
    /// Execution slots of `Mobius` modules on the audio chain (self-gated cubic-atom ring-out when no Env gates audio).
    pub mobius_slots: Box<[u8]>,
    /// SPOT Patchbay column index (`0..=3`) per execution slot (`column_layout.len() == ops.len()`).
    pub column_layout: Box<[u8]>,
}

impl CompiledZoneGraph {
    /// O(log N) lookup of topological execution slot from stable `module_id`.
    #[inline]
    pub fn slot_for_module(&self, module_id: u16) -> Option<usize> {
        self.module_slot_by_id
            .binary_search_by_key(&module_id, |&(id, _)| id)
            .ok()
            .map(|i| usize::from(self.module_slot_by_id[i].1))
    }

    /// O(log N) lookup of index into `self.cables` from stable `cable_id`.
    #[inline]
    pub fn cable_index_for_id(&self, cable_id: u16) -> Option<usize> {
        self.cable_index_by_id
            .binary_search_by_key(&cable_id, |&(id, _)| id)
            .ok()
            .map(|i| usize::from(self.cable_index_by_id[i].1))
    }

    /// True if any cable in this zone is a `z⁻¹` cycle/self-loop feedback edge.
    #[inline]
    pub fn has_feedback(&self) -> bool {
        self.cables.iter().any(|c| c.feedback)
    }
}

/// Deterministically compile a validated [`RackZone`] into a flat [`CompiledZoneGraph`].
///
/// - Uses Kahn's topological sort with deterministic `(column, module_id)` ordering.
/// - Cleanly breaks any feedback cycle or self-loop by marking back-edges `feedback: true`
///   (`from_slot >= to_slot`, reading `z⁻¹` previous-frame state).
/// - Guarantees every forward edge satisfies `from_slot < to_slot`.
pub fn compile_zone_graph(zone: &RackZone) -> Result<CompiledZoneGraph> {
    let n = zone.modules.len();
    ensure!(
        (1..=MAX_MODULES_PER_ZONE).contains(&n),
        "Zone '{}' must contain 1..={MAX_MODULES_PER_ZONE} modules",
        zone.name
    );
    ensure!(
        zone.cables.len() <= MAX_CABLES_PER_ZONE,
        "Zone '{}' exceeds maximum of {MAX_CABLES_PER_ZONE} cables",
        zone.name
    );

    // Map stable module_id -> original index inside zone.modules.
    let mut id_to_orig: Vec<(u16, usize)> = zone
        .modules
        .iter()
        .enumerate()
        .map(|(i, m)| (m.id, i))
        .collect();
    id_to_orig.sort_unstable_by_key(|&(id, _)| id);

    let orig_for_id = |id: u16| -> Option<usize> {
        id_to_orig
            .binary_search_by_key(&id, |&(mid, _)| mid)
            .ok()
            .map(|pos| id_to_orig[pos].1)
    };

    // Build directed adjacency list and in-degrees (excluding self-loops).
    let mut outgoing: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut in_degree: Vec<usize> = vec![0; n];
    for c in &zone.cables {
        let u = orig_for_id(c.from_module).with_context(|| {
            format!(
                "Cable #{} references missing source #{}",
                c.id, c.from_module
            )
        })?;
        let v = orig_for_id(c.to_module).with_context(|| {
            format!("Cable #{} references missing target #{}", c.id, c.to_module)
        })?;
        if u != v {
            outgoing[u].push(v);
            in_degree[v] += 1;
        }
    }

    let node_key = |idx: usize| -> (u8, u16) {
        let m = &zone.modules[idx];
        (m.kind.column(), m.id)
    };

    let mut placed = vec![false; n];
    let mut order: Vec<usize> = Vec::with_capacity(n);

    // Deterministic Kahn topological sort with cycle-breaking fallback.
    while order.len() < n {
        // Prefer any unplaced node with in_degree == 0, ordered by (column, module_id).
        let next = (0..n)
            .filter(|&i| !placed[i] && in_degree[i] == 0)
            .min_by_key(|&i| node_key(i))
            .or_else(|| {
                // Remaining unplaced nodes contain at least one directed cycle:
                // pick deterministically by lowest remaining in_degree, then (column, module_id).
                (0..n)
                    .filter(|&i| !placed[i])
                    .min_by_key(|&i| (in_degree[i], node_key(i)))
            })
            .expect("unplaced node exists");

        placed[next] = true;
        order.push(next);
        for &dst in &outgoing[next] {
            if !placed[dst] {
                in_degree[dst] = in_degree[dst].saturating_sub(1);
            }
        }
    }

    let mut slot_of_orig = vec![0u8; n];
    for (slot, &orig_idx) in order.iter().enumerate() {
        slot_of_orig[orig_idx] = slot as u8;
    }

    // Build compiled cables and group contiguously by destination slot.
    let mut compiled_cables: Vec<CompiledCable> = Vec::with_capacity(zone.cables.len());
    for (cable_idx, c) in zone.cables.iter().enumerate() {
        let u = orig_for_id(c.from_module).unwrap();
        let v = orig_for_id(c.to_module).unwrap();
        let from_slot = slot_of_orig[u];
        let to_slot = slot_of_orig[v];
        compiled_cables.push(CompiledCable {
            cable_id: c.id,
            cable_index: cable_idx as u8,
            from_slot,
            from_port: c.from_port,
            to_slot,
            to_port: c.to_port,
            amount: c.amount,
            feedback: from_slot >= to_slot,
        });
    }
    compiled_cables.sort_by_key(|c| (c.to_slot, c.to_port as u8, c.cable_id));

    let mut ops: Vec<CompiledOp> = Vec::with_capacity(n);
    let mut out_slot: Option<u8> = None;
    let mut sampler_slots: Vec<u8> = Vec::new();
    let mut mobius_slots: Vec<u8> = Vec::new();
    let mut cable_cursor = 0usize;

    for (slot, &orig_idx) in order.iter().enumerate() {
        let slot_u8 = slot as u8;
        let m = &zone.modules[orig_idx];
        let kind = m.kind.tag();
        if kind == ModuleKindTag::Out {
            ensure!(
                out_slot.is_none(),
                "Zone '{}' has multiple Out modules",
                zone.name
            );
            out_slot = Some(slot_u8);
        }
        if kind == ModuleKindTag::Sampler {
            sampler_slots.push(slot_u8);
        }
        if kind == ModuleKindTag::Mobius {
            mobius_slots.push(slot_u8);
        }
        while cable_cursor < compiled_cables.len()
            && compiled_cables[cable_cursor].to_slot < slot_u8
        {
            cable_cursor += 1;
        }
        let start = cable_cursor;
        while cable_cursor < compiled_cables.len()
            && compiled_cables[cable_cursor].to_slot == slot_u8
        {
            cable_cursor += 1;
        }
        ops.push(CompiledOp {
            module_id: m.id,
            module_index: orig_idx as u8,
            kind,
            column: kind.column(),
            in_cable_start: start as u16,
            in_cable_len: (cable_cursor - start) as u16,
        });
    }

    let out_slot = out_slot
        .with_context(|| format!("Zone '{}' is missing its required Out module", zone.name))?;

    // Reverse BFS over InPort::In cables from `out_slot` to identify modules on the audio path.
    let mut on_audio_chain = vec![false; n];
    on_audio_chain[usize::from(out_slot)] = true;
    let mut queue = vec![out_slot];
    while let Some(dst) = queue.pop() {
        let op = &ops[usize::from(dst)];
        let slice = &compiled_cables
            [usize::from(op.in_cable_start)..usize::from(op.in_cable_start + op.in_cable_len)];
        for c in slice {
            if c.to_port == InPort::In && !on_audio_chain[usize::from(c.from_slot)] {
                on_audio_chain[usize::from(c.from_slot)] = true;
                queue.push(c.from_slot);
            }
        }
    }

    // Lifetime envelopes: any Env patching into Cv of a Vca or Out that sits on the audio path.
    let mut lifetime_envs: Vec<u8> = Vec::new();
    for c in &compiled_cables {
        let dst = usize::from(c.to_slot);
        let src = usize::from(c.from_slot);
        if c.to_port == InPort::Cv
            && on_audio_chain[dst]
            && matches!(ops[dst].kind, ModuleKindTag::Vca | ModuleKindTag::Out)
            && ops[src].kind == ModuleKindTag::Env
            && !lifetime_envs.contains(&c.from_slot)
        {
            lifetime_envs.push(c.from_slot);
        }
    }
    lifetime_envs.sort_unstable();

    // Identify audio-path VCAs (and Out) whose `Cv` inputs are non-empty and strictly driven
    // by lifetime envelopes. Once those envelopes complete (and VCA bias is 0), upstream stateful
    // modules behind them cannot emit audio to `Out` and must not trap voice retirement.
    let slot_cv_strictly_lifetime = |slot: usize| -> bool {
        let op = &ops[slot];
        let slice = &compiled_cables
            [usize::from(op.in_cable_start)..usize::from(op.in_cable_start + op.in_cable_len)];
        let mut saw_cv = false;
        for c in slice {
            if c.to_port == InPort::Cv {
                saw_cv = true;
                if !lifetime_envs.contains(&c.from_slot) {
                    return false;
                }
            }
        }
        saw_cv
    };

    let mut is_env_gated_vca = vec![false; n];
    for slot in 0..n {
        if on_audio_chain[slot]
            && ops[slot].kind == ModuleKindTag::Vca
            && slot_cv_strictly_lifetime(slot)
        {
            is_env_gated_vca[slot] = true;
        }
    }

    let out_env_gated = slot_cv_strictly_lifetime(usize::from(out_slot));
    let mut env_gated_vcas: Vec<u8> = Vec::new();
    let mut post_gate_tail_slots: Vec<u8> = Vec::new();
    if out_env_gated {
        post_gate_tail_slots.push(out_slot);
    } else {
        let mut visited = vec![false; n];
        visited[usize::from(out_slot)] = true;
        let mut tail_queue = vec![out_slot];
        while let Some(dst) = tail_queue.pop() {
            let dst_idx = usize::from(dst);
            if matches!(
                ops[dst_idx].kind,
                ModuleKindTag::Filter
                    | ModuleKindTag::Drive
                    | ModuleKindTag::Resonator
                    | ModuleKindTag::Mobius
                    | ModuleKindTag::Out
            ) {
                post_gate_tail_slots.push(dst);
            }
            if is_env_gated_vca[dst_idx] {
                env_gated_vcas.push(dst);
                continue;
            }
            let op = &ops[dst_idx];
            let slice = &compiled_cables
                [usize::from(op.in_cable_start)..usize::from(op.in_cable_start + op.in_cable_len)];
            for c in slice {
                if c.to_port == InPort::In && !visited[usize::from(c.from_slot)] {
                    visited[usize::from(c.from_slot)] = true;
                    tail_queue.push(c.from_slot);
                }
            }
        }
        env_gated_vcas.sort_unstable();
        post_gate_tail_slots.sort_unstable();
    }

    let mut module_slot_by_id: Vec<(u16, u8)> = ops
        .iter()
        .enumerate()
        .map(|(slot, op)| (op.module_id, slot as u8))
        .collect();
    module_slot_by_id.sort_unstable_by_key(|&(id, _)| id);

    let mut cable_index_by_id: Vec<(u16, u8)> = compiled_cables
        .iter()
        .enumerate()
        .map(|(idx, c)| (c.cable_id, idx as u8))
        .collect();
    cable_index_by_id.sort_unstable_by_key(|&(id, _)| id);

    let column_layout: Box<[u8]> = ops.iter().map(|op| op.column).collect();

    Ok(CompiledZoneGraph {
        zone_id: zone.id,
        ops: ops.into_boxed_slice(),
        cables: compiled_cables.into_boxed_slice(),
        module_slot_by_id: module_slot_by_id.into_boxed_slice(),
        cable_index_by_id: cable_index_by_id.into_boxed_slice(),
        out_slot,
        lifetime_envs: lifetime_envs.into_boxed_slice(),
        env_gated_vcas: env_gated_vcas.into_boxed_slice(),
        post_gate_tail_slots: post_gate_tail_slots.into_boxed_slice(),
        sampler_slots: sampler_slots.into_boxed_slice(),
        mobius_slots: mobius_slots.into_boxed_slice(),
        column_layout,
    })
}
