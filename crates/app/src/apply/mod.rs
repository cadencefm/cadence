//! Action dispatch and shared preset I/O helpers.
use crate::*;

mod material;
mod mixer;
mod rack;
mod session;
mod transport;

/// Single point of truth for resolving an explicit preset file path directly or relative to `asset_base`.
pub(crate) fn resolve_preset_file_path_with(
    io: &dyn HostIo,
    asset_base: &Path,
    raw: &str,
) -> Option<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let direct = PathBuf::from(trimmed);
    if io.is_file(&direct) {
        return Some(direct);
    }
    if direct.is_relative() {
        let rel = asset_base.join(&direct);
        if io.is_file(&rel) {
            return Some(rel);
        }
    }
    None
}

/// Single point of truth for bounded loading and parsing of an external [`InstrumentPreset`] via [`HostIo`].
pub(crate) fn load_preset_via_io(io: &dyn HostIo, path: &Path) -> Result<InstrumentPreset> {
    let bytes = io.read_bounded(path, cadence_core::presets::MAX_PRESET_BYTES)?;
    InstrumentPreset::from_json_bytes(&bytes)
}

impl App {
    pub fn apply(&mut self, action: Action) -> Result<Outcome> {
        match action {
            Action::GetState => Ok(Outcome::default()),
            Action::SetArrangeView { view } => self.set_arrange_view(view),
            Action::SelectArrangementEvent { event } => self.select_arrangement_event(event),
            Action::InspectArrangementEvent { event } => {
                self.apply_inspect_arrangement_event(event)
            }
            Action::InspectAssets => Ok(self.apply_inspect_assets()),
            Action::ReloadSamples => Ok(self.apply_reload_samples()),
            Action::Inspect { pattern } => Ok(Outcome {
                inspection: Some(self.inspect(pattern)),
                ..Outcome::default()
            }),
            Action::SelectPattern { pattern } => {
                self.select_pattern(pattern)?;
                Ok(Outcome::default())
            }
            Action::SelectLane { lane } => self.apply_select_lane(lane),
            Action::SelectPlacement { placement } => {
                self.select_placement(placement)?;
                Ok(Outcome::default())
            }
            Action::SelectInstrument { instrument } => {
                self.open_instrument(instrument)?;
                Ok(Outcome::default())
            }
            Action::SelectChannel { channel } => {
                self.open_channel(channel)?;
                Ok(Outcome::default())
            }
            Action::Edit { edit } => {
                let label = self.describe_edit(&edit);
                self.commit(vec![edit], label)
            }
            Action::Transaction { edits } => {
                self.commit(edits, "Committed musical transaction".into())
            }
            Action::CreatePattern { name, kind } => self.apply_create_pattern(name, kind),
            Action::CreateLane { name } => self.apply_create_lane(name),
            Action::CreateInstrument {
                name,
                sound,
                channel,
            } => {
                let rack = self.materialize_sound_choice(&sound, false)?;
                self.create_instrument(name, rack, channel)
            }
            Action::CreatePart {
                name,
                pattern,
                instrument,
                transpose,
                velocity_scale,
            } => self.apply_create_part(name, pattern, instrument, transpose, velocity_scale),
            Action::Place {
                part,
                lane,
                start_step,
                length_steps,
                phase,
                offset_steps,
            } => self.apply_place(part, lane, start_step, length_steps, phase, offset_steps),
            Action::PlacedPart {
                name,
                sound,
                notes,
                start_step,
                length_steps,
                transpose,
                velocity_scale,
            } => self.apply_placed_part(material::PlacedPartSpec {
                name,
                sound,
                notes,
                start_step,
                length_steps,
                transpose,
                velocity_scale,
            }),
            Action::Fork { source, name } => self.apply_fork(source, name),
            Action::Freeze {
                source,
                name,
                start,
                length,
            } => self.apply_freeze(source, name, start, length),
            Action::SetParam { address, value } => {
                let label = self.describe_param(address, value);
                self.commit_param(address, value, label, false)
            }
            Action::SetRack { instrument, rack } => self.commit(
                vec![Edit::UpdateRackPatch {
                    id: instrument,
                    rack,
                }],
                "Updated Rack patch".into(),
            ),
            Action::RackAddModule {
                instrument,
                zone,
                kind,
                name,
            } => self.apply_rack_add_module(instrument, zone, kind, name),
            Action::RackRemoveModule {
                instrument,
                zone,
                module,
            } => self.apply_rack_remove_module(instrument, zone, module),
            Action::RackRenameModule {
                instrument,
                zone,
                module,
                name,
            } => self.apply_rack_rename_module(instrument, zone, module, name),
            Action::RackConnectCable {
                instrument,
                zone,
                from_module,
                to_module,
                to_port,
                amount,
            } => self.apply_rack_connect_cable(
                instrument,
                zone,
                from_module,
                to_module,
                to_port,
                amount,
            ),
            Action::RackDisconnectCable {
                instrument,
                zone,
                cable,
            } => self.apply_rack_disconnect_cable(instrument, zone, cable),
            Action::RackPinControl { instrument, pin } => {
                self.apply_rack_pin_control(instrument, pin)
            }
            Action::RackUnpinControl { instrument, index } => {
                self.apply_rack_unpin_control(instrument, index)
            }
            Action::LoadPreset { instrument, preset } => {
                self.load_instrument_preset(instrument, &preset)
            }
            Action::RenameInstrument { instrument, name } => {
                self.apply_rename_instrument(instrument, name)
            }
            Action::RenamePattern { pattern, name } => self.apply_rename_pattern(pattern, name),
            Action::RenamePart { part, name } => self.apply_rename_part(part, name),
            Action::RenameLane { lane, name } => self.apply_rename_lane(lane, name),
            Action::ReorderLane { lane, target_index } => {
                self.apply_reorder_lane(lane, target_index)
            }
            Action::DeleteInstrument { instrument } => self.apply_delete_instrument(instrument),
            Action::CreateChannel { name, kind } => self.apply_create_channel(name, kind),
            Action::RenameChannel { channel, name } => self.apply_rename_channel(channel, name),
            Action::DeleteChannel { channel } => self.delete_channel_safe(channel),
            Action::AddEffect { channel, kind } => self.apply_add_effect(channel, kind),
            Action::DeleteEffect { device } => self.apply_delete_effect(device),
            Action::MoveEffect {
                device,
                channel,
                position,
            } => self.apply_move_effect(device, channel, position),
            Action::SetEffectBypass { device, bypassed } => {
                self.apply_set_effect_bypass(device, bypassed)
            }
            Action::RenameEffect { device, name } => self.apply_rename_effect(device, name),
            Action::RouteInstrument {
                instrument,
                channel,
            } => self.apply_route_instrument(instrument, channel),
            Action::SetLaneMuted { lane, muted } => self.set_lane_flags(lane, Some(muted), None),
            Action::SetLaneSolo { lane, solo } => self.set_lane_flags(lane, None, Some(solo)),
            Action::SetChannelMuted { channel, muted } => {
                self.set_channel_flags(channel, Some(muted), None)
            }
            Action::SetChannelSolo { channel, solo } => {
                ensure!(channel != ChannelRef::Master, "Master cannot be soloed");
                self.set_channel_flags(channel, None, Some(solo))
            }
            Action::AddLanePart { lane, entry } => self.add_lane_part(lane, entry),
            Action::DropLanePart { lane, part } => self.drop_lane_part(lane, part),
            Action::Launch { lane, part, at } => self.launch(lane, part, at),
            Action::Release { lane, at } => self.release_lane(lane, at),
            Action::ReleaseAll { at } => self.release_all(at),
            Action::ToggleLane { lane, at } => self.toggle_lane(lane, at),
            Action::CancelPending { all } => self.cancel_pending(all),
            Action::Advance { steps } => Ok(self.advance(steps)),
            Action::SetTakeArmed { armed } => self.set_take_armed(armed),
            Action::ToggleTakeArmed => self.set_take_armed(!self.takes.armed),
            Action::DiscardTake => self.discard_take_action(),
            Action::SaveTake { path } => self.save_take_action(path),
            Action::LoadTake { path } => self.load_take_action(path),
            Action::KeepRange { start_bar, end_bar } => self.keep_take_range(start_bar, end_bar),
            Action::Stage => self.start_staging(),
            Action::FinishStaging { choice } => self.finish_staging(choice),
            Action::SetChannelOutput { channel, output } => {
                self.apply_set_channel_output(channel, output)
            }
            Action::SetChannelSend {
                channel,
                target,
                amount,
            } => self.apply_set_channel_send(channel, target, amount),
            Action::CreateSection {
                name,
                start_step,
                length_steps,
            } => self.apply_create_section(name, start_step, length_steps),
            Action::CreateModulation {
                name,
                target,
                pattern,
            } => self.apply_create_modulation(name, target, pattern),
            Action::CreateDuck {
                name,
                source_part,
                target,
                depth,
                recovery_ms,
            } => self.apply_create_duck(name, source_part, target, depth, recovery_ms),
            Action::ClearMeters { channel } => {
                self.status = match channel {
                    Some(ch) => format!("Cleared peak/clip hold on {}", self.channel_name(ch)),
                    None => "Cleared all peak/clip holds".into(),
                };
                Ok(Outcome {
                    clear_mixer_meters: Some(channel),
                    ..Outcome::default()
                })
            }
            Action::Play { playing } => self.apply_play(playing),
            Action::Stop => self.apply_stop(),
            Action::AudioPanic => self.apply_audio_panic(),
            Action::RestartAudio => self.apply_restart_audio(),
            Action::RescanAudio => self.apply_rescan_audio(),
            Action::RestartLoop => self.apply_restart_loop(),
            Action::CueSelection { play } => self.cue_song(self.arrange_ui.step as usize, play),
            Action::CueSong { step, play } => self.cue_song(step, play),
            Action::SetDeck { visible } => {
                self.set_deck_visible(visible)?;
                Ok(Outcome::default())
            }
            Action::PinDeck { pinned } => {
                self.pin_deck(pinned)?;
                Ok(Outcome::default())
            }
            Action::SetMonitor { visible } => {
                self.set_output_visible(visible);
                self.status = format!("Output scope {}", if visible { "shown" } else { "hidden" });
                Ok(Outcome::default())
            }
            Action::SetFollow { enabled } => {
                self.follow.set_enabled(enabled);
                self.describe_follow();
                Ok(Outcome::default())
            }
            Action::ToggleFollow => {
                self.follow.toggle();
                self.describe_follow();
                Ok(Outcome::default())
            }
            Action::CueBar { bar } => self.apply_cue_bar(bar),
            Action::Audition { pattern, length } => self.start_audition(pattern, length),
            Action::StopAudition => self.stop_audition(),
            Action::Undo => self.step_history(HistoryStep::Undo),
            Action::Redo => self.step_history(HistoryStep::Redo),
            Action::Save { path } => self.apply_save(path),
            Action::Load { path } => self.apply_load(path),
            Action::ImportRbs { path } => self.apply_import_rbs(path),
            Action::New { title } => self.apply_new(title),
            Action::Export { path, bars } => self.apply_export(path, bars),
            Action::ZoneSelect { instrument, zone } => self.apply_zone_select(instrument, &zone),
            Action::ZoneStep { instrument, delta } => self.apply_zone_step(instrument, delta),
            Action::ZoneAdd {
                instrument,
                name,
                midi,
                sample_path,
            } => self.apply_zone_add(instrument, name, midi, sample_path),
            Action::ZoneDuplicate { instrument, name } => {
                self.apply_zone_duplicate(instrument, name)
            }
            Action::ZoneRemove { instrument } => self.apply_zone_remove(instrument),
            Action::ZoneSetSample { instrument, path } => {
                self.apply_zone_set_sample(instrument, path)
            }
            Action::ZoneLoadPreset { instrument, preset } => {
                self.apply_zone_load_preset(instrument, &preset)
            }
            Action::ZoneRename { instrument, name } => self.apply_zone_rename(instrument, name),
            Action::ZoneSetRange {
                instrument,
                low_midi,
                high_midi,
                root_midi,
            } => self.apply_zone_set_range(instrument, low_midi, high_midi, root_midi),
            Action::ZoneSetVelocity {
                instrument,
                low_vel,
                high_vel,
            } => self.apply_zone_set_velocity(instrument, low_vel, high_vel),
            Action::ZoneSetChoke {
                instrument,
                choke_group,
            } => self.apply_zone_set_choke(instrument, choke_group),
            Action::ZoneImportFolder { instrument, folder } => {
                self.apply_zone_import_folder(instrument, folder)
            }
            Action::Bounce {
                scope,
                name,
                create_part,
                target,
            } => self.prepare_bounce(scope, name, create_part, target),
            Action::ToggleAudioRecord => self.toggle_audio_recording(),
            Action::StartAudioRecord {
                mode,
                target,
                place_in_arrange,
                bars,
                name,
            } => self.start_audio_recording(StartRecordingOpts {
                mode,
                target,
                place_in_arrange,
                bars,
                name,
            }),
            Action::StopAudioRecord => self.stop_audio_recording(),
            Action::CancelAudioRecord => self.cancel_audio_recording(),
            Action::CycleInputMonitor => self.cycle_input_monitor(),
            Action::SetMacro { index, value } => self.apply_set_macro(index, value),
            Action::RenameMacro { index, name } => self.apply_rename_macro(index, name),
            Action::BindMacro {
                index,
                target,
                min,
                max,
            } => self.apply_bind_macro(index, target, min, max),
            Action::UnbindMacro { index, binding } => self.apply_unbind_macro(index, binding),
            Action::ClearMacro { index } => self.apply_clear_macro(index),
            Action::Command { line } => self.execute_command(line.trim_start_matches(':')),
        }
    }
}
