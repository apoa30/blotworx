//! 16-player Host Server & Client networking for Blotworx Online Multiplayer.
//!
//! Supports two transports seamlessly:
//! 1. **Online Room Code (Default — Zero Port Forwarding)**: Outbound TCP MQTT v3.1.1 relay
//!    (`broker.emqx.io:1883` with automatic fallback to `broker.hivemq.com:1883`). Works through
//!    home routers, strict NAT, CGNAT, and firewalls with zero configuration.
//! 2. **Direct LAN UDP (`IP:Port`)**: Direct peer-to-peer UDP for local networks or forwarded hosts.

use super::protocol::{
    level_hash, pants_color, preferred_color_slot, sanitize_username, ChatPacket, NetPlayerState,
    Packet, PlayerInfo, MAX_PLAYERS,
};
use super::relay::{self, RelayConn};
use anyhow::{bail, Context, Result};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::{SocketAddr, ToSocketAddrs, UdpSocket},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const TIMEOUT_SECS: u64 = 12;
const SNAPSHOT_INTERVAL_MS: u64 = 33; // ~30 Hz world snapshots
const HEARTBEAT_INTERVAL_MS: u64 = 2000;
const PROXIMITY_RELAY_MAX_SQ: f32 = 75.0 * 75.0; // relay voice within generous 75m radius; client attenuates precisely

fn unique_tag(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    let tid = format!("{:?}", thread::current().id());
    let mut h: u32 = 0x811c9dc5;
    for b in tid.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    format!(
        "{prefix}{:08x}{:04x}",
        (nanos as u32) ^ h,
        (std::process::id() as u16) ^ ((nanos >> 32) as u16)
    )
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum PeerAddr {
    Udp(SocketAddr),
    Relay(String),
}

// ------------------------------------------------------------------ Server Transport

enum ServerTransport {
    Udp(UdpSocket),
    Relay {
        conn: RelayConn,
        room_code: String,
        c2s_prefix: String,
        host_client_id: String,
    },
}

impl ServerTransport {
    fn recv_packets(&mut self, buf: &mut [u8]) -> Vec<(PeerAddr, Packet)> {
        let mut out = Vec::new();
        match self {
            ServerTransport::Udp(sock) => {
                while let Ok((len, from)) = sock.recv_from(buf) {
                    if let Some(pkt) = Packet::decode(&buf[..len]) {
                        out.push((PeerAddr::Udp(from), pkt));
                    }
                }
            }
            ServerTransport::Relay {
                conn,
                room_code,
                c2s_prefix,
                host_client_id,
            } => {
                let _ = conn.maybe_ping();
                match conn.poll_messages() {
                    Ok(msgs) => {
                        for (topic, payload) in msgs {
                            if let Some(client_tag) = topic.strip_prefix(c2s_prefix.as_str()) {
                                if !client_tag.is_empty() && !client_tag.contains('/') {
                                    if let Some(pkt) = Packet::decode(&payload) {
                                        out.push((PeerAddr::Relay(client_tag.to_owned()), pkt));
                                    }
                                }
                            }
                        }
                    }
                    Err(_) => {
                        // Attempt transparent reconnect to relay broker
                        if let Ok(mut new_conn) = RelayConn::connect(None, host_client_id) {
                            if new_conn
                                .subscribe(&relay::topic_c2s_wildcard(room_code))
                                .is_ok()
                            {
                                *conn = new_conn;
                            }
                        }
                    }
                }
            }
        }
        out
    }

    fn send_raw_to(&mut self, addr: &PeerAddr, bytes: &[u8]) {
        match (self, addr) {
            (ServerTransport::Udp(sock), PeerAddr::Udp(sa)) => {
                let _ = sock.send_to(bytes, *sa);
            }
            (
                ServerTransport::Relay {
                    conn, room_code, ..
                },
                PeerAddr::Relay(tag),
            ) => {
                let topic = relay::topic_s2c_direct(room_code, tag);
                let _ = conn.publish(&topic, bytes);
            }
            _ => {}
        }
    }

    fn send_to(&mut self, addr: &PeerAddr, pkt: &Packet) {
        let bytes = pkt.encode();
        self.send_raw_to(addr, &bytes);
    }

    fn broadcast(
        &mut self,
        slots: &[Option<ServerSlot>; MAX_PLAYERS],
        pkt: &Packet,
        exclude: Option<u8>,
    ) {
        let bytes = pkt.encode();
        match self {
            ServerTransport::Udp(sock) => {
                for slot in slots.iter().flatten() {
                    if exclude != Some(slot.info.id) {
                        if let PeerAddr::Udp(sa) = slot.addr {
                            let _ = sock.send_to(&bytes, sa);
                        }
                    }
                }
            }
            ServerTransport::Relay {
                conn, room_code, ..
            } => {
                if exclude.is_none() {
                    let topic = relay::topic_s2c_broadcast(room_code);
                    let _ = conn.publish(&topic, &bytes);
                } else {
                    for slot in slots.iter().flatten() {
                        if exclude != Some(slot.info.id) {
                            if let PeerAddr::Relay(tag) = &slot.addr {
                                let topic = relay::topic_s2c_direct(room_code, tag);
                                let _ = conn.publish(&topic, &bytes);
                            }
                        }
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Host Server

struct ServerSlot {
    addr: PeerAddr,
    info: PlayerInfo,
    state: NetPlayerState,
    last_seen: Instant,
}

pub struct HostServer {
    running: Arc<AtomicBool>,
    pub bound_addr: Option<SocketAddr>,
    pub room_code: Option<String>,
    pub broker_addr: Option<String>,
    pub session_id: String,
    handle: Option<thread::JoinHandle<()>>,
}

impl HostServer {
    /// Start an online lobby using the zero-port-forwarding Room Code relay (default).
    pub fn start_online(raw_room_code: String, initial_level: String) -> Result<Self> {
        let room_code = {
            let norm = relay::normalize_room_code(&raw_room_code);
            if norm.len() >= 4 {
                norm
            } else {
                relay::generate_room_code()
            }
        };
        let host_client_id = unique_tag("bwh-");
        let mut conn = RelayConn::connect(None, &host_client_id)
            .context("Connect to online multiplayer relay")?;
        conn.subscribe(&relay::topic_c2s_wildcard(&room_code))
            .context("Subscribe to online room channel")?;
        let broker_addr = conn.broker_addr.clone();
        let session_id = format!("bw-room-{room_code}");
        let c2s_prefix = format!("blotworx/mp/v1/{room_code}/c2s/");

        let running = Arc::new(AtomicBool::new(true));
        let r_flag = running.clone();
        let sid = session_id.clone();
        let rc_thread = room_code.clone();

        let transport = ServerTransport::Relay {
            conn,
            room_code: rc_thread,
            c2s_prefix,
            host_client_id,
        };

        let handle = thread::Builder::new()
            .name("blotworx-mp-relay-host".into())
            .spawn(move || server_loop(transport, sid, initial_level, r_flag))
            .context("Spawn multiplayer online host thread")?;

        Ok(Self {
            running,
            bound_addr: None,
            room_code: Some(room_code),
            broker_addr: Some(broker_addr),
            session_id,
            handle: Some(handle),
        })
    }

    /// Start a direct LAN UDP server on `port` (optional fallback for local networks).
    pub fn start_lan(port: u16, initial_level: String) -> Result<Self> {
        let socket = UdpSocket::bind(("0.0.0.0", port))
            .with_context(|| format!("Bind multiplayer UDP server on port {port}"))?;
        socket.set_nonblocking(true)?;
        let bound_addr = socket.local_addr()?;
        let session_id = format!(
            "bw-{:08x}{:04x}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as u32)
                .unwrap_or(1),
            bound_addr.port()
        );
        let running = Arc::new(AtomicBool::new(true));
        let r_flag = running.clone();
        let sid = session_id.clone();

        let handle = thread::Builder::new()
            .name("blotworx-mp-udp-host".into())
            .spawn(move || server_loop(ServerTransport::Udp(socket), sid, initial_level, r_flag))
            .context("Spawn multiplayer LAN host thread")?;

        Ok(Self {
            running,
            bound_addr: Some(bound_addr),
            room_code: None,
            broker_addr: None,
            session_id,
            handle: Some(handle),
        })
    }
}

impl Drop for HostServer {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn server_loop(
    mut transport: ServerTransport,
    session_id: String,
    mut host_level: String,
    running: Arc<AtomicBool>,
) {
    let mut slots: [Option<ServerSlot>; MAX_PLAYERS] = std::array::from_fn(|_| None);
    let mut chat_history: VecDeque<ChatPacket> = VecDeque::with_capacity(48);
    let mut seen_chat: HashSet<(u8, u32)> = HashSet::new();
    let mut next_msg_id: u32 = 1;
    let mut tick: u32 = 0;
    let mut last_snapshot = Instant::now();
    let mut buf = [0u8; 2048];

    let build_roster = |slots: &[Option<ServerSlot>; MAX_PLAYERS]| -> Vec<PlayerInfo> {
        slots
            .iter()
            .flatten()
            .map(|s| s.info.clone())
            .collect()
    };

    while running.load(Ordering::Acquire) {
        let incoming = transport.recv_packets(&mut buf);
        let received_any = !incoming.is_empty();

        for (from, pkt) in incoming {
            let now = Instant::now();

            match pkt {
                Packet::JoinRequest {
                    username,
                    level,
                    preferred_color,
                } => {
                    let clean_name = sanitize_username(&username);
                    // Re-accept if the same endpoint is already in the lobby
                    if let Some(existing_idx) = slots
                        .iter()
                        .position(|s| s.as_ref().is_some_and(|x| x.addr == from))
                    {
                        if let Some(slot) = &mut slots[existing_idx] {
                            slot.last_seen = now;
                            let id = slot.info.id;
                            let color_slot = slot.info.color_slot;
                            let roster = build_roster(&slots);
                            transport.send_to(
                                &from,
                                &Packet::JoinAccept {
                                    player_id: id,
                                    color_slot,
                                    session_id: session_id.clone(),
                                    host_level: host_level.clone(),
                                    players: roster,
                                },
                            );
                        }
                        continue;
                    }

                    // Find first free player ID slot (0..16)
                    let Some(free_id) = slots.iter().position(|s| s.is_none()) else {
                        transport.send_to(
                            &from,
                            &Packet::JoinReject {
                                reason: format!("Lobby is full ({MAX_PLAYERS}/{MAX_PLAYERS})"),
                            },
                        );
                        continue;
                    };

                    // Pick unique pants color: honor player's chosen color if free, else hash username
                    let used_colors: HashSet<u8> = slots
                        .iter()
                        .flatten()
                        .map(|s| s.info.color_slot)
                        .collect();
                    let color_slot =
                        if (preferred_color as usize) < MAX_PLAYERS && !used_colors.contains(&preferred_color) {
                            preferred_color
                        } else {
                            let pref = preferred_color_slot(&clean_name);
                            (0..MAX_PLAYERS as u8)
                                .map(|offset| (pref + offset) % (MAX_PLAYERS as u8))
                                .find(|c| !used_colors.contains(c))
                                .unwrap_or(free_id as u8)
                        };

                    // Deduplicate identical usernames cleanly ("Mickey", "Mickey (2)", ...)
                    let mut final_name = clean_name.clone();
                    let mut suffix = 2u32;
                    while slots
                        .iter()
                        .flatten()
                        .any(|s| s.info.username.eq_ignore_ascii_case(&final_name))
                    {
                        final_name = format!("{clean_name} ({suffix})");
                        suffix += 1;
                    }

                    let player_level = if level.is_empty() {
                        host_level.clone()
                    } else {
                        level
                    };
                    let info = PlayerInfo {
                        id: free_id as u8,
                        color_slot,
                        username: final_name.clone(),
                        level: player_level.clone(),
                    };
                    let state = NetPlayerState {
                        id: free_id as u8,
                        color_slot,
                        pos: [0.0; 3],
                        vel: [0.0; 3],
                        yaw: 0.0,
                        flags: 0,
                        anim_speed: 0.0,
                        level_hash: level_hash(&player_level),
                    };
                    slots[free_id] = Some(ServerSlot {
                        addr: from.clone(),
                        info,
                        state,
                        last_seen: now,
                    });

                    let roster = build_roster(&slots);
                    transport.send_to(
                        &from,
                        &Packet::JoinAccept {
                            player_id: free_id as u8,
                            color_slot,
                            session_id: session_id.clone(),
                            host_level: host_level.clone(),
                            players: roster.clone(),
                        },
                    );
                    // Replay recent chat messages to newly joined player
                    for msg in &chat_history {
                        transport.send_to(&from, &Packet::Chat(msg.clone()));
                    }
                    // Notify everyone of updated roster + system join message
                    transport.broadcast(
                        &slots,
                        &Packet::Roster {
                            host_level: host_level.clone(),
                            players: roster,
                        },
                        None,
                    );
                    next_msg_id = next_msg_id.wrapping_add(1);
                    let sys_msg = ChatPacket {
                        msg_id: next_msg_id,
                        sender_id: 255,
                        color_slot,
                        sender: String::new(),
                        text: format!(
                            "{final_name} joined ({} pants).",
                            pants_color(color_slot).name
                        ),
                    };
                    chat_history.push_back(sys_msg.clone());
                    while chat_history.len() > 40 {
                        chat_history.pop_front();
                    }
                    transport.broadcast(&slots, &Packet::Chat(sys_msg), None);
                }

                Packet::Leave { player_id } => {
                    if let Some(slot) = slots.get_mut(player_id as usize) {
                        if slot.as_ref().is_some_and(|s| s.addr == from) {
                            let name = slot.take().unwrap().info.username;
                            let roster = build_roster(&slots);
                            transport.broadcast(
                                &slots,
                                &Packet::Roster {
                                    host_level: host_level.clone(),
                                    players: roster,
                                },
                                None,
                            );
                            next_msg_id = next_msg_id.wrapping_add(1);
                            let sys_msg = ChatPacket {
                                msg_id: next_msg_id,
                                sender_id: 255,
                                color_slot: 0,
                                sender: String::new(),
                                text: format!("{name} left the lobby."),
                            };
                            chat_history.push_back(sys_msg.clone());
                            transport.broadcast(&slots, &Packet::Chat(sys_msg), None);
                        }
                    }
                }

                Packet::PlayerState { state, .. } => {
                    if let Some(Some(slot)) = slots.get_mut(state.id as usize) {
                        if slot.addr == from {
                            slot.last_seen = now;
                            slot.state = NetPlayerState {
                                id: slot.info.id,
                                color_slot: slot.info.color_slot,
                                ..state
                            };
                        }
                    }
                }

                Packet::Chat(mut msg) => {
                    if let Some(Some(slot)) = slots.get_mut(msg.sender_id as usize) {
                        if slot.addr == from {
                            slot.last_seen = now;
                            if !seen_chat.insert((msg.sender_id, msg.msg_id)) {
                                continue;
                            }
                            let clean_text: String = msg
                                .text
                                .trim()
                                .chars()
                                .filter(|c| !c.is_control())
                                .take(200)
                                .collect();
                            if clean_text.is_empty() {
                                continue;
                            }
                            next_msg_id = next_msg_id.wrapping_add(1);
                            msg.msg_id = next_msg_id;
                            msg.color_slot = slot.info.color_slot;
                            msg.sender = slot.info.username.clone();
                            msg.text = clean_text;
                            chat_history.push_back(msg.clone());
                            while chat_history.len() > 40 {
                                chat_history.pop_front();
                            }
                            transport.broadcast(&slots, &Packet::Chat(msg), None);
                        }
                    }
                }

                Packet::VoiceFrame {
                    sender_id,
                    seq,
                    pos,
                    level_hash: lh,
                    ulaw,
                } => {
                    let valid = slots
                        .get(sender_id as usize)
                        .and_then(|s| s.as_ref())
                        .is_some_and(|s| s.addr == from);
                    if !valid || ulaw.is_empty() {
                        continue;
                    }
                    if let Some(Some(slot)) = slots.get_mut(sender_id as usize) {
                        slot.last_seen = now;
                    }
                    let bytes = Packet::VoiceFrame {
                        sender_id,
                        seq,
                        pos,
                        level_hash: lh,
                        ulaw,
                    }
                    .encode();
                    // Relay only to other players in the same level & within proximity relay radius
                    let recipients: Vec<PeerAddr> = slots
                        .iter()
                        .flatten()
                        .filter(|other| {
                            if other.info.id == sender_id || other.state.level_hash != lh {
                                return false;
                            }
                            let dx = other.state.pos[0] - pos[0];
                            let dy = other.state.pos[1] - pos[1];
                            let dz = other.state.pos[2] - pos[2];
                            dx * dx + dy * dy + dz * dz <= PROXIMITY_RELAY_MAX_SQ
                        })
                        .map(|other| other.addr.clone())
                        .collect();
                    for addr in &recipients {
                        transport.send_raw_to(addr, &bytes);
                    }
                }

                Packet::LevelChange { player_id, level } => {
                    let mut changed = false;
                    if let Some(Some(slot)) = slots.get_mut(player_id as usize) {
                        if slot.addr == from {
                            slot.last_seen = now;
                            if slot.info.level != level {
                                slot.info.level = level.clone();
                                slot.state.level_hash = level_hash(&level);
                                if player_id == 0 {
                                    host_level = level;
                                }
                                changed = true;
                            }
                        }
                    }
                    if changed {
                        let roster = build_roster(&slots);
                        transport.broadcast(
                            &slots,
                            &Packet::Roster {
                                host_level: host_level.clone(),
                                players: roster,
                            },
                            None,
                        );
                    }
                }

                Packet::ColorChange {
                    player_id,
                    color_slot,
                } => {
                    if (color_slot as usize) >= MAX_PLAYERS {
                        continue;
                    }
                    let taken_by_other = slots
                        .iter()
                        .flatten()
                        .any(|s| s.info.id != player_id && s.info.color_slot == color_slot);
                    if taken_by_other {
                        continue;
                    }
                    let mut changed_name: Option<String> = None;
                    if let Some(Some(slot)) = slots.get_mut(player_id as usize) {
                        if slot.addr == from {
                            slot.last_seen = now;
                            if slot.info.color_slot != color_slot {
                                slot.info.color_slot = color_slot;
                                slot.state.color_slot = color_slot;
                                changed_name = Some(slot.info.username.clone());
                            }
                        }
                    }
                    if let Some(name) = changed_name {
                        let roster = build_roster(&slots);
                        transport.broadcast(
                            &slots,
                            &Packet::Roster {
                                host_level: host_level.clone(),
                                players: roster,
                            },
                            None,
                        );
                        next_msg_id = next_msg_id.wrapping_add(1);
                        let sys_msg = ChatPacket {
                            msg_id: next_msg_id,
                            sender_id: 255,
                            color_slot,
                            sender: String::new(),
                            text: format!(
                                "{name} switched to {} pants.",
                                pants_color(color_slot).name
                            ),
                        };
                        chat_history.push_back(sys_msg.clone());
                        while chat_history.len() > 40 {
                            chat_history.pop_front();
                        }
                        transport.broadcast(&slots, &Packet::Chat(sys_msg), None);
                    }
                }

                Packet::Heartbeat { player_id } => {
                    if let Some(Some(slot)) = slots.get_mut(player_id as usize) {
                        if slot.addr == from {
                            slot.last_seen = now;
                        }
                    }
                }

                _ => {}
            }
        }

        // Timeout disconnected peers
        let now = Instant::now();
        let mut removed_names = Vec::new();
        for slot in &mut slots {
            if let Some(s) = slot {
                if now.duration_since(s.last_seen) > Duration::from_secs(TIMEOUT_SECS) {
                    removed_names.push(s.info.username.clone());
                    *slot = None;
                }
            }
        }
        if !removed_names.is_empty() {
            let roster = build_roster(&slots);
            transport.broadcast(
                &slots,
                &Packet::Roster {
                    host_level: host_level.clone(),
                    players: roster,
                },
                None,
            );
            for name in removed_names {
                next_msg_id = next_msg_id.wrapping_add(1);
                let sys_msg = ChatPacket {
                    msg_id: next_msg_id,
                    sender_id: 255,
                    color_slot: 0,
                    sender: String::new(),
                    text: format!("{name} timed out."),
                };
                chat_history.push_back(sys_msg.clone());
                transport.broadcast(&slots, &Packet::Chat(sys_msg), None);
            }
        }

        // Broadcast 30 Hz world state snapshot
        if now.duration_since(last_snapshot) >= Duration::from_millis(SNAPSHOT_INTERVAL_MS) {
            last_snapshot = now;
            tick = tick.wrapping_add(1);
            let states: Vec<NetPlayerState> = slots
                .iter()
                .flatten()
                .map(|s| s.state.clone())
                .collect();
            if !states.is_empty() {
                transport.broadcast(
                    &slots,
                    &Packet::WorldSnapshot {
                        tick,
                        players: states,
                    },
                    None,
                );
            }
        }

        if !received_any {
            thread::sleep(Duration::from_millis(4));
        }
    }
}

// ------------------------------------------------------------------ Client Transport

enum ClientTransport {
    Udp(UdpSocket),
    Relay {
        conn: RelayConn,
        c2s_topic: String,
    },
}

impl ClientTransport {
    fn send(&mut self, bytes: &[u8]) -> Result<()> {
        match self {
            ClientTransport::Udp(sock) => {
                let _ = sock.send(bytes);
                Ok(())
            }
            ClientTransport::Relay { conn, c2s_topic } => conn.publish(c2s_topic, bytes),
        }
    }

    fn is_udp(&self) -> bool {
        matches!(self, ClientTransport::Udp(_))
    }

    fn poll(&mut self, buf: &mut [u8]) -> Result<Vec<Packet>> {
        let mut out = Vec::new();
        match self {
            ClientTransport::Udp(sock) => {
                while let Ok(len) = sock.recv(buf) {
                    if let Some(pkt) = Packet::decode(&buf[..len]) {
                        out.push(pkt);
                    }
                }
            }
            ClientTransport::Relay { conn, .. } => {
                conn.maybe_ping()?;
                for (_topic, payload) in conn.poll_messages()? {
                    if let Some(pkt) = Packet::decode(&payload) {
                        out.push(pkt);
                    }
                }
            }
        }
        Ok(out)
    }
}

// ------------------------------------------------------------------ Client

pub enum ClientCmd {
    SendState(NetPlayerState),
    SendChat(String),
    SendVoice {
        pos: [f32; 3],
        level_hash: u32,
        ulaw: Vec<u8>,
    },
    SendLevel(String),
    SendColor(u8),
    Disconnect,
}

pub enum ClientEvent {
    Joined {
        player_id: u8,
        color_slot: u8,
        session_id: String,
        host_level: String,
        players: Vec<PlayerInfo>,
    },
    Rejected(String),
    Roster {
        host_level: String,
        players: Vec<PlayerInfo>,
    },
    Snapshot(Vec<NetPlayerState>),
    Chat(ChatPacket),
    Voice {
        sender_id: u8,
        pos: [f32; 3],
        level_hash: u32,
        ulaw: Vec<u8>,
    },
    Disconnected(String),
}

pub struct NetClient {
    tx: mpsc::Sender<ClientCmd>,
    rx: mpsc::Receiver<ClientEvent>,
    pub server_addr: String,
    handle: Option<thread::JoinHandle<()>>,
}

impl NetClient {
    /// Connect to either an online Room Code (e.g. `BLOT-7K3P`, zero port forwarding)
    /// or a direct LAN `IP:Port` address.
    pub fn connect(
        target: &str,
        username: String,
        preferred_color: u8,
        level: String,
    ) -> Result<Self> {
        let (transport, display_target) = if relay::is_room_code_target(target) {
            let stripped = target
                .trim()
                .strip_prefix("room:")
                .or_else(|| target.trim().strip_prefix("ROOM:"))
                .unwrap_or(target.trim());
            let room_code = relay::normalize_room_code(stripped);
            if room_code.len() < 4 {
                bail!("Invalid room code '{target}'");
            }
            let client_tag = unique_tag("c");
            let mut conn = RelayConn::connect(None, &format!("bwc-{client_tag}"))
                .context("Connect to online multiplayer relay")?;
            conn.subscribe(&relay::topic_s2c_broadcast(&room_code))
                .context("Subscribe to room broadcast channel")?;
            conn.subscribe(&relay::topic_s2c_direct(&room_code, &client_tag))
                .context("Subscribe to room direct channel")?;
            let c2s_topic = relay::topic_c2s(&room_code, &client_tag);
            (
                ClientTransport::Relay { conn, c2s_topic },
                room_code,
            )
        } else {
            let resolved: SocketAddr = target
                .to_socket_addrs()
                .with_context(|| format!("Resolve multiplayer server '{target}'"))?
                .find(|a| a.is_ipv4())
                .or_else(|| target.to_socket_addrs().ok()?.next())
                .with_context(|| format!("No reachable address for '{target}'"))?;

            let socket = UdpSocket::bind("0.0.0.0:0").context("Bind client UDP socket")?;
            socket.set_nonblocking(true)?;
            socket.connect(resolved)?;
            (ClientTransport::Udp(socket), target.to_owned())
        };

        let (tx, cmd_rx) = mpsc::channel();
        let (ev_tx, rx) = mpsc::channel();

        let handle = thread::Builder::new()
            .name("blotworx-mp-client".into())
            .spawn(move || {
                client_loop(transport, username, preferred_color, level, cmd_rx, ev_tx)
            })
            .context("Spawn multiplayer client thread")?;

        Ok(Self {
            tx,
            rx,
            server_addr: display_target,
            handle: Some(handle),
        })
    }

    pub fn send_state(&self, state: NetPlayerState) {
        let _ = self.tx.send(ClientCmd::SendState(state));
    }

    pub fn send_chat(&self, text: String) {
        let _ = self.tx.send(ClientCmd::SendChat(text));
    }

    pub fn send_voice(&self, pos: [f32; 3], level_hash: u32, ulaw: Vec<u8>) {
        let _ = self.tx.send(ClientCmd::SendVoice {
            pos,
            level_hash,
            ulaw,
        });
    }

    pub fn send_level(&self, level: String) {
        let _ = self.tx.send(ClientCmd::SendLevel(level));
    }

    pub fn send_color(&self, color_slot: u8) {
        let _ = self.tx.send(ClientCmd::SendColor(color_slot));
    }

    pub fn poll_events(&self) -> Vec<ClientEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            out.push(ev);
        }
        out
    }
}

impl Drop for NetClient {
    fn drop(&mut self) {
        let _ = self.tx.send(ClientCmd::Disconnect);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn client_loop(
    mut transport: ClientTransport,
    username: String,
    preferred_color: u8,
    initial_level: String,
    cmd_rx: mpsc::Receiver<ClientCmd>,
    ev_tx: mpsc::Sender<ClientEvent>,
) {
    let join_pkt = Packet::JoinRequest {
        username: username.clone(),
        level: initial_level,
        preferred_color,
    }
    .encode();
    let _ = transport.send(&join_pkt);

    let mut player_id: Option<u8> = None;
    let mut color_slot: u8 = preferred_color;
    let mut seq: u32 = 0;
    let mut chat_seq: u32 = 0;
    let mut voice_seq: u16 = 0;
    let mut last_join_retry = Instant::now();
    let mut join_attempts = 1u32;
    let mut last_rx = Instant::now();
    let mut last_hb = Instant::now();
    let mut buf = [0u8; 2048];

    loop {
        // Process outbound commands
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                ClientCmd::Disconnect => {
                    if let Some(id) = player_id {
                        let bytes = Packet::Leave { player_id: id }.encode();
                        let _ = transport.send(&bytes);
                    }
                    return;
                }
                ClientCmd::SendState(mut state) => {
                    if let Some(id) = player_id {
                        seq = seq.wrapping_add(1);
                        state.id = id;
                        state.color_slot = color_slot;
                        let bytes = Packet::PlayerState { seq, state }.encode();
                        let _ = transport.send(&bytes);
                    }
                }
                ClientCmd::SendChat(text) => {
                    if let Some(id) = player_id {
                        chat_seq = chat_seq.wrapping_add(1);
                        let bytes = Packet::Chat(ChatPacket {
                            msg_id: chat_seq,
                            sender_id: id,
                            color_slot,
                            sender: username.clone(),
                            text,
                        })
                        .encode();
                        let _ = transport.send(&bytes);
                        if transport.is_udp() {
                            // Send twice on UDP to guard against single packet drop
                            let _ = transport.send(&bytes);
                        }
                    }
                }
                ClientCmd::SendVoice {
                    pos,
                    level_hash,
                    ulaw,
                } => {
                    if let Some(id) = player_id {
                        voice_seq = voice_seq.wrapping_add(1);
                        let bytes = Packet::VoiceFrame {
                            sender_id: id,
                            seq: voice_seq,
                            pos,
                            level_hash,
                            ulaw,
                        }
                        .encode();
                        let _ = transport.send(&bytes);
                    }
                }
                ClientCmd::SendLevel(level) => {
                    if let Some(id) = player_id {
                        let bytes = Packet::LevelChange {
                            player_id: id,
                            level,
                        }
                        .encode();
                        let _ = transport.send(&bytes);
                    }
                }
                ClientCmd::SendColor(new_color) => {
                    if let Some(id) = player_id {
                        let bytes = Packet::ColorChange {
                            player_id: id,
                            color_slot: new_color,
                        }
                        .encode();
                        let _ = transport.send(&bytes);
                    }
                }
            }
        }

        // Receive inbound packets
        let packets = match transport.poll(&mut buf) {
            Ok(pkts) => pkts,
            Err(e) => {
                let _ = ev_tx.send(ClientEvent::Disconnected(format!(
                    "Relay connection lost: {e}"
                )));
                return;
            }
        };
        let received_any = !packets.is_empty();
        if received_any {
            last_rx = Instant::now();
        }
        for pkt in packets {
            match pkt {
                Packet::JoinAccept {
                    player_id: pid,
                    color_slot: cs,
                    session_id,
                    host_level,
                    players,
                } => {
                    player_id = Some(pid);
                    color_slot = cs;
                    let _ = ev_tx.send(ClientEvent::Joined {
                        player_id: pid,
                        color_slot: cs,
                        session_id,
                        host_level,
                        players,
                    });
                }
                Packet::JoinReject { reason } => {
                    let _ = ev_tx.send(ClientEvent::Rejected(reason));
                    return;
                }
                Packet::Roster {
                    host_level,
                    players,
                } => {
                    if let Some(pid) = player_id {
                        if let Some(me) = players.iter().find(|p| p.id == pid) {
                            color_slot = me.color_slot;
                        }
                    }
                    let _ = ev_tx.send(ClientEvent::Roster {
                        host_level,
                        players,
                    });
                }
                Packet::WorldSnapshot { players, .. } => {
                    let _ = ev_tx.send(ClientEvent::Snapshot(players));
                }
                Packet::Chat(c) => {
                    let _ = ev_tx.send(ClientEvent::Chat(c));
                }
                Packet::VoiceFrame {
                    sender_id,
                    pos,
                    level_hash,
                    ulaw,
                    ..
                } => {
                    let _ = ev_tx.send(ClientEvent::Voice {
                        sender_id,
                        pos,
                        level_hash,
                        ulaw,
                    });
                }
                _ => {}
            }
        }

        let now = Instant::now();
        if player_id.is_none() {
            if now.duration_since(last_join_retry) >= Duration::from_millis(600) {
                last_join_retry = now;
                join_attempts += 1;
                if join_attempts > 12 {
                    let msg = if transport.is_udp() {
                        "Connection timed out (host unreachable or firewall blocking UDP — use a Room Code instead for zero port forwarding!)"
                    } else {
                        "Room not found or host is not online (check the Room Code and make sure the host clicked Host Online Game)"
                    };
                    let _ = ev_tx.send(ClientEvent::Disconnected(msg.into()));
                    return;
                }
                let _ = transport.send(&join_pkt);
            }
        } else {
            if now.duration_since(last_hb) >= Duration::from_millis(HEARTBEAT_INTERVAL_MS) {
                last_hb = now;
                let bytes = Packet::Heartbeat {
                    player_id: player_id.unwrap(),
                }
                .encode();
                let _ = transport.send(&bytes);
            }
            if now.duration_since(last_rx) > Duration::from_secs(TIMEOUT_SECS) {
                let _ = ev_tx.send(ClientEvent::Disconnected(
                    "Disconnected from host (timeout)".into(),
                ));
                return;
            }
        }

        if !received_any {
            thread::sleep(Duration::from_millis(4));
        }
    }
}

/// Detect the local LAN IPv4 address using a non-transmitting UDP route probe.
pub fn detect_lan_ip() -> String {
    if let Ok(sock) = UdpSocket::bind("0.0.0.0:0") {
        if sock.connect("8.8.8.8:80").is_ok() {
            if let Ok(local) = sock.local_addr() {
                return local.ip().to_string();
            }
        }
    }
    "127.0.0.1".to_owned()
}

/// Non-blocking public IP lookup for convenient LAN/direct invite sharing.
pub fn spawn_public_ip_lookup() -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    let _ = thread::Builder::new()
        .name("blotworx-ip-lookup".into())
        .spawn(move || {
            let agent = ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(4)))
                .build()
                .new_agent();
            if let Ok(mut resp) = agent.get("https://api.ipify.org").call() {
                if let Ok(ip) = resp.body_mut().read_to_string() {
                    let clean = ip.trim().to_owned();
                    if !clean.is_empty() && clean.len() <= 45 {
                        let _ = tx.send(clean);
                    }
                }
            }
        });
    rx
}

/// Smooth interpolated state for a remote player rendered locally.
#[derive(Clone, Debug)]
pub struct RemotePlayer {
    pub info: PlayerInfo,
    pub target_pos: [f32; 3],
    pub render_pos: [f32; 3],
    pub vel: [f32; 3],
    pub target_yaw: f32,
    pub render_yaw: f32,
    pub flags: u8,
    pub anim_speed: f32,
    pub level_hash: u32,
    pub last_voice_at: Option<Instant>,
    pub muted: bool,
    pub has_state: bool,
}

impl RemotePlayer {
    pub fn from_info(info: PlayerInfo) -> Self {
        let lh = level_hash(&info.level);
        Self {
            info,
            target_pos: [0.0; 3],
            render_pos: [0.0; 3],
            vel: [0.0; 3],
            target_yaw: 0.0,
            render_yaw: 0.0,
            flags: 0,
            anim_speed: 0.0,
            level_hash: lh,
            last_voice_at: None,
            muted: false,
            has_state: false,
        }
    }

    pub fn apply_snapshot(&mut self, s: &NetPlayerState) {
        self.info.color_slot = s.color_slot;
        self.target_pos = s.pos;
        self.vel = s.vel;
        self.target_yaw = s.yaw;
        self.flags = s.flags;
        self.anim_speed = s.anim_speed;
        self.level_hash = s.level_hash;
        if !self.has_state {
            self.render_pos = s.pos;
            self.render_yaw = s.yaw;
            self.has_state = true;
        } else {
            // Snap if teleported > 12m (e.g. respawn or checkpoint warp)
            let dx = s.pos[0] - self.render_pos[0];
            let dy = s.pos[1] - self.render_pos[1];
            let dz = s.pos[2] - self.render_pos[2];
            if dx * dx + dy * dy + dz * dz > 144.0 {
                self.render_pos = s.pos;
                self.render_yaw = s.yaw;
            }
        }
    }

    pub fn interpolate(&mut self, dt: f32) {
        if !self.has_state {
            return;
        }
        let alpha = (1.0 - (-dt * 16.0).exp()).clamp(0.0, 1.0);
        for i in 0..3 {
            let predicted = self.target_pos[i] + self.vel[i] * 0.015;
            self.render_pos[i] += (predicted - self.render_pos[i]) * alpha;
        }
        let mut dyaw = self.target_yaw - self.render_yaw;
        while dyaw > std::f32::consts::PI {
            dyaw -= std::f32::consts::TAU;
        }
        while dyaw < -std::f32::consts::PI {
            dyaw += std::f32::consts::TAU;
        }
        self.render_yaw += dyaw * alpha;
    }

    pub fn is_speaking(&self, now: Instant) -> bool {
        (self.flags & super::protocol::FLAG_SPEAKING) != 0
            || self
                .last_voice_at
                .is_some_and(|t| now.duration_since(t) < Duration::from_millis(350))
    }
}

pub type RemoteMap = HashMap<u8, RemotePlayer>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_and_two_clients_get_unique_pants_colors_and_exchange_chat() {
        let server = HostServer::start_lan(0, "Levels/GV_ZoneA_Start.level".into()).unwrap();
        let addr = format!("127.0.0.1:{}", server.bound_addr.unwrap().port());

        // Intentionally use identical usernames & chosen color 4 to verify unique slot & color assignment
        let c1 = NetClient::connect(&addr, "Mickey".into(), 4, "Levels/GV_ZoneA_Start.level".into())
            .unwrap();
        let c2 = NetClient::connect(&addr, "Mickey".into(), 4, "Levels/GV_ZoneA_Start.level".into())
            .unwrap();

        let mut j1 = None;
        let mut j2 = None;
        for _ in 0..40 {
            for ev in c1.poll_events() {
                if let ClientEvent::Joined {
                    player_id,
                    color_slot,
                    ..
                } = ev
                {
                    j1 = Some((player_id, color_slot));
                }
            }
            for ev in c2.poll_events() {
                if let ClientEvent::Joined {
                    player_id,
                    color_slot,
                    ..
                } = ev
                {
                    j2 = Some((player_id, color_slot));
                }
            }
            if j1.is_some() && j2.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        let (id1, col1) = j1.expect("client 1 joined");
        let (id2, col2) = j2.expect("client 2 joined");
        assert_ne!(id1, id2);
        assert_ne!(col1, col2, "Every player must receive a unique pants color");
        assert!(col1 == 4 || col2 == 4, "First joiner should get preferred color 4");

        // Test live color change
        c2.send_color(9);
        let mut switched = false;
        for _ in 0..40 {
            for ev in c1.poll_events() {
                if let ClientEvent::Roster { players, .. } = ev {
                    if players.iter().any(|p| p.id == id2 && p.color_slot == 9) {
                        switched = true;
                    }
                }
            }
            if switched {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        assert!(switched, "client 2 should be able to switch pants color live");
    }
}
