//! Player movement settings read from the authored MickeyPlayerMotionControllerComponent
//! (shared by the EM1 scene importer and the EM2 playtest).
use anyhow::{ensure, Context, Result};

/// Fill `motion` from the player's authored properties (`property` looks one up by name
/// across the player's components). Properties that are absent keep their current value.
pub(super) fn apply<'a>(property: impl Fn(&str) -> Option<&'a serde_json::Value>, motion: &mut super::physics::Motion) -> Result<()> {
    if let Some(value) = property("JumpType") {
        let kind = value.as_u64().context("Invalid authored JumpType")?;
        ensure!(
            matches!(kind, 0 | 2),
            "Blotworx does not yet implement JumpType {kind}"
        );
        motion.jump_type = kind as u32;
    }
    if let Some(value) = property("JumpHeightMultipliers") {
        motion.jump_height_multipliers = value
            .as_array()
            .context("Invalid JumpHeightMultipliers")?
            .iter()
            .map(|v| {
                let n =
                    v.as_f64().context("Non-number in JumpHeightMultipliers")? as f32;
                ensure!(n.is_finite() && n >= 0.0, "Invalid jump height multiplier");
                Ok(n)
            })
            .collect::<Result<Vec<f32>>>()?;
    }
    if let Some(value) = property("Ground_UseFixedSpeeds") {
        motion.ground_use_fixed_speeds = value
            .as_bool()
            .context("Invalid authored Ground_UseFixedSpeeds")?;
    }
    for (name, target) in [
        ("Ground_FixedSpeeds", &mut motion.ground_fixed_speeds),
        ("Ground_StickValues", &mut motion.ground_stick_values),
    ] {
        if let Some(value) = property(name) {
            let array = value
                .as_array()
                .with_context(|| format!("Invalid authored {name}"))?;
            *target = array
                .iter()
                .map(|v| {
                    let n = v
                        .as_f64()
                        .with_context(|| format!("Non-number in {name}"))?
                        as f32;
                    ensure!(n.is_finite(), "Non-finite value in {name}");
                    Ok(n)
                })
                .collect::<Result<Vec<f32>>>()?;
        }
    }
    for (name, target) in [
        ("Max Air Speed", &mut motion.air_speed),
        (
            "AirControlAccel",
            &mut motion.air_control_acceleration,
        ),
        (
            "AirLeftRightAccel",
            &mut motion.air_side_acceleration,
        ),
        (
            "Stationary Turn Rate",
            &mut motion.stationary_turn_rate,
        ),
        ("Moving Turn Rate", &mut motion.moving_turn_rate),
        ("AirTurnRate", &mut motion.air_turn_rate),
        (
            "DoubleJumpAddedHeight",
            &mut motion.double_jump_height,
        ),
        (
            "MinDistAboveGroundForDoubleJump",
            &mut motion.double_jump_min_distance,
        ),
        (
            "MaxDistAboveGroundForJump",
            &mut motion.max_distance_for_jump,
        ),
        (
            "MaxRelevantDistanceOffGround",
            &mut motion.max_relevant_ground_distance,
        ),
        ("Max Slope", &mut motion.max_slope_degrees),
        (
            "InputLinearVelocity",
            &mut motion.input_linear_velocity,
        ),
        (
            "InputLengthVelocity",
            &mut motion.input_length_velocity,
        ),
        (
            "InputRotationVelocity",
            &mut motion.input_rotation_velocity,
        ),
        ("GroundControlAccel", &mut motion.ground_acceleration),
        (
            "GroundControlAccelStop",
            &mut motion.ground_stopping_acceleration,
        ),
    ] {
        if let Some(value) = property(name).and_then(|v| v.as_f64()) {
            ensure!(
                value.is_finite() && value >= 0. && (value as f32).is_finite(),
                "Invalid authored player {name}"
            );
            *target = value as f32;
        }
    }
    if let Some(value) = property("InputFilterType") {
        let mode = value.as_u64().context("Invalid authored InputFilterType")?;
        ensure!(mode <= 2, "Unsupported authored InputFilterType {mode}");
        motion.input_filter_type = mode as u32;
    }
    if let Some(value) = property("TerminalSpeed").and_then(|v| v.as_f64()) {
        ensure!(
            value.is_finite() && value < 0. && (value as f32).is_finite(),
            "Invalid authored player TerminalSpeed"
        );
        motion.terminal_speed = value as f32;
    }
    for (name, target) in [
        ("Max Move Speed", &mut motion.speed),
        ("Jump Height", &mut motion.jump_height),
        ("Character Gravity Up", &mut motion.gravity_up),
        ("Character Gravity Down", &mut motion.gravity_down),
    ] {
        if let Some(v) = property(name).and_then(|v| v.as_f64()) {
            *target = v.abs() as f32;
        }
    }
    Ok(())
}
