//! Native Lua 5.1 compilation with EM1's 32-bit chunk layout.
//! Compiling source does not execute it in the host process.
use anyhow::{bail, ensure, Context, Result};
#[path="havok_script.rs"]
pub(crate) mod havok_script;
#[path = "lua_cook/repository.rs"]
pub mod repository;

pub fn content_hash(data: &[u8]) -> String {
    crate::sha256(data)
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect()
}
/// A bounded process cache shared by preview, graph analysis and repeated packs.
fn cached(key: String, generate: impl FnOnce() -> Result<Vec<u8>>) -> Result<Vec<u8>> {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<std::collections::VecDeque<(String, Vec<u8>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    {
        let mut c = cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = c.iter().position(|(k, _)| *k == key) {
            let entry = c.remove(i).unwrap();
            let bytes = entry.1.clone();
            c.push_back(entry);
            return Ok(bytes);
        }
    }
    let bytes = generate()?;
    let mut c = cache.lock().unwrap_or_else(|e| e.into_inner());
    let mut size: usize = c.iter().map(|(_, v)| v.len()).sum();
    const LIMIT: usize = 32 * 1024 * 1024;
    while size + bytes.len() > LIMIT {
        if let Some((_, v)) = c.pop_front() {
            size -= v.len();
        } else {
            break;
        }
    }
    if bytes.len() <= LIMIT {
        c.push_back((key, bytes.clone()));
    }
    Ok(bytes)
}
pub fn native_source(data: &[u8]) -> Result<String> {
    String::from_utf8(cached(format!("native-v1:{}", content_hash(data)), || {
        Ok(decompile(data)?.into_bytes())
    })?)
    .map_err(Into::into)
}
pub fn source(data: &[u8]) -> Result<String> {
    if let Some(entry) = repository::lookup(data) {
        return Ok(entry.source.into());
    }
    native_source(data)
}
pub fn runtime_asset(path: &str, data: &[u8]) -> Result<Vec<u8>> {
    if !path.to_lowercase().ends_with(".lua") || data.starts_with(b"\x1bLua") {
        return Ok(data.to_vec());
    }
    compile(std::str::from_utf8(data)?).with_context(|| format!("Cannot cook Lua script {path}"))
}
pub fn compile(source: &str) -> Result<Vec<u8>> {
    cached(
        format!("compile-v1:{}", content_hash(source.as_bytes())),
        || compile_uncached(source),
    )
}
fn compile_uncached(source: &str) -> Result<Vec<u8>> {
    let lua = mlua::Lua::new();
    let function = lua
        .load(source)
        .set_name("@BlueThinner")
        .into_function()
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let bytes = function.dump(true);
    Ok(convert(&bytes, 4, true)?.0)
}
/// Reconstruct source through native control-flow analysis. Never executes game code.
pub fn decompile(data: &[u8]) -> Result<String> {
    ensure!(
        data.len() <= 16 * 1024 * 1024,
        "Lua chunk exceeds the editor limit"
    );
    strings(data)?; // Validate the chunk's bounded 5.1 layout before analysis.
    let options = unluac::decompile::DecompileOptions {
        dialect: unluac::decompile::DecompileDialect::Lua51,
        ..Default::default()
    };
    let result = std::panic::catch_unwind(|| unluac::decompile::decompile(data, options))
        .map_err(|_| anyhow::anyhow!("The Lua decompiler rejected this chunk"))?
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let source = result
        .state
        .generated
        .context("Decompiler returned no source")?
        .source;
    compile(&source).context("Reconstructed Lua did not pass Lua 5.1 syntax validation")?;
    Ok(source)
}
pub fn strings(data: &[u8]) -> Result<Vec<String>> {
    Ok(convert(data, 4, true)?.1)
}
pub fn globals(data: &[u8]) -> Result<Vec<String>> {
    Ok(convert(data, 4, true)?.2)
}
pub fn executable_signature(data: &[u8]) -> Result<String> {
    Ok(content_hash(&convert_inner(data, 4, true, true)?.0))
}
/// Adapt original chunks for the native host VM. EM1 instructions are retained;
/// EM2 HavokScript is translated directly, without source reconstruction.
/// Executing HKS callers must prepare its native helpers with prepare_host_chunk.
pub fn prepare_host_chunk(lua:&mlua::Lua,data:&[u8])->Result<Vec<u8>> {
    if data.get(..6)==Some(b"\x1bLua\x51\x0d") && !lua.named_registry_value::<bool>("blotworx_hks_helpers").unwrap_or(false) {
        havok_script::install(lua)?;
        lua.set_named_registry_value("blotworx_hks_helpers",true)?;
    }
    host_chunk(data)
}
pub fn host_chunk(data: &[u8]) -> Result<Vec<u8>> {
    if !data.starts_with(b"\x1bLua") {
        return Ok(data.to_vec());
    }
    let translated;
    let data=if data.get(..6)==Some(b"\x1bLua\x51\x0d") {
        translated=havok_script::translate(data)?;translated.as_slice()
    }else{data};
    Ok(convert(
        data,
        std::mem::size_of::<usize>(),
        cfg!(target_endian = "little"),
    )?
    .0)
}
fn convert(
    data: &[u8],
    target_size: usize,
    target_le: bool,
) -> Result<(Vec<u8>, Vec<String>, Vec<String>)> {
    convert_inner(data, target_size, target_le, false)
}
fn convert_inner(
    data: &[u8],
    target_size: usize,
    target_le: bool,
    normalize: bool,
) -> Result<(Vec<u8>, Vec<String>, Vec<String>)> {
    ensure!(
        data.len() >= 12 && &data[..6] == b"\x1bLua\x51\0",
        "Only standard Lua 5.1 chunks are supported"
    );
    ensure!(
        data[7] == 4 && [4, 8].contains(&data[8]) && data[9] == 4 && data[10] == 8 && data[11] == 0,
        "Unsupported Lua numeric layout"
    );
    struct Reader<'a> {
        data: &'a [u8],
        normalize: bool,
        pos: usize,
        le: bool,
        size: usize,
        target_size: usize,
        target_le: bool,
        out: Vec<u8>,
        strings: Vec<String>,
        globals: Vec<String>,
    }
    impl Reader<'_> {
        fn raw(&mut self, n: usize) -> Result<Vec<u8>> {
            let end = self.pos.checked_add(n).context("Lua offset overflow")?;
            let bytes = self
                .data
                .get(self.pos..end)
                .context("Truncated Lua chunk")?
                .to_vec();
            self.pos = end;
            Ok(bytes)
        }
        fn copy(&mut self, n: usize) -> Result<()> {
            let b = self.raw(n)?;
            self.out.extend(b);
            Ok(())
        }
        fn scalar(&mut self, n: usize) -> Result<u64> {
            let mut b = self.raw(n)?;
            let mut number = [0u8; 8];
            if self.le {
                number[..n].copy_from_slice(&b);
            } else {
                for (i, v) in b.iter().rev().enumerate() {
                    number[i] = *v;
                }
            }
            if self.le != self.target_le {
                b.reverse();
            }
            self.out.extend(b);
            Ok(u64::from_le_bytes(number))
        }
        fn count(&mut self) -> Result<usize> {
            let n = self.scalar(4)? as usize;
            ensure!(n <= self.data.len(), "Invalid Lua array count");
            Ok(n)
        }
        fn string(&mut self, constant: bool) -> Result<()> {
            let b = self.raw(self.size)?;
            let mut n = [0u8; 8];
            if self.le {
                n[..self.size].copy_from_slice(&b);
            } else {
                for (i, v) in b.iter().rev().enumerate() {
                    n[i] = *v;
                }
            }
            let n = u64::from_le_bytes(n);
            ensure!(
                n <= u32::MAX as u64 && n <= self.data.len() as u64,
                "Invalid Lua string size"
            );
            let encoded = if self.target_le {
                n.to_le_bytes().to_vec()
            } else {
                n.to_be_bytes().to_vec()
            };
            if self.target_le {
                self.out.extend(&encoded[..self.target_size]);
            } else {
                self.out.extend(&encoded[8 - self.target_size..]);
            }
            let bytes = self.raw(n as usize)?;
            if constant && n > 0 {
                if let Ok(s) = std::str::from_utf8(&bytes[..bytes.len() - 1]) {
                    self.strings.push(s.into());
                }
            }
            self.out.extend(bytes);
            Ok(())
        }
        fn prototype(&mut self, depth: usize) -> Result<()> {
            ensure!(depth < 200, "Lua prototype nesting is too deep");
            let metadata = self.out.len();
            self.string(false)?;
            self.scalar(4)?;
            self.scalar(4)?;
            if self.normalize {
                self.out.truncate(metadata);
                self.out.resize(metadata + self.target_size + 8, 0);
            }
            self.copy(4)?;
            let count = self.count()?;
            let mut assignments = Vec::new();
            for _ in 0..count {
                let instruction = self.scalar(4)? as u32;
                if instruction & 63 == 7 {
                    assignments.push((instruction >> 14) as usize);
                }
            }
            let count = self.count()?;
            for constant in 0..count {
                let previous_strings = self.strings.len();
                let tag = self.raw(1)?[0];
                self.out.push(tag);
                match tag {
                    0 => {}
                    1 => self.copy(1)?,
                    3 => {
                        self.scalar(8)?;
                    }
                    4 => self.string(true)?,
                    _ => bail!("Unknown Lua constant type {tag}"),
                }
                if assignments.contains(&constant) && self.strings.len() > previous_strings {
                    self.globals.push(self.strings.last().unwrap().clone());
                }
            }
            let count = self.count()?;
            for _ in 0..count {
                self.prototype(depth + 1)?;
            }
            let debug = self.out.len();
            let count = self.count()?;
            for _ in 0..count {
                self.scalar(4)?;
            }
            let count = self.count()?;
            for _ in 0..count {
                self.string(false)?;
                self.scalar(4)?;
                self.scalar(4)?;
            }
            let count = self.count()?;
            for _ in 0..count {
                self.string(false)?;
            }
            if self.normalize {
                self.out.truncate(debug);
                self.out.resize(debug + 12, 0);
            }
            Ok(())
        }
    }
    let mut r = Reader {
        data,
        normalize,
        pos: 12,
        le: data[6] != 0,
        size: data[8] as usize,
        target_size,
        target_le,
        out: data[..12].to_vec(),
        strings: vec![],
        globals: vec![],
    };
    r.out[6] = u8::from(target_le);
    r.out[8] = target_size as u8;
    r.prototype(0)?;
    ensure!(r.pos == data.len(), "Unexpected trailing Lua chunk data");
    Ok((r.out, r.strings, r.globals))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reconstructed_lua_preserves_synthetic_control_flow_and_closures() {
        for source in [
            "local function f(x) if x<0 then return -x else return x*2 end end; return f(-3)+f(4)",
            "local t={1,2,4};local sum=0;for i,v in ipairs(t) do sum=sum+v end;return sum",
            "local function counter(n) return function(x) n=n+x; return n end end;local f=counter(5);return f(2)+f(3)",
            "local i=0;repeat i=i+1 until i>5;while i<10 do i=i+2 end;return i",
        ] {
            let expected:f64=mlua::Lua::new().load(source).eval().unwrap();
            let rebuilt=decompile(&compile(source).unwrap()).unwrap();
            let actual:f64=mlua::Lua::new().load(&rebuilt).eval().unwrap();
            assert_eq!(actual,expected,"{rebuilt}");
        }
        let data = compile("function MickeyScale() return 1 end; levelFlag=true").unwrap();
        assert!(globals(&data).unwrap().contains(&"MickeyScale".into()));
    }
    #[test]
    fn executable_comparison_ignores_debug_only_and_detects_logic_changes() {
        let a = compile("function Test(value) return value + 1 end").unwrap();
        let renamed =
            compile("-- reviewed\n\nfunction Test(number)\n return number + 1\nend").unwrap();
        let changed = compile("function Test(value) return value + 2 end").unwrap();
        assert_eq!(
            executable_signature(&a).unwrap(),
            executable_signature(&renamed).unwrap()
        );
        assert_ne!(
            executable_signature(&a).unwrap(),
            executable_signature(&changed).unwrap()
        );
        assert!(executable_signature(&a[..a.len() - 1]).is_err());
    }
    #[test]
    fn compiles_native_source_to_em1_layout() {
        let data =
            compile("local function f(x) return x+2 end; return f(3), 'Custom/test.nif'").unwrap();
        assert_eq!(&data[..12], b"\x1bLuaQ\0\x01\x04\x04\x04\x08\0");
        assert!(strings(&data).unwrap().contains(&"Custom/test.nif".into()));
        let host = convert(
            &data,
            std::mem::size_of::<usize>(),
            cfg!(target_endian = "little"),
        )
        .unwrap()
        .0;
        let values: (i32, String) = mlua::Lua::new().load(&host).eval().unwrap();
        assert_eq!(values, (5, "Custom/test.nif".into()));
        assert!(compile("not valid !!!").is_err());
    }
}
