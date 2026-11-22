//! Native Discord Rich Presence IPC (`\\.\pipe\discord-ipc-0..9`) & Discord Invite integration.
//!
//! Connects directly to the local Discord desktop app over its named pipe with zero external SDKs:
//! - Publishes live party size (`X of 16`), current level, and join secret (`SET_ACTIVITY`) so
//!   Discord's native **"Invite to Play"** (`+` button in Discord DMs/channels) & **"Ask to Join"**
//!   cards work out of the box when using a registered Discord Application ID.
//! - Subscribes to `ACTIVITY_JOIN` (auto-joins when clicking a Discord invite!) and
//!   `ACTIVITY_JOIN_REQUEST` (shows an in-game Accept/Decline banner and replies via IPC).
//! - Also registers the per-user `blotworx://` URI protocol on Windows (`HKCU\Software\Classes`)
//!   and generates 1-click formatted Discord invite messages with `blotworx://join/<addr>` links.

use std::{
    fs::OpenOptions,
    io::{Read, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::Duration,
};

/// Default Discord Application ID for Blotworx Rich Presence (editable in the Multiplayer panel).
pub const DEFAULT_DISCORD_APP_ID: &str = "1352408917462245427";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiscordPresence {
    pub active: bool,
    pub session_id: String,
    pub join_secret: String,
    pub level_label: String,
    pub username: String,
    pub player_count: usize,
    pub max_players: usize,
}

#[derive(Clone, Debug)]
pub struct JoinRequest {
    pub user_id: String,
    pub username: String,
}

#[derive(Clone, Debug)]
pub enum DiscordEvent {
    Connected { discord_user: String },
    Disconnected,
    JoinInvite { addr: String },
    JoinRequest(JoinRequest),
}

enum DiscordCmd {
    UpdatePresence(DiscordPresence),
    RespondJoinRequest { user_id: String, accept: bool },
    Reconnect { app_id: String },
    Shutdown,
}

pub struct DiscordBridge {
    tx: mpsc::Sender<DiscordCmd>,
    rx: mpsc::Receiver<DiscordEvent>,
    connected: Arc<AtomicBool>,
    connected_user: Arc<Mutex<String>>,
    app_id: String,
    last_presence: DiscordPresence,
    worker: Option<thread::JoinHandle<()>>,
}

impl DiscordBridge {
    pub fn start(app_id: String) -> Self {
        let app_id = if app_id.trim().is_empty() {
            DEFAULT_DISCORD_APP_ID.to_owned()
        } else {
            app_id.trim().to_owned()
        };
        let (tx, cmd_rx) = mpsc::channel();
        let (ev_tx, rx) = mpsc::channel();
        let connected = Arc::new(AtomicBool::new(false));
        let connected_user = Arc::new(Mutex::new(String::new()));

        let c_flag = connected.clone();
        let c_user = connected_user.clone();
        let initial_id = app_id.clone();

        let worker = thread::Builder::new()
            .name("blotworx-discord-ipc".into())
            .spawn(move || ipc_worker(initial_id, cmd_rx, ev_tx, c_flag, c_user))
            .ok();

        Self {
            tx,
            rx,
            connected,
            connected_user,
            app_id,
            last_presence: DiscordPresence::default(),
            worker,
        }
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    pub fn connected_user(&self) -> String {
        self.connected_user
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    pub fn set_app_id(&mut self, app_id: String) {
        let clean = app_id.trim().to_owned();
        if clean.is_empty() || clean == self.app_id {
            return;
        }
        self.app_id = clean.clone();
        let _ = self.tx.send(DiscordCmd::Reconnect { app_id: clean });
    }

    pub fn update_presence(&mut self, presence: DiscordPresence) {
        if presence != self.last_presence {
            self.last_presence = presence.clone();
            let _ = self.tx.send(DiscordCmd::UpdatePresence(presence));
        }
    }

    pub fn respond_join_request(&self, user_id: &str, accept: bool) {
        let _ = self.tx.send(DiscordCmd::RespondJoinRequest {
            user_id: user_id.to_owned(),
            accept,
        });
    }

    pub fn poll_events(&self) -> Vec<DiscordEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            out.push(ev);
        }
        out
    }
}

impl Drop for DiscordBridge {
    fn drop(&mut self) {
        let _ = self.tx.send(DiscordCmd::Shutdown);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

/// Encode a multiplayer join address into a Discord `secrets.join` token (URL-safe ASCII).
pub fn encode_join_secret(addr: &str) -> String {
    let mut hex = String::with_capacity(addr.len() * 2 + 5);
    hex.push_str("bwmp_");
    for b in addr.trim().as_bytes() {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// Decode either a `bwmp_<hex>` Discord secret, a `blotworx://join/<addr>` link, or raw `host:port`.
pub fn parse_join_target(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches(|c| c == '"' || c == '<' || c == '>');
    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed
        .strip_prefix("blotworx://join/")
        .or_else(|| trimmed.strip_prefix("BLOTWORX://join/"))
    {
        let clean = rest.trim_matches('/').trim();
        if !clean.is_empty() {
            return Some(normalize_addr(clean));
        }
    }
    if let Some(hex) = trimmed.strip_prefix("bwmp_") {
        if hex.len() % 2 == 0 && !hex.is_empty() {
            let mut bytes = Vec::with_capacity(hex.len() / 2);
            let mut ok = true;
            for pair in hex.as_bytes().chunks_exact(2) {
                if let Ok(s) = std::str::from_utf8(pair) {
                    if let Ok(b) = u8::from_str_radix(s, 16) {
                        bytes.push(b);
                        continue;
                    }
                }
                ok = false;
                break;
            }
            if ok {
                if let Ok(decoded) = String::from_utf8(bytes) {
                    return Some(normalize_addr(&decoded));
                }
            }
        }
    }
    // Extract embedded blotworx://join/... or host:port from a pasted Discord invite message
    if let Some(pos) = trimmed.find("blotworx://join/") {
        let tail = &trimmed[pos + "blotworx://join/".len()..];
        let token = tail
            .split(|c: char| c.is_whitespace() || c == ')' || c == '`' || c == '>')
            .next()
            .unwrap_or("")
            .trim_matches('/');
        if !token.is_empty() {
            return Some(normalize_addr(token));
        }
    }
    Some(normalize_addr(trimmed))
}

fn normalize_addr(addr: &str) -> String {
    let s = addr.trim();
    if let Some(code) = s.strip_prefix("room:").or_else(|| s.strip_prefix("ROOM:")) {
        return super::relay::normalize_room_code(code);
    }
    if super::relay::is_room_code_target(s) {
        super::relay::normalize_room_code(s)
    } else if s.contains(':') {
        s.to_owned()
    } else {
        format!("{s}:{}", super::protocol::DEFAULT_PORT)
    }
}

/// Build a friendly, ready-to-paste Discord invite message with clickable deep link + room code.
pub fn format_discord_invite(addr: &str, level_label: &str, players: usize, max: usize) -> String {
    let label = if super::relay::is_room_code_target(addr) {
        "Room Code"
    } else {
        "Direct Join"
    };
    format!(
        "Join my **Blotworx Online Multiplayer** lobby! ({players}/{max} players · {level_label})\n\
         {label}: `{addr}`\n\
         Quick Link: <blotworx://join/{addr}>"
    )
}

/// Register per-user `blotworx://` URI protocol handler on Windows (`HKCU\Software\Classes\blotworx`)
/// so clicking a `blotworx://join/<addr>` link in Discord launches Blotworx straight into the lobby.
#[cfg(windows)]
pub fn register_uri_protocol() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let exe_str = exe.display().to_string();
    let cmd = format!("\"{exe_str}\" --join \"%1\"");
    let _ = std::process::Command::new("reg")
        .args(["add", r"HKCU\Software\Classes\blotworx", "/ve", "/d", "URL:Blotworx Multiplayer Protocol", "/f"])
        .output();
    let _ = std::process::Command::new("reg")
        .args(["add", r"HKCU\Software\Classes\blotworx", "/v", "URL Protocol", "/d", "", "/f"])
        .output();
    let _ = std::process::Command::new("reg")
        .args(["add", r"HKCU\Software\Classes\blotworx\shell\open\command", "/ve", "/d", &cmd, "/f"])
        .output();
}

#[cfg(not(windows))]
pub fn register_uri_protocol() {}

// ------------------------------------------------------------------ Discord IPC framing
const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;

fn write_ipc(pipe: &mut std::fs::File, op: u32, payload: &serde_json::Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(payload).unwrap_or_default();
    let mut header = [0u8; 8];
    header[0..4].copy_from_slice(&op.to_le_bytes());
    header[4..8].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
    pipe.write_all(&header)?;
    pipe.write_all(&bytes)?;
    pipe.flush()
}

fn read_ipc(pipe: &mut std::fs::File) -> std::io::Result<(u32, serde_json::Value)> {
    let mut header = [0u8; 8];
    pipe.read_exact(&mut header)?;
    let op = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
    if len > 256 * 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Oversized Discord IPC frame",
        ));
    }
    let mut buf = vec![0u8; len];
    pipe.read_exact(&mut buf)?;
    let json = serde_json::from_slice(&buf).unwrap_or(serde_json::Value::Null);
    Ok((op, json))
}

#[cfg(windows)]
fn open_discord_pipe() -> Option<std::fs::File> {
    for i in 0..10 {
        let path = format!(r"\\.\pipe\discord-ipc-{i}");
        if let Ok(file) = OpenOptions::new().read(true).write(true).open(&path) {
            return Some(file);
        }
    }
    None
}

#[cfg(not(windows))]
fn open_discord_pipe() -> Option<std::fs::File> {
    None
}

fn send_activity(pipe: &mut std::fs::File, p: &DiscordPresence, nonce: &mut u64) -> std::io::Result<()> {
    *nonce += 1;
    let pid = std::process::id();
    let activity = if p.active {
        let mut act = serde_json::json!({
            "details": format!("Online Multiplayer · {}", p.level_label),
            "state": format!("Playing as {}", p.username),
            "party": {
                "id": p.session_id,
                "size": [p.player_count.max(1), p.max_players.max(1)],
            },
            "instance": true,
        });
        if !p.join_secret.is_empty() {
            act["secrets"] = serde_json::json!({
                "join": p.join_secret,
            });
        }
        act
    } else {
        serde_json::Value::Null
    };
    let payload = serde_json::json!({
        "cmd": "SET_ACTIVITY",
        "args": {
            "pid": pid,
            "activity": activity,
        },
        "nonce": format!("bw-{nonce}"),
    });
    write_ipc(pipe, OP_FRAME, &payload)
}

fn ipc_worker(
    mut app_id: String,
    cmd_rx: mpsc::Receiver<DiscordCmd>,
    ev_tx: mpsc::Sender<DiscordEvent>,
    connected: Arc<AtomicBool>,
    connected_user: Arc<Mutex<String>>,
) {
    let mut latest_presence = DiscordPresence::default();
    let mut nonce: u64 = 1;

    loop {
        // Drain pending commands before trying to connect
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                DiscordCmd::UpdatePresence(p) => latest_presence = p,
                DiscordCmd::Reconnect { app_id: id } => app_id = id,
                DiscordCmd::RespondJoinRequest { .. } => {}
                DiscordCmd::Shutdown => return,
            }
        }

        let Some(mut pipe) = open_discord_pipe() else {
            // Wait up to 4s between reconnect checks while remaining responsive to Shutdown
            for _ in 0..40 {
                match cmd_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(DiscordCmd::UpdatePresence(p)) => latest_presence = p,
                    Ok(DiscordCmd::Reconnect { app_id: id }) => {
                        app_id = id;
                        break;
                    }
                    Ok(DiscordCmd::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    _ => {}
                }
            }
            continue;
        };

        // Handshake with Discord client
        let hs = serde_json::json!({ "v": 1, "client_id": app_id });
        if write_ipc(&mut pipe, OP_HANDSHAKE, &hs).is_err() {
            thread::sleep(Duration::from_secs(3));
            continue;
        }
        let Ok((op, reply)) = read_ipc(&mut pipe) else {
            thread::sleep(Duration::from_secs(3));
            continue;
        };
        if op == OP_CLOSE || reply.get("evt").and_then(|v| v.as_str()) == Some("ERROR") {
            thread::sleep(Duration::from_secs(5));
            continue;
        }

        let user_name = reply
            .pointer("/data/user/global_name")
            .or_else(|| reply.pointer("/data/user/username"))
            .and_then(|v| v.as_str())
            .unwrap_or("Discord Connected")
            .to_owned();
        if let Ok(mut u) = connected_user.lock() {
            *u = user_name.clone();
        }
        connected.store(true, Ordering::Release);
        let _ = ev_tx.send(DiscordEvent::Connected {
            discord_user: user_name,
        });

        // Subscribe to Discord native invite join events
        for evt in ["ACTIVITY_JOIN", "ACTIVITY_JOIN_REQUEST"] {
            nonce += 1;
            let _ = write_ipc(
                &mut pipe,
                OP_FRAME,
                &serde_json::json!({
                    "cmd": "SUBSCRIBE",
                    "evt": evt,
                    "nonce": format!("bw-{nonce}"),
                }),
            );
        }

        if latest_presence.active {
            let _ = send_activity(&mut pipe, &latest_presence, &mut nonce);
        }

        // Spawn reader helper on cloned handle so command writes never block on blocking pipe reads
        let reader_stop = Arc::new(AtomicBool::new(false));
        let (msg_tx, msg_rx) = mpsc::channel::<(u32, serde_json::Value)>();
        let reader_handle = pipe.try_clone().ok().map(|mut rpipe| {
            let stop = reader_stop.clone();
            thread::Builder::new()
                .name("blotworx-discord-rx".into())
                .spawn(move || {
                    while !stop.load(Ordering::Acquire) {
                        match read_ipc(&mut rpipe) {
                            Ok(frame) => {
                                let closed = frame.0 == OP_CLOSE;
                                let _ = msg_tx.send(frame);
                                if closed {
                                    break;
                                }
                            }
                            Err(_) => {
                                let _ = msg_tx.send((OP_CLOSE, serde_json::Value::Null));
                                break;
                            }
                        }
                    }
                })
                .ok()
        });

        // Connected event loop
        let mut disconnect = false;
        while !disconnect {
            while let Ok((op, json)) = msg_rx.try_recv() {
                match op {
                    OP_CLOSE => {
                        disconnect = true;
                        break;
                    }
                    OP_PING => {
                        let _ = write_ipc(&mut pipe, OP_PONG, &json);
                    }
                    OP_FRAME => {
                        if let Some(evt) = json.get("evt").and_then(|v| v.as_str()) {
                            match evt {
                                "ACTIVITY_JOIN" => {
                                    if let Some(secret) =
                                        json.pointer("/data/secret").and_then(|v| v.as_str())
                                    {
                                        if let Some(addr) = parse_join_target(secret) {
                                            let _ = ev_tx.send(DiscordEvent::JoinInvite { addr });
                                        }
                                    }
                                }
                                "ACTIVITY_JOIN_REQUEST" => {
                                    if let Some(uid) =
                                        json.pointer("/data/user/id").and_then(|v| v.as_str())
                                    {
                                        let uname = json
                                            .pointer("/data/user/global_name")
                                            .or_else(|| json.pointer("/data/user/username"))
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("Discord Player")
                                            .to_owned();
                                        let _ = ev_tx.send(DiscordEvent::JoinRequest(JoinRequest {
                                            user_id: uid.to_owned(),
                                            username: uname,
                                        }));
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }

            match cmd_rx.recv_timeout(Duration::from_millis(80)) {
                Ok(DiscordCmd::UpdatePresence(p)) => {
                    latest_presence = p;
                    if send_activity(&mut pipe, &latest_presence, &mut nonce).is_err() {
                        disconnect = true;
                    }
                }
                Ok(DiscordCmd::RespondJoinRequest { user_id, accept }) => {
                    nonce += 1;
                    let cmd_name = if accept {
                        "SEND_ACTIVITY_JOIN_INVITE"
                    } else {
                        "CLOSE_ACTIVITY_JOIN_REQUEST"
                    };
                    if write_ipc(
                        &mut pipe,
                        OP_FRAME,
                        &serde_json::json!({
                            "cmd": cmd_name,
                            "args": { "user_id": user_id },
                            "nonce": format!("bw-{nonce}"),
                        }),
                    )
                    .is_err()
                    {
                        disconnect = true;
                    }
                }
                Ok(DiscordCmd::Reconnect { app_id: id }) => {
                    app_id = id;
                    disconnect = true;
                }
                Ok(DiscordCmd::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = write_ipc(&mut pipe, OP_CLOSE, &serde_json::json!({}));
                    reader_stop.store(true, Ordering::Release);
                    drop(pipe);
                    if let Some(Some(h)) = reader_handle {
                        let _ = h.join();
                    }
                    return;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }

        reader_stop.store(true, Ordering::Release);
        connected.store(false, Ordering::Release);
        if let Ok(mut u) = connected_user.lock() {
            u.clear();
        }
        let _ = ev_tx.send(DiscordEvent::Disconnected);
        drop(pipe);
        if let Some(Some(h)) = reader_handle {
            let _ = h.join();
        }
        thread::sleep(Duration::from_secs(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_secret_and_uri_parsing() {
        let secret = encode_join_secret("192.168.1.42:27077");
        assert_eq!(parse_join_target(&secret).as_deref(), Some("192.168.1.42:27077"));
        assert_eq!(
            parse_join_target("blotworx://join/play.example.com:27077").as_deref(),
            Some("play.example.com:27077")
        );
        let msg = format_discord_invite("203.0.113.5:27077", "Gremlin Village", 3, 16);
        assert_eq!(parse_join_target(&msg).as_deref(), Some("203.0.113.5:27077"));
    }
}
