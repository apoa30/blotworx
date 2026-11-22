//! EM2 (PC, Wwise bank version 72) audio playback for EM2 Studio playtests.
//!
//! Every record layout below was checked against all 836 retail banks: each object's payload is
//! consumed exactly (sounds, actions, events, random/sequence, switch, layer, actor-mixer, bus,
//! attenuation, music segment/track/switch/playlist). Values come from the banks; streamed media
//! comes from the game's .caf packages (12-byte entries: media id, offset, size; 2048-aligned).
//!
//! Executed: Play (sounds, random/sequence step + continuous, switch, layer, actor-mixer, music),
//! Stop / Pause / Resume (one target, all, all-except; per object or global, with fades),
//! Mute / UnMute, Set/Reset Volume, Pitch and Bus Volume, SetState, SetSwitch; delay, fade-in and
//! probability; volume and pitch (with random ranges) through the actor-mixer and bus chain;
//! loop counts; 3D distance attenuation through the engine's own curve evaluation
//! (audio_conversion) and stereo panning from the listener; interactive music (switch by state,
//! playlists with loop counts, segments aligned on their entry/exit cues, transition fades).
//! Not reproduced: effects (reverb/filters/LPF), aux sends, RTPC curves, voice limits/priority,
//! music transition sync points other than immediate and segment end, Wwise's exact speaker panner.
use anyhow::{bail, ensure, Context, Result};
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{atomic::{AtomicU64, Ordering}, mpsc, Arc, Mutex},
};

const RATE: u32 = 48_000;
const GLOBAL: usize = 0x7fffffff;
/// Wwise name hashes: "Music_State" and its neutral exploration state "Exp_N".
const MUSIC_STATE: u32 = 0xe414d158;
const EXP_N: u32 = 4066163493;

// ---------------------------------------------------------------- bank records
struct Rd<'a> { b: &'a [u8], at: usize }
impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).context("size overflow")?;
        let v = self.b.get(self.at..end).context("truncated audio record")?; self.at = end; Ok(v)
    }
    fn u8(&mut self) -> Result<u8> { Ok(self.take(1)?[0]) }
    fn u16(&mut self) -> Result<u16> { Ok(u16::from_le_bytes(self.take(2)?.try_into()?)) }
    fn i16(&mut self) -> Result<i16> { Ok(i16::from_le_bytes(self.take(2)?.try_into()?)) }
    fn u32(&mut self) -> Result<u32> { Ok(u32::from_le_bytes(self.take(4)?.try_into()?)) }
    fn i32(&mut self) -> Result<i32> { Ok(i32::from_le_bytes(self.take(4)?.try_into()?)) }
    fn f32(&mut self) -> Result<f32> { Ok(f32::from_le_bytes(self.take(4)?.try_into()?)) }
    fn f64(&mut self) -> Result<f64> { Ok(f64::from_le_bytes(self.take(8)?.try_into()?)) }
    fn ids(&mut self) -> Result<Vec<u32>> { let n = self.u32()?; (0..n).map(|_| self.u32()).collect() }
}
fn bundles(r: &mut Rd) -> Result<(HashMap<u8, u32>, HashMap<u8, (u32, u32)>)> {
    let n = r.u8()? as usize; let ids = r.take(n)?.to_vec();
    let mut props = HashMap::new(); for id in ids { props.insert(id, r.u32()?); }
    let n = r.u8()? as usize; let ids = r.take(n)?.to_vec();
    let mut ranges = HashMap::new(); for id in ids { ranges.insert(id, (r.u32()?, r.u32()?)); }
    Ok((props, ranges))
}
#[derive(Clone, Default)]
struct Base { bus: u32, parent: u32, props: HashMap<u8, u32>, ranges: HashMap<u8, (u32, u32)>, pos_override: bool, has3d: bool, attenuation: u32 }
impl Base {
    fn f(&self, id: u8) -> Option<f32> { self.props.get(&id).map(|b| f32::from_bits(*b)) }
    fn parse(r: &mut Rd, feedback: bool) -> Result<Self> {
        r.u8()?; let n = r.u8()?; if n > 0 { r.u8()?; }
        for _ in 0..n { r.take(7)?; }
        let bus = r.u32()?; let parent = r.u32()?; r.take(2)?;
        let (props, ranges) = bundles(r)?;
        let flags = r.u8()?; let mut b = Base { bus, parent, props, ranges, pos_override: flags & 1 != 0, ..Default::default() };
        if flags & 1 != 0 {
            let has3d = r.u8()?;
            if has3d == 0 { r.u8()?; } else {
                b.has3d = true; let kind = r.u32()?; b.attenuation = r.u32()?; r.u8()?;
                if kind == 3 { r.u8()?; }
                if kind == 2 {
                    r.take(10)?; let n = r.u32()? as usize; r.take(16 * n)?;
                    let n = r.u32()? as usize; r.take(8 * n)?; r.take(8 * n)?;
                }
            }
        }
        let aux = r.take(4)?; if aux[3] != 0 { r.take(16)?; }
        r.take(9)?;
        let n = r.u32()?; for _ in 0..n { r.u32()?; r.u8()?; let m = r.u16()? as usize; r.take(8 * m)?; }
        let n = r.u16()?; for _ in 0..n { r.take(13)?; let m = r.u16()? as usize; r.take(12 * m)?; }
        if feedback { r.u32()?; }
        Ok(b)
    }
}
#[derive(Clone)]
struct Src { media: u32, stream: u32 }
fn source(r: &mut Rd) -> Result<Src> {
    let plugin = r.u32()?; let stream = r.u32()?; let media = r.u32()?; r.u32()?;
    if stream != 1 { r.take(8)?; }
    r.u8()?;
    if matches!(plugin & 15, 2 | 5) { let n = r.u32()? as usize; r.take(n)?; }
    Ok(Src { media, stream })
}
#[derive(Clone)]
struct Item { track: u32, source: u32, play_at: f64, begin: f64, end: f64, duration: f64 }
#[derive(Clone)]
struct Rule { src: i32, dst: i32, fade_out: i32, sync: u32, fade_in: i32 }
#[derive(Clone)]
struct PlItem { segment: u32, children: u32, kind: u32, loops: i16, weight: u32 }
#[derive(Clone)]
enum Obj {
    Sound { base: Base, src: Src },
    RanSeq { base: Base, loops: u16, avoid: u16, sequence: bool, continuous: bool, playlist: Vec<(u32, i32)> },
    Switch { base: Base, state: bool, group: u32, default: u32, groups: HashMap<u32, Vec<u32>> },
    Group { base: Base, kids: Vec<u32> }, // actor-mixer, layer
    Bus { parent: u32, props: HashMap<u8, u32> },
    Segment { base: Base, kids: Vec<u32>, duration: f64, markers: Vec<f64> },
    Track { base: Base, sources: Vec<Src>, items: Vec<Item>, subs: u32, kind: u32 },
    MSwitch { base: Base, rules: Vec<Rule>, state: bool, group: u32, default: u32, assoc: HashMap<u32, u32> },
    MPlaylist { base: Base, rules: Vec<Rule>, items: Vec<PlItem> },
    Action { kind: u16, target: u32, props: HashMap<u8, u32>, specific: Vec<u8> },
    Event { actions: Vec<u32> },
    Attenuation { volume_curve: u8, curves: Vec<(u8, Vec<super::audio_bank::RtpcPoint>)> },
}
impl Obj {
    fn base(&self) -> Option<&Base> {
        match self {
            Obj::Sound { base, .. } | Obj::RanSeq { base, .. } | Obj::Switch { base, .. } | Obj::Group { base, .. }
            | Obj::Segment { base, .. } | Obj::Track { base, .. } | Obj::MSwitch { base, .. } | Obj::MPlaylist { base, .. } => Some(base),
            _ => None,
        }
    }
}
fn music_node(r: &mut Rd, fb: bool) -> Result<(Base, Vec<u32>)> {
    let base = Base::parse(r, fb)?; let kids = r.ids()?;
    r.take(8 + 8 + 4 + 2 + 1)?;
    let n = r.u32()? as usize; r.take(24 * n)?;
    Ok((base, kids))
}
fn rules(r: &mut Rd) -> Result<Vec<Rule>> {
    let n = r.u32()?; let mut out = Vec::new();
    for _ in 0..n {
        let src = r.i32()?; let dst = r.i32()?;
        let fade_out = r.i32()?; r.u32()?; r.i32()?; let sync = r.u32()?; r.u32()?; r.u8()?;
        let fade_in = r.i32()?; r.take(4 + 4 + 4 + 4 + 2 + 1 + 1)?;
        r.u8()?; r.take(30)?;
        out.push(Rule { src, dst, fade_out, sync, fade_in });
    }
    Ok(out)
}
fn parse(kind: u8, p: &[u8], fb: bool) -> Result<Option<Obj>> {
    let r = &mut Rd { b: p, at: 0 };
    let obj = match kind {
        2 => { let src = source(r)?; Obj::Sound { base: Base::parse(r, fb)?, src } }
        3 => {
            let k = r.u16()?; let target = r.u32()?; r.u8()?; let (props, _) = bundles(r)?;
            { let specific = p[r.at..].to_vec(); r.at = p.len(); Obj::Action { kind: k, target, props, specific } }
        }
        4 => { let n = r.u32()?; Obj::Event { actions: (0..n).map(|_| r.u32()).collect::<Result<_>>()? } }
        5 => {
            let base = Base::parse(r, fb)?; let loops = r.u16()?; r.take(12)?; let avoid = r.u16()?;
            r.u8()?; r.u8()?; let mode = r.u8()?; let bits = r.take(5)?.to_vec(); r.ids()?;
            let n = r.u16()?; let playlist = (0..n).map(|_| Ok((r.u32()?, r.i32()?))).collect::<Result<_>>()?;
            Obj::RanSeq { base, loops, avoid, sequence: mode == 1, continuous: bits[3] != 0, playlist }
        }
        6 => {
            let base = Base::parse(r, fb)?; let gtype = r.u32()?; let group = r.u32()?; let default = r.u32()?; r.u8()?; r.ids()?;
            let n = r.u32()?; let mut groups = HashMap::new();
            for _ in 0..n { let s = r.u32()?; groups.insert(s, r.ids()?); }
            let n = r.u32()? as usize; r.take(18 * n)?;
            Obj::Switch { base, state: gtype == 1, group, default, groups }
        }
        7 => { let base = Base::parse(r, fb)?; Obj::Group { base, kids: r.ids()? } }
        8 => { let parent = r.u32()?; let (props, _) = bundles(r)?; return Ok(Some(Obj::Bus { parent, props })); }
        9 => {
            let base = Base::parse(r, fb)?; let kids = r.ids()?;
            let n = r.u32()?;
            for _ in 0..n {
                r.u32()?; let m = r.u16()?; for _ in 0..m { r.take(13)?; let k = r.u16()? as usize; r.take(12 * k)?; }
                r.u32()?; let m = r.u32()?; for _ in 0..m { r.u32()?; let k = r.u32()? as usize; r.take(12 * k)?; }
            }
            Obj::Group { base, kids }
        }
        10 => {
            let (base, kids) = music_node(r, fb)?; let duration = r.f64()?; let n = r.u32()?; let mut markers = Vec::new();
            for _ in 0..n { r.u32()?; markers.push(r.f64()?); let s = r.u32()? as usize; r.take(s)?; }
            Obj::Segment { base, kids, duration, markers }
        }
        11 => {
            let n = r.u32()?; let sources = (0..n).map(|_| source(r)).collect::<Result<Vec<_>>>()?;
            let n = r.u32()?; let mut items = Vec::new();
            for _ in 0..n { items.push(Item { track: r.u32()?, source: r.u32()?, play_at: r.f64()?, begin: r.f64()?, end: r.f64()?, duration: r.f64()? }); }
            let subs = if n > 0 { r.u32()? } else { 0 };
            let n = r.u32()?; for _ in 0..n { r.take(8)?; let m = r.u32()? as usize; r.take(12 * m)?; }
            let base = Base::parse(r, fb)?; let kind = r.u32()?; r.i32()?;
            Obj::Track { base, sources, items, subs, kind }
        }
        12 => {
            let (base, _) = music_node(r, fb)?; let rules = rules(r)?;
            let gtype = r.u32()?; let group = r.u32()?; let default = r.u32()?; r.u8()?;
            let n = r.u32()?; let mut assoc = HashMap::new(); for _ in 0..n { let s = r.u32()?; assoc.insert(s, r.u32()?); }
            Obj::MSwitch { base, rules, state: gtype == 1, group, default, assoc }
        }
        13 => {
            let (base, _) = music_node(r, fb)?; let rules = rules(r)?;
            let n = r.u32()?; let mut items = Vec::new();
            for _ in 0..n {
                let segment = r.u32()?; r.u32()?; let children = r.u32()?; let kind = r.u32()?; let loops = r.i16()?; let weight = r.u32()?;
                r.take(4)?; items.push(PlItem { segment, children, kind, loops, weight });
            }
            Obj::MPlaylist { base, rules, items }
        }
        14 => {
            let cone = r.u8()?; if cone != 0 { r.take(16)?; }
            let using = r.take(4)?.to_vec(); let n = r.u8()?; let mut curves = Vec::new();
            for _ in 0..n {
                let scaling = r.u8()?; let m = r.u16()?;
                let pts = (0..m).map(|_| Ok(super::audio_bank::RtpcPoint { x: r.f32()?, y: r.f32()?, interpolation: r.u32()? })).collect::<Result<_>>()?;
                curves.push((scaling, pts));
            }
            return Ok(Some(Obj::Attenuation { volume_curve: using[0], curves }));
        }
        _ => return Ok(None),
    };
    ensure!(r.at == p.len(), "kind {kind}: {} unread bytes", p.len() - r.at);
    Ok(Some(obj))
}

// ---------------------------------------------------------------- decoded audio
pub struct Pcm { rate: u32, channels: u16, data: Vec<i16> }
impl Pcm { fn frames(&self) -> usize { self.data.len() / self.channels.max(1) as usize } }
enum Slot { Loading, Ready(Arc<Pcm>), Failed }
enum Where { Bank(PathBuf, u32), Caf(PathBuf, u64, u32) }

fn caf_index(audio: &Path, language: &str) -> HashMap<u32, (PathBuf, u64, u32)> {
    let mut out = HashMap::new();
    let Ok(dir) = std::fs::read_dir(audio) else { return out };
    for entry in dir.flatten() {
        let path = entry.path();
        let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("caf")) { continue; }
        if !(name.to_ascii_lowercase().starts_with("sfx_") || name.eq_ignore_ascii_case(language)) { continue; }
        let mut read = || -> Result<()> {
            use std::io::Read;
            let mut f = std::fs::File::open(&path)?; let mut head = [0u8; 32]; f.read_exact(&mut head)?;
            let count = u32::from_le_bytes(head[4..8].try_into()?) as usize;
            ensure!(count < 1_000_000, "bad .caf entry count");
            let mut table = vec![0u8; count * 12]; f.read_exact(&mut table)?;
            for e in table.chunks_exact(12) {
                let w = |i: usize| u32::from_le_bytes(e[i..i + 4].try_into().unwrap());
                out.insert(w(0), (path.clone(), w(4) as u64, w(8)));
            }
            Ok(())
        };
        if let Err(e) = read() { log::warn!("EM2 audio: {}: {e:#}", path.display()); }
    }
    out
}
fn decode(at: &Where, media: u32, cache: &Path) -> Result<Pcm> {
    let mut decoder = match at {
        Where::Bank(path, subsong) => super::audio_codec::Decoder::open(&super::audio_codec::Source { bank: path.clone(), subsong: *subsong, media_id: media })?,
        Where::Caf(path, offset, size) => {
            let file = cache.join(format!("{media}.wem"));
            if !file.is_file() {
                use std::io::{Read, Seek};
                let mut f = std::fs::File::open(path)?; f.seek(std::io::SeekFrom::Start(*offset))?;
                let mut bytes = vec![0u8; *size as usize]; f.read_exact(&mut bytes)?;
                ensure!(&bytes[..4] == b"RIFF", ".caf entry {media} is not a RIFF/WEM");
                std::fs::create_dir_all(cache)?; std::fs::write(&file, bytes)?;
            }
            super::audio_codec::Decoder::open_file(&file, media)?
        }
    };
    let (rate, channels) = (decoder.info.rate, decoder.info.channels);
    let mut data = Vec::with_capacity(decoder.info.frames as usize * channels as usize);
    while let Some(chunk) = decoder.read_chunk()? { data.extend_from_slice(&chunk); }
    Ok(Pcm { rate, channels, data })
}

// ---------------------------------------------------------------- mixer thread
struct VoiceSpec { id: u64, pcm: Arc<Pcm>, start: u64, gain: [f32; 2], step: f64, loops: Option<u32>, from: usize, until: usize, fade_in: u32 }
enum Cmd { Start(VoiceSpec), Gain(u64, [f32; 2]), Fade(u64, f32, u32, bool), Pause(u64, bool), Step(u64, f64) }
struct MixVoice { s: VoiceSpec, pos: f64, gain: [f32; 2], fade: f32, fade_to: f32, fade_step: f32, stop_after_fade: bool, paused: bool, done: bool }
struct Mixer { tx: mpsc::Sender<Cmd>, clock: Arc<AtomicU64>, finished: Arc<Mutex<Vec<u64>>> }
impl Mixer {
    fn start() -> Result<Self> {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let clock = Arc::new(AtomicU64::new(0)); let finished = Arc::new(Mutex::new(Vec::new()));
        let (c, f) = (clock.clone(), finished.clone());
        let endpoint = super::audio_output::endpoint::Endpoint::open()?;
        std::thread::Builder::new().name("em2-audio-mixer".into()).spawn(move || {
            const BLOCK: usize = 480; // 10 ms
            let mut voices: Vec<MixVoice> = Vec::new(); let mut queued: VecDeque<super::audio_output::endpoint::Buffer> = VecDeque::new();
            let mut mix = vec![0f32; BLOCK * 2];
            loop {
                loop {
                    match rx.try_recv() {
                        Ok(Cmd::Start(s)) => voices.push(MixVoice { pos: s.from as f64, gain: s.gain, fade: if s.fade_in > 0 { 0. } else { 1. }, fade_to: 1., fade_step: if s.fade_in > 0 { 1. / s.fade_in as f32 } else { 0. }, stop_after_fade: false, paused: false, done: false, s }),
                        Ok(Cmd::Gain(id, g)) => for v in voices.iter_mut().filter(|v| v.s.id == id) { v.s.gain = g; },
                        Ok(Cmd::Fade(id, to, frames, stop)) => for v in voices.iter_mut().filter(|v| v.s.id == id) {
                            v.fade_to = to; v.stop_after_fade = stop;
                            v.fade_step = if frames == 0 { f32::INFINITY } else { (to - v.fade).abs() / frames as f32 };
                        },
                        Ok(Cmd::Pause(id, p)) => for v in voices.iter_mut().filter(|v| v.s.id == id) { v.paused = p; },
                        Ok(Cmd::Step(id, step)) => for v in voices.iter_mut().filter(|v| v.s.id == id) { v.s.step = step; },
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => return,
                    }
                }
                while queued.front().is_some_and(|b| b.done()) { queued.pop_front(); }
                if queued.len() >= 6 { std::thread::sleep(std::time::Duration::from_millis(2)); continue; }
                let now = c.load(Ordering::Acquire);
                mix.iter_mut().for_each(|s| *s = 0.);
                for v in voices.iter_mut() {
                    if v.done || v.paused { continue; }
                    let ch = v.s.pcm.channels.max(1) as usize; let until = v.s.until.min(v.s.pcm.frames());
                    for i in 0..BLOCK {
                        if now + (i as u64) < v.s.start { continue; }
                        let p = v.pos as usize;
                        if p + 1 >= until {
                            match &mut v.s.loops { Some(0) | Some(1) => { v.done = true; break; } Some(n) => { *n -= 1; v.pos = v.s.from as f64; } None => v.pos = v.s.from as f64 }
                            continue;
                        }
                        let t = (v.pos - p as f64) as f32; let d = &v.s.pcm.data;
                        let sample = |c: usize| { let a = d[p * ch + c] as f32; let b = d[(p + 1) * ch + c] as f32; (a + (b - a) * t) / 32768. };
                        let (l, r) = if ch == 1 { let m = sample(0); (m, m) } else { (sample(0), sample(1)) };
                        if v.fade != v.fade_to {
                            if v.fade < v.fade_to { v.fade = (v.fade + v.fade_step).min(v.fade_to) } else { v.fade = (v.fade - v.fade_step).max(v.fade_to) }
                        } else if v.stop_after_fade { v.done = true; break; }
                        v.gain[0] += (v.s.gain[0] - v.gain[0]) * 0.002; v.gain[1] += (v.s.gain[1] - v.gain[1]) * 0.002;
                        mix[i * 2] += l * v.gain[0] * v.fade; mix[i * 2 + 1] += r * v.gain[1] * v.fade;
                        v.pos += v.s.step;
                    }
                }
                let ended: Vec<u64> = voices.iter().filter(|v| v.done).map(|v| v.s.id).collect();
                if !ended.is_empty() { f.lock().unwrap().extend(ended); voices.retain(|v| !v.done); }
                let block: Vec<i16> = mix.iter().map(|s| (s.clamp(-1., 1.) * 32767.) as i16).collect();
                match endpoint.submit(block) { Ok(b) => queued.push_back(b), Err(e) => { log::error!("EM2 audio output: {e:#}"); return; } }
                c.fetch_add(BLOCK as u64, Ordering::AcqRel);
            }
        })?;
        Ok(Self { tx, clock, finished })
    }
}

// ---------------------------------------------------------------- runtime
pub struct Listener { pub position: [f32; 3], pub right: [f32; 3] }
struct Live {
    id: u64, owner: usize, playing: u32, event: u32, lineage: Vec<u32>, bus: Vec<u32>, db: f32, cents: f32,
    attenuation: Option<u32>, channels: u16, rate: u32, music: Option<u64>, started: bool,
}
struct PendingStart { live: Live, media: u32, at: Where, start: u64, loops: Option<u32>, from_ms: f64, until_ms: Option<f64>, fade_in: u32, queued: std::time::Instant }
struct Delayed { due: u64, action: u32, ctx: Ctx }
#[derive(Clone)]
struct Ctx { owner: usize, playing: u32, event: u32 }
struct Chain { owner: Ctx, container: u32, order: VecDeque<u32>, loops: Option<u32>, current: Vec<u64>, lineage: Vec<u32>, fade_in: u32 }
struct Cursor { item: usize, order: Vec<usize>, pos: usize, loops_done: i32, child: Option<Box<Cursor>>, leaf_done: bool }
struct Music { id: u64, top: u32, ctx: Ctx, child: Option<u32>, cursor: Option<Cursor>, next: u64, voices: Vec<u64>, fade_in: u32, switch_at_end: Option<(u32, u32)> }

pub struct Player {
    objs: HashMap<u32, Obj>,
    media: HashMap<u32, Where>,
    caf: HashMap<u32, (PathBuf, u64, u32)>,
    cache: PathBuf,
    pcm: Arc<Mutex<HashMap<u32, Slot>>>,
    decode_tx: mpsc::Sender<(u32, Where)>,
    mixer: Mixer,
    tables: super::audio_transition::Tables,
    states: HashMap<u32, u32>,
    switches: HashMap<(usize, u32), u32>,
    mutes: HashMap<u32, ()>,
    volume: HashMap<u32, f32>, pitch: HashMap<u32, f32>, bus_volume: HashMap<u32, f32>,
    seq: HashMap<u32, usize>, history: HashMap<u32, VecDeque<usize>>,
    live: Vec<Live>, starting: Vec<PendingStart>, delayed: Vec<Delayed>, chains: Vec<Chain>, music: Vec<Music>,
    next_id: u64, rng: u64,
    pub errors: Vec<String>, events_played: u64, voices_started: u64,
    /// The level's own music event (its streaming bank's event that plays a music node).
    level_music: Option<u32>,
    debug_voices: Vec<serde_json::Value>,
}

fn db(g: f32) -> f32 { 10f32.powf(g / 20.) }
fn ms_frames(ms: f64) -> u64 { (ms.max(0.) * RATE as f64 / 1000.) as u64 }

impl Player {
    /// Parse the mounted banks (Init, Global, the level's banks) and start the mixer.
    pub fn load(banks: &[PathBuf], audio_root: &Path, language: &str) -> Result<Self> {
        let mut objs = HashMap::new(); let mut media = HashMap::new(); let mut errors = Vec::new(); let mut streaming_events = Vec::new();
        for path in banks {
            let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
            let bank = super::audio_bank_framing::Bank::parse(&bytes, super::audio_bank_framing::Format::Pc72)?;
            let fb = super::audio_bank_framing::Format::Pc72.word(bank.header, 12)? & 1 != 0;
            for (i, m) in bank.media.iter().enumerate() { media.entry(m.id).or_insert(Where::Bank(path.clone(), i as u32 + 1)); }
            for o in &bank.objects {
                match parse(o.kind as u8, o.payload, fb) {
                    Ok(Some(obj)) => {
                        if matches!(obj, Obj::Event { .. }) && path.file_name().is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().contains("streaming")) { streaming_events.push(o.id); }
                        objs.entry(o.id).or_insert(obj);
                    }
                    Ok(None) => {}
                    Err(e) => errors.push(format!("{}: object {:08x}: {e:#}", path.display(), o.id)),
                }
            }
        }
        let caf = caf_index(audio_root, language);
        // EM2 names these Play_mx_<level> (e.g. Play_mx_mss in LVL_MSS_shared_streaming.bnk)
        let level_music = streaming_events.into_iter().find(|e| match objs.get(e) {
            Some(Obj::Event { actions }) => actions.iter().any(|a| matches!(objs.get(a), Some(Obj::Action { kind, target, .. }) if kind >> 8 == 0x04 && matches!(objs.get(target), Some(Obj::MSwitch { .. } | Obj::MPlaylist { .. } | Obj::Segment { .. })))),
            _ => false,
        });
        let cache = dirs::cache_dir().context("no cache directory")?.join("Blotworx Engine/em2-wem");
        let pcm: Arc<Mutex<HashMap<u32, Slot>>> = Arc::default();
        let (decode_tx, rx) = mpsc::channel::<(u32, Where)>();
        { let (pcm, cache) = (pcm.clone(), cache.clone());
          std::thread::Builder::new().name("em2-audio-decode".into()).spawn(move || {
            while let Ok((id, at)) = rx.recv() {
                let slot = match decode(&at, id, &cache) { Ok(p) => Slot::Ready(Arc::new(p)), Err(e) => { log::warn!("EM2 audio decode {id}: {e:#}"); Slot::Failed } };
                pcm.lock().unwrap().insert(id, slot);
            }
          })?; }
        Ok(Self {
            objs, media, caf, cache, pcm, decode_tx, mixer: Mixer::start()?, tables: super::audio_transition::Tables::retail()?,
            states: HashMap::new(), switches: HashMap::new(), mutes: HashMap::new(), volume: HashMap::new(), pitch: HashMap::new(), bus_volume: HashMap::new(),
            seq: HashMap::new(), history: HashMap::new(), live: Vec::new(), starting: Vec::new(), delayed: Vec::new(), chains: Vec::new(), music: Vec::new(),
            next_id: 1, rng: 0x9e3779b97f4a7c15, errors, events_played: 0, voices_started: 0, level_music, debug_voices: Vec::new(),
        })
    }
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({"objects":self.objs.len(),"embedded_media":self.media.len(),"streamed_media":self.caf.len(),"events_played":self.events_played,
            "voices_started":self.voices_started,"playing":self.live.len(),"waiting_for_decode":self.starting.len(),"music":self.music.iter().map(|m|serde_json::json!({"top":m.top,"child":m.child,"voices":m.voices.len(),"kind":match self.objs.get(&m.top){Some(Obj::MSwitch{group,default,assoc,..})=>format!("switch group {group} default {default} ({} states)",assoc.len()),Some(Obj::MPlaylist{..})=>"playlist".into(),Some(Obj::Segment{..})=>"segment".into(),_=>"?".into()}})).collect::<Vec<_>>(),"state_values":self.states.iter().map(|(g,s)|format!("{g}={s}")).collect::<Vec<_>>(),
            "states":self.states.len(),"voices":self.debug_voices,"level_music_event":self.level_music,"level_music_trigger":"inferred: the level streaming bank's music event, posted at load","errors":self.errors.iter().take(20).collect::<Vec<_>>()})
    }
    /// Start the level's music (inferred trigger: the game code that posts it isn't recovered).
    pub fn start_level_music(&mut self) {
        if !self.states.contains_key(&MUSIC_STATE) { self.set_state(MUSIC_STATE, 0); }
        if let Some(event) = self.level_music { if let Err(e) = self.post(event, Ctx { owner: GLOBAL, playing: 0, event }) { self.errors.push(format!("level music: {e:#}")); } }
    }
    fn rand(&mut self) -> f32 { self.rng ^= self.rng << 13; self.rng ^= self.rng >> 7; self.rng ^= self.rng << 17; (self.rng >> 40) as f32 / (1u64 << 24) as f32 }
    fn now(&self) -> u64 { self.mixer.clock.load(Ordering::Acquire) }

    /// One game tick: queued script/scene commands, listener and emitter positions.
    pub fn update(&mut self, pending: Vec<super::audio_post72::Pending>, listener: &Listener, position: impl Fn(usize) -> Option<[f32; 3]>) {
        use super::audio_post72::Pending;
        for p in pending {
            let r = match p {
                Pending::Event(c) => self.post(c.event, Ctx { owner: c.context.owner, playing: c.context.playing_id, event: c.event }),
                Pending::State { group, state, .. } => { self.set_state(group, state); Ok(()) }
                Pending::Switch { owner, group, value } => { self.switches.insert((owner, group), value); Ok(()) }
                Pending::Stop { owner, playing_id, event } => {
                    let ids: Vec<u64> = self.live.iter().filter(|l| owner.is_none_or(|o| o == l.owner) && playing_id.is_none_or(|p| p == l.playing) && event.is_none_or(|e| e == l.event)).map(|l| l.id).collect();
                    for id in ids { self.stop_voice(id, 0); }
                    if owner.is_none() && playing_id.is_none() && event.is_none() { self.music.clear(); self.chains.clear(); }
                    Ok(())
                }
                Pending::Parameter { .. } => Ok(()),
            };
            if let Err(e) = r { if self.errors.len() < 200 { self.errors.push(format!("{e:#}")); } }
        }
        let now = self.now();
        let due: Vec<Delayed> = { let (d, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.delayed).into_iter().partition(|d| d.due <= now); self.delayed = keep; d };
        for d in due { if let Err(e) = self.act(d.action, d.ctx, 1) { if self.errors.len() < 200 { self.errors.push(format!("{e:#}")); } } }
        self.start_ready();
        let finished: Vec<u64> = std::mem::take(&mut *self.mixer.finished.lock().unwrap());
        if !finished.is_empty() { self.live.retain(|l| !finished.contains(&l.id)); for m in &mut self.music { m.voices.retain(|v| !finished.contains(v)); } self.advance_chains(&finished); }
        self.advance_music();
        // positioned sounds follow their emitter
        let mut debug = Vec::new();
        let updates: Vec<(u64, [f32; 2])> = self.live.iter().filter(|l| l.started).map(|l| {
            let g = self.gain(l, listener, &position);
            if debug.len() < 12 { debug.push(serde_json::json!({"event":l.event,"owner":l.owner,"emitter":position(l.owner),"listener":listener.position,"attenuation":l.attenuation,"base_db":l.db,"gain":g})); }
            (l.id, g)
        }).collect();
        self.debug_voices = debug;
        for (id, g) in updates { let _ = self.mixer.tx.send(Cmd::Gain(id, g)); }
    }

    fn gain(&self, l: &Live, listener: &Listener, position: &impl Fn(usize) -> Option<[f32; 3]>) -> [f32; 2] {
        let mut total = l.db;
        for n in l.lineage.iter().chain(&l.bus) {
            if self.mutes.contains_key(n) { return [0., 0.]; }
            total += self.volume.get(n).copied().unwrap_or(0.) + self.bus_volume.get(n).copied().unwrap_or(0.);
        }
        let mut pan = [1f32, 1.];
        if let Some(att) = l.attenuation {
            if let Some(p) = position(l.owner).filter(|_| l.owner != GLOBAL) {
                let d = [p[0] - listener.position[0], p[1] - listener.position[1], p[2] - listener.position[2]];
                let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                if let Some(Obj::Attenuation { volume_curve, curves }) = self.objs.get(&att) {
                    if let Some((scaling, pts)) = curves.get(*volume_curve as usize) {
                        // curve shapes interpolated by the engine's own routine (no output scaling), then decoded:
                        // scaling 2 stores linear gain - 1 (all 2124 volume curves are in [-1, 0]); scaling 0 is dB
                        match super::audio_conversion::evaluate(pts, 0, dist, &self.tables) {
                            Ok(y) => total += if *scaling == 2 { 20. * (1. + y).max(1e-5).log10() } else { y },
                            Err(_) => return [0., 0.],
                        }
                    }
                }
                if dist > 0.01 {
                    let x = (d[0] * listener.right[0] + d[1] * listener.right[1] + d[2] * listener.right[2]) / dist;
                    let a = (x.clamp(-1., 1.) + 1.) * std::f32::consts::FRAC_PI_4;
                    pan = [a.cos() * std::f32::consts::SQRT_2, a.sin() * std::f32::consts::SQRT_2];
                    if l.channels > 1 { pan = [pan[0].min(1.), pan[1].min(1.)]; }
                }
            }
        }
        let g = db(total);
        [g * pan[0], g * pan[1]]
    }

    fn post(&mut self, event: u32, ctx: Ctx) -> Result<()> {
        let Some(Obj::Event { actions }) = self.objs.get(&event) else { bail!("event {event:08x} is not in the mounted banks") };
        self.events_played += 1;
        for a in actions.clone() { self.act(a, ctx.clone(), 0)?; }
        Ok(())
    }
    fn act(&mut self, action: u32, ctx: Ctx, _depth: u32) -> Result<()> {
        let Some(Obj::Action { kind, target, props, specific }) = self.objs.get(&action).cloned() else { bail!("action {action:08x} missing") };
        let prop_i = |id: u8| props.get(&id).map(|b| *b as i32).unwrap_or(0);
        let delay = prop_i(14);
        if delay > 0 && _depth == 0 {
            self.delayed.push(Delayed { due: self.now() + ms_frames(delay as f64), action, ctx }); return Ok(());
        }
        let fade = ms_frames(prop_i(15) as f64) as u32;
        let (ty, scope) = (kind >> 8, kind & 0xff);
        let on_object = scope & 1 != 0;
        let matches = |l: &Live, me: &Player| -> bool {
            let hit = match scope & !1 { 2 => l.lineage.contains(&target) || l.bus.contains(&target), 4 => true, 8 => !(l.lineage.contains(&target)), _ => l.lineage.contains(&target) };
            let _ = me; hit && (!on_object || l.owner == ctx.owner)
        };
        let value = || -> (u8, f32) { if specific.len() >= 6 { (specific[1], f32::from_le_bytes(specific[2..6].try_into().unwrap())) } else { (2, 0.) } };
        match ty {
            0x04 => {
                if let Some(p) = props.get(&16) { if self.rand() * 100. > f32::from_bits(*p) { return Ok(()); } }
                self.play(target, &ctx, fade, Vec::new())?;
            }
            0x01 | 0x02 | 0x03 => {
                let ids: Vec<u64> = self.live.iter().filter(|l| matches(l, self)).map(|l| l.id).collect();
                for id in ids {
                    match ty { 0x01 => self.stop_voice(id, fade), 0x02 => { let _ = self.mixer.tx.send(Cmd::Pause(id, true)); }, _ => { let _ = self.mixer.tx.send(Cmd::Pause(id, false)); } }
                }
                if ty == 0x01 {
                    let all = scope & !1 == 4;
                    self.music.retain(|m| !(all || m.top == target));
                    self.chains.retain(|c| !(all || c.container == target || c.lineage.contains(&target)));
                }
            }
            0x06 => { self.mutes.insert(target, ()); }
            0x07 => { if scope & !1 == 4 || scope & !1 == 8 { self.mutes.clear(); } else { self.mutes.remove(&target); } }
            0x0a | 0x0c | 0x08 => {
                let (meaning, v) = value();
                let map = match ty { 0x0a => &mut self.volume, 0x0c => &mut self.bus_volume, _ => &mut self.pitch };
                let base = if meaning == 2 { map.get(&target).copied().unwrap_or(0.) } else { 0. };
                map.insert(target, base + v);
                if ty == 0x08 { self.repitch(target); }
            }
            0x0b => { self.volume.remove(&target); }
            0x0d => { self.bus_volume.remove(&target); }
            0x09 => { self.pitch.remove(&target); self.repitch(target); }
            0x12 if specific.len() >= 8 => { let g = u32::from_le_bytes(specific[0..4].try_into()?); let s = u32::from_le_bytes(specific[4..8].try_into()?); self.set_state(g, s); }
            0x19 if specific.len() >= 8 => { let g = u32::from_le_bytes(specific[0..4].try_into()?); let s = u32::from_le_bytes(specific[4..8].try_into()?); self.switches.insert((ctx.owner, g), s); }
            _ => {}
        }
        Ok(())
    }
    fn repitch(&mut self, target: u32) {
        let steps: Vec<(u64, f64)> = self.live.iter().filter(|l| l.started && l.lineage.contains(&target)).map(|l| {
            let extra: f32 = l.lineage.iter().map(|n| self.pitch.get(n).copied().unwrap_or(0.)).sum();
            (l.id, l.rate as f64 / RATE as f64 * 2f64.powf((l.cents + extra) as f64 / 1200.))
        }).collect();
        for (id, step) in steps { let _ = self.mixer.tx.send(Cmd::Step(id, step)); }
    }
    fn stop_voice(&mut self, id: u64, fade: u32) { let _ = self.mixer.tx.send(Cmd::Fade(id, 0., fade.max(240), true)); }

    fn lineage(&self, mut id: u32) -> Vec<u32> {
        let mut out = Vec::new();
        while id != 0 && !out.contains(&id) { out.push(id); id = self.objs.get(&id).and_then(|o| o.base()).map(|b| b.parent).unwrap_or(0); }
        out
    }
    fn mix_values(&mut self, lineage: &[u32]) -> (f32, f32, Vec<u32>, Option<u32>, Option<Option<u32>>) {
        let (mut vol, mut cents, mut bus, mut att, mut loops) = (0f32, 0f32, 0u32, None, None);
        let mut positioned = false;
        for id in lineage {
            let Some(b) = self.objs.get(id).and_then(|o| o.base()).cloned() else { continue };
            vol += b.f(0).unwrap_or(0.); cents += b.f(2).unwrap_or(0.);
            for (pid, acc) in [(0u8, &mut vol), (2u8, &mut cents)] {
                if let Some((lo, hi)) = b.ranges.get(&pid) { let (lo, hi) = (f32::from_bits(*lo), f32::from_bits(*hi)); let r = self.rand(); *acc += lo + (hi - lo) * r; }
            }
            if loops.is_none() { if let Some(l) = b.props.get(&7) { loops = Some(if *l == 0 { None } else { Some(*l) }); } }
            if bus == 0 && b.bus != 0 { bus = b.bus; }
            if !positioned && (b.pos_override || b.parent == 0) { positioned = true; if b.has3d { att = Some(b.attenuation); } }
        }
        let mut buses = Vec::new(); let mut id = bus;
        while id != 0 && !buses.contains(&id) {
            buses.push(id);
            match self.objs.get(&id) { Some(Obj::Bus { parent, props }) => { vol += props.get(&4).map(|b| f32::from_bits(*b)).unwrap_or(0.) + props.get(&0).map(|b| f32::from_bits(*b)).unwrap_or(0.); id = *parent; } _ => break }
        }
        (vol, cents, buses, att, loops)
    }

    /// Play a node; returns the voices it started.
    fn play(&mut self, id: u32, ctx: &Ctx, fade_in: u32, chain_from: Vec<u32>) -> Result<Vec<u64>> {
        let Some(obj) = self.objs.get(&id).cloned() else { bail!("node {id:08x} is not in the mounted banks") };
        let _ = chain_from;
        Ok(match obj {
            Obj::Sound { src, .. } => self.sound(id, &src, ctx, fade_in, self.now(), None, 0., None).into_iter().collect(),
            Obj::RanSeq { loops, avoid, sequence, continuous, playlist, .. } => {
                if playlist.is_empty() { return Ok(vec![]); }
                if continuous {
                    let mut order = VecDeque::new();
                    if sequence { order.extend(playlist.iter().map(|p| p.0)); } else { for _ in 0..playlist.len() { order.push_back(self.pick(id, &playlist, avoid, false)); } }
                    let first = order.pop_front().unwrap();
                    let current = self.play(first, ctx, fade_in, vec![])?;
                    self.chains.push(Chain { owner: ctx.clone(), container: id, order, loops: if loops == 0 { None } else { Some(loops as u32) }, current: current.clone(), lineage: self.lineage(id), fade_in: 0 });
                    current
                } else {
                    let child = self.pick(id, &playlist, avoid, sequence);
                    self.play(child, ctx, fade_in, vec![])?
                }
            }
            Obj::Switch { state, group, default, groups, .. } => {
                let value = if state { self.states.get(&group).copied() } else { self.switches.get(&(ctx.owner, group)).or(self.switches.get(&(GLOBAL, group))).copied() }.unwrap_or(default);
                let kids = groups.get(&value).or(groups.get(&default)).cloned().unwrap_or_default();
                let mut out = Vec::new(); for k in kids { out.extend(self.play(k, ctx, fade_in, vec![])?); } out
            }
            Obj::Group { kids, .. } => { let mut out = Vec::new(); for k in kids { out.extend(self.play(k, ctx, fade_in, vec![])?); } out }
            Obj::MSwitch { .. } | Obj::MPlaylist { .. } | Obj::Segment { .. } => { self.start_music(id, ctx.clone(), fade_in); vec![] }
            _ => vec![],
        })
    }
    fn pick(&mut self, id: u32, playlist: &[(u32, i32)], avoid: u16, sequence: bool) -> u32 {
        if sequence { let i = self.seq.entry(id).or_insert(0); let child = playlist[*i % playlist.len()].0; *i += 1; return child; }
        let recent = self.history.get(&id).cloned().unwrap_or_default();
        let candidates: Vec<usize> = (0..playlist.len()).filter(|i| playlist.len() <= avoid as usize || !recent.contains(i)).collect();
        let total: f32 = candidates.iter().map(|&i| playlist[i].1.max(1) as f32).sum();
        let mut x = self.rand() * total; let mut chosen = candidates[0];
        for &i in &candidates { x -= playlist[i].1.max(1) as f32; if x <= 0. { chosen = i; break; } }
        let h = self.history.entry(id).or_default(); h.push_back(chosen);
        while h.len() > (avoid as usize).min(playlist.len().saturating_sub(1)) { h.pop_front(); }
        playlist[chosen].0
    }
    #[allow(clippy::too_many_arguments)]
    fn sound(&mut self, id: u32, src: &Src, ctx: &Ctx, fade_in: u32, start: u64, music: Option<u64>, from_ms: f64, until_ms: Option<f64>) -> Option<u64> {
        let lineage = self.lineage(id);
        let (vol, cents, bus, att, loops) = self.mix_values(&lineage);
        let at = if src.stream == 0 { match self.media.get(&src.media) { Some(Where::Bank(p, s)) => Where::Bank(p.clone(), *s), _ => return self.missing(src.media) } }
                 else { match self.caf.get(&src.media) { Some((p, o, s)) => Where::Caf(p.clone(), *o, *s), None => match self.media.get(&src.media) { Some(Where::Bank(p, s)) => Where::Bank(p.clone(), *s), _ => return self.missing(src.media) } } };
        let vid = self.next_id; self.next_id += 1;
        let live = Live { id: vid, owner: ctx.owner, playing: ctx.playing, event: ctx.event, lineage, bus, db: vol, cents, attenuation: att, channels: 2, rate: 48_000, music, started: false };
        let loops = if music.is_some() { Some(1) } else { loops.unwrap_or(Some(1)) };
        self.starting.push(PendingStart { live, media: src.media, at, start, loops, from_ms, until_ms, fade_in, queued: std::time::Instant::now() });
        self.start_ready();
        Some(vid)
    }
    fn missing(&mut self, media: u32) -> Option<u64> { if self.errors.len() < 200 { self.errors.push(format!("media {media} not found in banks or .caf")); } None }
    fn start_ready(&mut self) {
        let mut keep = Vec::new();
        for p in std::mem::take(&mut self.starting) {
            let slot = { let mut map = self.pcm.lock().unwrap();
                match map.get(&p.media) { Some(Slot::Ready(pcm)) => Some(Some(pcm.clone())), Some(Slot::Failed) => Some(None), Some(Slot::Loading) => None,
                    None => { map.insert(p.media, Slot::Loading); let at = match &p.at { Where::Bank(a, b) => Where::Bank(a.clone(), *b), Where::Caf(a, b, c) => Where::Caf(a.clone(), *b, *c) }; let _ = self.decode_tx.send((p.media, at)); None } } };
            match slot {
                Some(Some(pcm)) => {
                    let now = self.now();
                    // a sound that only became ready late still starts (short decode delay); music keeps its timeline
                    let late = p.live.music.is_none() && p.queued.elapsed().as_secs_f32() > 3.;
                    if late { continue; }
                    let mut start = p.start.max(now);
                    let mut from = (p.from_ms * pcm.rate as f64 / 1000.) as usize;
                    if p.live.music.is_some() && p.start < now { from += ((now - p.start) as f64 * pcm.rate as f64 / RATE as f64) as usize; start = now; }
                    let until = p.until_ms.map(|u| (u * pcm.rate as f64 / 1000.) as usize).unwrap_or(pcm.frames()).min(pcm.frames());
                    if from + 1 >= until { continue; }
                    let mut live = p.live; live.channels = pcm.channels; live.rate = pcm.rate; live.started = true;
                    let step = pcm.rate as f64 / RATE as f64 * 2f64.powf(live.cents as f64 / 1200.);
                    let gain = [0f32, 0.]; // the next update sets the real (attenuated, panned) gain
                    let _ = self.mixer.tx.send(Cmd::Start(VoiceSpec { id: live.id, pcm, start, gain, step, loops: p.loops, from, until, fade_in: p.fade_in }));
                    self.voices_started += 1;
                    if let Some(m) = live.music { if let Some(music) = self.music.iter_mut().find(|x| x.id == m) { music.voices.push(live.id); } }
                    self.live.push(live);
                }
                Some(None) => {}
                None => keep.push(p),
            }
        }
        self.starting = keep;
    }
    fn advance_chains(&mut self, finished: &[u64]) {
        let mut next = Vec::new();
        self.chains.retain_mut(|c| {
            c.current.retain(|v| !finished.contains(v));
            if !c.current.is_empty() { return true; }
            next.push(c.container); true
        });
        for container in next {
            let Some(pos) = self.chains.iter().position(|c| c.container == container && c.current.is_empty()) else { continue };
            let mut c = self.chains.remove(pos);
            if c.order.is_empty() {
                match &mut c.loops { Some(n) if *n <= 1 => continue, Some(n) => *n -= 1, None => {} }
                if let Some(Obj::RanSeq { sequence, playlist, avoid, .. }) = self.objs.get(&c.container).cloned() {
                    if sequence { c.order.extend(playlist.iter().map(|p| p.0)); } else { for _ in 0..playlist.len() { c.order.push_back(self.pick(c.container, &playlist, avoid, false)); } }
                }
            }
            let Some(child) = c.order.pop_front() else { continue };
            let ctx = c.owner.clone();
            c.current = self.play(child, &ctx, c.fade_in, vec![]).unwrap_or_default();
            if !c.current.is_empty() || !c.order.is_empty() { self.chains.push(c); }
        }
    }

    // ------------------------------------------------------------ music
    fn set_state(&mut self, group: u32, mut state: u32) {
        // Music_State with no override: the native music manager picks a playstyle default that
        // isn't recovered; a new save is neutral, so use Exp_N (inferred).
        if group == MUSIC_STATE && (state == 0 || state == 0x811c9dc5) { state = EXP_N; }
        if self.states.get(&group) == Some(&state) { return; }
        self.states.insert(group, state);
        for i in 0..self.music.len() {
            let (top, child) = (self.music[i].top, self.music[i].child);
            let Some(Obj::MSwitch { group: g, state: is_state, assoc, default, rules, .. }) = self.objs.get(&top).cloned() else { continue };
            if !is_state || g != group { continue; }
            let new = assoc.get(&state).or(assoc.get(&default)).copied();
            if new == child { continue; }
            let rule = rules.iter().rev().find(|r| (r.src == -1 || Some(r.src as u32) == child) && (r.dst == -1 || Some(r.dst as u32) == new)).cloned();
            let (fade_out, sync, fade_in) = rule.map(|r| (r.fade_out, r.sync, r.fade_in)).unwrap_or((0, 0, 0));
            let m = &mut self.music[i];
            if sync == 0 || child.is_none() {
                let voices = std::mem::take(&mut m.voices);
                m.child = new; m.cursor = None; m.next = 0; m.fade_in = ms_frames(fade_in as f64) as u32;
                for v in voices { self.stop_voice(v, ms_frames(fade_out as f64) as u32); }
            } else {
                m.switch_at_end = new.map(|n| (n, ms_frames(fade_in as f64) as u32));
            }
        }
    }
    fn start_music(&mut self, top: u32, ctx: Ctx, fade_in: u32) {
        if self.music.iter().any(|m| m.top == top) { return; }
        let child = match self.objs.get(&top) {
            Some(Obj::MSwitch { state, group, default, assoc, .. }) => {
                let value = if *state { self.states.get(group).copied() } else { self.switches.get(&(ctx.owner, *group)).copied() }.unwrap_or(*default);
                assoc.get(&value).or(assoc.get(default)).copied()
            }
            _ => Some(top),
        };
        let id = self.next_id; self.next_id += 1;
        self.music.push(Music { id, top, ctx, child, cursor: None, next: 0, voices: Vec::new(), fade_in, switch_at_end: None });
    }
    fn advance_music(&mut self) {
        let now = self.now();
        for i in 0..self.music.len() {
            if self.music[i].next > now + RATE as u64 { continue; } // schedule one second ahead
            let Some(child) = self.music[i].child else { continue };
            if let Some((n, fade)) = self.music[i].switch_at_end.take() { self.music[i].child = Some(n); self.music[i].cursor = None; self.music[i].fade_in = fade; continue; }
            let Some(segment) = self.next_segment(i, child) else { if !matches!(self.objs.get(&self.music[i].top), Some(Obj::MSwitch { .. })) { self.music[i].child = None; } continue };
            let start = self.music[i].next.max(now + RATE as u64 / 20);
            let length = self.play_segment(i, segment, start);
            self.music[i].next = start + length;
        }
        // a music switch with nothing for the current state keeps running silently (as in Wwise)
        self.music.retain(|m| m.child.is_some() || !m.voices.is_empty() || matches!(self.objs.get(&m.top), Some(Obj::MSwitch { .. })));
    }
    fn next_segment(&mut self, i: usize, node: u32) -> Option<u32> {
        match self.objs.get(&node)?.clone() {
            Obj::Segment { .. } => { if self.music[i].cursor.is_some() { return None; } self.music[i].cursor = Some(Cursor { item: 0, order: vec![], pos: 0, loops_done: 0, child: None, leaf_done: true }); Some(node) }
            Obj::MPlaylist { items, .. } => {
                if items.is_empty() { return None; }
                let mut cursor = self.music[i].cursor.take().unwrap_or_else(|| self.cursor(&items, 0));
                let seg = self.cursor_next(&mut cursor, &items);
                self.music[i].cursor = Some(cursor); seg
            }
            Obj::MSwitch { .. } => None,
            _ => None,
        }
    }
    fn cursor(&mut self, items: &[PlItem], item: usize) -> Cursor {
        let mut c = Cursor { item, order: vec![], pos: 0, loops_done: 0, child: None, leaf_done: false };
        self.reorder(&mut c, items); c
    }
    fn children(items: &[PlItem], item: usize) -> Vec<usize> {
        // items are stored depth-first; each group's children follow it
        let mut out = Vec::new(); let mut at = item + 1;
        for _ in 0..items[item].children {
            out.push(at);
            let mut stack = items[at].children as usize; at += 1;
            while stack > 0 { stack += items[at].children as usize; stack -= 1; at += 1; }
        }
        out
    }
    fn reorder(&mut self, c: &mut Cursor, items: &[PlItem]) {
        let kids = Self::children(items, c.item);
        c.pos = 0;
        c.order = match items[c.item].kind {
            0 => kids,
            1 => { let i = *self.seq.entry(0x8000_0000 | c.item as u32).or_insert(0); self.seq.insert(0x8000_0000 | c.item as u32, i + 1); kids.get(i % kids.len().max(1)).copied().into_iter().collect() }
            k => {
                let mut pick = || { let total: f32 = kids.iter().map(|&k| items[k].weight.max(1) as f32).sum(); let mut x = self.rand() * total; let mut chosen = kids.first().copied(); for &k in &kids { x -= items[k].weight.max(1) as f32; if x <= 0. { chosen = Some(k); break; } } chosen };
                if k == 2 { (0..kids.len()).filter_map(|_| pick()).collect() } else { pick().into_iter().collect() }
            }
        };
    }
    fn cursor_next(&mut self, c: &mut Cursor, items: &[PlItem]) -> Option<u32> {
        let it = items[c.item].clone();
        if it.children == 0 {
            if it.segment == 0 { return None; }
            if c.leaf_done { if it.loops != 0 && c.loops_done + 1 >= it.loops as i32 { return None; } c.loops_done += 1; }
            c.leaf_done = true; return Some(it.segment);
        }
        for _ in 0..64 {
            if let Some(child) = &mut c.child { if let Some(s) = self.cursor_next(child, items) { return Some(s); } c.child = None; }
            if c.pos >= c.order.len() {
                c.loops_done += 1;
                if it.loops != 0 && c.loops_done >= it.loops as i32 { return None; }
                self.reorder(c, items); if c.order.is_empty() { return None; }
            }
            let next = c.order[c.pos]; c.pos += 1;
            c.child = Some(Box::new(self.cursor(items, next)));
        }
        None
    }
    /// Start a segment's tracks so its entry cue lands on `start`; returns frames to its exit cue.
    fn play_segment(&mut self, i: usize, segment: u32, start: u64) -> u64 {
        let Some(Obj::Segment { kids, duration, markers, .. }) = self.objs.get(&segment).cloned() else { return RATE as u64 };
        let entry = markers.first().copied().unwrap_or(0.);
        let exit = if markers.len() >= 2 { *markers.last().unwrap() } else { duration };
        let (ctx, mid, fade) = (self.music[i].ctx.clone(), self.music[i].id, std::mem::take(&mut self.music[i].fade_in));
        for track in kids {
            let Some(Obj::Track { sources, items, subs, kind, .. }) = self.objs.get(&track).cloned() else { continue };
            let sub = match kind { 0 => None, 1 => Some((self.rand() * subs.max(1) as f32) as u32 % subs.max(1)), 2 => { let s = self.seq.entry(track).or_insert(0); let v = (*s as u32) % subs.max(1); *s += 1; Some(v) } _ => Some(0) };
            for item in items.iter().filter(|it| sub.is_none_or(|s| it.track == s)) {
                let Some(src) = sources.iter().find(|s| s.media == item.source).or(sources.first()).cloned() else { continue };
                let at_ms = item.play_at + item.begin - entry;
                let when = if at_ms >= 0. { start + ms_frames(at_ms) } else { start.saturating_sub(ms_frames(-at_ms)) };
                let until = item.duration + item.end;
                self.sound(track, &src, &ctx, fade, when, Some(mid), item.begin.max(0.), Some(until));
            }
        }
        ms_frames((exit - entry).max(1.))
    }
}
