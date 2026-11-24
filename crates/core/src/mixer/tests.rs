use super::*;
use crate::params::ChoiceEnum;

#[test]
fn generic_device_parameters_roundtrip_units_and_reject_wrong_device_controls() {
    let mut verb = EffectDevice::reverb(7);
    let descriptors = verb.parameter_descriptors();
    assert_eq!(descriptors.len(), 6);
    for descriptor in descriptors {
        let param = descriptor.param;
        for value in [0.0, 0.5, 1.0] {
            verb.write_normalized(param, value).unwrap();
            assert!((verb.read_normalized(param).unwrap() - value).abs() < 1e-5);
        }
    }
    let before = verb.clone();
    assert!(verb.write_normalized(EffectParam::Feedback, 0.2).is_err());
    assert!(verb.write_normalized(EffectParam::Mix, f32::NAN).is_err());
    assert_eq!(verb, before);
    let decay = EffectParam::Decay.parse_value("850ms").unwrap();
    assert!((EffectParam::Decay.denormalize(decay) - 0.85).abs() < 1e-5);
    let damping = EffectParam::Damping.parse_value("4.2kHz").unwrap();
    assert!((EffectParam::Damping.denormalize(damping) - 4200.0).abs() < 0.01);
    let encoded = serde_json::to_string(&verb).unwrap();
    assert_eq!(
        serde_json::from_str::<EffectDevice>(&encoded).unwrap(),
        verb
    );
}

#[test]
fn independent_audio_topology_and_envelopes_validate_as_a_bounded_graph() {
    let mut mixer = Mixer {
        channels: Vec::new(),
        ..Mixer::default()
    };
    mixer
        .channels
        .push(Channel::new(1, ChannelKind::Audio, "Bass"));
    mixer
        .channels
        .push(Channel::new(2, ChannelKind::Group, "Music"));
    mixer.channel_mut(1).unwrap().output = ChannelRef::Channel(2);
    mixer.ducks.push(Duck {
        id: 1,
        name: "Pump".into(),
        source_part: 99,
        target: ChannelRef::Channel(1),
        depth: 0.75,
        recovery_ms: 120.0,
    });
    mixer.validate(16).unwrap();
    mixer.channel_mut(2).unwrap().output = ChannelRef::Channel(1);
    assert!(mixer.validate(16).is_err());
    mixer.channel_mut(2).unwrap().output = ChannelRef::Master;
    mixer.ducks[0].target = ChannelRef::Channel(999);
    assert!(mixer.validate(16).is_err());
    mixer.ducks[0].target = ChannelRef::Channel(1);
    mixer.ducks[0].recovery_ms = f32::INFINITY;
    assert!(mixer.validate(16).is_err());
}

#[test]
fn effect_device_reports_exact_limiter_lookahead_latency() {
    let lim = EffectDevice::limiter(1);
    assert_eq!(lim.latency_frames(44_100), 66);
    assert_eq!(lim.latency_frames(48_000), 72);
    assert_eq!(lim.latency_frames(96_000), 144);
    assert_eq!(EffectDevice::eq(2).latency_frames(48_000), 0);
    assert_eq!(EffectDevice::compressor(3).latency_frames(48_000), 0);
    assert_eq!(EffectDevice::reverb(4).latency_frames(48_000), 0);
}

#[test]
fn group_to_group_routing_allows_dags_and_rejects_cycles() {
    let mut mixer = Mixer {
        channels: vec![
            Channel::new(1, ChannelKind::Audio, "Kick"),
            Channel::new(2, ChannelKind::Group, "Drums"),
            Channel::new(3, ChannelKind::Group, "Bus"),
            Channel::new(4, ChannelKind::Group, "Stem"),
            Channel::new(5, ChannelKind::Return, "Verb"),
        ],
        ..Mixer::default()
    };
    // Valid 3-deep chain: Kick -> Drums -> Bus -> Stem -> Master
    mixer.channel_mut(1).unwrap().output = ChannelRef::Channel(2);
    mixer.channel_mut(2).unwrap().output = ChannelRef::Channel(3);
    mixer.channel_mut(3).unwrap().output = ChannelRef::Channel(4);
    mixer.channel_mut(4).unwrap().output = ChannelRef::Master;
    mixer.validate(16).unwrap();

    // valid_output_targets excludes cycles cleanly!
    assert_eq!(
        valid_output_targets(&mixer.channels, 4),
        vec![ChannelRef::Master]
    );
    assert_eq!(
        valid_output_targets(&mixer.channels, 3),
        vec![ChannelRef::Master, ChannelRef::Channel(4)]
    );
    assert_eq!(
        valid_output_targets(&mixer.channels, 5),
        vec![ChannelRef::Master]
    );

    // Self-loop rejected.
    mixer.channel_mut(4).unwrap().output = ChannelRef::Channel(4);
    assert!(mixer.validate(16).is_err());

    // 3-node cycle Stem -> Drums -> Bus -> Stem rejected with cycle error.
    mixer.channel_mut(4).unwrap().output = ChannelRef::Channel(2);
    let err = mixer.validate(16).unwrap_err().to_string();
    assert!(
        err.to_ascii_lowercase().contains("cycle"),
        "expected cycle error, got: {err}"
    );

    // Group -> Audio rejected.
    mixer.channel_mut(4).unwrap().output = ChannelRef::Channel(1);
    assert!(mixer.validate(16).is_err());

    // Return -> Group rejected.
    mixer.channel_mut(4).unwrap().output = ChannelRef::Master;
    mixer.channel_mut(5).unwrap().output = ChannelRef::Channel(2);
    assert!(mixer.validate(16).is_err());
}

#[test]
fn delay_and_drive_modes_roundtrip_serde_params_parsing_and_nudging() {
    // 1. Enum methods: ALL, label, parse, to_index, from_index.
    for (i, &m) in DelayMode::ALL.iter().enumerate() {
        assert_eq!(m.to_index(), i);
        assert_eq!(DelayMode::from_index(i), m);
        assert_eq!(DelayMode::parse(m.label()), Some(m));
        assert_eq!(DelayMode::parse(&i.to_string()), Some(m));
    }
    assert_eq!(DelayMode::from_index(99), DelayMode::Stereo);
    assert_eq!(DelayMode::parse("ping_pong"), Some(DelayMode::PingPong));
    assert_eq!(DelayMode::parse("stereo"), Some(DelayMode::Stereo));
    assert_eq!(DelayMode::parse("invalid"), None);

    for (i, &m) in DriveMode::ALL.iter().enumerate() {
        assert_eq!(m.to_index(), i);
        assert_eq!(DriveMode::from_index(i), m);
        assert_eq!(DriveMode::parse(m.label()), Some(m));
        assert_eq!(DriveMode::parse(&i.to_string()), Some(m));
    }
    assert_eq!(DriveMode::from_index(99), DriveMode::Fold);
    assert_eq!(DriveMode::parse("tape"), Some(DriveMode::Tape));
    assert_eq!(DriveMode::parse("hard_clip"), Some(DriveMode::HardClip));
    assert_eq!(DriveMode::parse("fold"), Some(DriveMode::Fold));
    assert_eq!(DriveMode::parse("invalid"), None);

    // 2. EffectParam metadata, discrete classification, ALL canonical name roundtrip, and contextual `mode`.
    assert!(EffectParam::DelayMode.is_discrete());
    assert!(EffectParam::DriveMode.is_discrete());
    assert!(EffectParam::FilterMode.is_discrete());
    assert!(EffectParam::EnvPattern.is_discrete());
    assert!(!EffectParam::Drive.is_discrete());
    assert!(!EffectParam::Time.is_discrete());

    for p in EffectParam::ALL {
        assert_eq!(
            EffectParam::parse(p.name()),
            Some(p),
            "EffectParam::ALL name roundtrip failed for {p:?}"
        );
    }

    let mut delay = EffectDevice::delay(1);
    let mut drive = EffectDevice::drive(2);
    let flt = EffectDevice::filter(3);
    assert_eq!(delay.parse_param("mode"), Some(EffectParam::DelayMode));
    assert_eq!(
        delay.parse_param("delay_mode"),
        Some(EffectParam::DelayMode)
    );
    assert_eq!(drive.parse_param("mode"), Some(EffectParam::DriveMode));
    assert_eq!(
        drive.parse_param("drive_mode"),
        Some(EffectParam::DriveMode)
    );
    assert_eq!(flt.parse_param("mode"), Some(EffectParam::FilterMode));
    assert_eq!(EffectDevice::eq(4).parse_param("mode"), None);

    // 4. Device read/write/nudge/format/parse_value roundtrip for DelayMode & DriveMode.
    assert_eq!(delay.read_normalized(EffectParam::DelayMode), Some(0.0));
    delay.nudge(EffectParam::DelayMode, 1, false).unwrap();
    assert_eq!(delay.read_normalized(EffectParam::DelayMode), Some(1.0));
    assert_eq!(EffectParam::DelayMode.format_normalized(1.0), "Stereo");
    let parsed = EffectParam::DelayMode.parse_value("ping-pong").unwrap();
    delay
        .write_normalized(EffectParam::DelayMode, parsed)
        .unwrap();
    assert_eq!(delay.read_normalized(EffectParam::DelayMode), Some(0.0));
    assert_eq!(EffectParam::DelayMode.format_normalized(0.0), "Ping-Pong");

    assert_eq!(drive.read_normalized(EffectParam::DriveMode), Some(0.0));
    drive.nudge(EffectParam::DriveMode, 1, false).unwrap();
    assert_eq!(drive.read_normalized(EffectParam::DriveMode), Some(0.5));
    assert_eq!(EffectParam::DriveMode.format_normalized(0.5), "Hard Clip");
    drive.nudge(EffectParam::DriveMode, 1, false).unwrap();
    assert_eq!(drive.read_normalized(EffectParam::DriveMode), Some(1.0));
    assert_eq!(EffectParam::DriveMode.format_normalized(1.0), "Fold");
    let parsed = EffectParam::DriveMode.parse_value("hard_clip").unwrap();
    drive
        .write_normalized(EffectParam::DriveMode, parsed)
        .unwrap();
    assert_eq!(
        drive.native_value(EffectParam::DriveMode),
        Some(DriveMode::HardClip.to_index() as f32)
    );

    // Reject cross-device mode writes cleanly.
    assert!(delay.write_normalized(EffectParam::DriveMode, 0.5).is_err());
    assert!(drive.write_normalized(EffectParam::DelayMode, 1.0).is_err());
}

#[test]
fn channel_strip_validates_plus_6db_headroom_and_formats_normalized_targets() {
    for gain in [0.0, 0.5, 1.0, crate::dsp::db_to_lin(6.0), MAX_CHANNEL_GAIN] {
        ChannelStrip { gain, pan: 0.0 }.validate().unwrap();
    }
    for bad in [-0.01, MAX_CHANNEL_GAIN + 0.01, f32::NAN, f32::INFINITY] {
        assert!(
            ChannelStrip {
                gain: bad,
                pan: 0.0
            }
            .validate()
            .is_err(),
            "gain={bad} must fail ChannelStrip::validate"
        );
    }

    let gain_target = MixerParamTarget::Channel {
        channel: ChannelRef::Master,
        param: ChannelParam::Gain,
    };
    assert_eq!(gain_target.format_normalized(0.0), "-inf dB");
    assert_eq!(
        gain_target.format_normalized(CHANNEL_GAIN_UNITY_NORM),
        "+0.0 dB"
    );
    assert_eq!(gain_target.format_normalized(1.0), "+6.0 dB");

    let scale_target = MixerParamTarget::ChannelGainScale {
        channel: ChannelRef::Master,
    };
    assert_eq!(scale_target.format_normalized(0.75), "75%");
    let send_target = MixerParamTarget::Send {
        channel: ChannelRef::Channel(1),
        target: 2,
    };
    assert_eq!(send_target.format_normalized(0.5), "50%");

    // Shared Duck recovery range SPOT & FilterMode::parse delegation.
    assert_eq!(normalize_duck_recovery_ms(DUCK_RECOVERY_MIN_MS), 0.0);
    assert_eq!(normalize_duck_recovery_ms(DUCK_RECOVERY_MAX_MS), 1.0);
    assert_eq!(normalize_duck_recovery_ms(f32::NAN), 0.0);
    assert!((normalize_duck_recovery_ms(1005.0) - 0.5).abs() < 1e-6);
    assert_eq!(
        EffectParam::FilterMode.parse_value_native("303").unwrap(),
        3.0
    );
    assert_eq!(
        EffectParam::FilterMode.parse_value_native("lp6").unwrap(),
        4.0
    );
    assert_eq!(EffectParam::FilterMode.format_normalized(1.0), "LP 6dB");
    assert_eq!(
        EffectParam::FilterMode.parse_value_native("hp").unwrap(),
        2.0
    );

    // Drive asymmetry parameter parsing, formatting, nudging, and validation.
    let mut drv = EffectDevice::drive(99);
    assert_eq!(drv.parse_param("asym"), Some(EffectParam::Asymmetry));
    assert_eq!(drv.parse_param("bias"), Some(EffectParam::Asymmetry));
    assert_eq!(drv.native_value(EffectParam::Asymmetry), Some(0.0));
    assert_eq!(EffectParam::Asymmetry.format_native(0.0), "0%");
    assert_eq!(EffectParam::Asymmetry.format_native(0.35), "+35%");
    assert_eq!(EffectParam::Asymmetry.format_native(-0.4), "-40%");
    let parsed = EffectParam::Asymmetry.parse_value_native("-35%").unwrap();
    assert!((parsed + 0.35).abs() < 1e-6);
    drv.write_native(EffectParam::Asymmetry, parsed).unwrap();
    drv.nudge(EffectParam::Asymmetry, 1, true).unwrap();
    assert!((drv.native_value(EffectParam::Asymmetry).unwrap() + 0.30).abs() < 1e-5);

    // EffectKind::label covers all 8 native insert kinds.
    assert_eq!(EffectDevice::delay(1).kind.label(), "Delay");
    assert_eq!(EffectDevice::reverb(2).kind.label(), "Reverb");
    assert_eq!(EffectDevice::eq(3).kind.label(), "EQ");
    assert_eq!(EffectDevice::chorus(4).kind.label(), "Chorus");
    assert_eq!(EffectDevice::drive(5).kind.label(), "Drive");
    assert_eq!(EffectDevice::filter(6).kind.label(), "Filter");
    assert_eq!(EffectDevice::compressor(7).kind.label(), "Compressor");
    assert_eq!(EffectDevice::limiter(8).kind.label(), "Limiter");

    // ChannelParam::{normalize, denormalize} round-trip and NaN guards.
    assert_eq!(ChannelParam::Gain.normalize(0.0), 0.0);
    assert_eq!(ChannelParam::Gain.normalize(1.0), CHANNEL_GAIN_UNITY_NORM);
    assert_eq!(ChannelParam::Gain.normalize(MAX_CHANNEL_GAIN), 1.0);
    assert_eq!(ChannelParam::Gain.denormalize(CHANNEL_GAIN_UNITY_NORM), 1.0);
    assert_eq!(ChannelParam::Gain.denormalize(1.0), MAX_CHANNEL_GAIN);
    assert_eq!(ChannelParam::Gain.denormalize(f32::NAN), 0.0);

    assert_eq!(ChannelParam::Pan.normalize(-1.0), 0.0);
    assert_eq!(ChannelParam::Pan.normalize(0.0), 0.5);
    assert_eq!(ChannelParam::Pan.normalize(1.0), 1.0);
    assert_eq!(ChannelParam::Pan.normalize(f32::NAN), 0.5);
    assert_eq!(ChannelParam::Pan.denormalize(0.0), -1.0);
    assert_eq!(ChannelParam::Pan.denormalize(0.5), 0.0);
    assert_eq!(ChannelParam::Pan.denormalize(1.0), 1.0);
    assert_eq!(ChannelParam::Pan.denormalize(f32::NAN), 0.0);
}

#[test]
fn mixer_choice_enums_and_effect_kind_tag_dispatch_roundtrip() {
    for &tag in EffectKindTag::ALL {
        let dev = tag.default_device(42);
        assert_eq!(dev.kind.tag(), tag);
        assert_eq!(dev.kind.label(), tag.label());
        assert_eq!(dev.kind.short_tag(), tag.short_tag());
        assert_eq!(dev.kind.params(), tag.params());
        assert_eq!(EffectKindTag::parse(tag.slug()), Some(tag));
        assert!(dev.validate().is_ok());
        assert_eq!(EffectDevice::from_kind_name(42, tag.slug()), Some(dev));
    }
    assert_eq!(
        EffectDevice::from_kind_name(1, "comp").map(|d| d.kind.tag()),
        Some(EffectKindTag::Compressor)
    );
    assert_eq!(
        EffectDevice::from_kind_name(1, "sat").map(|d| d.kind.tag()),
        Some(EffectKindTag::Drive)
    );
    assert_eq!(
        EffectDevice::from_kind_name(1, "pcf").map(|d| d.kind.tag()),
        Some(EffectKindTag::Filter)
    );
    assert_eq!(
        EffectDevice::from_kind_name(1, "lim").map(|d| d.kind.tag()),
        Some(EffectKindTag::Limiter)
    );

    // ChoiceEnum generic cycle wrapping works identically on array-backed and slice-backed enums.
    fn round_cycle<T: ChoiceEnum + std::fmt::Debug>(v: T) {
        assert_eq!(v.cycle(1).cycle(-1), v);
    }
    round_cycle(DelayMode::PingPong);
    round_cycle(DriveMode::Tape);
    round_cycle(ChannelKind::Audio);
    round_cycle(ChannelParam::Gain);
    round_cycle(EffectKindTag::Delay);
}
