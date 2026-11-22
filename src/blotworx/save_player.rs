//! MICK3 record assembly216d8..2170c and live scalar subowners.
//! Record capture is not full native initialization, a GAME checkpoint or restore.
use super::save_game;
use anyhow::{ensure, Context, Result};

pub struct Capture {
    pub payload: Vec<u8>,
    pub playthrough: super::score_state::Playthrough,
}
impl Capture {
    pub fn summary(&self) -> Result<serde_json::Value> {
        let records = save_game::records(&self.payload)?;
        Ok(
            serde_json::json!({"version":3,"payload_bytes":self.payload.len(),"record_count":records.len(),
            "record_keys":records.iter().map(|record|record.name).collect::<Vec<_>>(),
            "spawn":super::player_spawn::saved_transform(&self.payload)?,"playthrough":self.playthrough,
            "known_record_groups_encoded":true,"runtime_initialization_complete":false,"scale_mutation_implemented":false,
            "parented_live_spawn_getters_implemented":false,"checkpoint_integrated":false,"gameplay_restore_implemented":false}),
        )
    }
}

pub const AMMO_NAMES: [&str; 8] = [
    "PaintAmmo",
    "ThinerAmmo",
    "PaintMax",
    "ThinnerMax",
    "PaintRechargeMax",
    "ThinnerRechargeMax",
    "PaintAmmoSegmentAmount",
    "ThinnerAmmoSegmentAmount",
];
pub const INVENTORY_NAMES: [&str; 5] = [
    "MaxTickets",
    "Tickets",
    "Sketch_Anvil",
    "Sketch_TV",
    "Sketch_Watch",
];

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct Inventory {
    pub maximum_tickets: i32,
    pub tickets: i32,
    pub sketch_counts: [i32; 3],
}
impl Inventory {
    ///20880..2094c uses fixed item names, not the configurable sketch order.
    ///344c2c initializes its indexed count destination to zero on lookup miss.
    pub fn capture(state: &super::inventory_state::Authored) -> Result<Self> {
        let scalar = |name: &str| -> Result<i32> {
            super::inventory_commands::checked_fctiwz(*state.floats.get(name).with_context(
                || {
                    format!(
                        "Live inventory {name} missing; native getter destination is uninitialized"
                    )
                },
            )?)
        };
        let mut sketch_counts = [0; 3];
        for (count, name) in sketch_counts.iter_mut().zip(&INVENTORY_NAMES[2..]) {
            if let Some(index) = state
                .sketches
                .iter()
                .position(|candidate| candidate.split('\0').next().unwrap_or("") == *name)
            {
                *count = state.counts.get(index).copied().unwrap_or(0);
            }
        }
        Ok(Self {
            maximum_tickets: scalar("ETicketMax")?,
            tickets: scalar("ETicket")?,
            sketch_counts,
        })
    }
    pub fn append(&self, output: &mut Vec<u8>) -> Result<()> {
        let values = [
            self.maximum_tickets,
            self.tickets,
            self.sketch_counts[0],
            self.sketch_counts[1],
            self.sketch_counts[2],
        ];
        for (name, value) in INVENTORY_NAMES.into_iter().zip(values) {
            save_game::append_record(output, name, &value.to_be_bytes(), 4)?;
        }
        Ok(())
    }
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let records = save_game::records(payload)?;
        let mut values = [0; 5];
        for (value, name) in values.iter_mut().zip(INVENTORY_NAMES) {
            *value = i32::from_be_bytes(word(&records, &name.to_ascii_uppercase())?);
        }
        Ok(Self {
            maximum_tickets: values[0],
            tickets: values[1],
            sketch_counts: [values[2], values[3], values[4]],
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct Health {
    pub maximum: i32,
    pub current: i32,
}
impl Health {
    ///20e2c reads the current component properties, not an authored snapshot.
    pub fn append(&self, output: &mut Vec<u8>) -> Result<()> {
        save_game::append_record(output, "MaxHealth", &self.maximum.to_be_bytes(), 4)?;
        save_game::append_record(output, "Health", &self.current.to_be_bytes(), 4)
    }
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let records = save_game::records(payload)?;
        Ok(Self {
            maximum: i32::from_be_bytes(word(&records, "MAXHEALTH")?),
            current: i32::from_be_bytes(word(&records, "HEALTH")?),
        })
    }
}

/// Preserve native floating-point words exactly, including signed zero. Numeric
/// queries such as available() may substitute100 or alias the paint channel;
/// neither policy changes the separately serialized stored amounts.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct Ammo {
    pub amount_bits: [u32; 2],
    pub maximum_bits: [u32; 2],
    pub recharge_ceiling_bits: [u32; 2],
    pub segment_amount_bits: [u32; 2],
}
fn word(records: &[save_game::Record<'_>], name: &str) -> Result<[u8; 4]> {
    //25e774 selects the first match and the native scalar reader requests4.
    let data = records
        .iter()
        .find(|record| record.name == name)
        .with_context(|| format!("Missing player save record {name}"))?
        .data;
    Ok(data
        .get(..4)
        .with_context(|| format!("Truncated player save record {name}"))?
        .try_into()?)
}
impl Ammo {
    pub fn capture(state: &super::ammo::State, segments: [f32; 2]) -> Result<Self> {
        let inventory = state
            .inventory
            .as_ref()
            .context("Player ammo inventory is not owned")?;
        Ok(Self {
            amount_bits: inventory.amounts.map(f32::to_bits),
            maximum_bits: [
                inventory.maxima[0]
                    .context("Paint maximum is not owned")?
                    .to_bits(),
                inventory.maxima[1]
                    .context("Thinner maximum is not owned")?
                    .to_bits(),
            ],
            recharge_ceiling_bits: state
                .settings
                .channels
                .map(|channel| channel.recharge_max.to_bits()),
            segment_amount_bits: segments.map(f32::to_bits),
        })
    }
    fn words(&self) -> [u32; 8] {
        [
            self.amount_bits[0],
            self.amount_bits[1],
            self.maximum_bits[0],
            self.maximum_bits[1],
            self.recharge_ceiling_bits[0],
            self.recharge_ceiling_bits[1],
            self.segment_amount_bits[0],
            self.segment_amount_bits[1],
        ]
    }
    pub fn append(&self, output: &mut Vec<u8>) -> Result<()> {
        for (name, value) in AMMO_NAMES.into_iter().zip(self.words()) {
            save_game::append_record(output, name, &value.to_be_bytes(), 4)?;
        }
        Ok(())
    }
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let records = save_game::records(payload)?;
        let mut values = [0; 8];
        for (value, name) in values.iter_mut().zip(AMMO_NAMES) {
            *value = u32::from_be_bytes(word(&records, &name.to_ascii_uppercase())?);
        }
        Ok(Self {
            amount_bits: [values[0], values[1]],
            maximum_bits: [values[2], values[3]],
            recharge_ceiling_bits: [values[4], values[5]],
            segment_amount_bits: [values[6], values[7]],
        })
    }
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({"raw_f32_bits":self,"stored_amounts":self.amount_bits.map(f32::from_bits),
            "maxima":self.maximum_bits.map(f32::from_bits),"recharge_ceilings":self.recharge_ceiling_bits.map(f32::from_bits),
            "segment_amounts":self.segment_amount_bits.map(f32::from_bits),"checkpoint_integrated":false,"restoration_implemented":false})
    }
}

impl super::assets::World {
    /// REL91b60 ->344c2c: explicit player-controller owner, first sketch slot.
    pub(super) fn sketch_count_lua(&mut self, lua: &mlua::Lua, mut args: mlua::MultiValue) -> mlua::Result<mlua::MultiValue> {
        if args.len() != 2 {
            self.report.warnings.push("GetSketchCount: expected two arguments; preserving native stack order".into());
        }
        let item = lua.coerce_string(args.pop_back().unwrap_or(mlua::Value::Nil))?;
        let item = item.as_ref().map(|v| v.to_str()).transpose()?;
        let item = item.as_deref().unwrap_or("").split('\0').next().unwrap_or("");
        let owner = self.scripts.resolve_native_entity_argument(lua,args.pop_back().unwrap_or(mlua::Value::Nil))?
            .filter(|owner| !self.removed_entities.contains(owner));
        let inventory = owner.and_then(|owner| self.entities.get(owner))
            .filter(|entity| entity.components.iter().any(|c| c.class_name == "MickeyPlayerControllerComponent"))
            .and_then(|entity| entity.components.iter().find(|c| c.class_name == "InventoryComponent"));
        let count = if let Some(inventory) = inventory {
            let slot = inventory.properties.iter().find(|p| p.name == "Sketch")
                .and_then(|p| p.value.as_array())
                .and_then(|names| names.iter().position(|v| v.as_str().is_some_and(|name| name.split('\0').next().unwrap_or("") == item)));
            slot.and_then(|slot| inventory.properties.iter().find(|p| p.name == "SketchCount")
                .and_then(|p| p.value.get(slot)).and_then(|v| v.as_i64())).unwrap_or(0)
        } else { 0 };
        Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Integer(count as mlua::Integer)]))
    }
    /// REL26b760 ignores its supplied owner and queries the primary player.
    pub(super) fn ticket_count_lua(&mut self, args: mlua::MultiValue) -> mlua::Result<mlua::MultiValue> {
        if args.len() != 1 {
            self.report.warnings.push("GetTicketCount: expected one argument".into());
        }
        let inventory = self.entities.get(self.player_entity)
            .filter(|_| !self.removed_entities.contains(&self.player_entity))
            .and_then(|entity| entity.components.iter().find(|c| c.class_name == "InventoryComponent"));
        let count = if let Some(inventory) = inventory {
            let value = inventory.properties.iter().find(|p| p.name == "ETicket")
                .and_then(|p| p.value.as_f64())
                .ok_or_else(|| mlua::Error::external("Live ETicket property is missing"))?;
            super::inventory_commands::checked_fctiwz(value as f32).map_err(mlua::Error::external)?
        } else { 0 };
        Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Integer(count as mlua::Integer)]))
    }
    ///216d8..2170c constructs one section from live owners, in native order.
    /// This captures the current reimplementation state; it does not claim
    /// original initialization/progression fidelity or create a GAME archive.
    pub fn capture_player_records(&self) -> Result<Capture> {
        let mut payload = Vec::new();
        super::player_spawn::append_records(&mut payload, self.player_spawn_save_transform()?)?;
        self.player_health_save_state()?.append(&mut payload)?;
        self.playstyle_points.append_record(&mut payload)?;
        let playthrough = self.playthrough;
        self.append_player_registered_ability_records(&mut payload)?;
        self.player_inventory_save_state()?.append(&mut payload)?;
        self.player_ammo_save_state()?.append(&mut payload)?;
        Ok(Capture {
            payload,
            playthrough,
        })
    }
    pub fn player_inventory_save_state(&self) -> Result<Inventory> {
        let owner = self
            .entities
            .get(self.player_entity)
            .context("Mickey is not owned")?;
        let components = owner
            .components
            .iter()
            .filter(|component| component.class_name == "InventoryComponent")
            .collect::<Vec<_>>();
        ensure!(
            components.len() == 1,
            "Player inventory component selection is ambiguous"
        );
        // Re-read current properties; authored_inventory is an import snapshot.
        Inventory::capture(&super::inventory_state::Authored::from_component(
            components[0],
        )?)
    }
    pub fn player_health_save_state(&self) -> Result<Health> {
        let owner = self
            .entities
            .get(self.player_entity)
            .context("Mickey is not owned")?;
        let components = owner
            .components
            .iter()
            .filter(|component| component.class_name == "HealthComponent")
            .collect::<Vec<_>>();
        ensure!(
            components.len() == 1,
            "Player health component selection is ambiguous"
        );
        let maximum = components[0]
            .properties
            .iter()
            .find(|property| property.name == "MaxHealth")
            .and_then(|property| property.value.as_i64())
            .context("Live MaxHealth property is missing")?;
        Ok(Health {
            maximum: i32::try_from(maximum)?,
            current: self
                .runtime_health
                .get(&self.player_entity)
                .context("Live player health is not owned")?
                .health,
        })
    }
    pub fn player_ammo_save_state(&self) -> Result<Ammo> {
        let owner = self
            .entities
            .get(self.player_entity)
            .context("Mickey is not owned")?;
        let components = owner
            .components
            .iter()
            .filter(|component| component.class_name == "InventoryComponent")
            .collect::<Vec<_>>();
        ensure!(
            components.len() == 1,
            "Player inventory component selection is ambiguous"
        );
        let mut segments = [0.; 2];
        for (destination, name) in segments
            .iter_mut()
            .zip(["PaintAmmoSegmentAmount", "ThinnerAmmoSegmentAmount"])
        {
            *destination = components[0]
                .properties
                .iter()
                .find(|property| property.name == name)
                .and_then(|property| property.value.as_f64())
                .with_context(|| format!("Live inventory {name} missing"))?
                as f32;
            ensure!(destination.is_finite(), "Nonfinite inventory {name}");
        }
        Ammo::capture(&self.ammo, segments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn player_float_records_preserve_bits_order_and_native_thinner_spelling() -> Result<()> {
        let state = Ammo {
            amount_bits: [0x80000000, 0x7fc01234],
            maximum_bits: [1, 2],
            recharge_ceiling_bits: [3, 4],
            segment_amount_bits: [5, 6],
        };
        let mut bytes = vec![];
        state.append(&mut bytes)?;
        assert_eq!(Ammo::decode(&bytes)?, state);
        let records = save_game::records(&bytes)?;
        assert_eq!(
            records.iter().map(|record| record.name).collect::<Vec<_>>(),
            AMMO_NAMES.map(str::to_ascii_uppercase)
        );
        assert_eq!(records[1].name, "THINERAMMO");
        assert_eq!(records[0].data, &0x80000000u32.to_be_bytes());
        let health = Health {
            maximum: 1,
            current: 5,
        };
        let mut bytes = vec![];
        health.append(&mut bytes)?;
        assert_eq!(Health::decode(&bytes)?, health);
        assert!(Ammo::decode(&bytes).is_err());
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_live_player_capture_uses_current_balances_and_component_fields() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = super::super::assets::load(
            std::path::Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        assert_eq!(world.playstyle_points.points(1), 0.);
        world.playstyle_points.replace(-44.25, 1);
        let capture = super::super::player_scalar_capture(&world);
        assert_eq!(capture["playstyle_points"]["saved_points"], -44.25);
        assert_eq!(capture["record_count"], 16);
        assert_eq!(capture["complete_mick"], false);
        let inventory = world
            .ammo
            .inventory
            .as_mut()
            .context("Original ammo owner missing")?;
        inventory.amounts = [1.25, 9.5];
        inventory.maxima = [Some(13.), Some(29.)];
        world.ammo.policy.infinite = true;
        world.ammo.policy.shared = true;
        world.ammo.settings.channels[0].recharge_max = 3.;
        world.ammo.settings.channels[1].recharge_max = 7.;
        for component in &mut world.entities[world.player_entity].components {
            for property in &mut component.properties {
                match (component.class_name.as_str(), property.name.as_str()) {
                    ("InventoryComponent", "PaintAmmoSegmentAmount") => {
                        property.value = serde_json::json!(2.)
                    }
                    ("InventoryComponent", "ThinnerAmmoSegmentAmount") => {
                        property.value = serde_json::json!(6.)
                    }
                    ("InventoryComponent", "ETicketMax") => {
                        property.value = serde_json::json!(1010.9)
                    }
                    ("InventoryComponent", "ETicket") => property.value = serde_json::json!(137.75),
                    ("InventoryComponent", "SketchCount") => {
                        property.value = serde_json::json!([7, -4, 11])
                    }
                    ("HealthComponent", "MaxHealth") => property.value = serde_json::json!(13),
                    _ => {}
                }
            }
        }
        world
            .runtime_health
            .get_mut(&world.player_entity)
            .unwrap()
            .health = 2;
        assert_eq!(
            world.player_health_save_state()?,
            Health {
                maximum: 13,
                current: 2
            }
        );
        assert_ne!(world.authored_health[&world.player_entity].maximum, 13);
        let inventory = world.player_inventory_save_state()?;
        assert_eq!(
            world.authored_inventory[&world.player_entity].sketches,
            ["Sketch_TV", "Sketch_Watch", "Sketch_Anvil"]
        );
        assert_eq!(
            inventory,
            Inventory {
                maximum_tickets: 1010,
                tickets: 137,
                sketch_counts: [11, 7, -4]
            }
        );
        assert_eq!(
            world.authored_inventory[&world.player_entity].floats["ETicket"],
            0.
        );
        assert_eq!(
            [world.ammo.available(0), world.ammo.available(1)],
            [100.; 2]
        );
        let snapshot = world.player_ammo_save_state()?;
        assert_eq!(snapshot.amount_bits.map(f32::from_bits), [1.25, 9.5]);
        assert_eq!(snapshot.maximum_bits.map(f32::from_bits), [13., 29.]);
        assert_eq!(snapshot.recharge_ceiling_bits.map(f32::from_bits), [3., 7.]);
        assert_eq!(snapshot.segment_amount_bits.map(f32::from_bits), [2., 6.]);
        let mut bytes = vec![];
        snapshot.append(&mut bytes)?;
        assert_eq!(Ammo::decode(&bytes)?, snapshot);
        world.ammo.inventory = None;
        assert!(world.player_ammo_save_state().is_err());
        Ok(())
    }
    #[test]
    #[ignore = "requires original populated EM1 save in BLOTWORX_TEST_SAVE"]
    fn original_mick_ammo_tail_reencodes_exactly_without_creating_partial_checkpoint() -> Result<()>
    {
        let path = std::env::var_os("BLOTWORX_TEST_SAVE").context("Set BLOTWORX_TEST_SAVE")?;
        let bytes = std::fs::read(&path)?;
        let archive = save_game::Archive::parse(&bytes)?;
        let section = archive.section(*b"MICK")?;
        assert_eq!(section.version, 3);
        let ammo = Ammo::decode(&section.payload)?;
        let records = save_game::records(&section.payload)?;
        let first = records
            .iter()
            .find(|record| record.name == "PAINTAMMO")
            .unwrap();
        let data_offset = first.data.as_ptr() as usize - section.payload.as_ptr() as usize;
        let start = data_offset - (5 + "PAINTAMMO".len()).div_ceil(4) * 4;
        // Existing prefix is used ONLY to validate this native record group.
        // Product live capture must supply the other MICK subowners itself.
        let mut rewritten = section.payload[..start].to_vec();
        ammo.append(&mut rewritten)?;
        rewritten.resize(rewritten.len().div_ceil(4) * 4, 0);
        assert_eq!(&rewritten, &*section.payload);
        let health = Health::decode(&section.payload)?;
        let first = records
            .iter()
            .find(|record| record.name == "MAXHEALTH")
            .unwrap();
        let last = records
            .iter()
            .find(|record| record.name == "HEALTH")
            .unwrap();
        let start = first.data.as_ptr() as usize
            - section.payload.as_ptr() as usize
            - (5 + "MAXHEALTH".len()).div_ceil(4) * 4;
        let end = last.data.as_ptr() as usize - section.payload.as_ptr() as usize + 4;
        let mut rewritten = section.payload[..start].to_vec();
        health.append(&mut rewritten)?;
        rewritten.extend_from_slice(&section.payload[end..]);
        assert_eq!(&rewritten, &*section.payload);
        let inventory = Inventory::decode(&section.payload)?;
        let first = records
            .iter()
            .find(|record| record.name == "MAXTICKETS")
            .unwrap();
        let last = records
            .iter()
            .find(|record| record.name == "SKETCH_WATCH")
            .unwrap();
        let start = first.data.as_ptr() as usize
            - section.payload.as_ptr() as usize
            - (5 + "MAXTICKETS".len()).div_ceil(4) * 4;
        let end = last.data.as_ptr() as usize - section.payload.as_ptr() as usize + 4;
        let mut rewritten = section.payload[..start].to_vec();
        inventory.append(&mut rewritten)?;
        rewritten.extend_from_slice(&section.payload[end..]);
        assert_eq!(&rewritten, &*section.payload);
        assert_eq!(std::fs::read(path)?, bytes);
        Ok(())
    }
    #[test]
    fn inventory_capture_uses_first_exact_sketch_slot_and_native_integer_conversion() -> Result<()>
    {
        let mut state = super::super::inventory_state::Authored {
            floats: [("ETicketMax".into(), 1000.9), ("ETicket".into(), -2.75)]
                .into_iter()
                .collect(),
            sketches: vec![
                "Sketch_TV".into(),
                "sketch_anvil".into(),
                "Sketch_TV".into(),
                "Sketch_Watch\0ignored".into(),
            ],
            counts: vec![7, 9, 88, -4],
            maxima: vec![],
        };
        let actual = Inventory::capture(&state)?;
        assert_eq!(
            actual,
            Inventory {
                maximum_tickets: 1000,
                tickets: -2,
                sketch_counts: [0, 7, -4]
            }
        );
        let mut bytes = vec![];
        actual.append(&mut bytes)?;
        assert_eq!(Inventory::decode(&bytes)?, actual);
        assert_eq!(
            save_game::records(&bytes)?
                .iter()
                .map(|r| r.name)
                .collect::<Vec<_>>(),
            INVENTORY_NAMES.map(str::to_ascii_uppercase)
        );
        state.counts.clear();
        assert_eq!(Inventory::capture(&state)?.sketch_counts, [0; 3]);
        for value in [f32::NAN, f32::INFINITY, 2147483648.] {
            state.floats.insert("ETicket".into(), value);
            assert!(Inventory::capture(&state).is_err());
        }
        state.floats.remove("ETicket");
        assert!(Inventory::capture(&state).is_err());
        Ok(())
    }
    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_live_mick_assembly_reads_committed_pose_and_prefers_manager_override() -> Result<()>
    {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let mut world = super::super::assets::load(
            std::path::Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
        )?;
        world.player_spawn.clear();
        let player = world.player_entity;
        let position = [13.25, -2.5, 9.75];
        world.entities[player].set_controller_pose(position, 0.65);
        world.playthrough.value = 17;
        let capture = world.capture_player_records()?;
        let records = save_game::records(&capture.payload)?;
        assert_eq!(records.len(), 32);
        assert_eq!(capture.playthrough.value, 17);
        let spawn = super::super::player_spawn::saved_transform(&capture.payload)?
            .context("Missing captured spawn")?;
        assert_eq!(spawn.position, position);
        assert_ne!(spawn.position, world.report.spawn);
        assert_eq!(spawn.rotation, world.entities[player].runtime_rotation);
        assert_ne!(spawn.rotation, world.entities[player].authored_rotation);
        assert_eq!(
            spawn.scale.to_bits(),
            world.entities[player].authored_scale.to_bits()
        );
        assert_eq!(
            &records[..6]
                .iter()
                .map(|record| record.name)
                .collect::<Vec<_>>(),
            &[
                "PLAYERSPAWNLOC",
                "PLAYERSPAWNROT",
                "PLAYERSPAWNSCALE",
                "MAXHEALTH",
                "HEALTH",
                "PLAYSTYLEPOINTS"
            ]
        );
        assert_eq!(records[6].name, "BLAST:0");
        assert_eq!(records[6].data, &0i32.to_be_bytes());
        assert_eq!(
            Inventory::decode(&capture.payload)?,
            world.player_inventory_save_state()?
        );
        assert_eq!(
            Ammo::decode(&capture.payload)?,
            world.player_ammo_save_state()?
        );
        assert!(
            save_game::Archive::parse(&capture.payload).is_err(),
            "MICK records are not a complete GAME checkpoint"
        );
        let rotation = [0., -1., 0., 1., 0., 0., 0., 0., 1.];
        let marker = [-4., 5., 6.];
        world.player_spawn.set(rotation, marker);
        world.entities[player].set_controller_pose([99.; 3], 1.2);
        let capture = world.capture_player_records()?;
        let spawn = super::super::player_spawn::saved_transform(&capture.payload)?.unwrap();
        assert_eq!(spawn.position, marker);
        assert_eq!(spawn.rotation, rotation);
        assert_eq!(spawn.scale.to_bits(), 0x3dcccccd);
        world.player_entity = usize::MAX;
        assert_eq!(
            world.player_spawn_save_transform()?.position,
            marker,
            "Override must bypass live transform getters"
        );
        world.player_spawn.clear();
        assert!(world.player_spawn_save_transform().is_err());
        Ok(())
    }
    #[test]
    #[ignore = "requires original native save in BLOTWORX_TEST_SAVE"]
    fn original_mick_all_record_groups_reassemble_without_borrowing_encoded_prefix() -> Result<()> {
        let path = std::env::var_os("BLOTWORX_TEST_SAVE").context("Set BLOTWORX_TEST_SAVE")?;
        let original = std::fs::read(&path)?;
        let archive = save_game::Archive::parse(&original)?;
        let section = archive.section(*b"MICK")?;
        let records = save_game::records(&section.payload)?;
        let mut output = Vec::new();
        super::super::player_spawn::append_records(
            &mut output,
            super::super::player_spawn::saved_transform(&section.payload)?
                .context("Missing native spawn")?,
        )?;
        Health::decode(&section.payload)?.append(&mut output)?;
        let mut points = super::super::playstyle_points::State::default();
        points.restore_record(&section.payload)?;
        points.append_record(&mut output)?;
        // Raw saved fields validate the codec only. Do not apply them to World
        // or invoke the non-equivalent native gameplay restoration/Give path.
        fn ability(output: &mut Vec<u8>, record: &save_game::Record<'_>) -> Result<()> {
            let (name, level) = record
                .name
                .rsplit_once(':')
                .context("Missing native ability suffix")?;
            let mask = i32::from_be_bytes(record.data.try_into()?);
            ensure!((0..=3).contains(&mask), "Unexpected ownership mask");
            let state = super::super::ability_ownership::Ownership {
                owned: [mask & 1 != 0, mask & 2 != 0],
                upgrades: [level.parse()?, i32::MIN],
                disabled: true,
            };
            super::super::ability_persistence::append(output, name, &state)
        }
        let mut index = 6;
        for name in super::super::ability_registry::PRIMARY
            .into_iter()
            .chain(super::super::ability_registry::SECONDARY)
        {
            assert_eq!(
                records[index].name.rsplit_once(':').unwrap().0,
                name.to_ascii_uppercase()
            );
            ability(&mut output, &records[index])?;
            index += 1;
        }
        let mut sketch = 0;
        while records[index].name.contains(':') {
            let name = records[index].name.rsplit_once(':').unwrap().0;
            ability(&mut output, &records[index])?;
            index += 1;
            assert_eq!(records[index].name, format!("{name}_{sketch}"));
            let maximum = f32::from_be_bytes(records[index].data.try_into()?);
            save_game::append_record(
                &mut output,
                records[index].name,
                &maximum.to_bits().to_be_bytes(),
                4,
            )?;
            index += 1;
            sketch += 1;
        }
        assert_eq!(records[index].name, "MAXTICKETS");
        Inventory::decode(&section.payload)?.append(&mut output)?;
        Ammo::decode(&section.payload)?.append(&mut output)?;
        output.resize(output.len().div_ceil(4) * 4, 0);
        assert_eq!(output, &*section.payload);
        assert_eq!(std::fs::read(path)?, original);
        Ok(())
    }
}

#[cfg(debug_assertions)]
pub(super) fn validate_live_inventory() -> anyhow::Result<()> {
    use anyhow::{Context,ensure};
    let root=std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
    let mut world=super::assets::load(std::path::Path::new(&root),"Levels/GV_ZoneA_Start.level",&egui::Context::default())?;
    let baseline=world.authored_inventory.clone();
    let owner=world.player_entity;
    let inventory=world.entities[owner].components.iter_mut().find(|c|c.class_name=="InventoryComponent").context("Missing player inventory")?;
    for (name,value) in [("Sketch",serde_json::json!(["SketchAnvil","SketchAnvil","SketchTV"])),("SketchCount",serde_json::json!([7,99,-2])),("SketchMax",serde_json::json!([10.,100.,10.])),("ETicket",serde_json::json!(16777216.)),("ETicketMax",serde_json::json!(0.))] {
        inventory.properties.iter_mut().find(|p|p.name==name).context("Missing inventory test field")?.value=value;
    }
    world.run_callback_fixture("assert(GetSketchCount(GetPlayer(),'SketchAnvil')==7); assert(GetSketchCount(GetPlayer(),'SketchTV')==-2); assert(GetSketchCount(nil,'SketchAnvil')==0); assert(GetSketchCount(GetPlayer(),'Missing')==0); assert(GetPropertyString(GetPlayer(),'Sketch',1)=='SketchAnvil'); assert(GetPropertyString(GetPlayer(),'Sketch',99)==nil)")?;
    ensure!(world.player_inventory_command("Add ETicket 1")? == false,"f32 rounding must reject an unchanged balance");
    ensure!(world.player_inventory_command("Add ETicket 2")?,"Nonpositive maximum must not impose a capacity");
    world.run_callback_fixture("assert(GetTicketCount(nil)==16777218)")?;
    ensure!(world.player_inventory_command("Add SketchAnvil 0.75")?,"Native sketch add admission must be preserved");
    world.run_callback_fixture("assert(GetSketchCount(GetPlayer(),'SketchAnvil')==7); assert(GetSketchCount(GetPlayer(),'SketchTV')==-2)")?;
    world.runtime_settings.insert("SketchDefaultMax".into(),"3.75suffix".into());
    ensure!(world.player_inventory_command("Add SketchNew 10.2suffix")?,"New sketch grant was rejected");
    world.run_callback_fixture("assert(GetSketchCount(GetPlayer(),'SketchNew')==3); assert(GetElementCount(GetPlayer(),'Sketch')==4); assert(GetPropertyString(GetPlayer(),'Sketch',3)=='SketchNew'); assert(GetPropertyFloat(GetPlayer(),'SketchMax',3)==3.75)")?;
    ensure!(world.player_inventory_command("Add SketchNew -1")?,"Signed sketch amount was rejected");
    world.run_callback_fixture("assert(GetSketchCount(GetPlayer(),'SketchNew')==2); SetPropertyInt(GetPlayer(),'SketchCount',-4,3); SetPropertyFloat(GetPlayer(),'SketchMax',8.5,3); assert(GetSketchCount(GetPlayer(),'SketchNew')==-4); assert(GetPropertyFloat(GetPlayer(),'SketchMax',3)==8.5)")?;
    ensure!(world.player_inventory_command("Add sketchNew -2")?,"Case-insensitive sketch-family admission failed");
    world.run_callback_fixture("assert(GetSketchCount(GetPlayer(),'sketchNew')==-2); assert(GetSketchCount(GetPlayer(),'SketchNew')==-4); assert(GetElementCount(GetPlayer(),'Sketch')==5)")?;
    ensure!(world.player_inventory_command("Add ETicket -2suffix")?,"Native signed numeric prefix was rejected");
    world.run_callback_fixture("assert(GetTicketCount(nil)==16777216)")?;
    ensure!(!world.player_inventory_command("Add\tETicket 2")?,"Native command separator must remain ASCII space");
    ensure!(world.authored_inventory==baseline,"Live commands modified authored inventory");
    println!("PASS: live inventory callbacks, new sketch grants/default maxima, numeric prefixes/signed amounts, indexed count/max writes, duplicate first-slot selection, f32 balances and immutable authored defaults");
    Ok(())
}
