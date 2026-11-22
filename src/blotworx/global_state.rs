//! Numeric Apprentice global slots used by REL2091a4/2092a8.
//! Save restoration and Apprentice/quest notification dispatch remain open.
use crate::em1_bsq::{Document, Node};
use anyhow::{ensure, Context, Result};
use std::collections::BTreeMap;
#[derive(Clone, Debug)]
pub struct State {
    names: BTreeMap<String, u16>,
    values: Vec<i16>,
    authored: Vec<(i16, bool)>,
    persistent: Vec<u8>,
}
///208fb0 registers c0734/c06d4: locale-folded hash and comparison through
/// DOL80024a50. Initial C locale folds ASCII only; signed high bytes bypass it.
fn name_key(name: &str) -> String {
    name.split('\0').next().unwrap_or("").to_ascii_lowercase()
}
impl State {
    pub fn load(path: &str, bytes: &[u8]) -> Result<Self> {
        let document = Document::load(path, bytes)?;
        ensure!(
            document.database,
            "Global state asset is not an Apprentice database"
        );
        let limit = document
            .root
            .value("_id_limit")
            .and_then(|v| v.as_u64())
            .context("Global state ID limit missing")?;
        let count = usize::try_from(limit)?
            .checked_add(1)
            .context("Global state slot count overflow")?;
        let Node::Record { fields, .. } = &document.root else {
            anyhow::bail!("Missing global database record")
        };
        let items = fields
            .iter()
            .find_map(|node| match node {
                Node::List { name, items, .. } if name == "States" => Some(items),
                _ => None,
            })
            .context("Global state records missing")?;
        let mut state = Self {
            names: BTreeMap::new(),
            values: vec![0; count],
            authored: vec![(0, false); count],
            persistent: vec![0; count],
        };
        for item in items {
            let name = item
                .value("Name")
                .and_then(|v| v.as_str())
                .context("Global state name missing")?;
            let id = u16::try_from(
                item.value("ID")
                    .and_then(|v| v.as_u64())
                    .context("Global state ID missing")?,
            )?;
            let initial = i16::try_from(
                item.value("Initial value")
                    .and_then(|v| v.as_i64())
                    .context("Global initial value missing")?,
            )?;
            let flag = item
                .value("Flag")
                .and_then(|v| v.as_bool())
                .context("Global authored flag missing")?;
            //2097e8 obtains an existing map value on duplicate identity and
            // overwrites its ID. All indexed default/live slots still exist.
            state.names.insert(name_key(name), id);
            *state
                .values
                .get_mut(id as usize)
                .context("Global state ID exceeds authored limit")? = initial;
            state.authored[id as usize] = (initial, flag);
            state.persistent[id as usize] = u8::from(flag);
        }
        Ok(state)
    }
    pub fn get(&self, name: &str) -> Option<i16> {
        self.names
            .get(&name_key(name))
            .map(|&id| self.values[id as usize])
    }
    /// The native slot receives extsh; SetGlobal returns the original integer.
    pub fn set(&mut self, name: &str, value: i32) -> bool {
        let Some(&id) = self.names.get(&name_key(name)) else {
            return false;
        };
        self.values[id as usize] = value as i16;
        true
    }
    pub fn names_len(&self) -> usize {
        self.names.len()
    }
    pub fn initial(&self, name: &str) -> Option<i16> {
        self.names
            .get(&name_key(name))
            .map(|&id| self.authored[id as usize].0)
    }
    /// Return every global slot whose live value differs from its authored initial value.
    pub fn diff_from_authored(&self) -> Vec<(u16, i16)> {
        self.values
            .iter()
            .zip(&self.authored)
            .enumerate()
            .filter_map(|(idx, (&val, &(init, _)))| {
                (val != init && idx <= u16::MAX as usize).then_some((idx as u16, val))
            })
            .collect()
    }
    /// Apply authoritative host global diffs onto this client's global state.
    pub fn apply_network_sync(&mut self, diffs: &[(u16, i16)]) {
        for (val, &(init, _)) in self.values.iter_mut().zip(&self.authored) {
            *val = init;
        }
        for &(id, val) in diffs {
            if let Some(slot) = self.values.get_mut(id as usize) {
                *slot = val;
            }
        }
    }
    /// GLOB v1, REL209850..2099ec: signed halfword, persistence byte,
    /// unused padding byte. Named records have native four-byte alignment.
    pub fn save_payload(&self) -> Result<Vec<u8>> {
        let count = u16::try_from(self.values.len()).context("Global count exceeds native u16")?;
        let mut output = Vec::new();
        super::save_game::append_record(
            &mut output,
            "GDMArraySize",
            &u32::from(count).to_be_bytes(),
            4,
        )?;
        let mut slots = Vec::with_capacity(self.values.len() * 4);
        for (&value, &persistent) in self.values.iter().zip(&self.persistent) {
            slots.extend(value.to_be_bytes());
            slots.push(persistent);
            slots.push(0);
        }
        super::save_game::append_record(&mut output, "GDMArray", &slots, 4)?;
        Ok(output)
    }
    /// Native209980 resets nonpersistent saved values to authored defaults.
    /// Cross-build slot-count migration requires separate compatibility work;
    /// reject it before changing live state rather than misassigning globals.
    pub fn restore_save_payload(&mut self, payload: &[u8]) -> Result<()> {
        let records = super::save_game::records(payload)?;
        let find = |name: &str| -> Result<&[u8]> {
            let mut matching = records.iter().filter(|record| record.name == name);
            let record = matching.next().context("Global save record missing")?;
            ensure!(matching.next().is_none(), "Duplicate global save record");
            Ok(record.data)
        };
        let count = find("GDMARRAYSIZE")?;
        ensure!(count.len() == 4, "Invalid global save count width");
        let count = u32::from_be_bytes(count.try_into()?) as usize;
        ensure!(count==self.values.len(),"Global save slot count {count} differs from current data {}; cross-build migration is not implemented",self.values.len());
        let slots = find("GDMARRAY")?;
        ensure!(
            slots.len() == count * 4,
            "Invalid global save slot byte count"
        );
        for (index, slot) in slots.chunks_exact(4).enumerate() {
            self.persistent[index] = slot[2];
            self.values[index] = if slot[2] == 0 {
                self.authored[index].0
            } else {
                i16::from_be_bytes([slot[0], slot[1]])
            };
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires original files and native save in BLOTWORX_TEST_DATA/BLOTWORX_TEST_SAVE"]
    fn native_header_global_metadata_matches_raw_saved_slots_and_inventory_limits() -> Result<()> {
        use super::super::{
            checkpoint_metadata::{Globals, InventoryLimits, GLOBAL_NAMES},
            save_game,
        };
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let path = std::env::var_os("BLOTWORX_TEST_SAVE").context("Set BLOTWORX_TEST_SAVE")?;
        let world = super::super::assets::load(
            std::path::Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        let original = std::fs::read(&path)?;
        let archive = save_game::Archive::parse(&original)?;
        let global_records = save_game::records(&archive.section(*b"GLOB")?.payload)?;
        let slots = global_records
            .iter()
            .find(|record| record.name == "GDMARRAY")
            .context("Native global array missing")?
            .data;
        // Inspect raw saved values; runtime restore legitimately resets transient
        // slots and is not equivalent to the checkpoint's display metadata.
        let expected = Globals::capture(|name| {
            world.globals.names.get(&name_key(name)).and_then(|&id| {
                slots
                    .get(usize::from(id) * 4..usize::from(id) * 4 + 2)
                    .map(|word| i16::from_be_bytes(word.try_into().unwrap()))
            })
        })?;
        assert_eq!(Globals::from_header(&original)?, expected);
        assert_eq!(GLOBAL_NAMES.len(), 25);
        let player = &archive.section(*b"MICK")?.payload;
        let inventory = super::super::save_player::Inventory::decode(player)?;
        let limits = InventoryLimits::from_header(&original)?;
        assert_eq!(limits.maximum_tickets, inventory.maximum_tickets);
        let records = save_game::records(player)?;
        for (saved, index) in limits.sketch_maxima_bits.into_iter().zip([2, 0, 1]) {
            let suffix = format!("_{index}");
            let matches = records
                .iter()
                .filter(|record| {
                    record.name.starts_with("SKETCH_") && record.name.ends_with(&suffix)
                })
                .collect::<Vec<_>>();
            ensure!(
                matches.len() == 1,
                "Native sketch index {index} is ambiguous"
            );
            assert_eq!(saved, u32::from_be_bytes(matches[0].data[..4].try_into()?));
        }
        let mut rewritten = original.clone();
        expected.write_header(&mut rewritten)?;
        limits.write_header(&mut rewritten)?;
        assert_eq!(rewritten, original);
        assert_eq!(std::fs::read(path)?, original);
        Ok(())
    }
    #[test]
    fn global_save_restores_persistent_slots_and_resets_transient_values() -> Result<()> {
        let mut state = State {
            names: BTreeMap::from([("kept".into(), 0), ("reset".into(), 1)]),
            values: vec![-123, 456],
            authored: vec![(7, true), (8, false)],
            persistent: vec![1, 0],
        };
        let saved = state.save_payload()?;
        let records = super::super::save_game::records(&saved)?;
        assert_eq!(records[1].data, &[0xff, 0x85, 1, 0, 1, 0xc8, 0, 0]);
        state.set("kept", 99);
        state.set("reset", 100);
        state.restore_save_payload(&saved)?;
        assert_eq!(state.get("kept"), Some(-123));
        assert_eq!(state.get("reset"), Some(8));
        let previous = state.values.clone();
        assert!(state
            .restore_save_payload(&saved[..saved.len() - 1])
            .is_err());
        assert_eq!(state.values, previous);
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_global_defaults_lua_signed_updates_and_checkpoint_roundtrip() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = super::super::assets::load(
            std::path::Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        assert_eq!(world.globals.names_len(), 5731);
        assert_eq!(world.globals.initial("PlayerFirstTimeInZone"), Some(1));
        // The real OnMapLoad_ProjectorSetup consumes this flag during import.
        assert_eq!(world.globals.get("PlayerFirstTimeInZone"), Some(0));
        assert_eq!(world.globals.get("2dDirection"), Some(1));
        let authored = world.globals.authored.clone();
        // Isolate the binding from unrelated entrance-task retire/swap order.
        // Coroutine scheduling has its own native-order regression tests.
        world.run_callback_fixture("assert(GetGlobal('BwxMissing')==nil); assert(SetGlobal('BwxMissing',1)==nil); assert(SetGlobal('GLOBAL_QUICKSTARTLOCATION',65535,true)==65535); assert(GetGlobal('global_quickstartlocation')==-1); assert(SetGlobal('Global_QuickstartLocation','2.9',true)==2); assert(GetGlobal('GLOBAL_quickstartLOCATION\\0ignored')==2)")?;
        assert_eq!(world.globals.get("Global_QuickstartLocation"), Some(2));
        world.run_callback_fixture("assert(SetGlobal('Global_QuickstartLocation',0,true)==0)")?;
        assert_eq!(world.globals.get("Global_QuickstartLocation"), Some(0));
        assert_eq!(world.globals.authored, authored);
        let state = &mut world.globals;
        let expected = state
            .values
            .iter()
            .enumerate()
            .map(|(id, &value)| {
                if state.persistent[id] == 0 {
                    state.authored[id].0
                } else {
                    value
                }
            })
            .collect::<Vec<_>>();
        let saved = state.save_payload()?;
        state.values.fill(-12345);
        state.restore_save_payload(&saved)?;
        assert_eq!(
            state.values, expected,
            "Original global flags must control checkpoint restoration"
        );
        assert!(
            !world
                .report
                .unsupported
                .iter()
                .any(|message| message.contains("runtime animation console")),
            "{:?}",
            world.report.unsupported
        );
        Ok(())
    }
}
