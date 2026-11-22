//! Native motion-controller animation names. The original REL constructs this
//! table in registration order (2ec040..2ee290); numeric IDs are not HKX-local.
use anyhow::{ensure, Context, Result};
use std::{collections::BTreeMap, path::Path};
pub struct Catalog {
    names: Vec<String>,
    indices: BTreeMap<String, u32>,
    variables: Vec<String>,
}
const SOURCE_SHA256: &str = "ac25d520f329a3d0507d656f0ab1cf1c34025bb36ff866187ad082ae3e0a4ebf";
impl Catalog {
    pub fn load(root: &Path) -> Result<Self> {
        use std::io::{Read, Seek, SeekFrom};
        for entry in std::fs::read_dir(root)? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("rel"))
            {
                let hash = crate::sha256_file(&entry.path())?;
                let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
                if hex == SOURCE_SHA256 {
                    // Read only the registration code window and string table window
                    // instead of allocating the entire 13.6 MB REL into RAM.
                    let mut file = std::fs::File::open(entry.path())?;
                    let code_base: usize = 0x2ebb98;
                    let code_len: usize = 0x2ee290 - code_base;
                    let mut code = vec![0u8; code_len];
                    file.seek(SeekFrom::Start(code_base as u64))?;
                    file.read_exact(&mut code)?;

                    let str_base: usize = (0x4c6bf4_i32 - 32768) as usize;
                    let str_len: usize = 65536 + 512;
                    let mut strings = vec![0u8; str_len];
                    file.seek(SeekFrom::Start(str_base as u64))?;
                    file.read_exact(&mut strings)?;

                    return Self::from_verified_windows(code_base, &code, str_base, &strings);
                }
            }
        }
        anyhow::bail!("Native animation event catalog requires a verified EM1 executable module; this game build is not mapped yet")
    }
    fn from_verified_rel(bytes: &[u8]) -> Result<Self> {
        ensure!(
            crate::mod_project::digest(bytes) == SOURCE_SHA256,
            "Unverified animation event registration code"
        );
        Self::from_verified_windows(0, bytes, 0, bytes)
    }
    fn from_verified_windows(
        code_base: usize,
        code: &[u8],
        str_base: usize,
        strings: &[u8],
    ) -> Result<Self> {
        let word = |at: usize| -> Result<u32> {
            let rel = at
                .checked_sub(code_base)
                .context("Animation registration address precedes code window")?;
            Ok(u32::from_be_bytes(
                code.get(rel..rel + 4)
                    .context("Truncated animation registration code")?
                    .try_into()
                    .unwrap(),
            ))
        };
        let table = |first: usize, last: usize, prefix: &str| -> Result<Vec<String>> {
            let mut names = Vec::new();
            for call in (first..=last).step_by(16) {
                // Each registration has mr r3,r30; addi r4,r31,string offset;
                // li r5,ordinal; bl register. The first swaps addi and li.
                let (a, b) = (word(call - 8)?, word(call - 4)?);
                let (name, id) = if a & 0xffff0000 == 0x389f0000 {
                    (a, b)
                } else {
                    (b, a)
                };
                ensure!(
                    name & 0xffff0000 == 0x389f0000 && id & 0xffff0000 == 0x38a00000,
                    "Animation registration instruction shape differs at {call:x}"
                );
                ensure!(
                    usize::from(id as u16) == names.len(),
                    "Animation registration ordinals differ"
                );
                let branch = word(call)?;
                ensure!(
                    branch & 0xfc000003 == 0x48000001,
                    "Animation registration call is not a branch"
                );
                let displacement = ((branch & 0x03ff_fffc) as i32) << 6 >> 6;
                ensure!(
                    call as i32 + displacement == 0x2eb91c,
                    "Unexpected animation registration target"
                );
                let at = (0x4c6bf4_i32 + i32::from(name as i16)) as usize;
                let str_rel = at
                    .checked_sub(str_base)
                    .context("Animation name pointer precedes string window")?;
                let tail = strings
                    .get(str_rel..)
                    .context("Animation name pointer exceeds module")?;
                let end = tail
                    .iter()
                    .position(|byte| *byte == 0)
                    .context("Unterminated animation name")?;
                let name = std::str::from_utf8(&tail[..end])?;
                ensure!(name.starts_with(prefix), "Invalid animation symbol name");
                names.push(name.to_owned());
            }
            Ok(names)
        };
        let mut catalog = Self::from_names(table(0x2ec098, 0x2ee278, "EVENT_")?)?;
        catalog.variables = table(0x2ebba8, 0x2ec028, "VAR_")?;
        Ok(catalog)
    }
    fn from_names(names: Vec<String>) -> Result<Self> {
        let mut indices = BTreeMap::new();
        for (id, name) in names.iter().enumerate() {
            ensure!(
                indices.insert(name.clone(), u32::try_from(id)?).is_none(),
                "Duplicate native animation event {name}"
            );
        }
        Ok(Self {
            names,
            indices,
            variables: Vec::new(),
        })
    }
    pub fn len(&self) -> usize {
        self.names.len()
    }
    pub fn entries(&self) -> impl Iterator<Item = (u32, &str)> {
        self.names
            .iter()
            .enumerate()
            .map(|(id, name)| (id as u32, name.as_str()))
    }
    pub fn name(&self, id: u32) -> Result<&str> {
        self.names
            .get(id as usize)
            .map(String::as_str)
            .context("Unknown native animation event ID")
    }
    pub fn id(&self, name: &str) -> Option<u32> {
        self.indices.get(name).copied()
    }
    pub fn variable_entries(&self) -> impl Iterator<Item = (u32, &str)> {
        self.variables
            .iter()
            .enumerate()
            .map(|(id, name)| (id as u32, name.as_str()))
    }
    pub fn variable_name(&self, id: i32) -> Option<&str> {
        usize::try_from(id)
            .ok()
            .and_then(|id| self.variables.get(id))
            .map(String::as_str)
    }
    /// DOL801ed624 event reverse mapping filters names absent from this graph.
    /// A native event may legitimately be unhandled; an unknown native ID is
    /// different and remains an error. Resolve this map once per actor graph.
    pub fn graph_map(&self, names: &[String]) -> Result<BTreeMap<u32, i32>> {
        let mut map = BTreeMap::new();
        for (local, name) in names.iter().enumerate() {
            if let Some(global) = self.id(name) {
                ensure!(
                    map.insert(global, i32::try_from(local)?).is_none(),
                    "Ambiguous graph event {name}"
                );
            }
        }
        Ok(map)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn graph_events_use_names_not_native_ordinals_and_filter_missing_events() -> Result<()> {
        let catalog = Catalog::from_names(vec![
            "EVENT_Walk".into(),
            "EVENT_Force_Idle".into(),
            "EVENT_Die".into(),
        ])?;
        let map = catalog.graph_map(&[
            "EVENT_Die".into(),
            "EVENT_Custom".into(),
            "EVENT_Force_Idle".into(),
        ])?;
        assert_eq!(map, BTreeMap::from([(1, 2), (2, 0)]));
        assert_eq!(map.get(&0), None);
        assert!(catalog.name(3).is_err());
        assert!(catalog
            .graph_map(&["EVENT_Die".into(), "EVENT_Die".into()])
            .is_err());
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_module_registers_all_native_animation_names_in_order() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let catalog = Catalog::load(Path::new(&root))?;
        assert_eq!(catalog.len(), 543);
        assert_eq!(catalog.name(0xc2)?, "EVENT_Force_Idle");
        assert_eq!(catalog.name(0)?, "EVENT_Start_Walking_On_Ground");
        assert_eq!(catalog.name(0x21e)?, "EVENT_PaintThinner");
        assert_eq!(catalog.variable_entries().count(), 73);
        assert_eq!(catalog.variable_name(17), Some("VAR_Mood_Type"));
        assert_eq!(catalog.variable_name(24), Some("VAR_Cutscene"));
        assert_eq!(catalog.variable_name(72), Some("VAR_In_Conversation"));
        assert_eq!(catalog.variable_name(-1), None);
        assert_eq!(catalog.variable_name(73), None);
        Ok(())
    }
}
