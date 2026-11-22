//! Binary UDP protocol and 16-player unique pants color palette for Blotworx Online Multiplayer.

use std::hash::{Hash, Hasher};

pub const MAX_PLAYERS: usize = 16;
pub const DEFAULT_PORT: u16 = 27077;
pub const PROTOCOL_MAGIC: &[u8; 4] = b"BWMP";
pub const PROTOCOL_VERSION: u8 = 1;

/// Action/animation bitflags packed into `PlayerState::flags`.
pub const FLAG_GROUNDED: u8 = 1 << 0;
pub const FLAG_JUMPING: u8 = 1 << 1;
pub const FLAG_SPINNING: u8 = 1 << 2;
pub const FLAG_SPRAY_PAINT: u8 = 1 << 3;
pub const FLAG_SPRAY_THINNER: u8 = 1 << 4;
pub const FLAG_SPEAKING: u8 = 1 << 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PantsColor {
    pub name: &'static str,
    pub rgb: [u8; 3],
}

impl PantsColor {
    pub fn egui_color(self) -> egui::Color32 {
        egui::Color32::from_rgb(self.rgb[0], self.rgb[1], self.rgb[2])
    }

    /// Brightened variant for crisp overhead username & chat readability on dark backgrounds.
    pub fn text_color(self) -> egui::Color32 {
        let lift = |c: u8| -> u8 {
            let f = c as f32;
            (f + (255.0 - f) * 0.22).round().clamp(0.0, 255.0) as u8
        };
        egui::Color32::from_rgb(lift(self.rgb[0]), lift(self.rgb[1]), lift(self.rgb[2]))
    }
}

/// 16 unmistakable, high-contrast Mickey shorts colors (one unique slot per player).
pub const PANTS_PALETTE: [PantsColor; MAX_PLAYERS] = [
    PantsColor { name: "Classic Red",     rgb: [224,  34,  38] },
    PantsColor { name: "Royal Blue",      rgb: [ 36, 114, 240] },
    PantsColor { name: "Emerald Green",   rgb: [ 32, 196,  86] },
    PantsColor { name: "Sunset Gold",     rgb: [244, 196,  28] },
    PantsColor { name: "Vivid Purple",    rgb: [158,  64, 240] },
    PantsColor { name: "Aqua Cyan",       rgb: [ 24, 206, 224] },
    PantsColor { name: "Tangerine",       rgb: [248, 124,  24] },
    PantsColor { name: "Hot Magenta",     rgb: [240,  52, 174] },
    PantsColor { name: "Lime Punch",      rgb: [148, 228,  32] },
    PantsColor { name: "Coral Pink",      rgb: [250, 110, 122] },
    PantsColor { name: "Electric Indigo", rgb: [ 92,  86, 242] },
    PantsColor { name: "Mint Teal",       rgb: [ 38, 216, 156] },
    PantsColor { name: "Amber Bronze",    rgb: [220, 148,  52] },
    PantsColor { name: "Sky Azure",       rgb: [ 96, 186, 252] },
    PantsColor { name: "Orchid",          rgb: [204, 122, 250] },
    PantsColor { name: "Crimson Berry",   rgb: [198,  26,  98] },
];

pub fn pants_color(slot: u8) -> PantsColor {
    PANTS_PALETTE[(slot as usize) % MAX_PLAYERS]
}

/// Deterministic preferred color slot derived from a player's username.
pub fn preferred_color_slot(username: &str) -> u8 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    username.trim().to_ascii_lowercase().hash(&mut h);
    (h.finish() as usize % MAX_PLAYERS) as u8
}

/// Stable 32-bit FNV-1a hash of a level path (case/slash normalized) for fast per-packet level matching.
pub fn level_hash(level: &str) -> u32 {
    let mut hash: u32 = 0x811c9dc5;
    for b in level.bytes() {
        let c = if b == b'\\' { b'/' } else { b.to_ascii_lowercase() };
        hash ^= c as u32;
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

pub fn sanitize_username(raw: &str) -> String {
    let clean: String = raw
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(24)
        .collect();
    if clean.is_empty() {
        "Mickey".to_owned()
    } else {
        clean
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlayerInfo {
    pub id: u8,
    pub color_slot: u8,
    pub username: String,
    pub level: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NetPlayerState {
    pub id: u8,
    pub color_slot: u8,
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    pub yaw: f32,
    pub flags: u8,
    pub anim_speed: f32,
    pub level_hash: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatPacket {
    pub msg_id: u32,
    pub sender_id: u8,
    pub color_slot: u8,
    pub sender: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Packet {
    JoinRequest {
        username: String,
        level: String,
    },
    JoinAccept {
        player_id: u8,
        color_slot: u8,
        session_id: String,
        host_level: String,
        players: Vec<PlayerInfo>,
    },
    JoinReject {
        reason: String,
    },
    Leave {
        player_id: u8,
    },
    Roster {
        host_level: String,
        players: Vec<PlayerInfo>,
    },
    PlayerState {
        seq: u32,
        state: NetPlayerState,
    },
    WorldSnapshot {
        tick: u32,
        players: Vec<NetPlayerState>,
    },
    Chat(ChatPacket),
    VoiceFrame {
        sender_id: u8,
        seq: u16,
        pos: [f32; 3],
        level_hash: u32,
        ulaw: Vec<u8>,
    },
    LevelChange {
        player_id: u8,
        level: String,
    },
    Heartbeat {
        player_id: u8,
    },
}

const TAG_JOIN_REQ: u8 = 1;
const TAG_JOIN_ACCEPT: u8 = 2;
const TAG_JOIN_REJECT: u8 = 3;
const TAG_LEAVE: u8 = 4;
const TAG_ROSTER: u8 = 5;
const TAG_PLAYER_STATE: u8 = 6;
const TAG_WORLD_SNAPSHOT: u8 = 7;
const TAG_CHAT: u8 = 8;
const TAG_VOICE: u8 = 9;
const TAG_LEVEL_CHANGE: u8 = 10;
const TAG_HEARTBEAT: u8 = 11;

fn put_u8(out: &mut Vec<u8>, v: u8) {
    out.push(v);
}
fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_f32(out: &mut Vec<u8>, v: f32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_str(out: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    let len = bytes.len().min(u16::MAX as usize);
    put_u16(out, len as u16);
    out.extend_from_slice(&bytes[..len]);
}
fn put_state(out: &mut Vec<u8>, s: &NetPlayerState) {
    put_u8(out, s.id);
    put_u8(out, s.color_slot);
    for f in s.pos {
        put_f32(out, f);
    }
    for f in s.vel {
        put_f32(out, f);
    }
    put_f32(out, s.yaw);
    put_u8(out, s.flags);
    put_f32(out, s.anim_speed);
    put_u32(out, s.level_hash);
}
fn put_info(out: &mut Vec<u8>, p: &PlayerInfo) {
    put_u8(out, p.id);
    put_u8(out, p.color_slot);
    put_str(out, &p.username);
    put_str(out, &p.level);
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let slice = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn f32(&mut self) -> Option<f32> {
        let v = f32::from_le_bytes(self.take(4)?.try_into().ok()?);
        if v.is_finite() { Some(v) } else { Some(0.0) }
    }
    fn str(&mut self) -> Option<String> {
        let len = self.u16()? as usize;
        let bytes = self.take(len)?;
        Some(String::from_utf8_lossy(bytes).into_owned())
    }
    fn state(&mut self) -> Option<NetPlayerState> {
        Some(NetPlayerState {
            id: self.u8()?,
            color_slot: self.u8()?,
            pos: [self.f32()?, self.f32()?, self.f32()?],
            vel: [self.f32()?, self.f32()?, self.f32()?],
            yaw: self.f32()?,
            flags: self.u8()?,
            anim_speed: self.f32()?,
            level_hash: self.u32()?,
        })
    }
    fn info(&mut self) -> Option<PlayerInfo> {
        Some(PlayerInfo {
            id: self.u8()?,
            color_slot: self.u8()?,
            username: self.str()?,
            level: self.str()?,
        })
    }
}

impl Packet {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(128);
        out.extend_from_slice(PROTOCOL_MAGIC);
        put_u8(&mut out, PROTOCOL_VERSION);
        match self {
            Packet::JoinRequest { username, level } => {
                put_u8(&mut out, TAG_JOIN_REQ);
                put_str(&mut out, username);
                put_str(&mut out, level);
            }
            Packet::JoinAccept {
                player_id,
                color_slot,
                session_id,
                host_level,
                players,
            } => {
                put_u8(&mut out, TAG_JOIN_ACCEPT);
                put_u8(&mut out, *player_id);
                put_u8(&mut out, *color_slot);
                put_str(&mut out, session_id);
                put_str(&mut out, host_level);
                put_u8(&mut out, players.len().min(MAX_PLAYERS) as u8);
                for p in players.iter().take(MAX_PLAYERS) {
                    put_info(&mut out, p);
                }
            }
            Packet::JoinReject { reason } => {
                put_u8(&mut out, TAG_JOIN_REJECT);
                put_str(&mut out, reason);
            }
            Packet::Leave { player_id } => {
                put_u8(&mut out, TAG_LEAVE);
                put_u8(&mut out, *player_id);
            }
            Packet::Roster {
                host_level,
                players,
            } => {
                put_u8(&mut out, TAG_ROSTER);
                put_str(&mut out, host_level);
                put_u8(&mut out, players.len().min(MAX_PLAYERS) as u8);
                for p in players.iter().take(MAX_PLAYERS) {
                    put_info(&mut out, p);
                }
            }
            Packet::PlayerState { seq, state } => {
                put_u8(&mut out, TAG_PLAYER_STATE);
                put_u32(&mut out, *seq);
                put_state(&mut out, state);
            }
            Packet::WorldSnapshot { tick, players } => {
                put_u8(&mut out, TAG_WORLD_SNAPSHOT);
                put_u32(&mut out, *tick);
                put_u8(&mut out, players.len().min(MAX_PLAYERS) as u8);
                for s in players.iter().take(MAX_PLAYERS) {
                    put_state(&mut out, s);
                }
            }
            Packet::Chat(c) => {
                put_u8(&mut out, TAG_CHAT);
                put_u32(&mut out, c.msg_id);
                put_u8(&mut out, c.sender_id);
                put_u8(&mut out, c.color_slot);
                put_str(&mut out, &c.sender);
                put_str(&mut out, &c.text);
            }
            Packet::VoiceFrame {
                sender_id,
                seq,
                pos,
                level_hash,
                ulaw,
            } => {
                put_u8(&mut out, TAG_VOICE);
                put_u8(&mut out, *sender_id);
                put_u16(&mut out, *seq);
                for f in *pos {
                    put_f32(&mut out, f);
                }
                put_u32(&mut out, *level_hash);
                let len = ulaw.len().min(1024);
                put_u16(&mut out, len as u16);
                out.extend_from_slice(&ulaw[..len]);
            }
            Packet::LevelChange { player_id, level } => {
                put_u8(&mut out, TAG_LEVEL_CHANGE);
                put_u8(&mut out, *player_id);
                put_str(&mut out, level);
            }
            Packet::Heartbeat { player_id } => {
                put_u8(&mut out, TAG_HEARTBEAT);
                put_u8(&mut out, *player_id);
            }
        }
        out
    }

    pub fn decode(data: &[u8]) -> Option<Self> {
        let mut r = Reader::new(data);
        if r.take(4)? != PROTOCOL_MAGIC {
            return None;
        }
        if r.u8()? != PROTOCOL_VERSION {
            return None;
        }
        match r.u8()? {
            TAG_JOIN_REQ => Some(Packet::JoinRequest {
                username: r.str()?,
                level: r.str()?,
            }),
            TAG_JOIN_ACCEPT => {
                let player_id = r.u8()?;
                let color_slot = r.u8()?;
                let session_id = r.str()?;
                let host_level = r.str()?;
                let count = (r.u8()? as usize).min(MAX_PLAYERS);
                let mut players = Vec::with_capacity(count);
                for _ in 0..count {
                    players.push(r.info()?);
                }
                Some(Packet::JoinAccept {
                    player_id,
                    color_slot,
                    session_id,
                    host_level,
                    players,
                })
            }
            TAG_JOIN_REJECT => Some(Packet::JoinReject { reason: r.str()? }),
            TAG_LEAVE => Some(Packet::Leave { player_id: r.u8()? }),
            TAG_ROSTER => {
                let host_level = r.str()?;
                let count = (r.u8()? as usize).min(MAX_PLAYERS);
                let mut players = Vec::with_capacity(count);
                for _ in 0..count {
                    players.push(r.info()?);
                }
                Some(Packet::Roster {
                    host_level,
                    players,
                })
            }
            TAG_PLAYER_STATE => Some(Packet::PlayerState {
                seq: r.u32()?,
                state: r.state()?,
            }),
            TAG_WORLD_SNAPSHOT => {
                let tick = r.u32()?;
                let count = (r.u8()? as usize).min(MAX_PLAYERS);
                let mut players = Vec::with_capacity(count);
                for _ in 0..count {
                    players.push(r.state()?);
                }
                Some(Packet::WorldSnapshot { tick, players })
            }
            TAG_CHAT => Some(Packet::Chat(ChatPacket {
                msg_id: r.u32()?,
                sender_id: r.u8()?,
                color_slot: r.u8()?,
                sender: r.str()?,
                text: r.str()?,
            })),
            TAG_VOICE => {
                let sender_id = r.u8()?;
                let seq = r.u16()?;
                let pos = [r.f32()?, r.f32()?, r.f32()?];
                let level_hash = r.u32()?;
                let len = (r.u16()? as usize).min(1024);
                let ulaw = r.take(len)?.to_vec();
                Some(Packet::VoiceFrame {
                    sender_id,
                    seq,
                    pos,
                    level_hash,
                    ulaw,
                })
            }
            TAG_LEVEL_CHANGE => Some(Packet::LevelChange {
                player_id: r.u8()?,
                level: r.str()?,
            }),
            TAG_HEARTBEAT => Some(Packet::Heartbeat { player_id: r.u8()? }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_packets() {
        let pkts = [
            Packet::JoinRequest {
                username: "Oswald".into(),
                level: "Levels/GV_ZoneA_Start.level".into(),
            },
            Packet::JoinAccept {
                player_id: 3,
                color_slot: 7,
                session_id: "bw-12345".into(),
                host_level: "Levels/GV_ZoneA_Start.level".into(),
                players: vec![PlayerInfo {
                    id: 0,
                    color_slot: 0,
                    username: "Host".into(),
                    level: "Levels/GV_ZoneA_Start.level".into(),
                }],
            },
            Packet::WorldSnapshot {
                tick: 42,
                players: vec![NetPlayerState {
                    id: 1,
                    color_slot: 2,
                    pos: [1.0, 2.0, 3.0],
                    vel: [0.5, 0.0, -0.5],
                    yaw: 1.57,
                    flags: FLAG_GROUNDED | FLAG_SPEAKING,
                    anim_speed: 4.2,
                    level_hash: level_hash("Levels/GV_ZoneA_Start.level"),
                }],
            },
            Packet::Chat(ChatPacket {
                msg_id: 99,
                sender_id: 1,
                color_slot: 2,
                sender: "Mickey2".into(),
                text: "Hello Wasteland!".into(),
            }),
            Packet::VoiceFrame {
                sender_id: 4,
                seq: 12,
                pos: [-10.0, 1.0, 4.0],
                level_hash: 0x12345678,
                ulaw: vec![0x7f; 320],
            },
        ];
        for p in pkts {
            let enc = p.encode();
            let dec = Packet::decode(&enc).expect("decode packet");
            assert_eq!(p, dec);
        }
    }
}
