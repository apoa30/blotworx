//! EM2 gameplay follow camera for the shared playtest (EM2 Studio).
//!
//! Behaviour measured frame by frame from recordings of the original PC game (as worked out in
//! the dem2-game fan port): the camera is framed on the ground Mickey last stood on and does not
//! follow him up a jump; it only rises once his head nears the top of the screen, and eases back
//! down as he falls. All vertical movement goes through one critically damped spring. It looks
//! at a point above his head (so he sits in the lower middle of the frame) and comes in closer
//! when level geometry is between him and the camera.

/// Mickey's height in level units.
const HEIGHT: f32 = 1.0;
/// How far up the screen (0 = middle, 1 = top edge) his head may go before the camera rises.
const TOP_MARGIN: f32 = 0.8;
const FRAME_TIME: f32 = 0.12;
const SETTLE_TIME: f32 = 0.25;
const LEDGE_TIME: f32 = 0.35;
const GROUND_TIME: f32 = 0.15;
const FALL_TIME: f32 = 0.03;
/// The original's default framing: 5 units back.
pub const DISTANCE: f32 = 5.0;

#[derive(Debug, Default)]
pub struct FollowCamera {
    focus_y: Option<f32>,
    lift: f32,
    lift_speed: f32,
    takeoff_y: f32,
    distance: Option<f32>,
}

fn camera_ground(ground: f32, y: f32, floor: Option<f32>, grounded: bool) -> f32 {
    if grounded || y < ground || y - ground > 8.0 { y }
    else if let Some(floor) = floor.filter(|f| *f > ground + 0.05) { floor.min(y) }
    else { ground }
}
fn headroom(dir_y: f32, dist: f32, half_fov: f32) -> f32 {
    let slope = TOP_MARGIN * half_fov.tan();
    let across = (1.0 - dir_y * dir_y).max(0.0).sqrt() + slope * dir_y;
    if across > 0.05 { slope * dist / across } else { f32::INFINITY }
}
fn smooth_damp(x: f32, speed: f32, target: f32, time: f32, dt: f32) -> (f32, f32) {
    let omega = 2.0 / time;
    let k = omega * dt;
    let decay = 1.0 / (1.0 + k + 0.48 * k * k + 0.235 * k * k * k);
    let change = x - target;
    let temp = (speed + omega * change) * dt;
    let (next, speed) = (target + (change + temp) * decay, (speed - omega * temp) * decay);
    if (target - x > 0.0) == (next > target) { (target, 0.0) } else { (next, speed) }
}

impl FollowCamera {
    pub fn reset(&mut self) { *self = Self::default(); }
    /// Look-at point and distance for this frame. `dir_y` is the upward part of the unit vector
    /// from the look-at point to the camera; `floor` finds the ground below a point (within a
    /// distance); `blocked` returns how far along the view ray (from the look-at point towards
    /// the camera) geometry is hit, if it is within `dist`.
    #[allow(clippy::too_many_arguments)]
    pub fn update(&mut self, position: [f32; 3], grounded: bool, dir_y: f32, half_fov: f32, dt: f32,
        floor: impl Fn([f32; 3], f32) -> Option<f32>, blocked: impl Fn([f32; 3], f32) -> Option<f32>) -> ([f32; 3], f32) {
        let dt = dt.clamp(0.0, 0.1);
        let y = position[1];
        let first = self.focus_y.is_none();
        let was = self.focus_y.unwrap_or(y);
        let below = (!grounded && y > was).then(|| floor([position[0], y + 0.3, position[2]], y - was + 0.3)).flatten();
        let dropping = !grounded && y < was;
        let ground = camera_ground(was, y, below, grounded);
        self.focus_y = Some(ground);
        if first { self.lift = ground; self.lift_speed = 0.0; }
        if first || grounded { self.takeoff_y = y; }
        let room = headroom(dir_y, DISTANCE, half_fov);
        let framed = y + HEIGHT - HEIGHT * 1.6 - room;
        let keeping = y > self.takeoff_y && framed > ground;
        let target_h = if keeping { framed } else { ground };
        let time = if dropping { FALL_TIME } else if grounded { GROUND_TIME } else if target_h < self.lift { SETTLE_TIME } else if keeping { FRAME_TIME } else { LEDGE_TIME };
        (self.lift, self.lift_speed) = smooth_damp(self.lift, self.lift_speed, target_h, time, dt);
        let look = [position[0], self.lift + HEIGHT * 1.6, position[2]];
        // come in closer when the view is blocked (but not for a ledge the camera is rising clear of)
        let mut want = DISTANCE;
        if let Some(now) = blocked(look, DISTANCE) {
            let rising = ground > self.lift + 0.05;
            let then = if rising { blocked([look[0], look[1] + (ground - self.lift), look[2]], DISTANCE) } else { Some(now) };
            want = then.map_or(DISTANCE, |then| (now.max(then) - 0.15).max(0.3));
        }
        let d = self.distance.unwrap_or(want);
        let d = d + (want - d) * (15.0 * dt).min(1.0);
        self.distance = Some(d);
        (look, d)
    }
}
