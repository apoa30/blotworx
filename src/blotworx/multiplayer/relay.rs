//! Zero-port-forwarding online multiplayer relay transport using MQTT v3.1.1 over outbound TCP.
//!
//! Works through home routers, strict NAT, CGNAT, mobile hotspots, and firewalls with ZERO port
//! forwarding or firewall configuration: both host and clients connect outbound to a public
//! low-latency pub/sub relay broker and exchange compact binary Blotworx packets on a room topic.

use anyhow::{bail, Context, Result};
use std::{
    io::{ErrorKind, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const DEFAULT_RELAY_BROKERS: &[&str] = &["broker.emqx.io:1883", "broker.hivemq.com:1883"];

/// Generate a friendly, easy-to-share online Room Code (e.g. `BLOT-7K3P`).
pub fn generate_room_code() -> String {
    const ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x5A17_9C31);
    let mut state = nanos ^ ((std::process::id() as u64) << 32) ^ 0x9E37_79B9_7F4A_7C15;
    let mut code = String::from("BLOT-");
    for _ in 0..4 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let idx = (state as usize) % ALPHABET.len();
        code.push(ALPHABET[idx] as char);
    }
    code
}

/// Normalize a user-typed room code (uppercase, alphanumeric + `-`).
pub fn normalize_room_code(raw: &str) -> String {
    raw.trim()
        .to_ascii_uppercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(20)
        .collect()
}

/// Returns `true` if `target` looks like a Room Code rather than an `IP:Port` or `host:port`.
pub fn is_room_code_target(target: &str) -> bool {
    let t = target.trim();
    if t.is_empty() {
        return false;
    }
    if t.starts_with("room:") || t.starts_with("ROOM:") {
        return true;
    }
    // IP:Port or domain:port has a colon or dot; room codes like `BLOT-7K3P` have neither
    !t.contains(':') && !t.contains('.')
}

pub fn topic_c2s(room: &str, client_tag: &str) -> String {
    format!("blotworx/mp/v1/{room}/c2s/{client_tag}")
}

pub fn topic_c2s_wildcard(room: &str) -> String {
    format!("blotworx/mp/v1/{room}/c2s/+")
}

pub fn topic_s2c_broadcast(room: &str) -> String {
    format!("blotworx/mp/v1/{room}/s2c/all")
}

pub fn topic_s2c_direct(room: &str, client_tag: &str) -> String {
    format!("blotworx/mp/v1/{room}/s2c/{client_tag}")
}

pub struct RelayConn {
    stream: TcpStream,
    rx_buf: Vec<u8>,
    last_ping: Instant,
    next_pkt_id: u16,
    pub broker_addr: String,
}

impl RelayConn {
    /// Connect to the first reachable relay broker (trying custom override if provided, then defaults).
    pub fn connect(custom_broker: Option<&str>, client_id: &str) -> Result<Self> {
        let mut candidates: Vec<&str> = Vec::new();
        if let Some(c) = custom_broker.map(str::trim).filter(|s| !s.is_empty()) {
            candidates.push(c);
        }
        for &b in DEFAULT_RELAY_BROKERS {
            if !candidates.contains(&b) {
                candidates.push(b);
            }
        }

        let mut last_err = anyhow::anyhow!("No relay brokers configured");
        for broker in candidates {
            match Self::connect_one(broker, client_id) {
                Ok(conn) => return Ok(conn),
                Err(e) => last_err = e,
            }
        }
        Err(last_err)
    }

    fn connect_one(broker: &str, client_id: &str) -> Result<Self> {
        let addr = broker
            .to_socket_addrs()
            .with_context(|| format!("Resolve relay broker {broker}"))?
            .find(|a| a.is_ipv4())
            .or_else(|| broker.to_socket_addrs().ok()?.next())
            .with_context(|| format!("No address for relay broker {broker}"))?;

        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(4))
            .with_context(|| format!("Connect to relay broker {broker}"))?;
        let _ = stream.set_nodelay(true);
        stream.set_read_timeout(Some(Duration::from_secs(4)))?;
        stream.set_write_timeout(Some(Duration::from_secs(4)))?;

        // Build MQTT v3.1.1 CONNECT packet
        let mut payload = Vec::with_capacity(64);
        // Protocol Name "MQTT"
        put_mqtt_str(&mut payload, "MQTT");
        payload.push(4); // Protocol Level 4 (MQTT 3.1.1)
        payload.push(0x02); // Clean Session
        payload.extend_from_slice(&30u16.to_be_bytes()); // Keep Alive 30s
        put_mqtt_str(&mut payload, client_id);

        let mut pkt = Vec::with_capacity(payload.len() + 5);
        pkt.push(0x10); // CONNECT
        encode_remaining_len(&mut pkt, payload.len());
        pkt.extend_from_slice(&payload);
        stream.write_all(&pkt)?;
        stream.flush()?;

        // Read 4-byte CONNACK
        let mut connack = [0u8; 4];
        stream
            .read_exact(&mut connack)
            .with_context(|| format!("Read CONNACK from {broker}"))?;
        if connack[0] != 0x20 || connack[1] != 0x02 || connack[3] != 0x00 {
            bail!("Relay broker {broker} rejected CONNECT (code={})", connack[3]);
        }

        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            rx_buf: Vec::with_capacity(8192),
            last_ping: Instant::now(),
            next_pkt_id: 1,
            broker_addr: broker.to_owned(),
        })
    }

    pub fn subscribe(&mut self, topic: &str) -> Result<()> {
        self.next_pkt_id = self.next_pkt_id.wrapping_add(1).max(1);
        let mut body = Vec::with_capacity(topic.len() + 6);
        body.extend_from_slice(&self.next_pkt_id.to_be_bytes());
        put_mqtt_str(&mut body, topic);
        body.push(0x00); // QoS 0

        let mut pkt = Vec::with_capacity(body.len() + 5);
        pkt.push(0x82); // SUBSCRIBE
        encode_remaining_len(&mut pkt, body.len());
        pkt.extend_from_slice(&body);
        self.write_all_nonblocking(&pkt)
    }

    pub fn publish(&mut self, topic: &str, data: &[u8]) -> Result<()> {
        let mut body = Vec::with_capacity(topic.len() + 2 + data.len());
        put_mqtt_str(&mut body, topic);
        body.extend_from_slice(data);

        let mut pkt = Vec::with_capacity(body.len() + 5);
        pkt.push(0x30); // PUBLISH QoS 0
        encode_remaining_len(&mut pkt, body.len());
        pkt.extend_from_slice(&body);
        self.write_all_nonblocking(&pkt)
    }

    pub fn maybe_ping(&mut self) -> Result<()> {
        if self.last_ping.elapsed() >= Duration::from_secs(12) {
            self.last_ping = Instant::now();
            self.write_all_nonblocking(&[0xC0, 0x00])?; // PINGREQ
        }
        Ok(())
    }

    fn write_all_nonblocking(&mut self, mut bytes: &[u8]) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !bytes.is_empty() {
            match self.stream.write(bytes) {
                Ok(0) => bail!("Relay connection closed during write"),
                Ok(n) => bytes = &bytes[n..],
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        bail!("Relay write timed out");
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// Poll for any received `(topic, payload)` PUBLISH messages. Returns `Err` if disconnected.
    pub fn poll_messages(&mut self) -> Result<Vec<(String, Vec<u8>)>> {
        let mut tmp = [0u8; 4096];
        loop {
            match self.stream.read(&mut tmp) {
                Ok(0) => bail!("Relay broker closed connection"),
                Ok(n) => self.rx_buf.extend_from_slice(&tmp[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }

        let mut out = Vec::new();
        loop {
            let Some((pkt_type, header_len, rem_len)) = peek_mqtt_frame(&self.rx_buf) else {
                break;
            };
            let total_len = header_len + rem_len;
            if self.rx_buf.len() < total_len {
                break;
            }
            let frame_body = self.rx_buf[header_len..total_len].to_vec();
            self.rx_buf.drain(..total_len);

            if (pkt_type & 0xF0) == 0x30 {
                // PUBLISH
                if frame_body.len() >= 2 {
                    let topic_len = u16::from_be_bytes([frame_body[0], frame_body[1]]) as usize;
                    let qos = (pkt_type >> 1) & 0x03;
                    let mut pos = 2 + topic_len;
                    if qos > 0 {
                        pos += 2; // skip packet identifier if broker sent QoS > 0
                    }
                    if pos <= frame_body.len() {
                        let topic =
                            String::from_utf8_lossy(&frame_body[2..2 + topic_len]).into_owned();
                        let payload = frame_body[pos..].to_vec();
                        out.push((topic, payload));
                    }
                }
            }
        }
        Ok(out)
    }
}

fn put_mqtt_str(out: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    out.extend_from_slice(&(b.len() as u16).to_be_bytes());
    out.extend_from_slice(b);
}

fn encode_remaining_len(out: &mut Vec<u8>, mut len: usize) {
    loop {
        let mut byte = (len % 128) as u8;
        len /= 128;
        if len > 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if len == 0 {
            break;
        }
    }
}

fn peek_mqtt_frame(buf: &[u8]) -> Option<(u8, usize, usize)> {
    if buf.len() < 2 {
        return None;
    }
    let pkt_type = buf[0];
    let mut multiplier: usize = 1;
    let mut value: usize = 0;
    let mut idx = 1usize;
    loop {
        if idx >= buf.len() {
            return None;
        }
        let encoded = buf[idx];
        value += ((encoded & 0x7F) as usize) * multiplier;
        idx += 1;
        if (encoded & 0x80) == 0 {
            return Some((pkt_type, idx, value));
        }
        multiplier *= 128;
        if idx > 5 {
            return None;
        }
    }
}
