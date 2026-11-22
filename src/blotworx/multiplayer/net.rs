//! 16-player UDP Host Server & Client networking for Blotworx Online Multiplayer.

use super::protocol::{
    level_hash, preferred_color_slot, sanitize_username, ChatPacket, NetPlayerState, Packet,
    PlayerInfo, MAX_PLAYERS,
};
use anyhow::{Context, Result};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::{SocketAddr, ToSocketAddrs, UdpSocket},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};

const TIMEOUT_SECS: u64 = 10;
const SNAPSHOT_INTERVAL_MS: u64 = 33; // ~30 Hz world snapshots
const HEARTBEAT_INTERVAL_MS: u64 = 2000;
const PROXIMITY_RELAY_MAX_SQ: f32 = 75.0 * 75.0; // relay voice within generous 75m radius; client attenuates precisely

// ------------------------------------------------------------------ Host Server

struct ServerSlot {
    addr: SocketAddr,
    info: PlayerInfo,
    state: NetPlayerState,
    last_seen: Instant,
}

pub struct HostServer {
    running: Arc<AtomicBool>,
    pub bound_addr: SocketAddr,
    pub session_id: String,
    handle: Option<thread::JoinHandle<()>>,
}

impl HostServer {
    pub fn start(port: u16, initial_level: String) -> Result<Self> {
        let socket = UdpSocket::bind(("0.0.0.0", port))
            .with_context(|| format!("Bind multiplayer UDP server on port {port}"))?;
        socket.set_nonblocking(true)?;
        let bound_addr = socket.local_addr()?;
        let session_id = format!(
            "bw-{:08x}{:04x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as u32)
                .unwrap_or(1),
            bound_addr.port()
        );
        let running = Arc::new(AtomicBool::new(true));
        let r_flag = running.clone();
        let sid = session_id.clone();

        let handle = thread::Builder::new()
            .name("blotworx-mp-server".into())
            .spawn(move || server_loop(socket, sid, initial_level, r_flag))
            .context("Spawn multiplayer host server thread")?;

        Ok(Self {
            running,
            bound_addr,
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
    socket: UdpSocket,
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

    let send_to = |sock: &UdpSocket, addr: SocketAddr, pkt: &Packet| {
        let bytes = pkt.encode();
        let _ = sock.send_to(&bytes, addr);
    };

    let broadcast = |sock: &UdpSocket,
                     slots: &[Option<ServerSlot>; MAX_PLAYERS],
                     pkt: &Packet,
                     exclude: Option<u8>| {
        let bytes = pkt.encode();
        for slot in slots.iter().flatten() {
            if exclude != Some(slot.info.id) {
                let _ = sock.send_to(&bytes, slot.addr);
            }
        }
    };

    let build_roster = |slots: &[Option<ServerSlot>; MAX_PLAYERS]| -> Vec<PlayerInfo> {
        slots
            .iter()
            .flatten()
            .map(|s| s.info.clone())
            .collect()
    };

    while running.load(Ordering::Acquire) {
        let mut received_any = false;
        while let Ok((len, from)) = socket.recv_from(&mut buf) {
            received_any = true;
            let Some(pkt) = Packet::decode(&buf[..len]) else {
                continue;
            };
            let now = Instant::now();

            match pkt {
                Packet::JoinRequest { username, level } => {
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
                            send_to(
                                &socket,
                                from,
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
                        send_to(
                            &socket,
                            from,
                            &Packet::JoinReject {
                                reason: format!("Lobby is full ({MAX_PLAYERS}/{MAX_PLAYERS})"),
                            },
                        );
                        continue;
                    };

                    // Pick unique pants color: start at deterministic hash of username, advance if taken
                    let used_colors: HashSet<u8> = slots
                        .iter()
                        .flatten()
                        .map(|s| s.info.color_slot)
                        .collect();
                    let pref = preferred_color_slot(&clean_name);
                    let color_slot = (0..MAX_PLAYERS as u8)
                        .map(|offset| (pref + offset) % (MAX_PLAYERS as u8))
                        .find(|c| !used_colors.contains(c))
                        .unwrap_or(free_id as u8);

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
                        addr: from,
                        info,
                        state,
                        last_seen: now,
                    });

                    let roster = build_roster(&slots);
                    send_to(
                        &socket,
                        from,
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
                        send_to(&socket, from, &Packet::Chat(msg.clone()));
                    }
                    // Notify everyone of updated roster + system join message
                    broadcast(
                        &socket,
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
                            super::protocol::pants_color(color_slot).name
                        ),
                    };
                    chat_history.push_back(sys_msg.clone());
                    while chat_history.len() > 40 {
                        chat_history.pop_front();
                    }
                    broadcast(&socket, &slots, &Packet::Chat(sys_msg), None);
                }

                Packet::Leave { player_id } => {
                    if let Some(slot) = slots.get_mut(player_id as usize) {
                        if slot.as_ref().is_some_and(|s| s.addr == from) {
                            let name = slot.take().unwrap().info.username;
                            let roster = build_roster(&slots);
                            broadcast(
                                &socket,
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
                            broadcast(&socket, &slots, &Packet::Chat(sys_msg), None);
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
                            broadcast(&socket, &slots, &Packet::Chat(msg), None);
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
                    for other in slots.iter().flatten() {
                        if other.info.id == sender_id || other.state.level_hash != lh {
                            continue;
                        }
                        let dx = other.state.pos[0] - pos[0];
                        let dy = other.state.pos[1] - pos[1];
                        let dz = other.state.pos[2] - pos[2];
                        if dx * dx + dy * dy + dz * dz <= PROXIMITY_RELAY_MAX_SQ {
                            let _ = socket.send_to(&bytes, other.addr);
                        }
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
                        broadcast(
                            &socket,
                            &slots,
                            &Packet::Roster {
                                host_level: host_level.clone(),
                                players: roster,
                            },
                            None,
                        );
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
            broadcast(
                &socket,
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
                broadcast(&socket, &slots, &Packet::Chat(sys_msg), None);
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
                broadcast(
                    &socket,
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
    pub fn connect(server_addr: &str, username: String, level: String) -> Result<Self> {
        let resolved: SocketAddr = server_addr
            .to_socket_addrs()
            .with_context(|| format!("Resolve multiplayer server '{server_addr}'"))?
            .find(|a| a.is_ipv4())
            .or_else(|| server_addr.to_socket_addrs().ok()?.next())
            .with_context(|| format!("No reachable address for '{server_addr}'"))?;

        let socket = UdpSocket::bind("0.0.0.0:0").context("Bind client UDP socket")?;
        socket.set_nonblocking(true)?;
        socket.connect(resolved)?;

        let (tx, cmd_rx) = mpsc::channel();
        let (ev_tx, rx) = mpsc::channel();
        let addr_str = server_addr.to_owned();

        let handle = thread::Builder::new()
            .name("blotworx-mp-client".into())
            .spawn(move || client_loop(socket, username, level, cmd_rx, ev_tx))
            .context("Spawn multiplayer client thread")?;

        Ok(Self {
            tx,
            rx,
            server_addr: addr_str,
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
    socket: UdpSocket,
    username: String,
    initial_level: String,
    cmd_rx: mpsc::Receiver<ClientCmd>,
    ev_tx: mpsc::Sender<ClientEvent>,
) {
    let join_pkt = Packet::JoinRequest {
        username: username.clone(),
        level: initial_level,
    }
    .encode();
    let _ = socket.send(&join_pkt);

    let mut player_id: Option<u8> = None;
    let mut color_slot: u8 = 0;
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
                        let _ = socket.send(&bytes);
                    }
                    return;
                }
                ClientCmd::SendState(mut state) => {
                    if let Some(id) = player_id {
                        seq = seq.wrapping_add(1);
                        state.id = id;
                        state.color_slot = color_slot;
                        let bytes = Packet::PlayerState { seq, state }.encode();
                        let _ = socket.send(&bytes);
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
                        // Send twice to guard against single UDP drop (server deduplicates by (sender_id, msg_id))
                        let _ = socket.send(&bytes);
                        let _ = socket.send(&bytes);
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
                        let _ = socket.send(&bytes);
                    }
                }
                ClientCmd::SendLevel(level) => {
                    if let Some(id) = player_id {
                        let bytes = Packet::LevelChange {
                            player_id: id,
                            level,
                        }
                        .encode();
                        let _ = socket.send(&bytes);
                    }
                }
            }
        }

        // Receive inbound UDP packets
        let mut received_any = false;
        while let Ok(len) = socket.recv(&mut buf) {
            received_any = true;
            last_rx = Instant::now();
            let Some(pkt) = Packet::decode(&buf[..len]) else {
                continue;
            };
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
                if join_attempts > 10 {
                    let _ = ev_tx.send(ClientEvent::Disconnected(
                        "Connection timed out (host unreachable or firewall blocking UDP)".into(),
                    ));
                    return;
                }
                let _ = socket.send(&join_pkt);
            }
        } else {
            if now.duration_since(last_hb) >= Duration::from_millis(HEARTBEAT_INTERVAL_MS) {
                last_hb = now;
                let bytes = Packet::Heartbeat {
                    player_id: player_id.unwrap(),
                }
                .encode();
                let _ = socket.send(&bytes);
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

/// Non-blocking public IP lookup for convenient online invite sharing.
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
        let server = HostServer::start(0, "Levels/GV_ZoneA_Start.level".into()).unwrap();
        let addr = format!("127.0.0.1:{}", server.bound_addr.port());

        // Intentionally use identical usernames to verify unique slot & color assignment
        let c1 = NetClient::connect(&addr, "Mickey".into(), "Levels/GV_ZoneA_Start.level".into()).unwrap();
        let c2 = NetClient::connect(&addr, "Mickey".into(), "Levels/GV_ZoneA_Start.level".into()).unwrap();

        let mut j1 = None;
        let mut j2 = None;
        for _ in 0..40 {
            for ev in c1.poll_events() {
                if let ClientEvent::Joined { player_id, color_slot, .. } = ev {
                    j1 = Some((player_id, color_slot));
                }
            }
            for ev in c2.poll_events() {
                if let ClientEvent::Joined { player_id, color_slot, .. } = ev {
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

        c1.send_chat("Hello from player 1!".into());
        let mut got_chat = false;
        for _ in 0..40 {
            for ev in c2.poll_events() {
                if let ClientEvent::Chat(c) = ev {
                    if c.text == "Hello from player 1!" && c.color_slot == col1 {
                        got_chat = true;
                    }
                }
            }
            if got_chat {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        assert!(got_chat, "client 2 should receive chat with sender's pants color");
    }
}
