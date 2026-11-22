//! Live memory checkpoint coordination, REL2bb3c/25fd10/2fa24.
//! Disk/frontend jobs and gameplay restoration are separate, unfinished stages.
use super::save_game::{self, Archive, MemoryWriter};
use anyhow::{ensure, Context, Result};
use std::sync::Arc;

pub const OWNER_ORDER: [([u8; 4], u32); 9] = [
    (*b"GLOB", 1),
    (*b"QUES", 4),
    (*b"EXTR", 1),
    (*b"COLL", 2),
    (*b"AISS", 2),
    (*b"SCPT", 1),
    (*b"MICK", 3),
    (*b"OPTN", 1),
    (*b"WRLD", 1),
];

#[derive(Debug)]
pub struct Snapshot {
    pub bytes: Vec<u8>,
    pub logical_size: usize,
}
impl Snapshot {
    pub(super) fn has_active_campaign(&self) -> bool {
        self.bytes.get(0x2c4) == Some(&0)
    }

    pub(super) fn completed_playthrough(&self) -> Result<bool> {
        let archive = Archive::parse(&self.bytes)?;
        if self.has_active_campaign() {
            return Ok(false);
        }
        ensure!(
            super::score_state::Playthrough::from_header(&self.bytes)?.value > 0
                && archive.sections.len() == 1
                && archive.sections[0].tag == save_game::LAST,
            "Invalid completed campaign slot"
        );
        Ok(true)
    }

    pub(super) fn level(&self) -> Result<String> {
        let archive = Archive::parse(&self.bytes)?;
        ensure!(self.has_active_campaign(), "This slot completed the campaign and has no active checkpoint");
        let level = archive.header_string(0x10, 60)?;
        ensure!(!level.is_empty(), "Checkpoint has no saved level");
        Ok(level.to_owned())
    }
}
/// REL2fcc8 selects the synchronous memory route or the save-slot read
/// callback. The host consumes the request outside the Lua/IO update.
#[derive(Clone, Copy, Debug)]
pub(super) enum Restore {
    Memory,
    Disk,
}

/// Retain one memory checkpoint across level replacement. Clone shares the
/// immutable snapshot; constructing the next checkpoint gets its own buffer.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub service: Option<super::save_service::Service>,
    pub latest: Option<Arc<Snapshot>>,
    pub pending_nine: Option<[u8; 9]>,
    pub pending_sixty_four: Option<[u8; 64]>,
    pub persistent_requested: bool,
    pub(super) ending_game: bool,
    pub(super) restore_requested: Option<Restore>,
    pub notification: Option<Notification>,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct Notification {
    pub event: &'static str,
    pub argument: &'static str,
    pub duration: i32,
}
impl State {
    pub(super) fn take_restore(&mut self) -> Result<Option<Arc<Snapshot>>> {
        match self.restore_requested.take() {
            Some(Restore::Memory) => Ok(self.latest.clone()),
            Some(Restore::Disk) => match &self.service {
                Some(service) => service.read_selected_checkpoint(),
                None => Ok(None),
            },
            None => Ok(None),
        }
    }

    fn storage(&self) -> Result<Vec<u8>> {
        if let Some(snapshot) = &self.latest {
            return Ok(snapshot.bytes.clone());
        }
        // Native memory allocation is64KiB. Only verified constructor ranges
        // have defined values; zero-initialized host allocation is not claimed
        // to reproduce the original allocator's indeterminate padding.
        let mut bytes = vec![0; 0x10000];
        save_game::initialize_header(&mut bytes)?;
        Ok(bytes)
    }
    pub fn summary(&self) -> serde_json::Value {
        let snapshot=self.latest.as_ref().map(|snapshot| {
            let digest=crate::sha256(&snapshot.bytes[..snapshot.logical_size]);
            let hash=digest.iter().map(|byte|format!("{byte:02x}")).collect::<String>();
            let archive=Archive::parse(&snapshot.bytes);
            match archive {
                Ok(archive)=>serde_json::json!({"logical_bytes":snapshot.logical_size,"storage_bytes":snapshot.bytes.len(),
                    "logical_sha256":hash,"callback":archive.header_string(0xc4,60).ok(),
                    "sections":archive.sections.iter().map(|section|serde_json::json!({
                        "tag":String::from_utf8_lossy(&section.tag),"version":section.version,"bytes":section.payload.len()})).collect::<Vec<_>>(),
                    "timing":super::checkpoint_time::Timing::from_header(&snapshot.bytes).ok().map(|timing|timing.summary()),
                    "spawn":archive.section(*b"MICK").ok().and_then(|section|super::player_spawn::saved_transform(&section.payload).ok().flatten())}),
                Err(error)=>serde_json::json!({"error":format!("{error:#}")}),
            }
        });
        serde_json::json!({"snapshot":snapshot,"memory_stream_complete":self.latest.is_some(),
            "pending_nine":self.pending_nine.is_some(),"pending_sixty_four":self.pending_sixty_four.is_some(),
            "persistent_requested":self.persistent_requested,"notification":self.notification,
            "notification_delivery_implemented":false,"native_initialization_complete":false,
            "persistent_jobs_implemented":self.service.is_some(),"persistent_service":self.service.as_ref().map(|service|service.summary()),
            "gameplay_restore_implemented":false})
    }
}

///2fa24 pops from the tail, with native defaults. It returns no Lua values,
/// including when a requested spawn marker is absent. Keep callback bytes
/// intact: the native header writer truncates bytes, not Unicode characters.
pub struct Request {
    pub callback: Vec<u8>,
    pub resolve_spawn: bool,
    pub spawn: Option<String>,
    pub persistent: bool,
}
impl Request {
    pub fn from_lua(lua: &mlua::Lua, mut args: mlua::MultiValue) -> mlua::Result<Self> {
        let count = args.len();
        if !(1..=5).contains(&count) {
            return Err(mlua::Error::external("SaveGameToMemory needs one to five arguments; native count diagnostic continuation is not implemented"));
        }
        if count > 4 {
            let _ = super::score_state::integer_argument(lua, args.pop_back().unwrap())?;
        }
        let persistent = if count > 3 {
            super::actor_scripts::native_boolean(lua, args.pop_back().unwrap())?
        } else {
            true
        };
        let spawn = if count > 2 {
            let value = lua
                .coerce_string(args.pop_back().unwrap())?
                .ok_or_else(|| {
                    mlua::Error::external(
                        "SaveGameToMemory spawn-name conversion is not implemented for this value",
                    )
                })?;
            Some(value.to_str()?.split('\0').next().unwrap_or("").to_owned())
        } else {
            None
        };
        let resolve_spawn = if count > 1 {
            super::actor_scripts::native_boolean(lua, args.pop_back().unwrap())?
        } else {
            true
        };
        let callback = lua
            .coerce_string(args.pop_back().unwrap())?
            .ok_or_else(|| {
                mlua::Error::external(
                    "SaveGameToMemory callback-name conversion is not implemented for this value",
                )
            })?
            .as_bytes()
            .to_vec();
        Ok(Self {
            callback,
            resolve_spawn,
            spawn,
            persistent,
        })
    }
}

impl super::assets::World {
    /// The native main-menu branch at REL2b030..2b0a4 restores the
    /// completed slot's header owners before it starts a fresh campaign.
    pub(super) fn seed_completed_playthrough(&mut self, bytes: &[u8]) -> Result<()> {
        self.playthrough = super::score_state::Playthrough::from_header(bytes)?;
        self.extras.restore_save_header(bytes)?;
        self.collectibles.restore_save_header(bytes, false)?;
        self.game_options = super::save_options::State::from_header(bytes)?;
        for (name, &byte) in super::endgame::PIN_GLOBALS.into_iter().zip(&bytes[0x2ce..]) {
            self.globals.set(name, i32::from((byte as i8).min(1)));
        }
        let carried = super::checkpoint_metadata::Globals::from_header(bytes)?;
        for (name, byte) in super::checkpoint_metadata::GLOBAL_NAMES.into_iter().zip(carried.0) {
            self.globals.set(name, i32::from(byte as i8));
        }
        self.playtime.set(0, &self.frame_clock.clock);
        Ok(())
    }

    /// Seed the persisted owners before the level's script modules execute.
    pub(super) fn restore_checkpoint_owners(&mut self, archive: &Archive<'_>, bytes: &[u8]) -> Result<()> {
        let section = |tag, version| -> Result<&[u8]> {
            let section = archive.section(tag)?;
            ensure!(section.version == version, "Unsupported checkpoint section {:?} version {}", tag, section.version);
            Ok(&section.payload)
        };
        self.globals.restore_save_payload(section(*b"GLOB", 1)?)?;
        self.quests.restore_payload(section(*b"QUES", 4)?)?;
        self.instance_changes.restore_payload(section(*b"AISS", 2)?)?;
        self.extras.restore_save_header(bytes)?;
        self.collectibles.restore_save_header(bytes, true)?;
        self.game_options = super::save_options::State::from_header(bytes)?;
        self.playthrough = super::score_state::Playthrough::from_header(bytes)?;
        self.playtime.restore_header(bytes, &self.frame_clock.clock)?;

        let player = section(*b"MICK", 3)?;
        self.playstyle_points.restore_record(player)?;
        self.player_spawn.restore(player)?;
        for name in super::ability_registry::PRIMARY.into_iter().chain(super::ability_registry::SECONDARY) {
            self.restore_player_ability_record(name, player)?;
        }
        let health = super::save_player::Health::decode(player)?;
        let items = super::save_player::Inventory::decode(player)?;
        let ammo = super::save_player::Ammo::decode(player)?;
        let amounts = ammo.amount_bits.map(f32::from_bits);
        let maxima = ammo.maximum_bits.map(f32::from_bits);
        let ceilings = ammo.recharge_ceiling_bits.map(f32::from_bits);
        let segments = ammo.segment_amount_bits.map(f32::from_bits);
        ensure!(amounts.into_iter().chain(maxima).chain(ceilings).chain(segments).all(f32::is_finite),
            "Nonfinite checkpoint ammunition");
        self.ammo.inventory = Some(super::ammo::Inventory { amounts, maxima: maxima.map(Some) });
        for (setting, value) in self.ammo.settings.channels.iter_mut().zip(ceilings) {
            setting.recharge_max = value;
        }

        let components = &mut self.entities[self.player_entity].components;
        let health_component = components.iter_mut().find(|c| c.class_name == "HealthComponent")
            .context("Checkpoint player has no health component")?;
        for (name, value) in [("Health", health.current), ("MaxHealth", health.maximum)] {
            health_component.properties.iter_mut().find(|p| p.name == name)
                .with_context(|| format!("Checkpoint health property {name} is missing"))?
                .value = serde_json::json!(value);
        }
        let inventory = components.iter_mut().find(|c| c.class_name == "InventoryComponent")
            .context("Checkpoint player has no inventory component")?;
        let mut state = super::inventory_state::Authored::from_component(inventory)?;
        self.player_ability_registry.restore_sketches(player, &mut state.maxima)?;
        for (name, count) in super::save_player::INVENTORY_NAMES[2..].iter().zip(items.sketch_counts) {
            if let Some(index) = state.sketches.iter()
                .position(|sketch| sketch.split('\0').next() == Some(*name))
            {
                // Some scene palettes supply names and capacities with an
                // empty count array. Materialize each restored named slot.
                if state.counts.len() <= index {
                    state.counts.resize(index + 1, 0);
                }
                let maximum = state.maxima.get(index).copied().unwrap_or(0.);
                // REL344868 clamps the f32 request to the indexed maximum
                // before converting it with the Wii integer conversion.
                state.counts[index] = super::inventory_commands::checked_fctiwz(
                    (count as f32).min(maximum),
                )?;
            }
        }
        for property in &mut inventory.properties {
            property.value = match property.name.as_str() {
                "ETicket" => serde_json::json!(items.tickets as f32),
                "ETicketMax" => serde_json::json!(items.maximum_tickets as f32),
                "Paint" => serde_json::json!(amounts[0]),
                "Thinner" => serde_json::json!(amounts[1]),
                "PaintAmmoSegmentAmount" => serde_json::json!(segments[0]),
                "ThinnerAmmoSegmentAmount" => serde_json::json!(segments[1]),
                "SketchCount" => serde_json::json!(state.counts),
                "SketchMax" => serde_json::json!(state.maxima),
                _ => continue,
            };
        }
        Ok(())
    }

    pub(super) fn schedule_checkpoint_callback(&mut self, archive: &Archive<'_>) -> Result<()> {
        if let Some(spawn) = self.player_spawn.override_transform {
            self.relocate_player_pose(spawn.position, spawn.rotation, true)?;
            self.script_player.spawn = spawn.position;
        }
        let callback = archive.header_string(0xc4, 60)?;
        if !callback.is_empty() {
            self.scripts.schedule_checkpoint_callback(callback, self.player_entity)?;
        }
        self.report.geometry.push(serde_json::json!({
            "kind":"disk_checkpoint_restore",
            "callback":callback, "spawn":self.player_spawn.override_transform,
            "quests":self.quests.summary(), "restoration_complete":false
        }));
        Ok(())
    }

    /// Capture current live owners, not records copied from an original save.
    /// A complete stream does not establish complete native owner initialization
    /// or a usable restoration path. Never write to the original game/save tree.
    pub fn capture_memory_checkpoint(
        &mut self,
        callback: &[u8],
        spawn: Option<usize>,
    ) -> Result<()> {
        if let Some(entity) = spawn {
            let source = self
                .entities
                .get(entity)
                .context("Checkpoint spawn entity is not owned")?;
            ensure!(
                !self.removed_entities.contains(&entity),
                "Checkpoint spawn entity was removed"
            );
            ensure!(
                source
                    .components
                    .iter()
                    .any(|component| component.class_name == "JPSTransformationComponent"),
                "Checkpoint spawn source has no resolved transform"
            );
            self.player_spawn.set(
                source.runtime_rotation,
                source.transform[12..15].try_into()?,
            );
        }
        let mut bytes = self.checkpoint.storage()?;
        //2bcb8..2bd38 consumes each retained override once, leaving the other
        // bytes of the prior header intact on subsequent checkpoint captures.
        if let Some(value) = self.checkpoint.pending_nine.take() {
            bytes[0x2c5..0x2ce].copy_from_slice(&value);
        }
        if let Some(value) = self.checkpoint.pending_sixty_four.take() {
            bytes[0x2ce..0x30e].copy_from_slice(&value);
        }
        self.write_save_header_owners(&mut bytes)?;
        self.level_resource
            .write_checkpoint_names(&mut bytes, callback)?;
        bytes[0x2c4] = 0;
        self.capture_owned_checkpoint_timing(&mut bytes)?;
        let mut writer = MemoryWriter::from_storage(bytes)?;
        writer.append_section(*b"GLOB", 1, &self.globals.save_payload()?)?;
        writer.append_section(*b"QUES", 4, &self.quests.save_payload()?)?;
        writer.append_section(*b"EXTR", 1, &[])?;
        writer.append_section(*b"COLL", 2, &[])?;
        writer.append_section(*b"AISS", 2, &self.instance_changes.save_payload()?)?;
        let (tag, version, payload) = super::save_options::SCRIPT_SECTION;
        writer.append_section(tag, version, payload)?;
        let player = self.capture_player_records()?;
        writer.append_section(*b"MICK", 3, &player.payload)?;
        writer.append_section(*b"OPTN", 1, &[])?;
        writer.append_section(
            *b"WRLD",
            1,
            &self.scene_resources.save_payload(&self.scene_requests)?,
        )?;
        let built = writer.finish()?;
        let snapshot = Arc::new(Snapshot {
            bytes: built.bytes,
            logical_size: built.logical_size,
        });
        if let Some(service) = &self.checkpoint.service {
            service.record_memory_checkpoint(&snapshot);
        }
        self.checkpoint.latest = Some(snapshot);
        self.checkpoint.persistent_requested = false;
        Ok(())
    }
    pub(super) fn store_endgame_outcomes(&mut self) {
        // REL345b4 snapshots the signed global values in this save-header order.
        let names = [
            "Daisy", "Donald", "Goofy", "Petetronic", "Hook",
            "Clocktower", "SmallPete", "Horace", "Pete",
        ];
        self.checkpoint.pending_nine = Some(names.map(|name| {
            u8::from(self.globals.get(&format!("Endgame_Var_Pos_{name}")).unwrap_or(0) != 0)
        }));
    }

    pub(super) fn record_endgame_pins(&mut self) {
        // REL353ac copies the low bytes of exactly 57 pin globals. The
        // remaining seven bytes of the pending header block are reserved.
        let mut pins = self.checkpoint.pending_sixty_four.unwrap_or([0; 64]);
        for (byte, name) in pins.iter_mut().zip(super::endgame::PIN_GLOBALS) {
            *byte = self.globals.get(name).unwrap_or(0) as u8;
        }
        self.checkpoint.pending_sixty_four = Some(pins);
    }

    pub(super) fn complete_save_slot(&mut self) -> Result<()> {
        let Some(source) = &self.checkpoint.latest else { return Ok(()); };
        let built = super::save_game::complete_playthrough(&source.bytes)?;
        let snapshot = Arc::new(Snapshot { bytes: built.bytes, logical_size: built.logical_size });
        let service = self.checkpoint.service.as_ref().context("Campaign completion needs a save slot")?;
        service.complete_playthrough(&snapshot)?;
        self.checkpoint.latest = Some(snapshot);
        self.checkpoint.persistent_requested = true;
        Ok(())
    }

    pub(super) fn save_game_to_memory(
        &mut self,
        lua: &mlua::Lua,
        args: mlua::MultiValue,
    ) -> mlua::Result<mlua::MultiValue> {
        let request = Request::from_lua(lua, args)?;
        //2bb3c declines frontend states4/5/6 before touching the marker,
        // live owners or the memory buffer; Lua returns zero values on decline.
        if let Some(service) = &self.checkpoint.service {
            service.poll().map_err(mlua::Error::external)?;
            if service.blocks_memory_capture() {
                return Ok(mlua::MultiValue::new());
            }
        }
        let spawn = if request.resolve_spawn {
            let entity = request
                .spawn
                .as_deref()
                .filter(|name| !name.is_empty())
                .and_then(|name| self.scripts.resolve_name(name))
                .filter(|entity| !self.removed_entities.contains(entity));
            let Some(entity) = entity else {
                return Ok(mlua::MultiValue::new());
            };
            Some(entity)
        } else {
            None
        };
        self.capture_memory_checkpoint(&request.callback, spawn)
            .map_err(mlua::Error::external)?;
        if request.persistent {
            self.checkpoint.persistent_requested = true;
            let snapshot = self.checkpoint.latest.as_ref().unwrap().clone();
            let sample = self.frame_clock.hardware_sample();
            let frequency = self.frame_clock.clock.rate.ticks_per_second.get();
            let service=self.checkpoint.service.as_mut().ok_or_else(||mlua::Error::external(
                "SaveGameToMemory captured all live section owners; the requested persistent save-slot/frontend job has no initialized storage backend"))?;
            service
                .request(&snapshot, sample, frequency)
                .map_err(mlua::Error::external)?;
        }
        //1f86ac calls20b60c with ShowCheckpoint, empty argument, and the
        // configured CheckpointDisplayTime (native default1). Keep a typed
        // request; original UI dispatch/movie rendering is still unfinished.
        //20b680 converts the signed integer in r6 to a Scaleform number.
        let duration = self
            .runtime_settings
            .get("CheckpointDisplayTime")
            .map(|value| value.parse::<i32>())
            .transpose()
            .map_err(|_| mlua::Error::external("Invalid integer CheckpointDisplayTime"))?
            .unwrap_or(1);
        self.checkpoint.notification = Some(Notification {
            event: "ShowCheckpoint",
            argument: "",
            duration,
        });
        Ok(mlua::MultiValue::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lua_checkpoint_defaults_and_tail_arguments_match_native_request() -> Result<()> {
        let lua = mlua::Lua::new();
        for (code, resolve, persistent, spawn) in [
            ("'cp'", true, true, None),
            ("'cp',false", false, true, None),
            ("'cp',true,'marker'", true, true, Some("marker")),
            ("'cp',false,'unused',false,8", false, false, Some("unused")),
        ] {
            let args = lua.load(format!("return {code}")).eval()?;
            let request = Request::from_lua(&lua, args)?;
            assert_eq!(request.callback, b"cp");
            assert_eq!(request.resolve_spawn, resolve);
            assert_eq!(request.persistent, persistent);
            assert_eq!(request.spawn.as_deref(), spawn);
        }
        assert!(Request::from_lua(&lua, mlua::MultiValue::new()).is_err());
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_world_checkpoint_assembles_live_owners_and_preserves_previous_header() -> Result<()>
    {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = super::super::assets::load(
            std::path::Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        // Missing markers follow the original early return, without a snapshot.
        world.run_callback_fixture(
            "assert(select('#',SaveGameToMemory('missing',true,'no such marker',false))==0)",
        )?;
        assert!(world.checkpoint.latest.is_none());
        let marker = world
            .scripts
            .resolve_name("gremlinvillage_zonea.CheckPointMarker 01")
            .context("Original marker missing")?;
        let position: [f32; 3] = world.entities[marker].transform[12..15].try_into()?;
        world.checkpoint.pending_nine = Some([0x31; 9]);
        world.checkpoint.pending_sixty_four = Some([0x72; 64]);
        world.run_callback_fixture("assert(select('#',SaveGameToMemory('GV_ZoneA_LoadCheckPoint',true,'gremlinvillage_zonea.CheckPointMarker 01',false))==0)")?;
        let first = world
            .checkpoint
            .latest
            .clone()
            .context("No checkpoint captured")?;
        let staged = world.checkpoint.clone();
        assert!(
            Arc::ptr_eq(staged.latest.as_ref().unwrap(), &first),
            "World replacement must share immutable checkpoint bytes"
        );
        let archive = Archive::parse(&first.bytes)?;
        let padded = |mut payload: Vec<u8>| {
            payload.resize((payload.len() + 3) & !3, 0);
            payload
        };
        assert_eq!(
            archive
                .sections
                .iter()
                .map(|section| (section.tag, section.version))
                .collect::<Vec<_>>(),
            OWNER_ORDER
                .into_iter()
                .chain([(save_game::LAST, 1)])
                .collect::<Vec<_>>()
        );
        assert_eq!(archive.logical_size, first.logical_size);
        assert_eq!(archive.header_string(0xc4, 60)?, "GV_ZoneA_LoadCheckPoint");
        assert_eq!(
            archive.section(*b"GLOB")?.payload.as_ref(),
            padded(world.globals.save_payload()?)
        );
        assert_eq!(
            archive.section(*b"QUES")?.payload.as_ref(),
            padded(world.quests.save_payload()?)
        );
        assert_eq!(
            archive.section(*b"AISS")?.payload.as_ref(),
            padded(world.instance_changes.save_payload()?)
        );
        assert_eq!(
            archive.section(*b"MICK")?.payload.as_ref(),
            padded(world.capture_player_records()?.payload)
        );
        assert_eq!(
            archive.section(*b"WRLD")?.payload.as_ref(),
            padded(world.scene_resources.save_payload(&world.scene_requests)?)
        );
        for tag in [*b"EXTR", *b"COLL", *b"SCPT", *b"OPTN"] {
            assert!(archive.section(tag)?.payload.is_empty());
        }
        let saved =
            super::super::player_spawn::saved_transform(&archive.section(*b"MICK")?.payload)?
                .context("Missing saved spawn")?;
        assert_eq!(saved.position, position);
        assert_eq!(saved.scale.to_bits(), 0x3dcccccd);
        assert_eq!(&first.bytes[0x2c5..0x2ce], &[0x31; 9]);
        assert_eq!(&first.bytes[0x2ce..0x30e], &[0x72; 64]);
        assert!(
            world.checkpoint.pending_nine.is_none()
                && world.checkpoint.pending_sixty_four.is_none()
        );
        assert_eq!(
            world.checkpoint.notification.as_ref().unwrap().event,
            "ShowCheckpoint"
        );
        // The second capture writes fresh gameplay state, retains header tails
        // and overrides, and never changes the previously published snapshot.
        world.playthrough.value = 17;
        world.globals.set("FR_TotalReels", 3);
        world.run_callback_fixture(
            "assert(select('#',SaveGameToMemory('next',false,'ignored',false))==0)",
        )?;
        let second = world.checkpoint.latest.clone().unwrap();
        let archive = Archive::parse(&second.bytes)?;
        assert_eq!(archive.header_string(0xc4, 60)?, "next");
        assert_eq!(second.bytes[0xc4 + 5], first.bytes[0xc4 + 5]);
        assert_eq!(&second.bytes[0x2c5..0x30e], &first.bytes[0x2c5..0x30e]);
        assert_eq!(
            super::super::score_state::Playthrough::from_header(&second.bytes)?.value,
            17
        );
        assert_eq!(second.bytes[0x2c4], 0);
        assert_eq!(second.bytes[0x298], 3);
        assert_ne!(
            archive.section(*b"GLOB")?.payload.as_ref(),
            Archive::parse(&first.bytes)?
                .section(*b"GLOB")?
                .payload
                .as_ref()
        );
        assert_eq!(
            Archive::parse(&first.bytes)?.header_string(0xc4, 60)?,
            "GV_ZoneA_LoadCheckPoint"
        );
        // A default persistent request is explicit, after the actual memory
        // capture. It must not masquerade as a completed disk save.
        world.run_callback_fixture("local ok,why=pcall(SaveGameToMemory,'disk',false); assert(not ok); assert(string.find(tostring(why),'persistent save%-slot/frontend job'))")?;
        assert!(world.checkpoint.persistent_requested);
        assert_eq!(
            Archive::parse(&world.checkpoint.latest.as_ref().unwrap().bytes)?
                .header_string(0xc4, 60)?,
            "disk"
        );
        let next = super::super::assets::load_with_score_state(
            std::path::Path::new(&root),
            "Levels/EB_VillageTest.level",
            &egui::Context::default(),
            None,
            None,
            world.game_random,
            world.render_options,
            super::super::score_state::InitialState {
                checkpoint: world.checkpoint.clone(),
                ..Default::default()
            },
        )?;
        assert!(
            Arc::ptr_eq(
                world.checkpoint.latest.as_ref().unwrap(),
                next.checkpoint.latest.as_ref().unwrap()
            ),
            "Replacement must retain the completed memory checkpoint before script initialization"
        );
        // A failed new capture must not publish a partial LAST-terminated save.
        let before = world.checkpoint.latest.clone().unwrap();
        let inventory = world.entities[world.player_entity]
            .components
            .iter_mut()
            .find(|component| component.class_name == "InventoryComponent")
            .unwrap();
        inventory
            .properties
            .retain(|property| property.name != "ETicket");
        assert!(world.capture_memory_checkpoint(b"invalid", None).is_err());
        assert!(Arc::ptr_eq(
            &before,
            world.checkpoint.latest.as_ref().unwrap()
        ));
        Ok(())
    }
}
