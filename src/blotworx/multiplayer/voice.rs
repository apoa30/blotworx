//! Real-time proximity voice chat: 16 kHz mono G.711 µ-law codec, Windows `winmm`
//! microphone capture (`waveIn*`), and 3D spatialized stereo voice playback (`waveOut*`).

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

pub const VOICE_SAMPLE_RATE: u32 = 16_000;
pub const VOICE_FRAME_SAMPLES: usize = 320; // 20 ms at 16 kHz -> 320 bytes µ-law (16 KB/s)
pub const DEFAULT_PROXIMITY_MAX_DIST: f32 = 28.0;
pub const PROXIMITY_FULL_DIST: f32 = 4.0;

/// ITU-T G.711 µ-law encode (linear i16 -> u8).
#[inline]
pub fn linear_to_ulaw(sample: i16) -> u8 {
    const BIAS: i32 = 0x84;
    const CLIP: i32 = 32635;
    let mut pcm = sample as i32;
    let sign = if pcm < 0 {
        pcm = -pcm;
        0x80u8
    } else {
        0x00u8
    };
    if pcm > CLIP {
        pcm = CLIP;
    }
    pcm += BIAS;
    let mut exponent = 7u8;
    let mut exp_mask = 0x4000;
    while exponent > 0 && (pcm & exp_mask) == 0 {
        exponent -= 1;
        exp_mask >>= 1;
    }
    let mantissa = ((pcm >> (exponent + 3)) & 0x0f) as u8;
    !(sign | (exponent << 4) | mantissa)
}

/// ITU-T G.711 µ-law decode (u8 -> linear i16).
#[inline]
pub fn ulaw_to_linear(u_val: u8) -> i16 {
    let u = !u_val;
    let t = (((u & 0x0f) as i32) << 3) + 0x84;
    let exp = ((u & 0x70) >> 4) as i32;
    let val = (t << exp) - 0x84;
    if (u & 0x80) != 0 {
        (-val) as i16
    } else {
        val as i16
    }
}

pub fn encode_ulaw_frame(pcm: &[i16]) -> Vec<u8> {
    pcm.iter().map(|&s| linear_to_ulaw(s)).collect()
}

pub fn decode_ulaw_frame(ulaw: &[u8]) -> Vec<i16> {
    ulaw.iter().map(|&b| ulaw_to_linear(b)).collect()
}

#[derive(Clone, Copy, Debug)]
pub struct ListenerPose {
    pub position: [f32; 3],
    pub right: [f32; 3],
}

impl Default for ListenerPose {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0, 0.0],
            right: [1.0, 0.0, 0.0],
        }
    }
}

/// Compute stereo `(left_gain, right_gain)` for a speaker at `speaker_pos` relative to `listener`.
pub fn proximity_stereo_gains(
    listener: ListenerPose,
    speaker_pos: [f32; 3],
    max_distance: f32,
    master_volume: f32,
) -> (f32, f32) {
    let dx = speaker_pos[0] - listener.position[0];
    let dy = speaker_pos[1] - listener.position[1];
    let dz = speaker_pos[2] - listener.position[2];
    let dist = (dx * dx + dy * dy + dz * dz).sqrt();
    let max_d = max_distance.max(PROXIMITY_FULL_DIST + 1.0);
    if dist >= max_d {
        return (0.0, 0.0);
    }
    let atten = if dist <= PROXIMITY_FULL_DIST {
        1.0
    } else {
        let t = ((dist - PROXIMITY_FULL_DIST) / (max_d - PROXIMITY_FULL_DIST)).clamp(0.0, 1.0);
        // Smooth quadratic proximity rolloff
        (1.0 - t) * (1.0 - t)
    };
    let gain = atten * master_volume.clamp(0.0, 2.0);
    if dist < 0.25 {
        return (gain, gain);
    }
    let inv = 1.0 / dist;
    let pan = (dx * inv * listener.right[0]
        + dy * inv * listener.right[1]
        + dz * inv * listener.right[2])
        .clamp(-1.0, 1.0);
    // Equal-power-ish stereo pan (keeps center warm and avoids hard 0 on opposite ear)
    let left = ((1.0 - pan * 0.72) * 0.5 + 0.5).clamp(0.22, 1.0) * gain;
    let right = ((1.0 + pan * 0.72) * 0.5 + 0.5).clamp(0.22, 1.0) * gain;
    (left, right)
}

#[derive(Clone, Debug)]
pub struct VoiceSettings {
    pub enabled: bool,
    pub push_to_talk: bool,
    pub mic_muted: bool,
    pub deafened: bool,
    pub mic_gain: f32,
    pub voice_volume: f32,
    pub vad_threshold: f32,
    pub proximity_range: f32,
    pub loopback_test: bool,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            push_to_talk: true,
            mic_muted: false,
            deafened: false,
            mic_gain: 1.25,
            voice_volume: 1.15,
            vad_threshold: 0.025,
            proximity_range: DEFAULT_PROXIMITY_MAX_DIST,
            loopback_test: false,
        }
    }
}

struct SpeakerQueue {
    samples: VecDeque<i16>,
    pos: [f32; 3],
    muted: bool,
}

struct SharedVoice {
    listener: Mutex<ListenerPose>,
    speakers: Mutex<HashMap<u8, SpeakerQueue>>,
    captured: Mutex<VecDeque<Vec<u8>>>,
    settings: Mutex<VoiceSettings>,
    ptt_held: AtomicBool,
    speaking_now: AtomicBool,
    mic_level_bits: AtomicU32,
    mic_available: AtomicBool,
    running: AtomicBool,
}

pub struct VoiceEngine {
    shared: Arc<SharedVoice>,
    capture_thread: Option<thread::JoinHandle<()>>,
    playback_thread: Option<thread::JoinHandle<()>>,
}

impl VoiceEngine {
    pub fn start(initial: VoiceSettings) -> Self {
        let shared = Arc::new(SharedVoice {
            listener: Mutex::new(ListenerPose::default()),
            speakers: Mutex::new(HashMap::new()),
            captured: Mutex::new(VecDeque::new()),
            settings: Mutex::new(initial),
            ptt_held: AtomicBool::new(false),
            speaking_now: AtomicBool::new(false),
            mic_level_bits: AtomicU32::new(0),
            mic_available: AtomicBool::new(false),
            running: AtomicBool::new(true),
        });

        #[cfg(windows)]
        let capture_thread = {
            let s = shared.clone();
            thread::Builder::new()
                .name("blotworx-voice-in".into())
                .spawn(move || winmm_voice::capture_loop(s))
                .ok()
        };
        #[cfg(not(windows))]
        let capture_thread = None;

        #[cfg(windows)]
        let playback_thread = {
            let s = shared.clone();
            thread::Builder::new()
                .name("blotworx-voice-out".into())
                .spawn(move || winmm_voice::playback_loop(s))
                .ok()
        };
        #[cfg(not(windows))]
        let playback_thread = None;

        Self {
            shared,
            capture_thread,
            playback_thread,
        }
    }

    pub fn update_settings(&self, settings: &VoiceSettings) {
        if let Ok(mut g) = self.shared.settings.lock() {
            *g = settings.clone();
        }
    }

    pub fn set_ptt_held(&self, held: bool) {
        self.shared.ptt_held.store(held, Ordering::Release);
    }

    pub fn update_listener(&self, pose: ListenerPose) {
        if let Ok(mut g) = self.shared.listener.lock() {
            *g = pose;
        }
    }

    pub fn set_player_muted(&self, player_id: u8, muted: bool) {
        if let Ok(mut map) = self.shared.speakers.lock() {
            map.entry(player_id)
                .or_insert_with(|| SpeakerQueue {
                    samples: VecDeque::new(),
                    pos: [0.0; 3],
                    muted,
                })
                .muted = muted;
        }
    }

    pub fn push_remote_voice(&self, sender_id: u8, pos: [f32; 3], ulaw: &[u8]) {
        if let Ok(mut map) = self.shared.speakers.lock() {
            let entry = map.entry(sender_id).or_insert_with(|| SpeakerQueue {
                samples: VecDeque::with_capacity(VOICE_FRAME_SAMPLES * 8),
                pos,
                muted: false,
            });
            entry.pos = pos;
            if entry.muted {
                return;
            }
            for &b in ulaw {
                entry.samples.push_back(ulaw_to_linear(b));
            }
            // Cap jitter buffer at 240 ms so latency never drifts
            let max_buf = VOICE_FRAME_SAMPLES * 12;
            while entry.samples.len() > max_buf {
                entry.samples.pop_front();
            }
        }
    }

    pub fn drain_captured_frames(&self) -> Vec<Vec<u8>> {
        if let Ok(mut q) = self.shared.captured.lock() {
            q.drain(..).collect()
        } else {
            Vec::new()
        }
    }

    pub fn is_speaking(&self) -> bool {
        self.shared.speaking_now.load(Ordering::Acquire)
    }

    pub fn mic_level(&self) -> f32 {
        f32::from_bits(self.shared.mic_level_bits.load(Ordering::Acquire))
    }

    pub fn mic_available(&self) -> bool {
        self.shared.mic_available.load(Ordering::Acquire)
    }
}

impl Drop for VoiceEngine {
    fn drop(&mut self) {
        self.shared.running.store(false, Ordering::Release);
        if let Some(h) = self.capture_thread.take() {
            let _ = h.join();
        }
        if let Some(h) = self.playback_thread.take() {
            let _ = h.join();
        }
    }
}

#[cfg(windows)]
mod winmm_voice {
    use super::*;

    const WAVE_MAPPER: u32 = u32::MAX;
    const WHDR_DONE: u32 = 0x0000_0001;

    #[repr(C, packed)]
    struct WaveFormatEx {
        tag: u16,
        channels: u16,
        rate: u32,
        bytes_per_sec: u32,
        block_align: u16,
        bits_per_sample: u16,
        extra: u16,
    }

    #[repr(C)]
    struct WaveHdr {
        data: *mut i8,
        buffer_len: u32,
        bytes_recorded: u32,
        user: usize,
        flags: u32,
        loops: u32,
        next: *mut WaveHdr,
        reserved: usize,
    }

    #[link(name = "winmm")]
    extern "system" {
        fn waveInOpen(
            handle: *mut usize,
            device: u32,
            fmt: *const WaveFormatEx,
            cb: usize,
            inst: usize,
            flags: u32,
        ) -> u32;
        fn waveInPrepareHeader(handle: usize, hdr: *mut WaveHdr, size: u32) -> u32;
        fn waveInAddBuffer(handle: usize, hdr: *mut WaveHdr, size: u32) -> u32;
        fn waveInStart(handle: usize) -> u32;
        fn waveInReset(handle: usize) -> u32;
        fn waveInUnprepareHeader(handle: usize, hdr: *mut WaveHdr, size: u32) -> u32;
        fn waveInClose(handle: usize) -> u32;

        fn waveOutOpen(
            handle: *mut usize,
            device: u32,
            fmt: *const WaveFormatEx,
            cb: usize,
            inst: usize,
            flags: u32,
        ) -> u32;
        fn waveOutPrepareHeader(handle: usize, hdr: *mut WaveHdr, size: u32) -> u32;
        fn waveOutWrite(handle: usize, hdr: *mut WaveHdr, size: u32) -> u32;
        fn waveOutReset(handle: usize) -> u32;
        fn waveOutUnprepareHeader(handle: usize, hdr: *mut WaveHdr, size: u32) -> u32;
        fn waveOutClose(handle: usize) -> u32;
    }

    pub fn capture_loop(shared: Arc<SharedVoice>) {
        let fmt = WaveFormatEx {
            tag: 1,
            channels: 1,
            rate: VOICE_SAMPLE_RATE,
            bytes_per_sec: VOICE_SAMPLE_RATE * 2,
            block_align: 2,
            bits_per_sample: 16,
            extra: 0,
        };
        let mut handle: usize = 0;
        let open_res = unsafe { waveInOpen(&mut handle, WAVE_MAPPER, &fmt, 0, 0, 0) };
        if open_res != 0 {
            shared.mic_available.store(false, Ordering::Release);
            return;
        }
        shared.mic_available.store(true, Ordering::Release);

        const NUM_BUFS: usize = 4;
        let hdr_size = std::mem::size_of::<WaveHdr>() as u32;
        let mut pcm_bufs: Vec<Vec<i16>> = (0..NUM_BUFS)
            .map(|_| vec![0i16; VOICE_FRAME_SAMPLES])
            .collect();
        let mut hdrs: Vec<Box<WaveHdr>> = pcm_bufs
            .iter_mut()
            .map(|b| {
                Box::new(WaveHdr {
                    data: b.as_mut_ptr().cast(),
                    buffer_len: (VOICE_FRAME_SAMPLES * 2) as u32,
                    bytes_recorded: 0,
                    user: 0,
                    flags: 0,
                    loops: 0,
                    next: std::ptr::null_mut(),
                    reserved: 0,
                })
            })
            .collect();

        for h in &mut hdrs {
            unsafe {
                let p: *mut WaveHdr = &mut **h;
                waveInPrepareHeader(handle, p, hdr_size);
                waveInAddBuffer(handle, p, hdr_size);
            }
        }
        unsafe { waveInStart(handle) };

        let mut vad_hangover: u32 = 0;
        while shared.running.load(Ordering::Acquire) {
            let mut processed_any = false;
            for idx in 0..NUM_BUFS {
                let flags = unsafe { std::ptr::read_volatile(&hdrs[idx].flags) };
                if (flags & WHDR_DONE) == 0 {
                    continue;
                }
                processed_any = true;
                let recorded_samples =
                    ((hdrs[idx].bytes_recorded as usize) / 2).min(VOICE_FRAME_SAMPLES);
                let cfg = shared
                    .settings
                    .lock()
                    .map(|g| g.clone())
                    .unwrap_or_default();

                if recorded_samples > 0 && cfg.enabled && !cfg.mic_muted {
                    let gain = cfg.mic_gain.clamp(0.1, 4.0);
                    let mut peak = 0.0f32;
                    let mut scaled = Vec::with_capacity(recorded_samples);
                    for &s in &pcm_bufs[idx][..recorded_samples] {
                        let v = ((s as f32) * gain).clamp(-32767.0, 32767.0);
                        peak = peak.max(v.abs() / 32768.0);
                        scaled.push(v as i16);
                    }
                    shared
                        .mic_level_bits
                        .store(peak.to_bits(), Ordering::Release);

                    let should_tx = if cfg.push_to_talk {
                        shared.ptt_held.load(Ordering::Acquire)
                    } else if peak >= cfg.vad_threshold {
                        vad_hangover = 12; // ~240 ms voice tail
                        true
                    } else if vad_hangover > 0 {
                        vad_hangover -= 1;
                        true
                    } else {
                        false
                    };
                    shared.speaking_now.store(should_tx, Ordering::Release);

                    if should_tx {
                        let encoded = encode_ulaw_frame(&scaled);
                        if cfg.loopback_test {
                            let listener_pos = shared
                                .listener
                                .lock()
                                .map(|l| l.position)
                                .unwrap_or([0.0; 3]);
                            if let Ok(mut map) = shared.speakers.lock() {
                                let entry = map.entry(255).or_insert_with(|| SpeakerQueue {
                                    samples: VecDeque::new(),
                                    pos: listener_pos,
                                    muted: false,
                                });
                                entry.pos = listener_pos;
                                for &s in &scaled {
                                    entry.samples.push_back(s);
                                }
                                while entry.samples.len() > VOICE_FRAME_SAMPLES * 8 {
                                    entry.samples.pop_front();
                                }
                            }
                        }
                        if let Ok(mut q) = shared.captured.lock() {
                            if q.len() < 16 {
                                q.push_back(encoded);
                            }
                        }
                    }
                } else {
                    shared.speaking_now.store(false, Ordering::Release);
                    shared.mic_level_bits.store(0, Ordering::Release);
                }

                // Re-queue buffer for continuous capture
                hdrs[idx].bytes_recorded = 0;
                unsafe {
                    let p: *mut WaveHdr = &mut *hdrs[idx];
                    waveInAddBuffer(handle, p, hdr_size);
                }
            }
            if !processed_any {
                thread::sleep(Duration::from_millis(5));
            }
        }

        unsafe {
            waveInReset(handle);
            for h in &mut hdrs {
                waveInUnprepareHeader(handle, &mut **h, hdr_size);
            }
            waveInClose(handle);
        }
    }

    pub fn playback_loop(shared: Arc<SharedVoice>) {
        let fmt = WaveFormatEx {
            tag: 1,
            channels: 2,
            rate: VOICE_SAMPLE_RATE,
            bytes_per_sec: VOICE_SAMPLE_RATE * 4,
            block_align: 4,
            bits_per_sample: 16,
            extra: 0,
        };
        let mut handle: usize = 0;
        if unsafe { waveOutOpen(&mut handle, WAVE_MAPPER, &fmt, 0, 0, 0) } != 0 {
            return;
        }

        const NUM_OUT: usize = 4;
        let hdr_size = std::mem::size_of::<WaveHdr>() as u32;
        let mut out_bufs: Vec<Vec<i16>> = (0..NUM_OUT)
            .map(|_| vec![0i16; VOICE_FRAME_SAMPLES * 2])
            .collect();
        let mut hdrs: Vec<Box<WaveHdr>> = out_bufs
            .iter_mut()
            .map(|b| {
                Box::new(WaveHdr {
                    data: b.as_mut_ptr().cast(),
                    buffer_len: (VOICE_FRAME_SAMPLES * 4) as u32,
                    bytes_recorded: 0,
                    user: 0,
                    flags: WHDR_DONE,
                    loops: 0,
                    next: std::ptr::null_mut(),
                    reserved: 0,
                })
            })
            .collect();
        let mut prepared = [false; NUM_OUT];
        let mut mix_l = vec![0.0f32; VOICE_FRAME_SAMPLES];
        let mut mix_r = vec![0.0f32; VOICE_FRAME_SAMPLES];

        while shared.running.load(Ordering::Acquire) {
            let mut any_ready = false;
            for idx in 0..NUM_OUT {
                let flags = unsafe { std::ptr::read_volatile(&hdrs[idx].flags) };
                if (flags & WHDR_DONE) == 0 {
                    continue;
                }
                let cfg = shared
                    .settings
                    .lock()
                    .map(|g| g.clone())
                    .unwrap_or_default();
                let listener = shared.listener.lock().map(|l| *l).unwrap_or_default();

                mix_l.fill(0.0);
                mix_r.fill(0.0);
                let mut had_audio = false;

                if cfg.enabled && !cfg.deafened {
                    if let Ok(mut map) = shared.speakers.lock() {
                        for (&id, spk) in map.iter_mut() {
                            if spk.muted || spk.samples.is_empty() {
                                spk.samples.clear();
                                continue;
                            }
                            let (gl, gr) = if id == 255 {
                                (cfg.voice_volume, cfg.voice_volume)
                            } else {
                                proximity_stereo_gains(
                                    listener,
                                    spk.pos,
                                    cfg.proximity_range,
                                    cfg.voice_volume,
                                )
                            };
                            let count = spk.samples.len().min(VOICE_FRAME_SAMPLES);
                            if gl > 0.001 || gr > 0.001 {
                                had_audio = true;
                                for i in 0..count {
                                    let s = spk.samples.pop_front().unwrap_or(0) as f32 / 32768.0;
                                    mix_l[i] += s * gl;
                                    mix_r[i] += s * gr;
                                }
                            } else {
                                // Out of proximity range — drain silently so buffer stays fresh
                                for _ in 0..count {
                                    spk.samples.pop_front();
                                }
                            }
                        }
                    }
                } else if let Ok(mut map) = shared.speakers.lock() {
                    for spk in map.values_mut() {
                        spk.samples.clear();
                    }
                }

                if !had_audio {
                    continue;
                }
                any_ready = true;
                for i in 0..VOICE_FRAME_SAMPLES {
                    // Soft-clip mixed proximity voices so multiple simultaneous speakers never harshly distort
                    let sl = mix_l[i].tanh();
                    let sr = mix_r[i].tanh();
                    out_bufs[idx][i * 2] = (sl * 32767.0) as i16;
                    out_bufs[idx][i * 2 + 1] = (sr * 32767.0) as i16;
                }
                unsafe {
                    let p: *mut WaveHdr = &mut *hdrs[idx];
                    if !prepared[idx] {
                        waveOutPrepareHeader(handle, p, hdr_size);
                        prepared[idx] = true;
                    }
                    waveOutWrite(handle, p, hdr_size);
                }
            }
            if !any_ready {
                thread::sleep(Duration::from_millis(8));
            }
        }

        unsafe {
            waveOutReset(handle);
            for idx in 0..NUM_OUT {
                if prepared[idx] {
                    waveOutUnprepareHeader(handle, &mut *hdrs[idx], hdr_size);
                }
            }
            waveOutClose(handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ulaw_roundtrip_speech_levels() {
        for sample in [-24000i16, -8000, -1200, 0, 1200, 8000, 24000] {
            let enc = linear_to_ulaw(sample);
            let dec = ulaw_to_linear(enc);
            assert!(
                (sample as i32 - dec as i32).abs() <= (sample.abs() as i32 / 12 + 160),
                "sample={sample} dec={dec}"
            );
        }
    }

    #[test]
    fn proximity_attenuates_and_pans_correctly() {
        let listener = ListenerPose {
            position: [0.0, 0.0, 0.0],
            right: [1.0, 0.0, 0.0],
        };
        let (l_close, r_close) = proximity_stereo_gains(listener, [1.0, 0.0, 0.0], 28.0, 1.0);
        let (l_far, r_far) = proximity_stereo_gains(listener, [20.0, 0.0, 0.0], 28.0, 1.0);
        let (l_out, r_out) = proximity_stereo_gains(listener, [40.0, 0.0, 0.0], 28.0, 1.0);
        assert!(r_close > l_close, "right speaker should pan right");
        assert!(r_close > r_far && r_far > 0.0);
        assert_eq!((l_out, r_out), (0.0, 0.0));
    }
}
