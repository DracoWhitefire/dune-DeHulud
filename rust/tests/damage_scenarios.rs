//! The same frame scripts exercise both applications against a real fake-game child.
use dehulud::json::{self, Value};

#[cfg(feature = "test-source")]
mod common;

#[test]
fn damage_fixture_has_frame_scripts_and_expected_channels() {
    let fixture = json::parse(include_str!("../../tests/damage_scenarios.json")).unwrap();
    assert_eq!(fixture.get("version"), Some(&Value::Int(Some(1))));
    let Some(Value::Array(scenarios)) = fixture.get("scenarios") else { panic!("missing scenarios") };
    assert!(!scenarios.is_empty());
    for scenario in scenarios {
        assert!(matches!(scenario.get("name"), Some(Value::Str(_))));
        let Some(Value::Array(frames)) = scenario.get("frames") else { panic!("missing frames") };
        assert!(!frames.is_empty());
        for frame in frames {
            assert!(matches!(frame.get("at_ms"), Some(Value::Int(Some(_)))));
            let expected = frame.get("expected").expect("expected emissions");
            for channel in ["taken", "health", "shield"] {
                assert!(matches!(expected.get(channel), Some(Value::Array(_))));
            }
        }
    }
}

#[cfg(feature = "test-source")]
fn encode(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(v) => v.to_string(),
        Value::Int(Some(v)) => v.to_string(),
        Value::Float(v) => v.to_string(),
        Value::Str(v) => json::quote(v),
        Value::Array(v) => format!("[{}]", v.iter().map(encode).collect::<Vec<_>>().join(",")),
        Value::Object(v) => format!(
            "{{{}}}",
            v.iter().map(|(k, v)| format!("{}:{}", json::quote(k), encode(v))).collect::<Vec<_>>().join(",")
        ),
        Value::Int(None) => panic!("fixture integer out of range"),
    }
}

#[cfg(feature = "test-source")]
fn emissions(value: &Value) -> Vec<f64> {
    match value {
        Value::Null => vec![],
        Value::Int(Some(n)) => vec![*n as f64],
        Value::Float(n) => vec![*n],
        Value::Array(items) => items.iter().flat_map(emissions).collect(),
        other => panic!("invalid emissions: {other:?}"),
    }
}

#[cfg(feature = "test-source")]
fn scenario(name: &str) {
    use common::*;
    if !readable() {
        return;
    }
    let fixture = json::parse(include_str!("../../tests/damage_scenarios.json")).unwrap();
    let Some(Value::Array(scenarios)) = fixture.get("scenarios") else { panic!("missing scenarios") };
    let script = scenarios.iter().find(|v| v.get("name") == Some(&Value::Str(name.into()))).expect("named scenario");
    let Some(Value::Array(frames)) = script.get("frames") else { panic!("missing frames") };
    let mut run = Run::start(
        Opts::new(Start::Beside)
            .combat()
            .fake(&["--build", "26333948", "--damage"])
            .set("damage_step", "0")
            .set("at_ms", "0")
            .set("enabled", ""),
    );
    let keys = ["worm", "damage_taken", "damage_dealt"];
    let mut enabled = [true; 3];
    for (i, frame) in frames.iter().enumerate() {
        let step = (i + 1) as i64;
        let metadata = run.fake().damage_frame(&encode(frame));
        if let Some(switches) = frame.get("switches") {
            for (i, key) in keys.iter().enumerate() {
                if let Some(Value::Bool(on)) = switches.get(key) {
                    enabled[i] = *on;
                }
            }
        }
        let active: Vec<_> = keys.iter().enumerate().filter_map(|(i, key)| enabled[i].then_some(*key)).collect();
        let poll = run.damage_step(step, int(frame, "at_ms"), &active, metadata.get("failure_ranges").unwrap());
        let expected = frame.get("expected").unwrap();
        assert_eq!(
            emissions(poll.get("taken").unwrap()),
            emissions(expected.get("taken").unwrap()),
            "{name} frame {i} taken: {poll:?}"
        );
        let (health, shield) = match poll.get("dealt").unwrap() {
            Value::Array(channels) => (&channels[0], &channels[1]),
            Value::Null => (&Value::Null, &Value::Null),
            other => panic!("invalid dealt trace: {other:?}"),
        };
        assert_eq!(emissions(health), emissions(expected.get("health").unwrap()), "{name} frame {i} health: {poll:?}");
        assert_eq!(emissions(shield), emissions(expected.get("shield").unwrap()), "{name} frame {i} shield: {poll:?}");
        let reads: Vec<_> = run
            .traced()
            .into_iter()
            .filter(|v| {
                v.get("event") == Some(&Value::Str("read".into()))
                    && v.get("damage_step") == Some(&Value::Int(Some(step)))
            })
            .collect();
        if active.is_empty() {
            assert!(reads.is_empty(), "all-off frame read memory: {reads:?}");
        }
        let ranges = metadata.get("feature_ranges").unwrap();
        for (_, key) in keys.iter().enumerate().filter(|(j, _)| !enabled[*j]) {
            let Some(Value::Array(ranges)) = ranges.get(key) else { panic!("feature ranges") };
            for range in ranges {
                let Value::Array(pair) = range else { panic!("range pair") };
                let [Value::Int(Some(start)), Value::Int(Some(end))] = pair.as_slice() else { panic!("range numbers") };
                for read in &reads {
                    let address = int(read, "address");
                    assert!(
                        !(address < *end && *start < address + int(read, "size")),
                        "{name} frame {i} disabled {key} read {read:?}"
                    );
                }
            }
        }
    }
    assert_eq!(run.stop(), 0);
}

#[cfg(feature = "test-source")]
macro_rules! scenarios {
    ($($name:ident),* $(,)?) => { $(#[test] fn $name() { scenario(stringify!($name)); })* };
}

#[cfg(feature = "test-source")]
scenarios!(
    steady_hits,
    killing_blow_role_change,
    heal_mixed_with_hit,
    target_swap,
    serial_reuse,
    vehicle_target,
    read_failure,
    world_change,
    image_change,
    poll_gap_boundary,
    maximum_change,
    own_death_respawn,
    worm_off,
    taken_off,
    dealt_off,
    all_off
);
