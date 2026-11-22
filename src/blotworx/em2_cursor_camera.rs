//! EM2 PC mouse camera: the cursor moves freely inside a box in the middle of the screen,
//! and mouse movement that would carry it past the box's edge turns the camera instead.
//!
//! Values come from the game's own ConfigFiles.ini (PC build), read at startup:
//!   PlayerMouseSteering                      steering by mouse on/off
//!   Input_MouseCursor_SafeZoneX / _Y         size of the box
//!   Input_MouseCursor_SafeZoneSpeedX / _Y    recorded; meaning not confirmed (see below)
//!   Input_MouseCursor_SafeZoneNoInputReset   seconds without mouse input before the cursor recentres
//!   [FreeCamera_Limits] CameraMinPitch/CameraMaxPitch   camera pitch range (degrees)
//!
//! Confirmed from data: every number above. Not confirmed (the logic is in the executable):
//!   - the box is taken as SafeZone x the half-screen on each side of the centre
//!     (0.5 = the middle half of the screen);
//!   - the turn per pixel past the edge is the fan port's measurement from recordings of the
//!     original (0.0025 rad/px), not derived from SafeZoneSpeed, whose unit is unknown.

const TURN_PER_PIXEL: f32 = 0.0025;

#[derive(Debug, Clone, serde::Serialize)]
pub(super) struct Settings {
    pub steering: bool,
    pub safe_zone: [f32; 2],
    pub safe_zone_speed: [f32; 2],
    pub no_input_reset: f32,
    /// Degrees: camera below (negative) / above (positive) the player.
    pub pitch_degrees: [f32; 2],
}
impl Default for Settings {
    fn default() -> Self {
        Self { steering: true, safe_zone: [0.5, 0.5], safe_zone_speed: [0.02, 0.02], no_input_reset: 8.0, pitch_degrees: [-40.0, 75.0] }
    }
}
impl Settings {
    /// Read from ConfigFiles.ini in the game folder; anything missing keeps the retail value.
    pub fn load(root: &std::path::Path) -> Self {
        let mut s = Self::default();
        let Ok(text) = std::fs::read_to_string(root.join("ConfigFiles.ini")) else { return s };
        let mut section = String::new();
        for line in text.lines() {
            let line = line.trim();
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = name.to_owned();
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            let number = value.parse::<f32>().ok().filter(|v| v.is_finite());
            match (section.as_str(), key) {
                (_, "PlayerMouseSteering") => s.steering = value.eq_ignore_ascii_case("true"),
                (_, "Input_MouseCursor_SafeZoneX") => if let Some(v) = number { s.safe_zone[0] = v },
                (_, "Input_MouseCursor_SafeZoneY") => if let Some(v) = number { s.safe_zone[1] = v },
                (_, "Input_MouseCursor_SafeZoneSpeedX") => if let Some(v) = number { s.safe_zone_speed[0] = v },
                (_, "Input_MouseCursor_SafeZoneSpeedY") => if let Some(v) = number { s.safe_zone_speed[1] = v },
                (_, "Input_MouseCursor_SafeZoneNoInputReset") => if let Some(v) = number { s.no_input_reset = v },
                ("FreeCamera_Limits", "CameraMinPitch") => if let Some(v) = number { s.pitch_degrees[0] = v },
                ("FreeCamera_Limits", "CameraMaxPitch") => if let Some(v) = number { s.pitch_degrees[1] = v },
                _ => {}
            }
        }
        s
    }
    pub fn pitch_limits(&self) -> [f32; 2] {
        [self.pitch_degrees[0].to_radians(), self.pitch_degrees[1].to_radians()]
    }
}

#[derive(Debug, Default)]
pub(super) struct CursorCamera {
    pub settings: Settings,
    /// Cursor position, -1..1 from the centre to the screen edge (y up).
    pub cursor: [f32; 2],
    idle: f32,
}
impl CursorCamera {
    pub fn new(settings: Settings) -> Self {
        Self { settings, ..Default::default() }
    }
    /// Move the cursor by a raw mouse movement (`delta` in pixels, y down) on a screen whose
    /// half-size is `half` pixels. Returns the camera turn [yaw, pitch] in radians for the part
    /// of the movement that didn't fit inside the box (pitch positive = camera moves up).
    pub fn update(&mut self, delta: [f32; 2], half: [f32; 2], dt: f32) -> [f32; 2] {
        let half = [half[0].max(1.), half[1].max(1.)];
        if delta == [0.; 2] {
            self.idle += dt;
            if self.idle >= self.settings.no_input_reset {
                self.cursor = [0.; 2];
            }
            return [0.; 2];
        }
        self.idle = 0.;
        let want = [self.cursor[0] + delta[0] / half[0], self.cursor[1] - delta[1] / half[1]];
        let zone = self.settings.safe_zone;
        let kept = [want[0].clamp(-zone[0], zone[0]), want[1].clamp(-zone[1], zone[1])];
        self.cursor = kept;
        if !self.settings.steering {
            return [0.; 2];
        }
        // overflow in pixels (x right, y up)
        let over = [(want[0] - kept[0]) * half[0], (want[1] - kept[1]) * half[1]];
        [-over[0] * TURN_PER_PIXEL, -over[1] * TURN_PER_PIXEL]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn moves_inside_the_box_then_turns() {
        let mut c = CursorCamera::new(Settings::default());
        let half = [640., 400.];
        // a small move only moves the cursor
        assert_eq!(c.update([64., 0.], half, 0.016), [0., 0.]);
        assert!((c.cursor[0] - 0.1).abs() < 1e-6);
        // pushing past the right edge (0.5) turns by the leftover pixels
        let turn = c.update([320., 0.], half, 0.016);
        assert!((c.cursor[0] - 0.5).abs() < 1e-6);
        assert!((turn[0] + 64. * TURN_PER_PIXEL).abs() < 1e-5, "{turn:?}");
        // moving back comes off the edge without turning
        assert_eq!(c.update([-64., 0.], half, 0.016), [0., 0.]);
        // eight seconds without input recentres it
        for _ in 0..600 { c.update([0., 0.], half, 1. / 60.); }
        assert_eq!(c.cursor, [0., 0.]);
    }
}
