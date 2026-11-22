//! Recovered retail skid/reversal actions. Runtime phase ordering and the
//! animation/game-event dispatcher still require integration and validation.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Turn,
    Skid,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Update {
    Inactive,
    Turning(f32),
    Skidding,
    Finished,
}

/// Native gameplay-clock timestamp used by fast-history and turn cooldown.
/// Constructor: REL 0x14baa4; zero-offset reset: 0x14bae4; read: 0x14bb74.
/// Tick-to-seconds conversion belongs to the shared clock (0x14b640), not
/// to an action-local f32 delta accumulator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClockStamp {
    ticks: u64,
}
impl ClockStamp {
    /// Portable timer storage keeps the original two timestamp words. Loading
    ///them is distinct from reset, which changes a zero timestamp to one.
    pub const fn from_native_ticks(ticks: u64) -> Self { Self { ticks } }
    pub const fn native_ticks(&self) -> u64 { self.ticks }
    pub fn is_running(&self) -> bool {
        self.ticks != 0
    }
    pub fn clear(&mut self) {
        self.ticks = 0;
    }
    pub fn reset(&mut self, game_ticks: u64) {
        self.ticks = if game_ticks == 0 { 1 } else { game_ticks };
    }
    pub fn elapsed_ticks(&self, game_ticks: u64) -> i64 {
        game_ticks.wrapping_sub(self.ticks) as i64
    }
}

/// Motion-owned history (+0x3b0/+0x3c0/+0x3f0/+0x3f8). Both timers
/// read the scaled gameplay clock, so pausing cannot expire an action window.
#[derive(Default)]
pub struct ActionHistory {
    pub fast: bool,
    pub direction: [f32; 3],
    fast_stamp: ClockStamp,
    turn_stamp: ClockStamp,
}
impl ActionHistory {
    /// REL 0x96d70..0x96d9c initializes the two zero-offset timers.
    pub fn initialize_timers(&mut self, clock: &super::game_clock::Clock) {
        self.fast_stamp.reset(clock.scaled_ticks);
        self.turn_stamp.reset(clock.scaled_ticks);
    }
    pub fn refresh(
        &mut self,
        clock: &super::game_clock::Clock,
        character_state: u32,
        sample: Option<&super::ground_history::Sample>,
        maximum_speed: f32,
    ) {
        if refresh_fast_history(
            &mut self.fast,
            character_state,
            sample.map(|s| s.speed),
            maximum_speed,
        ) {
            self.fast_stamp.reset(clock.scaled_ticks);
            self.direction = sample.expect("refresh requires a ground sample").direction;
        }
    }
    pub fn since_fast(&self, clock: &super::game_clock::Clock) -> f32 {
        clock
            .rate
            .seconds(self.fast_stamp.elapsed_ticks(clock.scaled_ticks))
    }
    pub fn since_turn(&self, clock: &super::game_clock::Clock) -> f32 {
        clock
            .rate
            .seconds(self.turn_stamp.elapsed_ticks(clock.scaled_ticks))
    }
    /// Consume only the timer effect; forward all other effects to their owners.
    pub fn apply(&mut self, clock: &super::game_clock::Clock, effect: Effect) -> Option<Effect> {
        if effect == Effect::ResetTurnTimer {
            self.turn_stamp.reset(clock.scaled_ticks);
            None
        } else {
            Some(effect)
        }
    }
}

/// REL 0x14b640: whole seconds plus a separately rounded fractional part.
/// The clock owns the frequency and division threshold; neither is guessed.
/// Replace its repeated-subtraction branch algebraically, keeping its strict
/// comparison at exact multiples and its wrapping 32-bit whole-second count.
pub fn ticks_to_seconds(
    ticks: i64,
    frequency: std::num::NonZeroU32,
    division_threshold: i32,
) -> f32 {
    let ticks = ticks as i128;
    let frequency = frequency.get() as i128;
    let divide = if ticks >= 0 {
        ticks > division_threshold as i128
    } else {
        ticks < -(division_threshold as i128)
    };
    let whole = if divide {
        ticks / frequency
    } else if ticks > 0 {
        (ticks - 1) / frequency
    } else if ticks < 0 {
        -((-ticks - 1) / frequency)
    } else {
        0
    };
    let remainder = ticks - whole * frequency;
    // DOL 0x8000de88 converts signed 64-bit through double then frsp.
    let fraction = ((remainder as i64 as f64) as f32) / (frequency as f32);
    (whole as i32 as f32) + fraction
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    Animation(&'static str),
    /// Native event 0x2b carries a float zero on skid entry.
    GameEvent(u32, f32),
    BeginSkid,
    BeginTurn {
        direction: [f32; 3],
        stopping_acceleration: f32,
    },
    Facing([f32; 3]),
    ResetTurnTimer,
    /// Clear direction override / reverse flag, restore stopping acceleration,
    /// restore +0x214 bit 0, then clear +0x210 and set +0x211.
    ResetMotion,
}

pub struct Lifecycle {
    pub action: Action,
    pub remaining: f32,
    target: [f32; 3],
}
impl Default for Lifecycle {
    fn default() -> Self {
        Self {
            action: Action::None,
            remaining: 0.,
            target: [0.; 3],
        }
    }
}
impl Lifecycle {
    /// Entry ordering from REL 0x97cfc..0x97f1c. History timers and the
    /// normalized fast direction belong to the caller, not wall-clock time.
    pub fn enter(
        &mut self,
        s: &Settings,
        i: &EntryInput,
        fast_direction: [f32; 3],
        mut emit: impl FnMut(Effect),
    ) {
        for action in entries(self.action, &s.entry, i).into_iter().flatten() {
            match action {
                Action::Skid => {
                    emit(Effect::Animation("EVENT_Start_Run_Skid"));
                    self.action = action;
                    self.remaining = s.skid_duration;
                    emit(Effect::BeginSkid);
                    emit(Effect::GameEvent(0x2b, 0.));
                }
                Action::Turn => {
                    emit(Effect::Animation("EVENT_Start_180_Turn"));
                    self.action = action;
                    self.remaining = s.turn_duration;
                    self.target = turn_facing(fast_direction, 1.);
                    emit(Effect::BeginTurn {
                        direction: fast_direction,
                        stopping_acceleration: s.turn_deceleration,
                    });
                }
                Action::None => unreachable!(),
            }
        }
    }
    /// REL 0x98940 emits the same stop event for either action, including a
    /// forced reset. Do not erase the remaining timer: the native reset does not.
    pub fn reset(&mut self, send_animation: bool, mut emit: impl FnMut(Effect)) {
        if send_animation {
            emit(Effect::Animation("EVENT_Stop_Run_Skid"));
        }
        self.action = Action::None;
        emit(Effect::ResetMotion);
    }
    pub fn update(
        &mut self,
        character_state: u32,
        speed: f32,
        s: &Settings,
        dt: f32,
        mut emit: impl FnMut(Effect),
    ) {
        let previous = self.action;
        match advance(
            &mut self.action,
            &mut self.remaining,
            character_state,
            speed,
            s.turn_duration,
            dt,
        ) {
            Update::Turning(fraction) => emit(Effect::Facing(turn_facing(self.target, fraction))),
            Update::Finished => {
                if previous == Action::Turn {
                    emit(Effect::Facing(self.target));
                    emit(Effect::ResetTurnTimer);
                }
                self.reset(true, &mut emit);
            }
            Update::Inactive | Update::Skidding => {}
        }
    }
}

/// EM2 playtest driver for skid / 180 turn, per the Xbox 360 MickeyPlayerMotionController
/// (0x829fb718 per tick: update the running action, then try entry once on the ground long
/// enough; 0x829f6bf0 fast history; 0x829f9348 skid and 0x829f9058 turn entry). Timers use the
/// fixed simulation step. Animation events are queued in `events` for the behaviour graph.
pub struct Em2Driver {
    settings: Settings,
    min_allowed: f32,
    life: Lifecycle,
    fast: bool,
    fast_direction: [f32; 3],
    since_fast: f32,
    since_turn: f32,
    ground_time: f32,
    pub events: Vec<&'static str>,
}
impl Em2Driver {
    pub fn new(settings: Settings, min_allowed: f32) -> Self {
        Self { settings, min_allowed, life: Lifecycle::default(), fast: false, fast_direction: [0., 0., 1.],
            since_fast: 0., since_turn: 0., ground_time: 0., events: Vec::new() }
    }
    pub fn action(&self) -> Action { self.life.action }
    /// `desired` is this tick's world-space movement input (length <= 1).
    pub fn tick(&mut self, player: &mut super::physics::Player, desired: [f32; 3], dt: f32) {
        let grounded = player.grounded;
        let state = if grounded { 0 } else { 1 };
        self.since_fast += dt;
        self.since_turn += dt;
        self.ground_time = if grounded { self.ground_time + dt } else { 0. };
        let horizontal = [player.velocity[0], 0., player.velocity[2]];
        let speed = (horizontal[0] * horizontal[0] + horizontal[2] * horizontal[2]).sqrt();
        let sample = grounded.then_some(speed);
        let maximum = player.max_move_speed();
        // EM2: threshold = 0.95 * Max Move Speed * The180MinAllowedSpeed
        if refresh_fast_history(&mut self.fast, state, sample, maximum * self.min_allowed) && speed > 1e-6 {
            self.since_fast = 0.;
            self.fast_direction = [horizontal[0] / speed, 0., horizontal[2] / speed];
        }
        let mut effects = Vec::new();
        if !grounded && self.life.action != Action::None {
            self.life.reset(true, |e| effects.push(e));
        }
        self.life.update(state, speed, &self.settings, dt, |e| effects.push(e));
        let input_length = (desired[0] * desired[0] + desired[2] * desired[2]).sqrt();
        let direction_dot = if input_length > 1e-6 {
            (self.fast_direction[0] * desired[0] + self.fast_direction[2] * desired[2]) / input_length
        } else { 1. };
        let input = EntryInput { latest_speed: sample, maximum_speed: maximum, input_length, direction_dot,
            fast_history: self.fast, since_fast: self.since_fast, since_turn: self.since_turn,
            ground_time: self.ground_time, enabled: true, blocked: false };
        let before = self.life.action;
        self.life.enter(&self.settings, &input, self.fast_direction, |e| effects.push(e));
        if std::env::var_os("BLOTWORX_MOVES_LOG").is_some() && !effects.is_empty() {
            eprintln!("[moves] {:?} -> {:?} speed {speed:.2} input {input_length:.2} dot {direction_dot:.2} fast {} since_fast {:.2} ground {:.2}: {:?}",
                before, self.life.action, self.fast, self.since_fast, self.ground_time, effects);
        }
        for effect in effects {
            match effect {
                Effect::Animation(name) => self.events.push(name),
                Effect::ResetTurnTimer => self.since_turn = 0.,
                Effect::GameEvent(..) => {}
                other => { let _ = player.apply_action_motion(other); }
            }
        }
    }
}

/// REL 0x2229dc..0x222a18: motion+0x213 permits a negative speed
/// when the current facing (in the support frame) opposes desired input.
/// This branch is bypassed entirely when CharacterMaintainVelocity is set.
/// The caller supplies the dot after both vectors enter the same frame.
pub fn signed_ground_speed(speed: f32, facing_dot_input: f32, allow_reverse: bool) -> f32 {
    if allow_reverse && facing_dot_input < 0. {
        speed * -1.0
    } else {
        speed
    }
}

/// REL 0x97b84..0x97bd4 rotates the stored target about +Y by
/// (remaining / The180Time) * pi. DOL 0x8014bea8 builds a quaternion
/// then a matrix; preserve that construction rather than interpolating yaw.
/// Input is already the native pre-decrement fraction returned by advance.
pub fn turn_facing(target: [f32; 3], fraction: f32) -> [f32; 3] {
    let angle = fraction.mul_add(std::f32::consts::PI, 0.);
    let half = 0.5f32 * angle;
    let s = (half as f64).sin() as f32;
    let c = (half as f64).cos() as f32;
    let twice_s = s + s;
    let cosine = 1.0 - twice_s * s;
    let sine = twice_s * c;
    [
        sine.mul_add(target[2], cosine * target[0]),
        target[1],
        cosine.mul_add(target[2], -sine * target[0]),
    ]
}

/// Authored entry properties. No guessed defaults: the importer must supply
/// the values from the resolved Mickey motion component.
#[derive(Clone, Debug, serde::Serialize)]
pub struct EntrySettings {
    pub skid_max_input: f32,
    pub skid_min_speed: f32,
    pub skid_time_to_check: f32,
    pub turn_min_input: f32,
    pub turn_min_speed: f32,
    pub turn_time_to_check: f32,
    pub turn_cosine: f32,
    pub turn_min_interval: f32,
    pub min_ground_time: f32,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct Settings {
    pub entry: EntrySettings,
    pub skid_duration: f32,
    pub turn_duration: f32,
    pub turn_deceleration: f32,
}
impl Settings {
    /// Read resolved component properties, preserving authored values rather
    /// than substituting schema examples for missing data.
    pub fn load<'a>(
        property: impl Fn(&str) -> Option<&'a serde_json::Value>,
    ) -> anyhow::Result<Self> {
        let number = |name: &str| -> anyhow::Result<f32> {
            let value = property(name)
                .and_then(serde_json::Value::as_f64)
                .ok_or_else(|| {
                    anyhow::anyhow!("Missing or nonnumeric Mickey locomotion property {name}")
                })?;
            let value = value as f32;
            anyhow::ensure!(
                value.is_finite(),
                "Non-finite Mickey locomotion property {name}"
            );
            Ok(value)
        };
        Ok(Self {
            entry: EntrySettings {
                skid_max_input: number("SkidMaxInput")?,
                skid_min_speed: number("SkidMinSpeed")?,
                skid_time_to_check: number("SkidTimeToCheck")?,
                turn_min_input: number("The180MinInput")?,
                turn_min_speed: number("The180MinSpeed")?,
                turn_time_to_check: number("The180TimeToCheck")?,
                turn_cosine: number("The180Cosine")?,
                turn_min_interval: number("The180MinTimeBetween180s")?,
                min_ground_time: number("The180OrSkidMinTimeOnGround")?,
            },
            skid_duration: number("SkidTime")?,
            turn_duration: number("The180Time")?,
            turn_deceleration: number("The180DecelRate")?,
        })
    }
}
pub struct EntryInput {
    pub latest_speed: Option<f32>,
    pub maximum_speed: f32,
    pub input_length: f32,
    /// Dot of the stored fast-history direction and normalized desired input.
    pub direction_dot: f32,
    pub fast_history: bool,
    pub since_fast: f32,
    pub since_turn: f32,
    pub ground_time: f32,
    pub enabled: bool,
    pub blocked: bool,
}

/// REL 0x97cbc..0x97e7c evaluates skid first, then turn against the updated
/// action state. Return both entries in dispatch order when authored settings
/// permit both; do not silently turn the native two tests into an either/or.
pub fn entries(action: Action, s: &EntrySettings, i: &EntryInput) -> [Option<Action>; 2] {
    let mut result = [None, None];
    if i.blocked || !(i.ground_time > s.min_ground_time) || !i.enabled {
        return result;
    }
    let Some(speed) = i.latest_speed else {
        return result;
    };
    if i.maximum_speed <= 0. {
        return result;
    }
    let mut current = action;
    if current == Action::None
        && i.input_length <= s.skid_max_input
        && speed >= s.skid_min_speed * i.maximum_speed
        && (i.fast_history || i.since_fast < s.skid_time_to_check)
    {
        result[0] = Some(Action::Skid);
        current = Action::Skid;
    }
    if current != Action::Turn
        && i.since_turn >= s.turn_min_interval
        && i.input_length >= s.turn_min_input
        && (current == Action::Skid || speed >= s.turn_min_speed * i.maximum_speed)
        && (i.fast_history || i.since_fast < s.turn_time_to_check)
        && i.direction_dot < s.turn_cosine
    {
        result[1] = Some(Action::Turn);
    }
    result
}

/// Ground-only action update from REL 0x97a64..0x97c8c. Turning returns
/// remaining/duration for the native rotation from the stored target direction.
/// Completion is checked before subtraction; overshoot is not clamped.
pub fn advance(
    action: &mut Action,
    remaining: &mut f32,
    character_state: u32,
    latest_speed: f32,
    turn_duration: f32,
    dt: f32,
) -> Update {
    if character_state != 0 || *action == Action::None {
        return Update::Inactive;
    }
    let update = match *action {
        Action::Turn if *remaining <= 0. => Update::Finished,
        Action::Turn => Update::Turning(*remaining / turn_duration),
        Action::Skid if *remaining <= 0. && latest_speed < 0.01 => Update::Finished,
        Action::Skid => Update::Skidding,
        Action::None => unreachable!(),
    };
    if update == Update::Finished {
        *action = Action::None;
    }
    *remaining -= dt;
    update
}

/// REL 0x9971c..0x99830 updates motion+0x3f0 from the latest ground sample.
/// A true result also resets timer+0x3b0 to zero and copies that sample's
/// direction to +0x3c0. No sample means no writes, including no flag reset.
pub fn refresh_fast_history(
    flag: &mut bool,
    character_state: u32,
    latest_speed: Option<f32>,
    maximum_speed: f32,
) -> bool {
    let Some(speed) = latest_speed else {
        return false;
    };
    if character_state != 0 {
        *flag = false;
    }
    let threshold = 0.95f32 * maximum_speed; // REL constant 0x4ac7a8
    if speed < threshold && *flag {
        *flag = false;
    } else if speed >= threshold && !*flag {
        *flag = true;
    }
    *flag
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn action_history_uses_scaled_time_and_refreshes_each_fast_sample() {
        use super::super::game_clock::{Clock, Rate, Step};
        let mut clock = Clock::new(Rate::from_bus_clock(4000).unwrap(), 100);
        let mut history = ActionHistory::default();
        history.initialize_timers(&clock);
        let mut sample = super::super::ground_history::Sample {
            speed: 5.,
            direction: [1., 0., 0.],
            ..Default::default()
        };
        clock.sample(Step::Realtime, 200, 0.5);
        history.refresh(&clock, 0, Some(&sample), 5.);
        assert_eq!(history.since_fast(&clock), 0.);
        assert_eq!(history.since_turn(&clock), 0.05);
        clock.sample(Step::Frozen, 1200, 1.);
        assert_eq!(history.since_turn(&clock), 0.05);
        clock.sample(Step::Realtime, 1300, 0.5);
        sample.direction = [0., 0., 1.];
        history.refresh(&clock, 0, Some(&sample), 5.);
        assert_eq!(history.direction, sample.direction);
        assert_eq!(history.since_fast(&clock), 0.);
        history.refresh(&clock, 2, None, 5.);
        assert!(history.fast, "missing samples do not mutate native history");
        sample.speed = 0.;
        history.refresh(&clock, 0, Some(&sample), 5.);
        assert!(!history.fast);
        clock.sample(Step::Realtime, 1400, 1.);
        assert_eq!(history.since_fast(&clock), 0.1);
        assert_eq!(history.apply(&clock, Effect::ResetTurnTimer), None);
        assert_eq!(history.since_turn(&clock), 0.);
        assert_eq!(
            history.apply(&clock, Effect::ResetMotion),
            Some(Effect::ResetMotion)
        );
    }
    #[test]
    fn clock_seconds_matches_native_subtraction_and_division_branches() {
        let rate = std::num::NonZeroU32::new(7).unwrap();
        for ticks in -300i64..=300 {
            let mut remaining = ticks;
            let mut whole = 0i32;
            while remaining > 7 {
                remaining -= 7;
                whole += 1;
            }
            while remaining < -7 {
                remaining += 7;
                whole -= 1;
            }
            let expected = whole as f32 + (remaining as f32 / 7.);
            assert_eq!(
                ticks_to_seconds(ticks, rate, 1000).to_bits(),
                expected.to_bits()
            );
            let expected = (ticks / 7) as i32 as f32 + ((ticks % 7) as f32 / 7.);
            assert_eq!(
                ticks_to_seconds(ticks, rate, 0).to_bits(),
                expected.to_bits()
            );
        }
        for ticks in [i64::MIN, i64::MAX] {
            let expected = (ticks / 7) as i32 as f32 + ((ticks % 7) as f32 / 7.);
            assert_eq!(ticks_to_seconds(ticks, rate, 1000), expected);
        }
    }
    #[test]
    fn clock_stamp_preserves_zero_reset_signed_elapsed_and_large_clock_precision() {
        let mut timer = ClockStamp::default();
        assert_eq!(timer.elapsed_ticks(0), 0);
        timer.reset(0);
        assert_eq!(timer.elapsed_ticks(0), -1);
        assert_eq!(timer.elapsed_ticks(1), 0);
        let large = (1u64 << 54) + 13;
        timer.reset(large);
        assert_eq!(timer.elapsed_ticks(large + 1), 1);
        assert_eq!(timer.elapsed_ticks(large - 1), -1);
        timer.reset(u64::MAX);
        assert_eq!(timer.elapsed_ticks(0), 1);
    }
    #[test]
    fn lifecycle_keeps_entry_dispatch_order_and_finishes_on_ground_only() {
        let mut s = Settings {
            entry: settings(),
            skid_duration: 0.2,
            turn_duration: 0.4,
            turn_deceleration: 12.,
        };
        s.entry.turn_min_input = 0.1;
        let mut state = Lifecycle::default();
        let mut effects = vec![];
        state.enter(&s, &input(), [0., 0., 1.], |e| effects.push(e));
        assert_eq!(
            effects,
            vec![
                Effect::Animation("EVENT_Start_Run_Skid"),
                Effect::BeginSkid,
                Effect::GameEvent(0x2b, 0.),
                Effect::Animation("EVENT_Start_180_Turn"),
                Effect::BeginTurn {
                    direction: [0., 0., 1.],
                    stopping_acceleration: 12.
                }
            ]
        );
        assert_eq!(state.action, Action::Turn);
        effects.clear();
        state.update(2, 0., &s, 1., |e| effects.push(e));
        assert!(effects.is_empty());
        assert_eq!(state.remaining, 0.4);
        state.update(0, 0., &s, 0.5, |e| effects.push(e));
        assert!(matches!(effects.as_slice(), [Effect::Facing(_)]));
        effects.clear();
        state.update(0, 0., &s, 0.1, |e| effects.push(e));
        assert!(matches!(
            effects.as_slice(),
            [
                Effect::Facing(_),
                Effect::ResetTurnTimer,
                Effect::Animation("EVENT_Stop_Run_Skid"),
                Effect::ResetMotion
            ]
        ));
        assert_eq!(state.action, Action::None);
        assert!(state.remaining < 0.);
    }
    #[test]
    fn silent_forced_reset_restores_motion_without_inventing_timer_reset() {
        let mut state = Lifecycle {
            action: Action::Skid,
            remaining: 0.17,
            target: [0.; 3],
        };
        let mut effects = vec![];
        state.reset(false, |e| effects.push(e));
        assert_eq!(effects, vec![Effect::ResetMotion]);
        assert_eq!(state.action, Action::None);
        assert_eq!(state.remaining, 0.17);
    }
    #[test]
    fn turn_facing_follows_fixed_positive_y_arc_without_clamping_time() {
        let target = [0., 0., -1.];
        for (fraction, expected) in [
            (1., [0., 0., 1.]),
            (0.5, [-1., 0., 0.]),
            (0., target),
            (1.5, [1., 0., 0.]),
        ] {
            let actual = turn_facing(target, fraction);
            assert!((0..3).all(|i| (actual[i] - expected[i]).abs() < 1e-6));
        }
        assert_eq!(turn_facing([0., 2., -1.], 0.5)[1], 2.);
    }
    #[test]
    fn reversal_sign_uses_input_not_velocity_and_strict_negative_dot() {
        assert_eq!(signed_ground_speed(3., -1., true), -3.);
        assert_eq!(signed_ground_speed(3., -1., false), 3.);
        assert_eq!(signed_ground_speed(3., 0., true), 3.);
        assert_eq!(signed_ground_speed(3., -0., true), 3.);
        assert_eq!(signed_ground_speed(3., 1., true), 3.);
        assert_eq!(signed_ground_speed(3., -f32::MIN_POSITIVE, true), -3.);
    }
    #[test]
    fn authored_settings_preserve_values_and_reject_missing_or_overflowing_data() {
        let mut values = serde_json::json!({
            "SkidMaxInput":0.1,"SkidMinSpeed":0.4,"SkidTimeToCheck":0.09999,
            "The180MinInput":0.225,"The180MinSpeed":0.2,"The180TimeToCheck":0.16666,
            "The180Cosine":-0.6,"The180MinTimeBetween180s":0.09999,
            "The180OrSkidMinTimeOnGround":0.09999,"SkidTime":0.2,
            "The180Time":0.39999,"The180DecelRate":12.
        });
        let loaded = Settings::load(|name| values.get(name)).unwrap();
        assert_eq!(loaded.entry.turn_cosine, -0.6f32);
        assert_eq!(loaded.turn_duration, 0.39999f32);
        assert_eq!(loaded.turn_deceleration, 12.);
        values["The180Time"] = serde_json::json!(0.);
        assert_eq!(
            Settings::load(|name| values.get(name))
                .unwrap()
                .turn_duration,
            0.
        );
        values["The180DecelRate"] = serde_json::json!(1e100);
        assert!(Settings::load(|name| values.get(name))
            .unwrap_err()
            .to_string()
            .contains("The180DecelRate"));
        values.as_object_mut().unwrap().remove("SkidMaxInput");
        assert!(Settings::load(|name| values.get(name))
            .unwrap_err()
            .to_string()
            .contains("SkidMaxInput"));
    }
    fn settings() -> EntrySettings {
        EntrySettings {
            skid_max_input: 0.1,
            skid_min_speed: 0.4,
            skid_time_to_check: 0.1,
            turn_min_input: 0.225,
            turn_min_speed: 0.2,
            turn_time_to_check: 0.16,
            turn_cosine: -0.6,
            turn_min_interval: 0.1,
            min_ground_time: 0.1,
        }
    }
    fn input() -> EntryInput {
        EntryInput {
            latest_speed: Some(2.),
            maximum_speed: 5.,
            input_length: 0.1,
            direction_dot: -1.,
            fast_history: false,
            since_fast: 0.05,
            since_turn: 1.,
            ground_time: 1.,
            enabled: true,
            blocked: false,
        }
    }
    #[test]
    fn entry_gates_preserve_authored_threshold_boundaries() {
        let s = settings();
        let mut i = input();
        assert_eq!(entries(Action::None, &s, &i), [Some(Action::Skid), None]);
        i.ground_time = s.min_ground_time;
        assert_eq!(entries(Action::None, &s, &i), [None, None]);
        i.ground_time = 1.;
        i.since_fast = s.skid_time_to_check;
        assert_eq!(entries(Action::None, &s, &i), [None, None]);
        i.fast_history = true;
        assert_eq!(entries(Action::None, &s, &i), [Some(Action::Skid), None]);
        i.enabled = false;
        assert_eq!(entries(Action::None, &s, &i), [None, None]);
    }
    #[test]
    fn turn_requires_angle_and_cooldown_but_skid_bypasses_speed_gate() {
        let s = settings();
        let mut i = input();
        i.input_length = s.turn_min_input;
        i.since_turn = s.turn_min_interval;
        i.latest_speed = Some(0.);
        assert_eq!(entries(Action::None, &s, &i), [None, None]);
        assert_eq!(entries(Action::Skid, &s, &i), [None, Some(Action::Turn)]);
        i.direction_dot = s.turn_cosine;
        assert_eq!(entries(Action::Skid, &s, &i), [None, None]);
    }
    #[test]
    fn skid_entry_can_enable_turn_in_the_same_update() {
        let mut s = settings();
        let i = input();
        s.turn_min_input = 0.1;
        s.turn_min_speed = 0.9;
        assert_eq!(
            entries(Action::None, &s, &i),
            [Some(Action::Skid), Some(Action::Turn)]
        );
    }
    #[test]
    fn fast_history_retains_exact_threshold_and_missing_sample_rules() {
        let mut flag = true;
        assert!(!refresh_fast_history(&mut flag, 2, None, 5.));
        assert!(flag, "no history must not mutate the latch");
        assert!(!refresh_fast_history(&mut flag, 0, Some(4.7), 5.));
        assert!(!flag);
        assert!(refresh_fast_history(&mut flag, 0, Some(4.75), 5.));
        // The state reset happens before the speed comparison. Do not turn it
        // into an early airborne return: the native routine can set it again.
        assert!(refresh_fast_history(&mut flag, 2, Some(4.75), 5.));
        assert!(!refresh_fast_history(&mut flag, 2, Some(0.), 5.));
    }
    #[test]
    fn skid_needs_both_elapsed_time_and_stopped_motion() {
        let mut action = Action::Skid;
        let mut remaining = 0.02;
        assert_eq!(
            advance(&mut action, &mut remaining, 0, 0., 0.4, 0.03),
            Update::Skidding
        );
        assert!(remaining < 0.);
        assert_eq!(
            advance(&mut action, &mut remaining, 0, 0.01, 0.4, 0.03),
            Update::Skidding
        );
        assert_eq!(
            advance(&mut action, &mut remaining, 0, 0.009, 0.4, 0.03),
            Update::Finished
        );
        assert_eq!(action, Action::None);
    }
    #[test]
    fn turn_uses_predecrement_fraction_and_pauses_outside_ground_state() {
        let mut action = Action::Turn;
        let mut remaining = 0.1;
        assert_eq!(
            advance(&mut action, &mut remaining, 2, 5., 0.4, 0.2),
            Update::Inactive
        );
        assert_eq!(remaining, 0.1);
        assert_eq!(
            advance(&mut action, &mut remaining, 0, 5., 0.4, 0.2),
            Update::Turning(0.25)
        );
        assert_eq!(
            advance(&mut action, &mut remaining, 0, 5., 0.4, 0.2),
            Update::Finished
        );
        assert_eq!(action, Action::None);
        assert!(remaining < -0.2);
    }
}
