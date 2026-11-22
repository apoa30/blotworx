//! Blotworx Online Multiplayer (up to 16 players):
//! - Authoritative UDP host/client session (up to 16 players)
//! - Unique per-player Mickey pants color matched to each player's overhead username & chat name
//! - In-game text chat overlay (`T` to type, `Enter` to send, `Esc` to close)
//! - Real-time 3D spatialized proximity voice chat (`V` Push-to-Talk or Open Mic)
//! - Native Discord Rich Presence IPC & Discord Invite system (`blotworx://join/<addr>` + IPC)

pub mod discord;
pub mod net;
pub mod pants;
pub mod protocol;
pub mod voice;

use crate::render_support::viewport3d::GpuDrawCommand;
use discord::{
    encode_join_secret, format_discord_invite, parse_join_target, register_uri_protocol,
    DiscordBridge, DiscordEvent, DiscordPresence, JoinRequest, DEFAULT_DISCORD_APP_ID,
};
use net::{
    detect_lan_ip, spawn_public_ip_lookup, ClientEvent, HostServer, NetClient, RemoteMap,
    RemotePlayer,
};
use pants::{append_remote_player_draws, remote_player_matrix, PantsTextureCache};
use protocol::{
    level_hash, pants_color, preferred_color_slot, sanitize_username, NetPlayerState,
    DEFAULT_PORT, FLAG_GROUNDED, FLAG_JUMPING, FLAG_SPEAKING, FLAG_SPINNING, FLAG_SPRAY_PAINT,
    FLAG_SPRAY_THINNER, MAX_PLAYERS,
};
use std::{
    collections::{HashSet, VecDeque},
    sync::mpsc,
    time::{Duration, Instant},
};
use voice::{ListenerPose, VoiceEngine, VoiceSettings};

#[derive(Clone, Debug)]
struct ChatEntry {
    sender: String,
    text: String,
    color_slot: u8,
    is_system: bool,
    received_at: Instant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Offline,
    Connecting(String),
    Hosting { port: u16 },
    Connected { addr: String },
}

pub struct Multiplayer {
    pub panel_open: bool,
    pub username: String,
    pub host_port: String,
    pub public_addr_override: String,
    pub join_input: String,
    pub discord_app_id_input: String,

    lan_ip: String,
    public_ip: Option<String>,
    public_ip_rx: Option<mpsc::Receiver<String>>,

    state: ConnectionState,
    status_notice: Option<(String, Instant)>,
    last_error: Option<String>,

    server: Option<HostServer>,
    client: Option<NetClient>,
    local_id: Option<u8>,
    local_color_slot: u8,
    session_id: String,
    host_level: String,
    current_level: String,
    remotes: RemoteMap,
    last_state_send: Instant,
    last_tick: Instant,

    pants_cache: PantsTextureCache,
    voice_settings: VoiceSettings,
    voice: VoiceEngine,
    discord: DiscordBridge,
    pending_discord_requests: Vec<JoinRequest>,
    pending_level_switch: Option<String>,
    pending_auto_join: Option<String>,

    chat_open: bool,
    chat_focus_requested: bool,
    chat_draft: String,
    chat_log: VecDeque<ChatEntry>,
    seen_chat_ids: HashSet<u32>,
}

impl Multiplayer {
    pub fn new(initial_level: &str, auto_join: Option<String>) -> Self {
        register_uri_protocol();
        let default_name = std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .map(|s| sanitize_username(&s))
            .unwrap_or_else(|_| "Mickey".to_owned());
        let color = preferred_color_slot(&default_name);
        let voice_settings = VoiceSettings::default();
        let voice = VoiceEngine::start(voice_settings.clone());
        let discord = DiscordBridge::start(DEFAULT_DISCORD_APP_ID.to_owned());
        let lan_ip = detect_lan_ip();
        let public_ip_rx = Some(spawn_public_ip_lookup());
        let now = Instant::now();

        let mut mp = Self {
            panel_open: false,
            username: default_name,
            host_port: DEFAULT_PORT.to_string(),
            public_addr_override: String::new(),
            join_input: auto_join.clone().unwrap_or_default(),
            discord_app_id_input: DEFAULT_DISCORD_APP_ID.to_owned(),
            lan_ip,
            public_ip: None,
            public_ip_rx,
            state: ConnectionState::Offline,
            status_notice: None,
            last_error: None,
            server: None,
            client: None,
            local_id: None,
            local_color_slot: color,
            session_id: String::new(),
            host_level: initial_level.to_owned(),
            current_level: initial_level.to_owned(),
            remotes: RemoteMap::new(),
            last_state_send: now,
            last_tick: now,
            pants_cache: PantsTextureCache::default(),
            voice_settings,
            voice,
            discord,
            pending_discord_requests: Vec::new(),
            pending_level_switch: None,
            pending_auto_join: auto_join,
            chat_open: false,
            chat_focus_requested: false,
            chat_draft: String::new(),
            chat_log: VecDeque::with_capacity(64),
            seen_chat_ids: HashSet::new(),
        };
        if let Some(addr) = mp.pending_auto_join.take() {
            if let Some(target) = parse_join_target(&addr) {
                mp.join_input = target.clone();
                mp.join(&target);
            }
        }
        mp
    }

    pub fn is_online(&self) -> bool {
        !matches!(self.state, ConnectionState::Offline)
    }

    pub fn player_count(&self) -> usize {
        if self.is_online() {
            (1 + self.remotes.len()).min(MAX_PLAYERS)
        } else {
            1
        }
    }

    pub fn local_color_slot(&self) -> u8 {
        if self.local_id.is_some() {
            self.local_color_slot
        } else {
            preferred_color_slot(&self.username)
        }
    }

    /// True whenever the text chat box or multiplayer panel is consuming keyboard input.
    pub fn wants_keyboard_input(&self) -> bool {
        self.chat_open || self.panel_open
    }

    pub fn is_chat_open(&self) -> bool {
        self.chat_open
    }

    /// Call when a new level finishes loading so GPU texture caches stay fresh and the lobby knows our level.
    pub fn on_level_loaded(&mut self, level: &str) {
        self.pants_cache.clear();
        self.current_level = level.to_owned();
        if let Some(client) = &self.client {
            client.send_level(self.current_level.clone());
        }
        self.sync_discord_presence();
    }

    /// Optional level switch requested by clicking "Follow Host Level" in the lobby UI.
    pub fn take_requested_level_switch(&mut self) -> Option<String> {
        self.pending_level_switch.take()
    }

    pub fn host(&mut self) {
        self.disconnect();
        let port = self
            .host_port
            .trim()
            .parse::<u16>()
            .unwrap_or(DEFAULT_PORT);
        let uname = sanitize_username(&self.username);
        self.username = uname.clone();
        match HostServer::start(port, self.current_level.clone()) {
            Ok(srv) => {
                let bound_port = srv.bound_addr.port();
                let local_addr = format!("127.0.0.1:{bound_port}");
                self.session_id = srv.session_id.clone();
                match NetClient::connect(&local_addr, uname, self.current_level.clone()) {
                    Ok(cli) => {
                        self.server = Some(srv);
                        self.client = Some(cli);
                        self.state = ConnectionState::Hosting { port: bound_port };
                        self.last_error = None;
                        self.push_system_chat(format!(
                            "Hosting online lobby on UDP port {bound_port} (up to {MAX_PLAYERS} players)."
                        ));
                        self.set_notice(format!("Hosting on port {bound_port}"));
                        self.sync_discord_presence();
                    }
                    Err(e) => {
                        self.last_error = Some(format!("Local host client failed: {e:#}"));
                    }
                }
            }
            Err(e) => {
                self.last_error = Some(format!("Failed to host on port {port}: {e:#}"));
            }
        }
    }

    pub fn join(&mut self, raw_target: &str) {
        let Some(addr) = parse_join_target(raw_target) else {
            self.last_error = Some("Enter a server address (e.g. 192.168.1.50:27077) or Discord invite link.".into());
            return;
        };
        self.disconnect();
        let uname = sanitize_username(&self.username);
        self.username = uname.clone();
        match NetClient::connect(&addr, uname, self.current_level.clone()) {
            Ok(cli) => {
                self.client = Some(cli);
                self.state = ConnectionState::Connecting(addr.clone());
                self.last_error = None;
                self.push_system_chat(format!("Connecting to {addr}..."));
                self.set_notice(format!("Connecting to {addr}..."));
            }
            Err(e) => {
                self.last_error = Some(format!("Could not connect to {addr}: {e:#}"));
            }
        }
    }

    pub fn disconnect(&mut self) {
        let was_online = self.is_online();
        self.client = None;
        self.server = None;
        self.local_id = None;
        self.remotes.clear();
        self.seen_chat_ids.clear();
        self.pending_discord_requests.clear();
        self.state = ConnectionState::Offline;
        self.sync_discord_presence();
        if was_online {
            self.push_system_chat("Disconnected from multiplayer lobby.".into());
        }
    }

    fn invite_address(&self) -> Option<String> {
        match &self.state {
            ConnectionState::Hosting { port } => {
                let ov = self.public_addr_override.trim();
                if !ov.is_empty() {
                    return Some(if ov.contains(':') {
                        ov.to_owned()
                    } else {
                        format!("{ov}:{port}")
                    });
                }
                let host = self.public_ip.as_deref().unwrap_or(&self.lan_ip);
                Some(format!("{host}:{port}"))
            }
            ConnectionState::Connected { addr } => Some(addr.clone()),
            _ => None,
        }
    }

    fn pretty_level_name(level: &str) -> String {
        let file = level
            .rsplit(|c| c == '/' || c == '\\')
            .next()
            .unwrap_or(level);
        file.split('.').next().unwrap_or(file).replace('_', " ")
    }

    fn sync_discord_presence(&mut self) {
        let presence = if let Some(addr) = self.invite_address() {
            DiscordPresence {
                active: true,
                session_id: if self.session_id.is_empty() {
                    format!("bw-{}", level_hash(&addr))
                } else {
                    self.session_id.clone()
                },
                join_secret: encode_join_secret(&addr),
                level_label: Self::pretty_level_name(&self.current_level),
                username: self.username.clone(),
                player_count: self.player_count(),
                max_players: MAX_PLAYERS,
            }
        } else {
            DiscordPresence::default()
        };
        self.discord.update_presence(presence);
    }

    fn push_system_chat(&mut self, text: String) {
        self.chat_log.push_back(ChatEntry {
            sender: String::new(),
            text,
            color_slot: 0,
            is_system: true,
            received_at: Instant::now(),
        });
        while self.chat_log.len() > 60 {
            self.chat_log.pop_front();
        }
    }

    fn set_notice(&mut self, msg: String) {
        self.status_notice = Some((msg, Instant::now()));
    }

    /// Advance network, Discord IPC, proximity voice, and remote player interpolation once per frame.
    #[allow(clippy::too_many_arguments)]
    pub fn tick(
        &mut self,
        ctx: &egui::Context,
        level: &str,
        player_pos: [f32; 3],
        player_vel: [f32; 3],
        player_yaw: f32,
        grounded: bool,
        spinning: bool,
        spraying_ink: Option<usize>,
        listener_pos: [f32; 3],
        listener_right: [f32; 3],
    ) {
        let now = Instant::now();
        let dt = now
            .duration_since(self.last_tick)
            .as_secs_f32()
            .clamp(0.0, 0.1);
        self.last_tick = now;

        // Public IP async result
        if let Some(rx) = &self.public_ip_rx {
            if let Ok(ip) = rx.try_recv() {
                self.public_ip = Some(ip);
                self.public_ip_rx = None;
                self.sync_discord_presence();
            }
        }

        if self.current_level != level {
            self.current_level = level.to_owned();
            if let Some(client) = &self.client {
                client.send_level(self.current_level.clone());
            }
            self.sync_discord_presence();
        }

        // Hotkeys (only when not typing in another widget)
        let wants_kb = ctx.egui_wants_keyboard_input();
        if ctx.input(|i| i.key_pressed(egui::Key::F9)) {
            self.panel_open = !self.panel_open;
        }
        if !self.chat_open && !wants_kb && ctx.input(|i| i.key_pressed(egui::Key::T)) {
            self.chat_open = true;
            self.chat_focus_requested = true;
        }

        // Proximity voice Push-to-Talk (`V` key) + listener pose
        let ptt_held = !self.chat_open
            && !wants_kb
            && ctx.input(|i| i.key_down(egui::Key::V));
        self.voice.set_ptt_held(ptt_held);
        self.voice.update_listener(ListenerPose {
            position: listener_pos,
            right: listener_right,
        });

        // Poll Discord IPC events (connected, join invite clicked, join request received)
        for ev in self.discord.poll_events() {
            match ev {
                DiscordEvent::Connected { discord_user } => {
                    self.push_system_chat(format!("Discord connected as {discord_user}."));
                    self.sync_discord_presence();
                }
                DiscordEvent::Disconnected => {}
                DiscordEvent::JoinInvite { addr } => {
                    self.join_input = addr.clone();
                    self.push_system_chat(format!("Joining Discord invite ({addr})..."));
                    self.join(&addr);
                }
                DiscordEvent::JoinRequest(req) => {
                    if !self
                        .pending_discord_requests
                        .iter()
                        .any(|r| r.user_id == req.user_id)
                    {
                        self.push_system_chat(format!(
                            "Discord join request from {} — accept in top banner!",
                            req.username
                        ));
                        self.pending_discord_requests.push(req);
                    }
                }
            }
        }

        // Poll multiplayer network events
        let mut disconnect_reason: Option<String> = None;
        let events = self
            .client
            .as_ref()
            .map(NetClient::poll_events)
            .unwrap_or_default();
        let mut presence_dirty = false;

        for ev in events {
            match ev {
                ClientEvent::Joined {
                    player_id,
                    color_slot,
                    session_id,
                    host_level,
                    players,
                } => {
                    self.local_id = Some(player_id);
                    self.local_color_slot = color_slot;
                    self.session_id = session_id;
                    self.host_level = host_level;
                    if let ConnectionState::Connecting(addr) = &self.state {
                        self.state = ConnectionState::Connected { addr: addr.clone() };
                    }
                    self.apply_roster(players);
                    let cname = pants_color(color_slot).name;
                    self.push_system_chat(format!(
                        "Connected as {}! Your unique pants color: {cname}.",
                        self.username
                    ));
                    self.set_notice(format!("Connected ({cname} pants)"));
                    presence_dirty = true;
                }
                ClientEvent::Rejected(reason) => {
                    disconnect_reason = Some(format!("Join rejected: {reason}"));
                }
                ClientEvent::Roster {
                    host_level,
                    players,
                } => {
                    self.host_level = host_level;
                    self.apply_roster(players);
                    presence_dirty = true;
                }
                ClientEvent::Snapshot(states) => {
                    for s in states {
                        if Some(s.id) == self.local_id {
                            self.local_color_slot = s.color_slot;
                            continue;
                        }
                        if let Some(rp) = self.remotes.get_mut(&s.id) {
                            rp.apply_snapshot(&s);
                        }
                    }
                }
                ClientEvent::Chat(c) => {
                    if self.seen_chat_ids.insert(c.msg_id) {
                        self.chat_log.push_back(ChatEntry {
                            sender: c.sender,
                            text: c.text,
                            color_slot: c.color_slot,
                            is_system: c.sender_id == 255,
                            received_at: Instant::now(),
                        });
                        while self.chat_log.len() > 60 {
                            self.chat_log.pop_front();
                        }
                    }
                }
                ClientEvent::Voice {
                    sender_id,
                    pos,
                    level_hash: lh,
                    ulaw,
                } => {
                    if Some(sender_id) != self.local_id && lh == level_hash(&self.current_level) {
                        let muted = self.remotes.get(&sender_id).is_some_and(|r| r.muted);
                        if !muted {
                            self.voice.push_remote_voice(sender_id, pos, &ulaw);
                        }
                        if let Some(rp) = self.remotes.get_mut(&sender_id) {
                            rp.last_voice_at = Some(now);
                        }
                    }
                }
                ClientEvent::Disconnected(reason) => {
                    disconnect_reason = Some(reason);
                }
            }
        }

        if let Some(reason) = disconnect_reason {
            self.last_error = Some(reason.clone());
            self.disconnect();
            self.push_system_chat(reason);
        }

        if presence_dirty {
            self.sync_discord_presence();
        }

        // Interpolate all remote players smoothly every frame
        for rp in self.remotes.values_mut() {
            rp.interpolate(dt);
        }

        // Send local player pose at 30 Hz + any captured proximity voice frames immediately
        if let Some(client) = &self.client {
            let lh = level_hash(&self.current_level);
            for frame in self.voice.drain_captured_frames() {
                client.send_voice(player_pos, lh, frame);
            }
            if now.duration_since(self.last_state_send) >= Duration::from_millis(33) {
                self.last_state_send = now;
                let mut flags = 0u8;
                if grounded {
                    flags |= FLAG_GROUNDED;
                } else {
                    flags |= FLAG_JUMPING;
                }
                if spinning {
                    flags |= FLAG_SPINNING;
                }
                if spraying_ink == Some(0) {
                    flags |= FLAG_SPRAY_PAINT;
                } else if spraying_ink == Some(1) {
                    flags |= FLAG_SPRAY_THINNER;
                }
                if self.voice.is_speaking() {
                    flags |= FLAG_SPEAKING;
                }
                let horiz_speed = (player_vel[0] * player_vel[0] + player_vel[2] * player_vel[2]).sqrt();
                client.send_state(NetPlayerState {
                    id: self.local_id.unwrap_or(0),
                    color_slot: self.local_color_slot,
                    pos: player_pos,
                    vel: player_vel,
                    yaw: player_yaw,
                    flags,
                    anim_speed: horiz_speed,
                    level_hash: lh,
                });
            }
        } else {
            // Drain loopback frames cleanly when offline so buffer never stalls
            let _ = self.voice.drain_captured_frames();
        }
    }

    fn apply_roster(&mut self, players: Vec<protocol::PlayerInfo>) {
        let mut active_ids = HashSet::new();
        for p in players {
            if Some(p.id) == self.local_id {
                self.local_color_slot = p.color_slot;
                self.username = p.username;
                continue;
            }
            active_ids.insert(p.id);
            self.remotes
                .entry(p.id)
                .and_modify(|rp| {
                    rp.info = p.clone();
                })
                .or_insert_with(|| RemotePlayer::from_info(p));
        }
        self.remotes.retain(|id, _| active_ids.contains(id));
    }

    /// Apply the local player's unique pants color to Mickey's draws AND append all remote Mickeys
    /// in the same level wearing their own unique pants colors.
    pub fn apply_player_draws(
        &mut self,
        world: &super::assets::World,
        draws: &mut Vec<GpuDrawCommand>,
    ) {
        if !self.is_online() {
            return;
        }
        let local_color = self.local_color_slot();
        // Snapshot the unmodified skinned Mickey draws before recoloring local Mickey
        let base_mickey: Vec<GpuDrawCommand> = world
            .player_draws
            .iter()
            .filter_map(|&idx| draws.get(idx).cloned())
            .collect();

        // Recolor local player's pants to match their assigned unique color slot
        for &idx in &world.player_draws {
            if let Some(d) = draws.get_mut(idx) {
                self.pants_cache.apply_to_draw(d, local_color);
            }
        }

        if base_mickey.is_empty() {
            return;
        }
        let my_level_hash = level_hash(&self.current_level);
        let entity_template = &world.entities[world.player_entity];
        let time_s = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        // Render every remote player currently in this level with their own unique pants color
        let mut remote_ids: Vec<u8> = self.remotes.keys().copied().collect();
        remote_ids.sort_unstable();
        for id in remote_ids {
            let Some(rp) = self.remotes.get(&id) else {
                continue;
            };
            if !rp.has_state || rp.level_hash != my_level_hash {
                continue;
            }
            let mat = remote_player_matrix(
                rp.render_pos,
                rp.render_yaw,
                rp.anim_speed,
                rp.flags,
                time_s,
                entity_template,
            );
            append_remote_player_draws(
                &mut self.pants_cache,
                &base_mickey,
                mat,
                rp.info.color_slot,
                rp.flags,
                draws,
            );
        }
    }

    /// Draw floating overhead usernames (colored to match each player's unique pants color),
    /// text chat overlay, Discord join-request toast, voice HUD badge, and F9 lobby window.
    pub fn draw_overlays(
        &mut self,
        ui: &mut egui::Ui,
        viewport: egui::Rect,
        view_projection: &[f32; 16],
        local_pos: [f32; 3],
    ) {
        self.draw_nameplates(ui, viewport, view_projection, local_pos);
        self.draw_top_right_badge(ui.ctx(), viewport);
        self.draw_discord_request_toast(ui.ctx(), viewport);
        self.draw_chat_overlay(ui.ctx(), viewport);
        self.draw_floating_lobby_window(ui.ctx());
    }

    fn draw_nameplates(
        &self,
        ui: &egui::Ui,
        viewport: egui::Rect,
        view_projection: &[f32; 16],
        local_pos: [f32; 3],
    ) {
        if !self.is_online() {
            return;
        }
        let my_level_hash = level_hash(&self.current_level);
        let now = Instant::now();
        let painter = ui.painter().with_clip_rect(viewport);

        for rp in self.remotes.values() {
            if !rp.has_state || rp.level_hash != my_level_hash {
                continue;
            }
            let head = [rp.render_pos[0], rp.render_pos[1] + 2.15, rp.render_pos[2]];
            let dx = head[0] - local_pos[0];
            let dy = head[1] - local_pos[1];
            let dz = head[2] - local_pos[2];
            let dist = (dx * dx + dy * dy + dz * dz).sqrt();
            if dist > 70.0 {
                continue;
            }

            let clip: [f32; 4] = std::array::from_fn(|row| {
                view_projection[row] * head[0]
                    + view_projection[4 + row] * head[1]
                    + view_projection[8 + row] * head[2]
                    + view_projection[12 + row]
            });
            if !clip.iter().all(|v| v.is_finite())
                || clip[3] <= 0.1
                || clip[0].abs() > clip[3] * 1.1
                || clip[1].abs() > clip[3] * 1.1
                || clip[2].abs() > clip[3]
            {
                continue;
            }
            let screen_pos = viewport.center()
                + egui::vec2(
                    0.5 * viewport.width() * clip[0] / clip[3],
                    -0.5 * viewport.height() * clip[1] / clip[3],
                );

            let pcol = pants_color(rp.info.color_slot);
            let speaking = rp.is_speaking(now);
            let label = if speaking {
                format!("● {} [VOICE]", rp.info.username)
            } else {
                format!("● {}", rp.info.username)
            };
            let font_size = (15.5 - dist * 0.08).clamp(11.0, 15.5);
            let galley = painter.layout_no_wrap(
                label,
                egui::FontId::proportional(font_size),
                pcol.text_color(),
            );
            let pad = egui::vec2(8.0, 3.5);
            let pill_size = galley.size() + pad * 2.0;
            let pill_rect = egui::Rect::from_center_size(screen_pos, pill_size);

            painter.rect_filled(
                pill_rect,
                6.0,
                egui::Color32::from_rgba_unmultiplied(12, 15, 22, 205),
            );
            let stroke_col = if speaking {
                egui::Color32::from_rgb(90, 240, 130)
            } else {
                pcol.egui_color()
            };
            painter.rect_stroke(
                pill_rect,
                6.0,
                egui::Stroke::new(if speaking { 2.0 } else { 1.3 }, stroke_col),
                egui::StrokeKind::Inside,
            );
            painter.galley(pill_rect.min + pad, galley, pcol.text_color());
        }
    }

    fn draw_top_right_badge(&mut self, ctx: &egui::Context, viewport: egui::Rect) {
        let pos = egui::pos2(viewport.right() - 12.0, viewport.top() + 12.0);
        egui::Area::new(egui::Id::new("blotworx_mp_badge"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::RIGHT_TOP)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_unmultiplied(15, 19, 27, 210))
                    .stroke(egui::Stroke::new(
                        1.0,
                        if self.is_online() {
                            pants_color(self.local_color_slot()).egui_color()
                        } else {
                            egui::Color32::from_rgb(65, 74, 86)
                        },
                    ))
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if self.is_online() {
                                let pcol = pants_color(self.local_color_slot());
                                ui.colored_label(
                                    pcol.text_color(),
                                    format!(
                                        "● ONLINE {}/{}",
                                        self.player_count(),
                                        MAX_PLAYERS
                                    ),
                                );
                                if self.voice_settings.enabled {
                                    let (v_lbl, v_col) = if self.voice_settings.mic_muted {
                                        ("MIC MUTED", egui::Color32::from_rgb(220, 90, 90))
                                    } else if self.voice.is_speaking() {
                                        ("SPEAKING", egui::Color32::from_rgb(85, 240, 125))
                                    } else if self.voice_settings.push_to_talk {
                                        ("[V] Voice", egui::Color32::from_rgb(175, 185, 200))
                                    } else {
                                        ("Open Mic", egui::Color32::from_rgb(120, 215, 160))
                                    };
                                    ui.separator();
                                    ui.colored_label(v_col, v_lbl);
                                }
                            } else {
                                ui.weak("Offline");
                            }
                            ui.separator();
                            if ui.small_button("Multiplayer · F9").clicked() {
                                self.panel_open = !self.panel_open;
                            }
                        });
                    });
            });
    }

    fn draw_discord_request_toast(&mut self, ctx: &egui::Context, viewport: egui::Rect) {
        if self.pending_discord_requests.is_empty() {
            return;
        }
        let req = self.pending_discord_requests[0].clone();
        egui::Area::new(egui::Id::new("blotworx_discord_req"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::CENTER_TOP)
            .fixed_pos(egui::pos2(viewport.center().x, viewport.top() + 14.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(28, 32, 48))
                    .stroke(egui::Stroke::new(1.5, egui::Color32::from_rgb(88, 101, 242)))
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::symmetric(14, 8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.colored_label(
                                egui::Color32::from_rgb(138, 150, 255),
                                format!("Discord Invite Request: {} wants to join!", req.username),
                            );
                            if ui.button("Accept").clicked() {
                                self.discord.respond_join_request(&req.user_id, true);
                                self.pending_discord_requests.remove(0);
                                self.push_system_chat(format!(
                                    "Accepted Discord join request from {}.",
                                    req.username
                                ));
                            }
                            if ui.button("Decline").clicked() {
                                self.discord.respond_join_request(&req.user_id, false);
                                self.pending_discord_requests.remove(0);
                            }
                        });
                    });
            });
    }

    fn draw_chat_overlay(&mut self, ctx: &egui::Context, viewport: egui::Rect) {
        let now = Instant::now();
        let has_recent = self
            .chat_log
            .back()
            .is_some_and(|m| now.duration_since(m.received_at) < Duration::from_secs(11));
        if !self.chat_open && !has_recent {
            return;
        }

        let chat_width = (viewport.width() * 0.34).clamp(280.0, 440.0);
        let pos = egui::pos2(viewport.left() + 14.0, viewport.bottom() - 18.0);
        egui::Area::new(egui::Id::new("blotworx_mp_chat"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                let bg_alpha = if self.chat_open { 215 } else { 155 };
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_unmultiplied(12, 15, 22, bg_alpha))
                    .stroke(egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_unmultiplied(70, 80, 96, bg_alpha),
                    ))
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.set_width(chat_width);
                        let max_msgs = if self.chat_open { 12 } else { 6 };
                        let total = self.chat_log.len();
                        let skip = total.saturating_sub(max_msgs);
                        egui::ScrollArea::vertical()
                            .max_height(if self.chat_open { 190.0 } else { 120.0 })
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                for msg in self.chat_log.iter().skip(skip) {
                                    if !self.chat_open
                                        && now.duration_since(msg.received_at)
                                            > Duration::from_secs(11)
                                    {
                                        continue;
                                    }
                                    ui.horizontal_wrapped(|ui| {
                                        ui.spacing_mut().item_spacing.x = 5.0;
                                        if msg.is_system {
                                            ui.colored_label(
                                                egui::Color32::from_rgb(155, 195, 235),
                                                &msg.text,
                                            );
                                        } else {
                                            let c = pants_color(msg.color_slot).text_color();
                                            ui.colored_label(c, format!("{}:", msg.sender));
                                            ui.label(&msg.text);
                                        }
                                    });
                                }
                            });

                        if self.chat_open {
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                let resp = ui.add_sized(
                                    egui::vec2(chat_width - 66.0, 24.0),
                                    egui::TextEdit::singleline(&mut self.chat_draft)
                                        .hint_text("Type message (Enter to send, Esc to close)..."),
                                );
                                if self.chat_focus_requested {
                                    resp.request_focus();
                                    self.chat_focus_requested = false;
                                }
                                let send_enter =
                                    resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                let send_btn = ui.button("Send").clicked();
                                if send_enter || send_btn {
                                    self.submit_chat_draft();
                                    self.chat_open = false;
                                }
                                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                    self.chat_draft.clear();
                                    self.chat_open = false;
                                }
                            });
                        } else {
                            ui.weak("Press T to chat · Hold V for proximity voice");
                        }
                    });
            });
    }

    fn submit_chat_draft(&mut self) {
        let text = self.chat_draft.trim().to_owned();
        self.chat_draft.clear();
        if text.is_empty() {
            return;
        }
        if let Some(client) = &self.client {
            client.send_chat(text);
        } else {
            // Allow local chat testing even before hosting/joining
            let color_slot = self.local_color_slot();
            self.chat_log.push_back(ChatEntry {
                sender: self.username.clone(),
                text,
                color_slot,
                is_system: false,
                received_at: Instant::now(),
            });
        }
    }

    fn draw_floating_lobby_window(&mut self, ctx: &egui::Context) {
        if !self.panel_open {
            return;
        }
        let mut open = self.panel_open;
        egui::Window::new("Blotworx Online Multiplayer (16 Players)")
            .id(egui::Id::new("blotworx_mp_window"))
            .open(&mut open)
            .resizable(true)
            .default_width(560.0)
            .show(ctx, |ui| {
                self.draw_lobby_contents(ui, ctx);
            });
        self.panel_open = open;
    }

    /// Reusable lobby UI (shown in both `F9` window and `F10 -> Multiplayer` options tab).
    pub fn draw_lobby_contents(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let my_color = pants_color(self.local_color_slot());

        // 1. Player Profile & Unique Pants Color preview
        ui.horizontal_wrapped(|ui| {
            ui.strong("Username:");
            let edit = ui.add_enabled(
                !self.is_online(),
                egui::TextEdit::singleline(&mut self.username)
                    .desired_width(160.0)
                    .char_limit(24),
            );
            if edit.changed() && !self.is_online() {
                self.local_color_slot = preferred_color_slot(&self.username);
            }
            let (swatch_rect, _) =
                ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
            ui.painter()
                .rect_filled(swatch_rect, 4.0, my_color.egui_color());
            ui.painter().rect_stroke(
                swatch_rect,
                4.0,
                egui::Stroke::new(1.0, egui::Color32::WHITE),
                egui::StrokeKind::Inside,
            );
            ui.colored_label(my_color.text_color(), format!("{} Pants", my_color.name));
        });

        if let Some(err) = &self.last_error {
            ui.colored_label(egui::Color32::from_rgb(245, 110, 110), err);
        }
        if let Some((notice, when)) = &self.status_notice {
            if when.elapsed() < Duration::from_secs(6) {
                ui.colored_label(egui::Color32::from_rgb(110, 230, 150), notice);
            }
        }

        ui.separator();

        // 2. Host / Join Controls
        if !self.is_online() {
            ui.columns(2, |cols| {
                cols[0].strong("Host Online Lobby (up to 16)");
                cols[0].horizontal(|ui| {
                    ui.label("UDP Port:");
                    ui.add(egui::TextEdit::singleline(&mut self.host_port).desired_width(72.0));
                });
                cols[0].horizontal(|ui| {
                    ui.label("Public Address:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.public_addr_override)
                            .desired_width(150.0)
                            .hint_text(self.public_ip.as_deref().unwrap_or(&self.lan_ip)),
                    );
                });
                if cols[0].button("Host Online Game").clicked() {
                    self.host();
                }
                cols[0].weak(format!("LAN IP: {}", self.lan_ip));

                cols[1].strong("Join Online Lobby");
                cols[1].add(
                    egui::TextEdit::singleline(&mut self.join_input)
                        .desired_width(220.0)
                        .hint_text("IP:Port or blotworx://join/..."),
                );
                if cols[1].button("Connect").clicked() {
                    let target = self.join_input.clone();
                    self.join(&target);
                }
                cols[1].weak("Paste an IP:Port or Discord invite message.");
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                match &self.state {
                    ConnectionState::Hosting { port } => {
                        ui.colored_label(
                            egui::Color32::from_rgb(100, 235, 145),
                            format!(
                                "Hosting Lobby ({}/{MAX_PLAYERS}) · UDP :{port}",
                                self.player_count()
                            ),
                        );
                    }
                    ConnectionState::Connected { addr } => {
                        ui.colored_label(
                            egui::Color32::from_rgb(100, 210, 250),
                            format!(
                                "Connected to {addr} ({}/{MAX_PLAYERS})",
                                self.player_count()
                            ),
                        );
                    }
                    ConnectionState::Connecting(addr) => {
                        ui.colored_label(
                            egui::Color32::YELLOW,
                            format!("Connecting to {addr}..."),
                        );
                    }
                    ConnectionState::Offline => {}
                }
                if ui.button("Disconnect").clicked() {
                    self.disconnect();
                }
                if !self.host_level.is_empty()
                    && !self.host_level.eq_ignore_ascii_case(&self.current_level)
                {
                    if ui
                        .button(format!(
                            "Warp to Host Level ({})",
                            Self::pretty_level_name(&self.host_level)
                        ))
                        .clicked()
                    {
                        self.pending_level_switch = Some(self.host_level.clone());
                    }
                }
            });
        }

        ui.separator();

        // 3. Discord Invite Integration
        ui.collapsing("Discord Invites & Rich Presence", |ui| {
            let dc_connected = self.discord.is_connected();
            if dc_connected {
                ui.colored_label(
                    egui::Color32::from_rgb(138, 150, 255),
                    format!(
                        "● Discord Desktop Connected ({}) — Rich Presence & Join Invites active!",
                        self.discord.connected_user()
                    ),
                );
            } else {
                ui.weak("Discord Desktop IPC not detected (launch Discord desktop for native Rich Presence invites, or use Copy Discord Invite below).");
            }
            ui.horizontal_wrapped(|ui| {
                if let Some(addr) = self.invite_address() {
                    let invite_text = format_discord_invite(
                        &addr,
                        &Self::pretty_level_name(&self.current_level),
                        self.player_count(),
                        MAX_PLAYERS,
                    );
                    if ui.button("Copy Discord Invite").clicked() {
                        ctx.copy_text(invite_text);
                        self.set_notice("Copied Discord invite (with blotworx:// link) to clipboard!".into());
                    }
                    if ui.button("Copy Direct Address").clicked() {
                        ctx.copy_text(addr.clone());
                        self.set_notice(format!("Copied {addr} to clipboard!"));
                    }
                    if ui.button("Open Discord").clicked() {
                        let _ = open::that("https://discord.com/channels/@me");
                    }
                } else {
                    ui.weak("Host or join a lobby to generate a Discord invite.");
                }
            });
            ui.horizontal(|ui| {
                ui.label("Discord App ID:");
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.discord_app_id_input).desired_width(180.0),
                );
                if ui.small_button("Apply").clicked() || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                    let id = self.discord_app_id_input.clone();
                    self.discord.set_app_id(id);
                }
            });
        });

        ui.separator();

        // 4. Proximity Voice Chat
        ui.collapsing("Proximity Voice Chat", |ui| {
            let mut changed = false;
            ui.horizontal_wrapped(|ui| {
                changed |= ui
                    .checkbox(&mut self.voice_settings.enabled, "Enable Proximity Voice")
                    .changed();
                changed |= ui
                    .checkbox(&mut self.voice_settings.push_to_talk, "Push-to-Talk (Hold V)")
                    .changed();
                changed |= ui
                    .checkbox(&mut self.voice_settings.mic_muted, "Mute Mic")
                    .changed();
                changed |= ui
                    .checkbox(&mut self.voice_settings.deafened, "Deafen")
                    .changed();
            });
            if !self.voice.mic_available() {
                ui.colored_label(
                    egui::Color32::from_rgb(240, 180, 90),
                    "No microphone detected (you can still hear other players in proximity chat).",
                );
            } else {
                let level = self.voice.mic_level().clamp(0.0, 1.0);
                ui.horizontal(|ui| {
                    ui.label("Mic Level:");
                    ui.add(
                        egui::ProgressBar::new(level)
                            .desired_width(180.0)
                            .text(if self.voice.is_speaking() { "Transmitting" } else { "Idle" }),
                    );
                    changed |= ui
                        .checkbox(&mut self.voice_settings.loopback_test, "Mic Loopback Test")
                        .changed();
                });
            }
            changed |= ui
                .add(egui::Slider::new(&mut self.voice_settings.voice_volume, 0.0..=2.0).text("Voice Volume"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut self.voice_settings.mic_gain, 0.2..=3.0).text("Mic Gain"))
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut self.voice_settings.proximity_range, 8.0..=60.0)
                        .suffix(" m")
                        .text("Proximity Range"),
                )
                .changed();
            if !self.voice_settings.push_to_talk {
                changed |= ui
                    .add(
                        egui::Slider::new(&mut self.voice_settings.vad_threshold, 0.005..=0.15)
                            .text("Open-Mic Sensitivity"),
                    )
                    .changed();
            }
            if changed {
                self.voice.update_settings(&self.voice_settings);
            }
        });

        ui.separator();

        // 5. Connected Players Roster (up to 16)
        ui.strong(format!("Players ({}/{MAX_PLAYERS})", self.player_count()));
        egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
            // Local player row
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                ui.painter().rect_filled(r, 3.0, my_color.egui_color());
                ui.colored_label(my_color.text_color(), format!("{} (You)", self.username));
                ui.weak(format!(
                    "· {} · {}",
                    my_color.name,
                    Self::pretty_level_name(&self.current_level)
                ));
                if self.voice.is_speaking() {
                    ui.colored_label(egui::Color32::from_rgb(90, 240, 130), "[SPEAKING]");
                }
            });
            // Remote player rows
            let now = Instant::now();
            let mut ids: Vec<u8> = self.remotes.keys().copied().collect();
            ids.sort_unstable();
            for id in ids {
                if let Some(rp) = self.remotes.get_mut(&id) {
                    let pcol = pants_color(rp.info.color_slot);
                    let speaking = rp.is_speaking(now);
                    ui.horizontal(|ui| {
                        let (r, _) =
                            ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                        ui.painter().rect_filled(r, 3.0, pcol.egui_color());
                        ui.colored_label(pcol.text_color(), &rp.info.username);
                        ui.weak(format!(
                            "· {} · {}",
                            pcol.name,
                            Self::pretty_level_name(&rp.info.level)
                        ));
                        if speaking {
                            ui.colored_label(egui::Color32::from_rgb(90, 240, 130), "[VOICE]");
                        }
                        let lbl = if rp.muted { "Unmute" } else { "Mute" };
                        if ui.small_button(lbl).clicked() {
                            rp.muted = !rp.muted;
                            self.voice.set_player_muted(id, rp.muted);
                        }
                    });
                }
            }
        });

        ui.add_space(6.0);
        ui.weak("Shortcuts: F9 · Multiplayer Panel   T · Text Chat   Hold V · Proximity Voice");
    }
}
