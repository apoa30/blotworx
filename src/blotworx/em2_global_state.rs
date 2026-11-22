//! EM2 PC GlobalDataManager state. Source: PC a0bec0/a0c310/a0c4a0,
//! a0c910, a4e680 and JPS readers a4be30..a4be98; Wii U corroboration.
use anyhow::{ensure, Context, Result};
use std::collections::BTreeMap;
#[derive(Clone, Debug)]
struct Variable {
    name: String,
    id: u16,
    value: i16,
    initial: i16,
    flags: i16,
    initial_flags: i16,
}
#[derive(Debug)]
pub struct State {
    values: BTreeMap<u32, Variable>,
    pub id_limit: u16,
    pub records: usize,
}
/// The native key is zlib CRC32 seeded by the lowercased C-string length.
/// Its temporary C buffer retains at most 999 bytes (PC a0bec0).
fn key(name: &str) -> u32 {
    let bytes = name
        .as_bytes()
        .split(|b| *b == 0)
        .next()
        .unwrap_or_default();
    let bytes = &bytes[..bytes.len().min(999)];
    let mut crc = !(bytes.len() as u32);
    for b in bytes {
        crc ^= b.to_ascii_lowercase() as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ ((0u32.wrapping_sub(crc & 1)) & 0xedb88320);
        }
    }
    !crc
}
impl State {
    pub fn call(
        &mut self,
        lua: &mlua::Lua,
        name: &str,
        args: mlua::MultiValue,
    ) -> mlua::Result<mlua::MultiValue> {
        use mlua::{FromLua, Value};
        if !matches!(name, "GetGlobal" | "SetGlobal") {
            // EM2 Studio playtests: a service that isn't implemented yet does nothing (logged once)
            // instead of stopping the level's scripts. BLOTWORX_STRICT_EM2=1 restores the error.
            if std::env::var_os("BLOTWORX_STRICT_EM2").is_some() {
                return Err(mlua::Error::external(format!(
                    "EM2 world service is not implemented: {name}"
                )));
            }
            static SEEN: std::sync::Mutex<Option<std::collections::HashSet<String>>> = std::sync::Mutex::new(None);
            if SEEN.lock().unwrap().get_or_insert_with(Default::default).insert(name.to_string()) {
                log::warn!("EM2 world service not implemented (ignored): {name}");
            }
            return Ok(mlua::MultiValue::new());
        }
        let key = String::from_lua(args.front().cloned().unwrap_or(Value::Nil), lua)?;
        let result = if name == "GetGlobal" {
            self.get(&key).map(|v| Value::Integer(v as i64))
        } else {
            let value = i32::from_lua(args.get(1).cloned().unwrap_or(Value::Integer(0)), lua)?;
            self.set(&key, value).map(|v| Value::Integer(v as i64))
        };
        Ok(mlua::MultiValue::from_vec(vec![
            result.unwrap_or(Value::Nil)
        ]))
    }
    pub fn load(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() >= 8, "Truncated EM2 global database header");
        ensure!(
            u32::from_le_bytes(bytes[..4].try_into()?) == 44,
            "Unsupported EM2 global database version"
        );
        let mut state = Self {
            values: BTreeMap::new(),
            id_limit: u16::from_le_bytes(bytes[4..6].try_into()?),
            records: 0,
        };
        let mut at = 8;
        while at < bytes.len() {
            let header = bytes
                .get(at..at + 2)
                .context("Truncated EM2 global name header")?;
            let (stride, length) = (header[0] as usize, header[1] as usize);
            ensure!(
                stride >= 2 && stride % 4 == 0 && length <= stride - 2,
                "Invalid EM2 global name span at {at:#x}"
            );
            let data = bytes
                .get(at..at + stride)
                .context("Truncated EM2 global name")?;
            let name = if length == 0 {
                ""
            } else {
                let name = &data[2..2 + length];
                ensure!(
                    name.last() == Some(&0) && !name[..length - 1].contains(&0),
                    "Invalid EM2 global name terminator"
                );
                std::str::from_utf8(&name[..length - 1])
                    .context("Invalid EM2 global name encoding")?
            };
            at += stride;
            let fields = bytes
                .get(at..at + 12)
                .context("Truncated EM2 global value fields")?;
            let id = u16::from_le_bytes(fields[..2].try_into()?);
            let value = i16::from_le_bytes(fields[4..6].try_into()?);
            let flags = i16::from_le_bytes(fields[8..10].try_into()?);
            if !name.is_empty() {
                state.values.insert(
                    key(name),
                    Variable {
                        name: name.into(),
                        id,
                        value,
                        initial: value,
                        flags,
                        initial_flags: flags,
                    },
                );
            }
            state.records += 1;
            at += 12;
        }
        Ok(state)
    }
    pub fn get(&self, name: &str) -> Option<i16> {
        if name.as_bytes().first().is_none_or(|b| *b == 0) {
            return None;
        }
        self.values.get(&key(name)).map(|v| v.value)
    }
    /// SetGlobal stores the low signed halfword, but returns the original i32.
    /// Its optional third argument is popped but unused by the native setter.
    pub fn set(&mut self, name: &str, value: i32) -> Option<i32> {
        if name.as_bytes().first().is_none_or(|b| *b == 0) {
            return None;
        }
        self.values.get_mut(&key(name)).map(|v| {
            v.value = value as i16;
            value
        })
    }
    pub fn reset(&mut self, retain_mask: i16) {
        for v in self.values.values_mut() {
            if v.flags & retain_mask == 0 {
                v.value = v.initial;
                v.flags = v.initial_flags;
            }
        }
    }
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({"source_records":self.records,"variables":self.values.len(),"authored_id_limit":self.id_limit,
   "modified":self.values.values().filter(|v|v.value!=v.initial || v.flags!=v.initial_flags).map(|v|serde_json::json!({"name":v.name,"id":v.id,"value":v.value,"initial":v.initial,"flags":v.flags})).collect::<Vec<_>>(),
   "save_restoration_connected":false})
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn database() -> Vec<u8> {
        let mut b = 44u32.to_le_bytes().to_vec();
        b.extend([1, 0, 0xcd, 0xcd]);
        for (name, value, flags) in [("A", -2i16, 0i16), ("B", 1, 1)] {
            b.extend([4, 2, name.as_bytes()[0], 0]);
            for n in [0, value as u16, flags as u16] {
                b.extend((n as u16).to_le_bytes());
                b.extend([0xcd, 0xcd]);
            }
        }
        b
    }
    #[test]
    fn keys_match_pc_crc_call_seed_and_case_folding() {
        assert_eq!(key("YSL_SetupLevel"), 1427362874);
        assert_eq!(key("ysl_setuplevel"), 1427362874);
        assert_eq!(key("A"), 2679148245);
        assert_eq!(key("A\0ignored"), key("A"));
        assert_eq!(key(&"a".repeat(1100)), key(&"a".repeat(999)));
    }
    #[test]
    fn missing_names_stay_missing_and_set_returns_original_integer() -> Result<()> {
        let mut s = State::load(&database())?;
        assert_eq!(s.get("a"), Some(-2));
        assert_eq!(s.get("missing"), None);
        assert_eq!(s.set("missing", 8), None);
        assert_eq!(s.set("", 8), None);
        assert_eq!(s.set("A", 65535), Some(65535));
        assert_eq!(s.get("A"), Some(-1));
        s.set("B", 42);
        s.reset(1);
        assert_eq!(s.get("A"), Some(-2));
        assert_eq!(s.get("B"), Some(42));
        s.reset(0);
        assert_eq!(s.get("B"), Some(1));
        Ok(())
    }
    #[test]
    fn lua_host_reads_writes_and_returns_nil_for_unknown_names() -> Result<()> {
        let mut state = State::load(&database())?;
        let lua = mlua::Lua::new();
        lua.scope(|scope| {
            let host = scope.create_function_mut(|lua, (name, args): (String, mlua::MultiValue)| {
                state.call(lua, &name, args)
            })?;
            lua.globals().set("Host", host)?;
            lua.load("assert(Host('GetGlobal','a') == -2); assert(Host('SetGlobal','A',65535,true) == 65535); assert(Host('GetGlobal','A') == -1); assert(Host('GetGlobal','unknown') == nil); assert(Host('SetGlobal','unknown',4) == nil); assert(not pcall(Host,'Unsupported'))").exec()
        }).map_err(anyhow::Error::msg)?;
        assert_eq!(state.get("A"), Some(-1));
        Ok(())
    }
    #[test]
    fn partial_record_and_bad_version_are_rejected() {
        let b = database();
        for n in 9..24 {
            assert!(State::load(&b[..n]).is_err(), "length {n}");
        }
        let mut bad = b.clone();
        bad[0] = 30;
        assert!(State::load(&bad).is_err());
        bad = b;
        bad[8] = 0;
        assert!(State::load(&bad).is_err());
    }
}
