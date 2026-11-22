//! Native Lua 5.1 animation bridge. Original bytecode executes directly after
//! endian/size_t conversion; no decompiler or reconstructed source is involved.
use anyhow::{ensure, Context, Result};
use mlua::{LightUserData, Lua, Thread, ThreadStatus, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

/// Accept scalar Lua paths as well as list-valued script fields.
/// Preserve authored order; invalid entries must not disappear via filter_map.
pub fn authored_paths(value: &serde_json::Value) -> Result<Vec<String>> {
    let entries: Vec<&serde_json::Value> = match value {
        serde_json::Value::String(_) => vec![value],
        serde_json::Value::Array(values) => values.iter().collect(),
        _ => anyhow::bail!("Lua File Path must be a string or a string array"),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let path = entry
            .as_str()
            .context("Lua File Path contains a non-string entry")?;
        ensure!(
            !path.contains('\0'),
            "Lua File Path contains an embedded NUL"
        );
        if !path.is_empty() {
            paths.push(path.to_owned());
        }
    }
    Ok(paths)
}

#[cfg(test)]
mod authored_path_tests {
    use super::*;
    #[test]
    fn scalar_and_array_paths_retain_order_and_reject_bad_entries() -> Result<()> {
        assert_eq!(
            authored_paths(&serde_json::json!(
                "Environments/_Shared/Scripts/Sketches.lua"
            ))?,
            ["Environments/_Shared/Scripts/Sketches.lua"]
        );
        assert_eq!(
            authored_paths(&serde_json::json!(["a.lua", "", "b.lua", "a.lua"]))?,
            ["a.lua", "b.lua", "a.lua"]
        );
        assert!(authored_paths(&serde_json::json!("")).unwrap().is_empty());
        for invalid in [
            serde_json::json!(null),
            serde_json::json!(["a.lua", 3]),
            serde_json::json!("a\u{0}b"),
        ] {
            assert!(authored_paths(&invalid).is_err());
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum Command {
    Sequence { entity: usize, name: String, notify: bool },
    Reset { entity: usize },
    Output { entity: usize, name: String },
    Retire { entity: usize },
}
#[derive(Clone, Copy, Default, Debug)]
pub struct CallbackContext {
    pub entity: Option<usize>,
    pub activator: Option<usize>,
}
struct Task {
    name: String,
    thread: Thread,
    wake: f32,
    context: CallbackContext,
}
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct MemorySummary {
    pub lua_heap_bytes: usize,
    pub imported_source_payload_bytes: usize,
    pub imported_source_path_bytes: usize,
    pub imported_source_count: usize,
    pub loaded_script_count: usize,
    pub pending_task_count: usize,
    pub pending_task_record_capacity_bytes: usize,
}
#[derive(Default)]
struct Spawned {
    tasks: Vec<Task>,
    errors: Vec<String>,
    time: f32,
}
enum Callout {
    Script {
        name: String,
        context: CallbackContext,
        parameters: Vec<Value>,
        nil_target: bool,
    },
    Native {
        name: String,
        args: mlua::MultiValue,
    },
}
#[cfg(test)]
mod missing_io_tests {
    use super::*;
    #[test]
    fn absent_targets_reach_callbacks_as_nil_in_direct_and_detached_dispatch() -> Result<()> {
        let mut facade = Scripts::new(&[(0, "owner".into())])?;
        facade.bind_world_api("QueueMissing")?;
        facade.load(
            "nil-io.lua",
            br#"seen=0
            function ObserveMissing(target,parameter)
              assert(target==nil and parameter=='50'); seen=seen+1
              coroutine.yield(0.1); assert(target==nil); seen=seen+1
            end
            function Invoke() QueueMissing() end"#,
        )?;
        facade.advance(0.)?;
        let context = CallbackContext {
            entity: Some(0),
            activator: None,
        };
        facade.call_missing_io("ObserveMissing", context, "50")?;
        facade.advance(0.2)?;
        facade.evaluate_callback_fixture("assert(seen==2)")?;
        let mut execution = facade.detach_scheduler()?;
        execution.with_world_host(
            |_, name, _| {
                assert_eq!(name, "QueueMissing");
                facade
                    .queue_missing_io_callback("ObserveMissing", context, "50")
                    .map_err(mlua::Error::external)?;
                Ok(mlua::MultiValue::new())
            },
            |scripts| scripts.call("Invoke", 0),
        )?;
        execution.advance(0.5)?;
        facade.restore_scheduler(execution)?;
        facade.evaluate_callback_fixture("assert(seen==4)")?;
        assert!(
            facade
                .call_missing_io("ActuallyMissingFunction", context, "50")
                .is_err(),
            "Unknown functions must still fail"
        );
        assert!(facade.errors.is_empty());
        Ok(())
    }
}
fn drain_callouts(
    lua: &Lua,
    function: &mlua::Function,
    queue: &Arc<Mutex<Vec<Callout>>>,
    contexts: &Arc<Mutex<CallbackContext>>,
    spawned: &Arc<Mutex<Spawned>>,
    bindings: &Arc<Mutex<EntityBindings>>,
) -> mlua::Result<()> {
    // Keep the parent's continuation locally. A callback's nested API call
    // drains only its own new work, never the parent's later operations.
    let work = std::mem::take(
        &mut *queue
            .lock()
            .map_err(|_| mlua::Error::external("Callout queue poisoned"))?,
    );
    for callout in work {
        match callout {
            Callout::Native { name, mut args } => {
                args.push_front(Value::String(lua.create_string(name)?));
                function.call::<mlua::MultiValue>(args)?;
                drain_callouts(lua, function, queue, contexts, spawned, bindings)?;
            }
            Callout::Script {
                name,
                context,
                parameters,
                nil_target,
            } => {
                let callback: mlua::Function = lua.globals().get(name.as_str())?;
                let thread = lua.create_thread(callback)?;
                let target = handle(
                    bindings,
                    context
                        .entity
                        .ok_or_else(|| mlua::Error::external("Callback has no owner"))?,
                )
                .map_err(mlua::Error::external)?;
                let mut args = mlua::MultiValue::from_vec(vec![if nil_target {
                    Value::Nil
                } else {
                    Value::LightUserData(target)
                }]);
                args.extend(parameters);
                let values = resume_context(&thread, contexts, context, args)?;
                if thread.status() == ThreadStatus::Resumable {
                    let wait = delay(&values).map_err(mlua::Error::external)?;
                    let mut pending = spawned
                        .lock()
                        .map_err(|_| mlua::Error::external("Spawned task queue poisoned"))?;
                    let wake = pending.time + wait;
                    pending.tasks.push(Task {
                        name,
                        thread,
                        wake,
                        context,
                    });
                }
            }
        }
    }
    Ok(())
}
fn resume_context<A: mlua::IntoLuaMulti>(
    thread: &Thread,
    slot: &Arc<Mutex<CallbackContext>>,
    context: CallbackContext,
    args: A,
) -> mlua::Result<mlua::MultiValue> {
    struct Restore {
        slot: Arc<Mutex<CallbackContext>>,
        previous: CallbackContext,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Ok(mut slot) = self.slot.lock() {
                *slot = self.previous;
            }
        }
    }
    let previous = {
        let mut slot = slot
            .lock()
            .map_err(|_| mlua::Error::external("Callback context lock poisoned"))?;
        std::mem::replace(&mut *slot, context)
    };
    let _restore = Restore {
        slot: slot.clone(),
        previous,
    };
    thread.resume(args)
}
struct EntityBindings {
    handles: super::script_handles::Registry,
    names: BTreeMap<usize, String>,
    registry: super::entity_registry::Registry,
    /// World indices are append-only. A high-water mark prevents stale Lua
    /// handles from resolving to later spawns without retaining tombstones.
    highest_issued: Option<usize>,
}
#[derive(Clone, Copy)]
struct CompanionBinding {entity:usize,ai:bool}
pub struct Scripts {
    lua: Lua,
    tasks: Vec<Task>,
    commands: Arc<Mutex<Vec<Command>>>,
    pub errors: Vec<String>,
    time: f32,
    time_origin: f32,
    player: Arc<Mutex<Option<usize>>>,
    companion: Arc<Mutex<Option<CompanionBinding>>>,
    entities: Arc<Mutex<EntityBindings>>,
    prefab_data: Arc<Mutex<BTreeMap<usize, mlua::Table>>>,
    prefab_owners: Arc<Mutex<BTreeMap<usize, usize>>>,
    retained_prefabs: Arc<Mutex<std::collections::BTreeSet<usize>>>,
    world_host: Arc<Mutex<Option<mlua::Function>>>,
    scheduler_detached: bool,
    callback_context: Arc<Mutex<CallbackContext>>,
    loaded_files: Arc<Mutex<std::collections::BTreeSet<String>>>,
    spawned: Arc<Mutex<Spawned>>,
    callouts: Arc<Mutex<Vec<Callout>>>,
    source_files: Arc<Mutex<BTreeMap<String, Arc<[u8]>>>>,
}
impl Drop for Scripts {
    fn drop(&mut self) {
        // A Lua closure owns the pending queue; pending Thread handles own the
        // VM. Break that cycle when the execution owner/world is retired.
        if !self.scheduler_detached {
            if let Ok(mut queue) = self.callouts.lock() {
                queue.clear();
            }
            if let Ok(mut pending) = self.spawned.lock() {
                pending.tasks.clear();
            }
        }
    }
}
impl Scripts {
    /// Move the execution owner out before releasing the old world's large
    /// asset/physics payloads. The replacement VM is only a drop-safe facade.
    pub(super) fn take_for_transition(&mut self) -> Result<Self> {
        ensure!(!self.scheduler_detached,"Cannot transfer a detached Lua scheduler");
        ensure!(self.world_host.lock().map_err(|_|anyhow::anyhow!("World host lock poisoned"))?.is_none(),
            "Cannot transfer Lua during an active world callback");
        Ok(std::mem::replace(self,Self::new(&[])?))
    }
    /// Retain compiled globals, cached Lua functions and the player identity.
    /// Scene-owned tasks are retired at the world boundary; global/player
    /// tasks retain their deadlines on the carried simulation clock.
    pub(super) fn adopt_world(&mut self, destination:&Scripts, old_player:usize, new_player:usize) -> Result<serde_json::Value> {
        ensure!(!self.scheduler_detached && self.world_host.lock().map_err(|_|anyhow::anyhow!("World host lock poisoned"))?.is_none(),
            "Lua world adoption requires a quiescent execution owner");
        ensure!(!Arc::ptr_eq(&self.entities,&destination.entities),"Lua world cannot adopt its own bindings");
        let incoming=destination.entities.lock().map_err(|_|anyhow::anyhow!("Destination entity lock poisoned"))?;
        ensure!(incoming.names.contains_key(&new_player),"Destination player is not registered");
        let retain_player=self.has_entity(old_player);
        {
            let mut bindings=self.entities.lock().map_err(|_|anyhow::anyhow!("Entity registry lock poisoned"))?;
            bindings.handles.replace_world(incoming.names.keys().map(|&entity|
                (entity, (retain_player && entity==new_player).then_some(old_player))))
                .map_err(anyhow::Error::msg)?;
            bindings.names=incoming.names.clone();
            bindings.registry=incoming.registry.clone();
            bindings.highest_issued=incoming.highest_issued;
        }
        drop(incoming);
        self.time_origin=self.time;
        let remap=|context:&mut CallbackContext| {
            for id in [&mut context.entity,&mut context.activator] {
                if let Some(old)=*id {
                    if retain_player && old==old_player { *id=Some(new_player); } else { return false; }
                }
            }
            true
        };
        let mut retired=Vec::new();
        self.tasks.retain_mut(|task| {
            let keep=remap(&mut task.context);
            if !keep { retired.push(task.name.clone()); }
            keep
        });
        {
            let mut pending=self.spawned.lock().map_err(|_|anyhow::anyhow!("Pending Lua task lock poisoned"))?;
            pending.tasks.retain_mut(|task| {
                let keep=remap(&mut task.context);
                if !keep { retired.push(task.name.clone()); }
                keep
            });
            pending.time=self.time;
        }
        self.commands.lock().map_err(|_|anyhow::anyhow!("Lua command lock poisoned"))?.clear();
        self.callouts.lock().map_err(|_|anyhow::anyhow!("Lua callout lock poisoned"))?.clear();
        *self.callback_context.lock().map_err(|_|anyhow::anyhow!("Lua context lock poisoned"))?=CallbackContext::default();
        *self.player.lock().map_err(|_|anyhow::anyhow!("Player lock poisoned"))?=Some(new_player);
        *self.companion.lock().map_err(|_|anyhow::anyhow!("Companion lock poisoned"))?=
            *destination.companion.lock().map_err(|_|anyhow::anyhow!("Destination companion lock poisoned"))?;
        {
            let mut data=self.prefab_data.lock().map_err(|_|anyhow::anyhow!("Prefab data lock poisoned"))?;
            let player=if retain_player { data.remove(&old_player) } else { None };
            data.clear();
            self.prefab_owners.lock().map_err(|_|anyhow::anyhow!("Prefab owner lock poisoned"))?.clear();
            self.retained_prefabs.lock().map_err(|_|anyhow::anyhow!("Retained prefab lock poisoned"))?.clear();
            if let Some(table)=player {
                data.insert(new_player,table);
                self.prefab_owners.lock().map_err(|_|anyhow::anyhow!("Prefab owner lock poisoned"))?.insert(new_player,new_player);
                self.retained_prefabs.lock().map_err(|_|anyhow::anyhow!("Retained prefab lock poisoned"))?.insert(new_player);
            }
        }
        let global=script_resource_key("Environments/_Shared/Scripts/globalscripts.lua")?;
        self.loaded_files.lock().map_err(|_|anyhow::anyhow!("Loaded module lock poisoned"))?.retain(|key|key==&global);
        Ok(serde_json::json!({"kind":"lua_world_transfer","player_identity_retained":retain_player,
            "retired_scene_tasks":retired,"clock":self.time,
            "persistent_entity_scope":"player only; other global entities currently reinitialize",
            "scope":"compiled globals and global/player tasks retained; scene contexts invalidated; native lifecycle phase parity unverified"}))
    }
    fn entity_name(&self, entity: usize) -> Option<String> {
        self.entities
            .lock()
            .expect("Entity registry lock poisoned")
            .names
            .get(&entity)
            .cloned()
    }
    pub(super) fn has_entity(&self, entity: usize) -> bool {
        self.entities
            .lock()
            .expect("Entity registry lock poisoned")
            .names
            .contains_key(&entity)
    }
    /// Publish a newly allocated World index to existing Lua closures, including
    /// coroutines already waiting. Never reuse an index issued by this World.
    pub fn register_entity(&mut self, entity: usize, name: &str, aliases: &[String]) -> Result<()> {
        let mut bindings = self
            .entities
            .lock()
            .map_err(|_| anyhow::anyhow!("Entity registry lock poisoned"))?;
        ensure!(
            bindings.highest_issued.is_none_or(|last| entity > last),
            "Runtime entity index was already issued or is not append-only"
        );
        bindings.handles.register(entity).map_err(anyhow::Error::msg)?;
        bindings.names.insert(entity, name.into());
        bindings.registry.register(name, entity);
        for alias in aliases {
            bindings.registry.register(alias, entity);
        }
        bindings.highest_issued = Some(entity);
        Ok(())
    }
    /// Additional cooked scene names can refer to the existing primary player.
    /// Do not allocate another handle, owner, body or animation graph.
    pub(super) fn register_existing_alias(&mut self, entity:usize, alias:&str)->Result<()> {
        let mut bindings=self.entities.lock().map_err(|_|anyhow::anyhow!("Entity registry lock poisoned"))?;
        ensure!(bindings.names.contains_key(&entity),"Alias target is not registered");
        bindings.registry.register(alias,entity);
        Ok(())
    }
    /// Factory source objects retain their canonical lookup and handles, but
    /// their inherited gameplay groups belong to the constructed instances.
    pub(super) fn suppress_source_groups(&mut self,entity:usize)->Result<()> {
        let mut bindings=self.entities.lock().map_err(|_|anyhow::anyhow!("Entity registry lock poisoned"))?;
        let primary=bindings.names.get(&entity).context("Template source is not registered")?.clone();
        bindings.registry.mark_factory_source(entity,&primary);
        Ok(())
    }
    /// Retire name/alias lookup and invalidate cached opaque handles. External
    /// references to a prefab table remain ordinary Lua values, not live owners.
    pub fn unregister_entity(&mut self, entity: usize) -> Result<()> {
        {
            let mut bindings = self
                .entities
                .lock()
                .map_err(|_| anyhow::anyhow!("Entity registry lock poisoned"))?;
            bindings.names.remove(&entity);
            bindings.handles.retire(entity);
            bindings.registry.remove_entity(entity);
        }
        self.prefab_data
            .lock()
            .map_err(|_| anyhow::anyhow!("Prefab data lock poisoned"))?
            .remove(&entity);
        self.prefab_owners
            .lock()
            .map_err(|_| anyhow::anyhow!("Prefab ownership lock poisoned"))?
            .retain(|&member, owner| member != entity && *owner != entity);
        let mut player = self
            .player
            .lock()
            .map_err(|_| anyhow::anyhow!("Player identity lock poisoned"))?;
        if *player == Some(entity) {
            *player = None;
        }
        let mut companion=self.companion.lock().map_err(|_|anyhow::anyhow!("Companion lock poisoned"))?;
        if companion.is_some_and(|binding|binding.entity==entity) {*companion=None;}
        drop(companion);
        self.commands
            .lock()
            .map_err(|_| anyhow::anyhow!("Script command queue poisoned"))?
            .retain(|command| {
                let owner = match command {
                    Command::Sequence { entity, .. }
                    | Command::Reset { entity }
                    | Command::Output { entity, .. }
                    | Command::Retire { entity } => *entity,
                };
                owner != entity
            });
        Ok(())
    }
    pub(super) fn native_binding_audit() -> Result<serde_json::Value> {
        let mut scripts = Self::with_aliases(&[(0,"a".into()),(1,"b".into())], &[(0,"group".into()),(1,"group".into()),(0,"group".into())])?;
        scripts.lua.load(r#"
            assert(GetEntityGroupSize("group") == 2)
            assert(GetEntityGroupSize("missing") == 0)
            assert(GetEntityGroupSize("group\0ignored") == 2)
            assert(rawequal(Show, Unhide))
            assert(rawequal(EnableMotion_Player, EnableMotion))
            assert(rawequal(DisableMotion_Player, DisableMotion))
            assert(rawequal(Exists, IsValidHandle))
            assert(rawequal(GetPaladinPercent, GetHeroPercent))
        "#).exec()?;
        scripts.unregister_entity(1)?;
        scripts.lua.load("assert(GetEntityGroupSize('group')==1)").exec()?;
        let mut bound = Vec::new();
        for pair in scripts.lua.globals().pairs::<String, mlua::Value>() {
            let (name,value) = pair?;
            if matches!(value,mlua::Value::Function(_)) { bound.push(name); }
        }
        bound.sort();
        let catalog:serde_json::Value=serde_json::from_str(include_str!("../../tools/blotworx/fixtures/native_lua_exports.json"))?;
        let functions=catalog["functions"].as_array().context("Native export catalog malformed")?;
        let entries=functions.iter().map(|entry|serde_json::json!({"name":entry["name"],"registrations":entry["registrations"],
            "bound_in_bootstrap_vm":entry["name"].as_str().is_some_and(|n|bound.binary_search_by(|s|s.as_str().cmp(n)).is_ok()),
            "behavioral_parity":"not established by binding audit"})).collect::<Vec<_>>();
        Ok(serde_json::json!({"rel_sha256":catalog["rel_sha256"],"scope":catalog["scope"],"native_exports":entries,
            "bootstrap_function_globals":bound,"registration_count":functions.len(),"connection_checks_passed":9,
            "complete_native_lua_implementation":false}))
    }
    pub fn new(entities: &[(usize, String)]) -> Result<Self> {
        Self::with_aliases(entities, &[])
    }
    /// A separate diagnostic VM shares immutable imported resources, never
    /// the World's globals, handles, task scheduler or prefab data.
    pub(super) fn resource_inspection(&self, name: &str) -> Result<Self> {
        let mut scripts = Self::new(&[(0, name.into())])?;
        scripts.source_files = self.source_files.clone();
        Ok(scripts)
    }
    /// REL328c70..328d2c accepts light userdata or Lua/C functions only.
    /// A table with __call is deliberately not admitted by retail IO.
    pub(super) fn io_callback_admitted(&self,name:&str)->mlua::Result<bool> {
        Ok(matches!(self.lua.globals().get::<Value>(name)?,Value::Function(_)|Value::LightUserData(_)))
    }
    pub(super) fn has_callback(&self, name: &str) -> mlua::Result<bool> {
        Ok(matches!(
            self.lua.globals().get::<Value>(name)?,
            Value::Function(_)
        ))
    }
    pub(super) fn ensure_resource_module(&mut self, path: &str) -> Result<()> {
        let key = script_resource_key(path)?;
        let bytes = self
            .source_files
            .lock()
            .map_err(|_| anyhow::anyhow!("Script resource lock poisoned"))?
            .get(&key)
            .cloned()
            .with_context(|| format!("Script module is not imported: {path}"))?;
        self.ensure_global_module(path, &bytes)
    }
    pub(super) fn pending_tasks(&self) -> Result<usize> {
        Ok(self.tasks.len()
            + self
                .spawned
                .lock()
                .map_err(|_| anyhow::anyhow!("Script task lock poisoned"))?
                .tasks
                .len())
    }
    pub(super) fn queued_world_commands(&self)->Result<usize> {
        Ok(self.commands.lock().map_err(|_|anyhow::anyhow!("Lua command lock poisoned"))?.len())
    }
    pub fn memory_summary(&self) -> MemorySummary {
        let (source_count, source_payload, source_paths) = self
            .source_files
            .lock()
            .map(|sources| {
                sources.iter().fold(
                    (0usize, 0usize, 0usize),
                    |(count, payload, paths), (path, bytes)| {
                        (
                            count + 1,
                            payload.saturating_add(bytes.len()),
                            paths.saturating_add(path.capacity()),
                        )
                    },
                )
            })
            .unwrap_or_default();
        let loaded_script_count = self
            .loaded_files
            .lock()
            .map(|files| files.len())
            .unwrap_or_default();
        let (spawned_task_count, spawned_task_capacity) = self
            .spawned
            .lock()
            .map(|spawned| (spawned.tasks.len(), spawned.tasks.capacity()))
            .unwrap_or_default();
        let task_capacity = self.tasks.capacity().saturating_add(spawned_task_capacity);
        MemorySummary {
            lua_heap_bytes: self.lua.used_memory(),
            imported_source_payload_bytes: source_payload,
            imported_source_path_bytes: source_paths,
            imported_source_count: source_count,
            loaded_script_count,
            pending_task_count: self.tasks.len().saturating_add(spawned_task_count),
            pending_task_record_capacity_bytes: task_capacity
                .saturating_mul(std::mem::size_of::<Task>()),
        }
    }
    pub fn with_aliases(entities: &[(usize, String)], aliases: &[(usize, String)]) -> Result<Self> {
        let lua = Lua::new();
        super::lua_vector::install(&lua)?;
        // REL365b58 pushes2;365bac binds it as PTCS_Thinner.
        lua.globals().set("PTCS_Thinner",2)?;
        let commands = Arc::new(Mutex::new(Vec::new()));
        let entity_names = entities.iter().cloned().collect::<BTreeMap<_, _>>();
        ensure!(
            entity_names.len() == entities.len(),
            "Duplicate runtime entity identity in Lua registry"
        );
        let handles = super::script_handles::Registry::initial(entity_names.keys().copied()).map_err(anyhow::Error::msg)?;
        let mut groups = BTreeMap::<usize, Vec<&str>>::new();
        for (id, name) in aliases {
            ensure!(
                entity_names.contains_key(id),
                "Alias member is not a registered entity"
            );
            groups.entry(*id).or_default().push(name);
        }
        let mut names = super::entity_registry::Registry::default();
        for (id, name) in entities {
            names.register(name, *id);
            for alias in groups.get(id).into_iter().flatten() {
                names.register(alias, *id);
            }
        }
        let highest_issued = entity_names.keys().next_back().copied();
        let entities = Arc::new(Mutex::new(EntityBindings {
            handles,
            names: entity_names,
            registry: names,
            highest_issued,
        }));
        let identities = entities.clone();
        let lookup = entities.clone();
        let resolve = move |value: Value| -> mlua::Result<usize> {
            let entities = identities
                .lock()
                .map_err(|_| mlua::Error::external("Entity registry lock poisoned"))?;
            match value {
                Value::LightUserData(token) => {
                    let id = entities.handles.resolve(token.0 as usize)
                        .ok_or_else(|| mlua::Error::external("Invalid or expired entity handle"))?;
                    Ok(id)
                }
                Value::String(name) => {
                    let name = name.to_str()?;
                    entities.registry.first(name.as_ref()).ok_or_else(|| {
                        mlua::Error::external(format!("Entity/group {name:?} not found"))
                    })
                }
                _ => Err(mlua::Error::external(
                    "Expected an entity handle or exact entity name",
                )),
            }
        };
        let sequence_queue = commands.clone();
        let sequence_resolve = resolve.clone();
        lua.globals().set("AnimGBSequence",lua.create_function(move |lua,args:mlua::MultiValue| {
            if !(2..=3).contains(&args.len()) {return Err(mlua::Error::external("AnimGBSequence requires an entity, a sequence and an optional end-event flag"))}
            let entity=sequence_resolve(args[0].clone())?;
            let Value::String(name)=&args[1] else{return Err(mlua::Error::external("Animation sequence name must be a string"))};
            let notify = args.get(2).cloned().map(|value| native_boolean(lua, value)).transpose()?.unwrap_or(false);
            sequence_queue.lock().map_err(|_|mlua::Error::external("Script command queue poisoned"))?.push(Command::Sequence{entity,name:name.to_str()?.to_owned(),notify});Ok(())
        })?)?;
        let reset_queue = commands.clone();
        let reset_resolve = resolve.clone();
        lua.globals().set(
            "AnimGBReset",
            lua.create_function(move |_, value: Value| {
                reset_queue
                    .lock()
                    .map_err(|_| mlua::Error::external("Script command queue poisoned"))?
                    .push(Command::Reset {
                        entity: reset_resolve(value)?,
                    });
                Ok(())
            })?,
        )?;
        for number in 1..=4 {
            let queue = commands.clone();
            let bindings = entities.clone();
            lua.globals().set(
                format!("FireUser{number}"),
                lua.create_function(move |lua, value: Value| {
                    // An authored target can be absent in the current visit's
                    // streamed layer. The native broadcast then has no source.
                    let entity = {
                        let entities = bindings.lock().map_err(|_| mlua::Error::external("Entity registry lock poisoned"))?;
                        match value {
                            Value::LightUserData(token) => entities.handles.resolve(token.0 as usize),
                            Value::String(_) | Value::Integer(_) | Value::Number(_) => {
                                let text = lua.coerce_string(value)?.ok_or_else(|| mlua::Error::external("Lua string conversion failed"))?;
                                let text = text.to_str()?;
                                entities.registry.first(text.split('\0').next().unwrap_or(""))
                            }
                            _ => None,
                        }
                    };
                    let Some(entity) = entity else { return Ok(()) };
                    queue
                        .lock()
                        .map_err(|_| mlua::Error::external("Script command queue poisoned"))?
                        .push(Command::Output {
                            entity,
                            name: format!("OnUser{number}"),
                        });
                    Ok(())
                })?,
            )?;
        }
        lua.globals().set(
            "GetEntity",
            lua.create_function(move |_, name: String| {
                let Some(id) = lookup
                    .lock()
                    .map_err(|_| mlua::Error::external("Entity registry lock poisoned"))?
                    .registry
                    .first(&name)
                else {
                    return Ok(Value::Nil);
                };
                Ok(Value::LightUserData(
                    handle(&lookup, id).map_err(mlua::Error::external)?,
                ))
            })?,
        )?;
        // REL26d96c: callback(entity, args[3..]) for every current group node;
        // discard callback results and return zero Lua values. The registry
        // lock must never span the callback, which can call back into World.
        let group_entities = entities.clone();
        lua.globals().set("ForEachEntityInGroup",lua.create_function(move |lua,args:mlua::MultiValue| {
            if args.len()<2 {return Err(mlua::Error::external("ForEachEntityInGroup needs callback and group; native argument diagnostics are not reconstructed"));}
            let group=lua.coerce_string(args[1].clone())?
                .ok_or_else(||mlua::Error::external("Invalid entity group name"))?;
            let group=group.to_str()?.split('\0').next().unwrap_or("").to_owned();
            let mut current=group_entities.lock().map_err(|_|mlua::Error::external("Entity registry lock poisoned"))?.registry.first_live(&group);
            while let Some(entity)=current {
                let Value::Function(callback)=&args[0] else{return Err(mlua::Error::external("Group callback must be a function"));};
                let mut call=mlua::MultiValue::new();
                call.push_back(Value::LightUserData(handle(&group_entities, entity).map_err(mlua::Error::external)?));
                call.extend(args.iter().skip(2).cloned());
                callback.call::<mlua::MultiValue>(call)?;
                current=group_entities.lock().map_err(|_|mlua::Error::external("Entity registry lock poisoned"))?
                    .registry.next(&group,entity).map_err(mlua::Error::external)?;
            }
            Ok(mlua::MultiValue::new())
        })?)?;
        let name_resolve = resolve.clone();
        let reverse = entities.clone();
        lua.globals().set(
            "GetName",
            lua.create_function(move |_, value: Value| {
                // REL269634 returns zero results when entity resolution fails.
                // Lua observes nil; other mutating APIs still reject bad handles.
                let id = name_resolve(value).ok();
                Ok(id.and_then(|id| {
                    reverse
                        .lock()
                        .expect("Entity registry lock poisoned")
                        .names
                        .get(&id)
                        .cloned()
                }))
            })?,
        )?;
        let prefab_data = Arc::new(Mutex::new(BTreeMap::<usize, mlua::Table>::new()));
        let prefab_tables = prefab_data.clone();
        let prefab_resolve = resolve.clone();
        lua.globals().set(
            "GetPrefabData",
            lua.create_function(move |_, value: Value| {
                let Some(entity) = prefab_resolve(value).ok() else {
                    return Ok(None);
                };
                Ok(prefab_tables
                    .lock()
                    .map_err(|_| mlua::Error::external("Prefab data lock poisoned"))?
                    .get(&entity)
                    .cloned())
            })?,
        )?;
        let player = Arc::new(Mutex::new(None));
        // REL269b30 requires zero arguments and pushes entity-manager +0x68
        // through REL278554: nil for ID zero, light userdata otherwise.
        for name in ["GetPlayer", "_player"] {
            let player = player.clone();
            let bindings = entities.clone();
            lua.globals().set(
                name,
                lua.create_function(move |_, args: mlua::MultiValue| {
                    if !args.is_empty() {
                        return Err(mlua::Error::external("GetPlayer takes no arguments"));
                    }
                    let id = *player
                        .lock()
                        .map_err(|_| mlua::Error::external("Player identity lock poisoned"))?;
                    match id {
                        Some(id) => Ok(Value::LightUserData(
                            handle(&bindings, id).map_err(mlua::Error::external)?,
                        )),
                        None => Ok(Value::Nil),
                    }
                })?,
            )?;
        }
        lua.globals().set(
            "Print",
            lua.create_function(|_, value: Value| {
                #[cfg(not(target_os = "horizon"))]
                eprintln!("[game Lua] {value:?}");
                Ok(())
            })?,
        )?;
        lua.load("function wait(seconds) assert(type(seconds)=='number' and seconds >= 0, 'Invalid wait duration'); return coroutine.yield(seconds) end; Wait = wait").exec()?;
        // Retail scripts yield until queued scene loads finish. Keep this as a
        // Lua coroutine so the world can publish one scene per normal tick.
        lua.load("function WaitForLevelToLoad(...) while BwxIsLevelLoadPending(...) do coroutine.yield(0) end end; function WaitForLevelLoad(...) return WaitForLevelToLoad(...) end").exec()?;
        let callback_context = Arc::new(Mutex::new(CallbackContext::default()));
        let spawned = Arc::new(Mutex::new(Spawned::default()));
        let pending = spawned.clone();
        let child_context = callback_context.clone();
        // REL26d8a4 ->275ee0 with delay zero: create and resume now, then
        // retain a yielded child. Its arguments exclude the function itself;
        // native execution context is inherited independently of arguments.
        lua.globals().set("FireThread",lua.create_function(move|lua,args:mlua::MultiValue| {
            if args.is_empty() {return Err(mlua::Error::external("FireThread needs a function; native argument diagnostics are not reconstructed"));}
            let context=*child_context.lock().map_err(|_|mlua::Error::external("Callback context lock poisoned"))?;
            let time=pending.lock().map_err(|_|mlua::Error::external("Spawned task queue poisoned"))?.time;
            let result=(||->mlua::Result<Option<Task>> {
                let Value::Function(function)=&args[0] else{return Err(mlua::Error::external("FireThread non-function/callable value is not reconstructed"));};
                let thread=lua.create_thread(function.clone())?;
                let arguments=mlua::MultiValue::from_vec(args.into_iter().skip(1).collect());
                let values=resume_context(&thread,&child_context,context,arguments)?;
                if thread.status()!=ThreadStatus::Resumable {return Ok(None);}
                let wait=delay(&values).map_err(mlua::Error::external)?;
                Ok(Some(Task{name:"FireThread".into(),thread,wake:time+wait,context}))
            })();
            let mut pending=pending.lock().map_err(|_|mlua::Error::external("Spawned task queue poisoned"))?;
            match result {
                Ok(Some(task))=>pending.tasks.push(task),Ok(None)=>{},
                // REL278064 logs resume errors and retires the child rather
                // than raising the error into the parent coroutine.
                Err(error)=>pending.errors.push(format!("FireThread: {error}")),
            }
            Ok(mlua::MultiValue::new())
        })?)?;
        // REL268cc4/268dcc read current script execution +0xc/+0x10,
        // then278554 pushes the stored opaque ID (nil only for ID zero).
        for name in ["_self", "_activator"] {
            let slot = callback_context.clone();
            let bindings = entities.clone();
            let activator = name == "_activator";
            lua.globals().set(
                name,
                lua.create_function(move |_, args: mlua::MultiValue| {
                    if !args.is_empty() {
                        return Err(mlua::Error::external(
                            "Callback context getters take no arguments",
                        ));
                    }
                    let context = *slot
                        .lock()
                        .map_err(|_| mlua::Error::external("Callback context lock poisoned"))?;
                    match if activator {
                        context.activator
                    } else {
                        context.entity
                    } {
                        Some(entity) => Ok(Value::LightUserData(
                            handle(&bindings, entity).map_err(mlua::Error::external)?,
                        )),
                        None => Ok(Value::Nil),
                    }
                })?,
            )?;
        }
        // Verified stimulus registration: REL28a5bc..28a818. Keep these
        // original IDs rather than conflating health damage with knockback.
        for (name, value) in [("ST_THINNER", 0), ("ST_PAINT", 1), ("ST_PHYSICAL", 2),
            ("ST_KNOCKBACK", 3), ("ST_BREAKABLE", 5), ("ST_SPIN", 8)] {
            lua.globals().set(name, value)?;
        }
        lua.globals().set(
            "StimulusEvent_HasStimulusType",
            lua.create_function(|_, (event, kind): (mlua::AnyUserData, u32)| {
                let event = event.borrow::<super::actor_io::EventPayload>()?;
                let super::actor_io::EventPayload::Combat(stimuli) = &*event else {
                    return Err(mlua::Error::external("Expected stimulus event"));
                };
                Ok(stimuli.iter().any(|&(id, _)| id == kind))
            })?,
        )?;
        // REL3230f8/32326c search the same ID array. The amount callback reads
        // the first matching effective value, or zero if the ID is absent.
        for (name, read_amount) in [
            ("DamageEvent_HasStimulusType", false),
            ("DamageEvent_EffectiveAmount", true),
        ] {
            lua.globals().set(name, lua.create_function(move |lua, args: mlua::MultiValue| {
                if args.len() != 2 {
                    return Err(mlua::Error::external(format!("{name} requires an event and stimulus type")));
                }
                let kind = super::score_state::integer_argument(lua, args[1].clone())? as u32;
                let mlua::Value::UserData(event) = &args[0] else {
                    return Err(mlua::Error::external("Expected damage event"));
                };
                let event = event.borrow::<super::actor_io::EventPayload>()?;
                let super::actor_io::EventPayload::Combat(stimuli) = &*event else {
                    return Err(mlua::Error::external("Expected damage event"));
                };
                let matched = stimuli.iter().find(|&&(id, _)| id == kind);
                Ok(if read_amount {
                    Value::Number(matched.map_or(0., |&(_, amount)| f64::from(amount)))
                } else {
                    Value::Boolean(matched.is_some())
                })
            })?)?;
        }
        // REL2f86b0 reads the completed sequence name from the event.
        lua.globals().set("AnimGBSequenceEndedName", lua.create_function(|lua, mut args: mlua::MultiValue| {
            let [Value::UserData(event)] = args.make_contiguous() else {
                return Err(mlua::Error::external("AnimGBSequenceEndedName requires an animation end event"));
            };
            let event = event.borrow::<super::actor_io::EventPayload>()?;
            let super::actor_io::EventPayload::AnimationEnded(name) = &*event else {
                return Err(mlua::Error::external("Expected animation end event"));
            };
            lua.create_string(name.as_bytes())
        })?)?;
        let source_files: Arc<Mutex<BTreeMap<String, Arc<[u8]>>>> = Default::default();
        let sources = source_files.clone();
        // Retail REL26cd5c returns the literal FINAL. This is the supported
        // retail runtime's build identity, not a host/debug-profile setting.
        lua.globals()
            .set("BuildConfig", lua.create_function(|_, ()| Ok("FINAL"))?)?;
        // REL26d6e0 loads the resource and executes it with zero arguments and
        // zero results. It inherits the current script context and does not
        // advance the coroutine scheduler or deduplicate repeated Openfile.
        lua.globals().set(
            "Openfile",
            lua.create_function(move |lua, path: String| {
                let key = script_resource_key(&path).map_err(mlua::Error::external)?;
                let bytes = sources
                    .lock()
                    .map_err(|_| mlua::Error::external("Script resource lock poisoned"))?
                    .get(&key)
                    .cloned()
                    .ok_or_else(|| {
                        mlua::Error::external(format!("Openfile resource is not imported: {path}"))
                    })?;
                let bytes = crate::lua_cook::prepare_host_chunk(lua, &bytes).map_err(mlua::Error::external)?;
                lua.load(&bytes).set_name(&path).exec()
            })?,
        )?;
        let mut scripts = Self {
            lua,
            tasks: Vec::new(),
            commands,
            errors: Vec::new(),
            time: 0.,
            time_origin: 0.,
            player,
            companion:Default::default(),
            entities,
            prefab_data,
            prefab_owners: Default::default(),
            retained_prefabs: Default::default(),
            world_host: Arc::new(Mutex::new(None)),
            scheduler_detached: false,
            callback_context,
            loaded_files: Default::default(),
            spawned,
            callouts: Default::default(),
            source_files,
        };
        let group_bindings = scripts.entities.clone();
        scripts.lua.globals().set("GetEntityGroupSize", scripts.lua.create_function(move |lua, args: mlua::MultiValue| {
            if args.len() != 1 { return Err(mlua::Error::external("GetEntityGroupSize requires one group name")); }
            let name = lua.coerce_string(args[0].clone())?;
            let count = if let Some(name) = name {
                let name = name.to_str()?;
                group_bindings.lock().map_err(|_|mlua::Error::external("Entity registry lock poisoned"))?
                    .registry.live_members(name.split('\0').next().unwrap_or("")).count()
            } else { 0 };
            Ok(count)
        })?)?;
        scripts.bind_world_api_with_recorder("AnimGBSequence")?;
        scripts.bind_world_api_with_recorder("AnimGBReset")?;
        scripts.bind_world_api("GetPosition")?;
        scripts.bind_world_api("GetTicketCount")?;
        scripts.bind_world_api("SetStayInCombatIdle")?;
        for name in ["PlayerHasSketchAbilities","PlayerHasSketchAbilitiesSD","PlayerHasPaintThinnerAbilities","PlayerHasPaintThinnerAbilitiesSD"] {scripts.bind_world_api(name)?;}
        for name in ["RelayEnable","RelayDisable","RelayToggle","RelayTrigger"] {scripts.bind_world_api(name)?;}
        scripts.lua.globals().set("OnRelayTrigger",0x4e)?;
        scripts.lua.globals().set("OnUse",0x0e)?;
        scripts.bind_world_api("SetVulnerability")?;
        scripts.bind_world_api("ResetTicketCount")?;
        scripts.bind_world_api("GetSketchCount")?;
        scripts.bind_world_api("SetFacing")?;
        scripts.bind_world_api("PaintThinnerCounterReset")?;
        scripts.bind_world_api("PaintThinnerCounter_GetStatus")?;
        scripts.bind_world_api("PaintThinnerCounter_GetPaintedPercentage")?;
        scripts.bind_world_api("GetUp")?;
        scripts.bind_world_api("GetLeft")?;
        scripts.bind_world_api("GetDistanceBetweenEntities")?;
        scripts.bind_world_api("EnableMovementInput")?;
        scripts.bind_world_api("DisableMovementInput")?;
        scripts.bind_world_api("Hide")?;
        scripts.bind_world_api("Unhide")?;
        scripts.bind_world_api("RemovePhysicsFromWorld")?;
        scripts.bind_world_api("EnableMotion")?;
        scripts.bind_world_api("SetNPCState")?;
        scripts.bind_world_api("AIBB_SetReevaluateRequired")?;
        scripts.bind_world_api("AI_ClearTarget")?;
        scripts.bind_world_api("DisableMotion")?;
        scripts.bind_world_api("EnableComponent")?;
        scripts.bind_world_api("DisableComponent")?;
        scripts.bind_world_api("IsHidden")?;
        scripts.bind_world_api("GetChildEntityByName")?;
        scripts.bind_world_api("GetPropertyFloat")?;
        scripts.bind_world_api("GetPropertyString")?;
        scripts.bind_world_api("GetPropertyInt")?;
        scripts.bind_world_api("GetPropertyBool")?;
        scripts.bind_world_api("SetPropertyBool")?;
        scripts.bind_world_api("SetPropertyFloat")?;
        scripts.bind_world_api("SplineKnotComponent_SetBranchProbability")?;
        scripts.bind_world_api("SetPropertyVector")?;
        scripts.bind_world_api("SetPropertyEntity")?;
        scripts.bind_world_api("SetPropertyString")?;
        scripts.bind_world_api("SetPropertyInt")?;
        scripts.bind_world_api("SetR3MTConstRegOnSubGraph")?;
        scripts.bind_world_api("SetR3MTColorReg")?;
        scripts.bind_world_api("SetR3MTConstReg")?;
        scripts.bind_world_api("SetR3MTLockAnimation")?;
        scripts.bind_world_api("SetR3MTUnlockAnimation")?;
        scripts.bind_world_api("StartEmitterByName")?;
        scripts.bind_world_api("StartEmitterByNameFireForget")?;
        scripts.bind_world_api("StopEmitterByName")?;
        scripts.bind_world_api("AudioPostEventOn")?;
        for name in ["SetCombatIntensityBias", "GetCombatIntensityBias", "CombatIntensity_SetBias", "CombatIntensity_GetBias"] {
            scripts.bind_world_api(name)?;
        }
        scripts.bind_world_api("Rumble")?;
        scripts.bind_world_api("ShakeCamera")?;
        scripts.bind_world_api("AudioSetSwitch")?;
        scripts.bind_world_api("MultiPosSoundStart")?;
        scripts.bind_world_api("MultiPosSoundStop")?;
        scripts.bind_world_api("SetMap")?;
        scripts.bind_world_api("PlayerHasUnlockedExtra")?;
        scripts.bind_world_api("SetUnlockedExtra")?;
        for name in [
            "HasCollectible",
            "GiveCollectible",
            "IncrementCollectibleCount",
        ] {
            scripts.bind_world_api(name)?;
        }
        scripts.bind_world_api("SetPlayerSpawnTransform")?;
        scripts.bind_world_api("GetPlaythrough")?;
        scripts.bind_world_api("SetPlaythrough")?;
        for name in ["AnimVarInt", "AnimVarFloat"] {
            scripts.bind_world_api(name)?;
        }
        for name in ["EnterCutscene", "ExitCutscene", "IsInCutscene"] {
            scripts.bind_world_api(name)?;
        }
        scripts.bind_world_api("SaveGameToMemory")?;
        scripts.bind_world_api("StoreEndgameData")?;
        scripts.bind_world_api("UpdatePins")?;
        scripts.bind_world_api("SetEndingGame")?;
        scripts.bind_world_api("GetWasEndGame")?;
        scripts.bind_world_api("GetWasQuitGame")?;
        scripts.bind_world_api("StartSequence")?;
        scripts.bind_world_api("CleanLoadLevel")?;
        scripts.bind_world_api("ResetSaveGameSlot")?;
        scripts.bind_world_api("LoadGameFromMemory")?;
        scripts.bind_world_api("RestartCurrentLevel")?;
        scripts.bind_world_api("SetPlayerSpawnTransformEntity")?;
        scripts.bind_world_api("SetDefaultCameraValues")?;
        for name in [
            "ResetDefaultCameraValues",
            "ResetCombatCameraValues",
            "ResetTravelCameraValues",
        ] {
            scripts.bind_world_api(name)?;
        }
        scripts.bind_world_api("SetCombatCameraValues")?;
        scripts.bind_world_api("SetTravelCameraValues")?;
        scripts.bind_world_api("SetDrawSilhouettes")?;
        for name in [
            "StartRotateToPosition",
            "StopRotateToPosition",
            "InstantRotateToPosition",
        ] {
            scripts.bind_world_api(name)?;
        }
        scripts.bind_world_api("SetLedgeGrabEnabled")?;
        scripts.bind_world_api("SetWallPushingEnabled")?;
        scripts.bind_world_api("GrabCameraNif")?;
        scripts.bind_world_api("GrabCamera")?;
        scripts.bind_world_api("SetLetterbox")?;
        scripts.bind_world_api("ToggleLetterbox")?;
        scripts.bind_world_api("DestroyEntity")?;
        scripts.bind_world_api("Enable")?;
        scripts.bind_world_api("Disable")?;
        let math = scripts.lua.globals().get::<mlua::Table>("math")?;
        for (method, api) in [
            ("random", "#bwx_math_random"),
            ("randomseed", "#bwx_math_randomseed"),
        ] {
            scripts.bind_world_api(api)?;
            math.set(method, scripts.lua.globals().get::<mlua::Function>(api)?)?;
            scripts.lua.globals().set(api, Value::Nil)?;
        }
        scripts.bind_world_api("AnimGBSetIdle")?;
        scripts.bind_world_api("PauseAllAI")?;
        scripts.bind_world_api("UnpauseAllAI")?;
        scripts.bind_world_api("MoveToEntity")?;
        // REL26f778 and26f7c8 register both names to callback26b1dc.
        scripts.bind_world_api("TeleportToEntity")?;
        scripts.bind_world_api("RestrictCutSceneInput")?;
        scripts.bind_world_api("UnrestrictCutSceneInput")?;
        scripts.bind_world_api("EnableGuardianSystem")?;
        scripts.bind_world_api("GetHeroPercent")?;
        scripts.bind_world_api("AddHeroPoints")?;
        scripts.bind_world_api("AddScrapperPoints")?;
        scripts.bind_world_api("GetScrapperPercent")?;
        scripts.bind_world_api("ActivateScene")?;
        scripts.bind_world_api("DisableGuardianSystem")?;
        scripts.bind_world_api("GetGlobal")?;
        scripts.bind_world_api("SetGlobal")?;
        scripts.bind_world_api("StartFadeOut")?;
        scripts.bind_world_api("StartFadeIn")?;
        scripts.bind_world_api("StartFadeInNoHUD")?;
        scripts.bind_world_api("GetRelativePrefabEntity")?;
        scripts.bind_world_api("DisableAbility_Player")?;
        scripts.bind_world_api("EnableAbility_Player")?;
        scripts.bind_world_api("StimulusSphereWithAlias")?;
        scripts.bind_world_api("StimulusEntityWithAliasCalculateDirectionInRadius")?;
        scripts.bind_world_api("StimulusEntityWithAliasInEntityDirectionInRadius")?;
        scripts.bind_world_api("GetConfigVariableFloat")?;
        scripts.bind_world_api("GetConfigVariableBool")?;
        scripts.bind_world_api("GetConfigVariableString")?;
        scripts.bind_world_api("SetShadowScale")?;
        scripts.bind_world_api("RestoreShadowScale")?;
        scripts.bind_world_api("SetHudFor2D")?;
        scripts.bind_world_api("ForcePainted")?;
        scripts.bind_world_api("ForceSketched")?;
        scripts.bind_world_api("GetPropertyEntity")?;
        scripts.bind_world_api("GetElementCount")?;
        scripts.bind_world_api("GetChildEntityByIndex")?;
        scripts.bind_world_api("SetParentEntity")?;
        scripts.bind_world_api("ClearParent")?;
        scripts.bind_world_api("SetTransformation")?;
        scripts.bind_world_api("AI_SetDisabled")?;
        scripts.bind_world_api("AIController_SetDisabled")?;
        scripts.bind_world_api("GetHealth")?;
        scripts.bind_world_api("Damage")?;
        scripts.bind_world_api("SetHealth")?;
        for api in ["TimerEnable","TimerDisable","TimerReset","TimerSetInterval"] {scripts.bind_world_api(api)?;}
        for api in ["TimerStart","TimerStop","TimerPause","TimerUnpause","TimerGetTime","TimerGetState","TimerAddTime","TimerSetTime"] {
            scripts.bind_world_api(api)?;
        }
        scripts.bind_world_api("GetFacing")?;
        scripts.bind_world_api("IsValidHandle")?;
        scripts.bind_world_api("IsAlive")?;
        scripts.bind_world_api("IsPlayer")?;
        scripts.bind_world_api("IsCharacter")?;
        // Explicit developer diagnostic; never substitute it for an authored
        // game callback or claim AI selection is connected by invoking it.
        scripts.bind_world_api("BwxNativeAnimationEvent")?;
        scripts.bind_world_api("AnimEvent")?;
        scripts.bind_world_api("PlayerPickup_SetPendingActivation")?;
        scripts.bind_world_api("ForceSpawn")?;
        scripts.bind_world_api("SetOverrideTurnRate")?;
        scripts.bind_world_api("ResetOverrideTurnRate")?;
        scripts.bind_world_api("RequestRobotArmChangeIdle")?;
        for name in [
            "SetRobotArmAttackTypeAngleInfo",
            "SetRobotArmAttackTypeCooldownInfo",
            "SetRobotArmAttackTypeDelayInfo",
            "SetRobotArmAttackTypeEnabled",
            "SetRobotArmSpotlightEntity",
            "SetRobotArmSpotlightDisabled",
            "SetRobotArmSpotlightStatus",
            "RequestRobotArmSmash",
            "RequestRobotArmElectrocute",
        ] {
            scripts.bind_world_api(name)?;
        }
        scripts.bind_world_api("BwxQueueMovie")?;
        scripts.bind_world_api("TransitionToLevel")?;
        scripts.bind_world_api("ForceSequence")?;
        scripts.bind_world_api("Bark")?;
        // Demo gameplay scripts are compiled into per-script environments
        // during level import. Bind these adapters before loading those
        // environments so their closures never capture nil API globals.
        for name in [
            "FireSequence",
            "ClearAllCameraAttributesTimed",
            "ReleaseCamera",
            "SetRotatorMaxSpeed",
            "ShowHud",
            "HideHud",
            "HideReticleOnly",
            "ShowReticleOnly",
            "PlayerSetDripsVisible",
            "SetSplineFollowerInitialSpeed",
            "SplineFollower_StopAtPosition",
            "SplineFollower_ClearStopPosition",
            "SetNextPatrolNode",
            "ClearNextPatrolNode",
            "ResumePatrol",
            "GetCurrentTeam",
            "CameraReset",
            "AudioSetState",
            "StartEmitters",
            "StopEmitters",
            "ForcePainted",
            "ForceThinned",
            "StartTrackingCurrentLevelForScoring",
            "EndTrackingCurrentLevelForScoring",
            "SetCharacterInConversation",
            "SetCameraAttributes",
            "SetCameraAttributesForPlayer",
            "ClearAllCameraAttributes",
            "ClearAllCameraAttributesSD",
            "ClearAllCameraAttributesTimedSD",
            "SaveCameraOverride",
            "SaveCurrentCameraOverride",
            "LoadSavedCameraOverride",
            "NativeAudioSetState",
            "NativeAudioSetSwitch",
            "CounterAdd",
            "CounterSubtract",
            "Reverse",
            "SetParentEntity",
            "ClearParent",
            "SetAppCulled",
            "ClearAppCulled",
            "SetSpeed",
            "SplineFollower_SetDisabled",
            "SplineFollower_TeleportToKnot",
        ] {
            scripts.bind_missing_world_api(name)?;
        }
        // Retail pairs use identical callbacks; Exists has the same resolved-ID
        // boolean result as IsValidHandle (26a2d0 vs26a008).
        for (alias, original) in [("EnableMotion_Player", "EnableMotion"),
            ("DisableMotion_Player", "DisableMotion"), ("Show", "Unhide"), ("Exists", "IsValidHandle"),
            ("GetPaladinPercent", "GetHeroPercent")] {
            scripts.lua.globals().set(alias, scripts.lua.globals().get::<mlua::Function>(original)?)?;
        }
        scripts
            .lua
            .load(
                r#"
            function PlayMovieInternal(...)
                local queued = BwxQueueMovie(...)
                if queued and coroutine.running() ~= nil then
                    coroutine.yield(0.10000000149011612)
                end
            end
        "#,
            )
            .set_name("native movie handoff")
            .exec()?;
        Ok(scripts)
    }
    /// Transfer coroutine ownership without creating another Lua VM. The
    /// original remains a lookup facade; it cannot run a competing scheduler.
    pub fn detach_scheduler(&mut self) -> Result<Self> {
        ensure!(
            !self.scheduler_detached,
            "Script scheduler already detached"
        );
        ensure!(
            self.world_host
                .lock()
                .map_err(|_| anyhow::anyhow!("World host lock poisoned"))?
                .is_none(),
            "Cannot detach an active host"
        );
        self.scheduler_detached = true;
        Ok(Self {
            lua: self.lua.clone(),
            tasks: std::mem::take(&mut self.tasks),
            commands: self.commands.clone(),
            errors: std::mem::take(&mut self.errors),
            time: self.time,
            time_origin: self.time_origin,
            player: self.player.clone(),
            companion: self.companion.clone(),
            prefab_owners: self.prefab_owners.clone(),
            retained_prefabs: self.retained_prefabs.clone(),
            entities: self.entities.clone(),
            prefab_data: self.prefab_data.clone(),
            world_host: self.world_host.clone(),
            scheduler_detached: false,
            callback_context: self.callback_context.clone(),
            loaded_files: self.loaded_files.clone(),
            spawned: self.spawned.clone(),
            callouts: self.callouts.clone(),
            source_files: self.source_files.clone(),
        })
    }
    pub fn restore_scheduler(&mut self, execution: Self) -> Result<()> {
        ensure!(
            self.scheduler_detached && !execution.scheduler_detached,
            "Invalid scheduler ownership restoration"
        );
        ensure!(
            Arc::ptr_eq(&self.world_host, &execution.world_host),
            "Scheduler belongs to a different Lua runtime"
        );
        ensure!(
            self.tasks.is_empty() && self.errors.is_empty(),
            "Lookup facade acquired execution state"
        );
        *self = execution;
        Ok(())
    }
    /// Persistent API indirection: cached Lua functions never retain the
    /// borrowed scoped host. Actual native handlers must be supplied separately.
    pub fn bind_world_api(&mut self, name: &str) -> Result<()> {
        ensure!(
            matches!(self.lua.globals().get::<Value>(name)?, Value::Nil),
            "World API already bound: {name}"
        );
        self.bind_world_api_inner(name, None)
    }
    /// Recovered PC native declarations and PlayerManager helpers (97973e..9797bd).
    /// Install before original global scripts enable strict global checking.
    pub(super) fn configure_em2_interface(&mut self) -> Result<()> {
        self.lua.globals().set("PLAYER_ONE", 0)?;
        self.lua.globals().set("PLAYER_TWO", 1)?;
        // Register the recovered PC SDK before strict.lua sees the globals.
        // Preserve existing implemented helpers. Unimplemented services retain
        // the common explicit world-host error, never an empty success stub.
        for name in include_str!("em2_native_api.txt").lines().map(str::trim)
            .filter(|name|!name.is_empty() && !name.starts_with('#')) {
            self.bind_missing_world_api(name)?;
        }
        // PC GetPlayer2 (978060), GetPlayerOrAI (9780d0) and
        // FindAIBuddy/GetOswaldAI (979590) share the native opaque identities.
        for name in ["GetPlayer2","_player2","GetPlayerOrAI","GetOswaldAI"] {
            let companion=self.companion.clone();let primary=self.player.clone();let entities=self.entities.clone();
            self.lua.globals().set(name,self.lua.create_function(move |lua,args:mlua::MultiValue| {
                let index=if name=="GetPlayerOrAI" {
                    <i32 as mlua::FromLua>::from_lua(args.front().cloned().unwrap_or(Value::Integer(0)),lua)?
                } else {1};
                if !(0..2).contains(&index) {return Err(mlua::Error::external("Unsupported player index"));}
                let secondary=*companion.lock().map_err(|_|mlua::Error::external("Companion lock poisoned"))?;
                let owner=if index==0 {*primary.lock().map_err(|_|mlua::Error::external("Player lock poisoned"))?}
                    else {secondary.filter(|binding|match name {"GetOswaldAI"=>binding.ai,"GetPlayer2"|"_player2"=>!binding.ai,_=>true}).map(|b|b.entity)};
                match owner {Some(entity)=>Ok(Value::LightUserData(handle(&entities,entity).map_err(mlua::Error::external)?)),None=>Ok(Value::Nil)}
            })?)?;
        }
        Ok(())
    }
    pub(super) fn bind_missing_world_api(&mut self, name: &str) -> Result<()> {
        if matches!(self.lua.globals().get::<Value>(name)?, Value::Nil) {
            self.bind_world_api(name)?;
        }
        Ok(())
    }
    pub(super) fn has_callable(&self, name: &str) -> Result<bool> {
        Ok(matches!(
            self.lua.globals().get::<Value>(name)?,
            Value::Function(_)
        ))
    }
    pub(super) fn schedule_retirement(&mut self, entity: usize) -> Result<()> {
        self.commands
            .lock()
            .map_err(|_| anyhow::anyhow!("Script command queue poisoned"))?
            .push(Command::Retire { entity });
        Ok(())
    }
    /// Standalone script inspection records commands without a World. Live
    /// gameplay always uses the current scoped host, synchronously. A registry
    /// key avoids a Rust Function -> Lua VM ownership cycle in the wrapper.
    fn bind_world_api_with_recorder(&mut self, name: &str) -> Result<()> {
        let recorder = self.lua.globals().get::<mlua::Function>(name)?;
        let key = self.lua.create_registry_value(recorder)?;
        self.bind_world_api_inner(name, Some(key))
    }
    fn bind_world_api_inner(
        &mut self,
        name: &str,
        recorder: Option<mlua::RegistryKey>,
    ) -> Result<()> {
        let slot = self.world_host.clone();
        let api = name.to_owned();
        let queue = self.callouts.clone();
        let contexts = self.callback_context.clone();
        let spawned = self.spawned.clone();
        let bindings = self.entities.clone();
        self.lua.globals().set(
            name,
            self.lua
                .create_function(move |lua, mut args: mlua::MultiValue| {
                    let function = slot
                        .lock()
                        .map_err(|_| mlua::Error::external("World host lock poisoned"))?
                        .clone();
                    let Some(function) = function else {
                        if let Some(key) = &recorder {
                            return lua
                                .registry_value::<mlua::Function>(key)?
                                .call::<mlua::MultiValue>(args);
                        }
                        return Err(mlua::Error::external("Native world host is not active"));
                    };
                    args.push_front(Value::String(lua.create_string(&api)?));
                    let result = function.call::<mlua::MultiValue>(args);
                    drain_callouts(lua, &function, &queue, &contexts, &spawned, &bindings)?;
                    result
                })?,
        )?;
        Ok(())
    }
    pub fn is_execution_detached(&self) -> bool {
        self.scheduler_detached
    }
    pub fn queue_callback(&mut self, name: &str, entity: usize) -> Result<()> {
        ensure!(
            self.scheduler_detached && self.has_entity(entity),
            "Callback callout requires a live detached owner"
        );
        self.callouts
            .lock()
            .map_err(|_| anyhow::anyhow!("Callout queue poisoned"))?
            .push(Callout::Script {
                name: name.into(),
                context: CallbackContext {
                    entity: Some(entity),
                    activator: None,
                },
                parameters: Vec::new(),
                nil_target: false,
            });
        Ok(())
    }
    pub fn queue_io_callback(
        &mut self,
        name: &str,
        context: CallbackContext,
        parameter: &str,
    ) -> Result<()> {
        self.queue_io_target(name, context, parameter, false, context.entity, None)
    }
    pub fn queue_missing_io_callback(
        &mut self,
        name: &str,
        context: CallbackContext,
        parameter: &str,
    ) -> Result<()> {
        self.queue_io_target(name, context, parameter, true, context.entity, None)
    }
    pub(super) fn queue_io_from(
        &mut self, name: &str, context: CallbackContext, parameter: &str,
        owner: usize, nil_target: bool, payload: Option<&super::actor_io::EventPayload>,
    ) -> Result<()> {
        self.queue_io_target(name, context, parameter, nil_target, Some(owner), payload)
    }
    fn queue_io_target(
        &mut self,
        name: &str,
        context: CallbackContext,
        parameter: &str,
        nil_target: bool,
        io_owner: Option<usize>,
        payload: Option<&super::actor_io::EventPayload>,
    ) -> Result<()> {
        ensure!(
            self.scheduler_detached && context.entity.is_some_and(|entity| self.has_entity(entity)),
            "IO callout requires a live detached owner"
        );
        let parameters = self.io_parameter_from(context, parameter, io_owner, payload)?;
        self.callouts
            .lock()
            .map_err(|_| anyhow::anyhow!("Callout queue poisoned"))?
            .push(Callout::Script {
                name: name.into(),
                context,
                parameters,
                nil_target,
            });
        Ok(())
    }
    pub fn queue_native_callout(&mut self, name: &str, args: mlua::MultiValue) -> Result<()> {
        ensure!(
            self.scheduler_detached,
            "Native callout requires a detached execution owner"
        );
        self.callouts
            .lock()
            .map_err(|_| anyhow::anyhow!("Callout queue poisoned"))?
            .push(Callout::Native {
                name: name.into(),
                args,
            });
        Ok(())
    }
    /// Safe synchronous host borrow for one script execution boundary. The
    /// permanent API above looks up this host anew after every coroutine wait.
    pub fn with_world_host<R>(
        &mut self,
        mut host: impl FnMut(&Lua, &str, mlua::MultiValue) -> mlua::Result<mlua::MultiValue>,
        run: impl FnOnce(&mut Self) -> Result<R>,
    ) -> Result<R> {
        struct Clear(Arc<Mutex<Option<mlua::Function>>>);
        impl Drop for Clear {
            fn drop(&mut self) {
                if let Ok(mut slot) = self.0.lock() {
                    *slot = None;
                }
            }
        }
        ensure!(
            self.world_host
                .lock()
                .map_err(|_| anyhow::anyhow!("World host lock poisoned"))?
                .is_none(),
            "World host already active"
        );
        let lua = self.lua.clone();
        let slot = self.world_host.clone();
        lua.scope(|scope| {
            let function = scope.create_function_mut(|lua, mut args: mlua::MultiValue| {
                let Some(Value::String(name)) = args.pop_front() else {
                    return Err(mlua::Error::external("Missing world API identity"));
                };
                host(lua, name.to_str()?.as_ref(), args)
            })?;
            *slot
                .lock()
                .map_err(|_| mlua::Error::external("World host lock poisoned"))? = Some(function);
            let _clear = Clear(slot.clone());
            Ok(run(self))
        })?
    }
    /// REL34de70 processes defaults then overrides, keeps first key order,
    /// and executes each final value as `return <expression>` in game Lua.
    /// REL34e91c retrieves the cached table; script writes must persist.
    pub fn prepare_prefab_data(
        &mut self,
        entity: usize,
        defaults: &[String],
        overrides: &[String],
    ) -> Result<()> {
        if self.retained_prefabs.lock().map_err(|_|anyhow::anyhow!("Retained prefab lock poisoned"))?.remove(&entity) {
            return Ok(());
        }
        let entity_name = self
            .entity_name(entity)
            .context("Prefab data owner is not registered")?;
        let mut keys = Vec::<String>::new();
        let mut expressions = BTreeMap::new();
        for row in defaults.iter().chain(overrides) {
            let (key, expression) = prefab_assignment(row)?;
            if !expressions.contains_key(&key) {
                keys.push(key.clone());
            }
            expressions.insert(key, expression);
        }
        let table = self.lua.create_table()?;
        for key in keys {
            let expression = &expressions[&key];
            let values: mlua::MultiValue = self
                .lua
                .load(format!("return {expression}"))
                .set_name(format!("@prefab {entity_name}.{key}"))
                .eval()
                .with_context(|| format!("Prefab value {entity_name}.{key}"))?;
            // Native initialization discards extra return values. No returned
            // value means no field; nil removes the field through Lua setfield.
            if let Some(value) = values.front() {
                table.raw_set(key, value.clone())?;
            }
        }
        let mut data = self
            .prefab_data
            .lock()
            .map_err(|_| anyhow::anyhow!("Prefab data lock poisoned"))?;
        ensure!(!data.contains_key(&entity), "Prefab data initialized twice");
        data.insert(entity, table);
        self.prefab_owners
            .lock()
            .map_err(|_| anyhow::anyhow!("Prefab ownership lock poisoned"))?
            .insert(entity, entity);
        Ok(())
    }
    pub fn bind_prefab_member(&mut self, member: usize, owner: usize) -> Result<()> {
        ensure!(self.has_entity(member), "Prefab member is not registered");
        let mut data = self
            .prefab_data
            .lock()
            .map_err(|_| anyhow::anyhow!("Prefab data lock poisoned"))?;
        let table = data
            .get(&owner)
            .context("Prefab owner has no initialized data")?
            .clone();
        if let Some(existing) = data.get(&member) {
            ensure!(existing.to_pointer()==table.to_pointer(),"Entity belongs to conflicting prefab data contexts; nested ownership requires resolution");
        } else {
            data.insert(member, table);
        }
        self.prefab_owners
            .lock()
            .map_err(|_| anyhow::anyhow!("Prefab ownership lock poisoned"))?
            .insert(member, owner);
        Ok(())
    }
    pub fn prefab_owner(&self, member: usize) -> Result<Option<usize>> {
        Ok(self
            .prefab_owners
            .lock()
            .map_err(|_| anyhow::anyhow!("Prefab ownership lock poisoned"))?
            .get(&member)
            .copied())
    }
    pub fn resolve_name(&self, name: &str) -> Option<usize> {
        self.entities
            .lock()
            .expect("Entity registry lock poisoned")
            .registry
            .first(name)
    }
    pub(super) fn set_scene_names_available(&mut self,entities:&[usize],available:bool)->Result<()> {
        let mut bindings=self.entities.lock().map_err(|_|anyhow::anyhow!("Entity registry lock poisoned"))?;
        if available{for &entity in entities{bindings.registry.resume(entity);}}
        else{bindings.registry.suspend_many(entities);}
        Ok(())
    }
    /// IO's _target_group snapshots names before any receiver executes
    /// (REL328aec onward), unlike Lua's live group iterators.
    pub fn group_entity_names(&self, group: &str) -> Result<Vec<String>> {
        let bindings = self
            .entities
            .lock()
            .map_err(|_| anyhow::anyhow!("Entity registry lock poisoned"))?;
        bindings
            .registry
            .live_members(group)
            .map(|entity| {
                bindings
                    .names
                    .get(&entity)
                    .cloned()
                    .context("Group member has no entity name")
            })
            .collect()
    }
    /// Native26b1dc pops optional values from the top before resolving target
    /// and source. Extra arguments are therefore not silently ignored.
    pub fn move_to_entity_request(
        &self,
        lua: &Lua,
        mut args: mlua::MultiValue,
    ) -> mlua::Result<Option<super::entity_relocation::Request>> {
        if args.len() < 2 {
            return Err(mlua::Error::external("MoveToEntity requires two entity arguments; native diagnostic text is not reconstructed"));
        }
        let count = args.len();
        let mut reset = false;
        let mut probe = true;
        if count > 3 {
            reset = native_boolean(lua, args.pop_back().unwrap())?;
        }
        if count > 2 {
            probe = native_boolean(lua, args.pop_back().unwrap())?;
        }
        let target = self.resolve_world_argument(&args.pop_back().unwrap())?;
        let entity = self.resolve_world_argument(&args.pop_back().unwrap())?;
        let (Some(entity), Some(target)) = (entity, target) else {
            return Ok(None);
        };
        if *self
            .player
            .lock()
            .map_err(|_| mlua::Error::external("Player identity lock poisoned"))?
            == Some(entity)
        {
            reset = true;
            probe = true;
        }
        Ok(Some(super::entity_relocation::Request {
            entity,
            target,
            reset,
            probe,
        }))
    }
    pub fn resolve_world_argument(&self, value: &Value) -> mlua::Result<Option<usize>> {
        match value {
            Value::Nil => Ok(None),
            Value::Integer(0) => Ok(self.resolve_name("0")),
            Value::Number(value) if *value == 0. => Ok(self.resolve_name("0")),
            Value::String(name) => Ok(self.resolve_name(name.to_str()?.as_ref())),
            Value::LightUserData(token) => Ok(self.resolve_token(token.0 as usize)),
            _ => Err(mlua::Error::external(
                "Native entity argument type is not implemented",
            )),
        }
    }
    /// Same dynamically scoped entity used by _self and native player-context APIs.
    pub(super) fn current_callback_context(&self)->Result<CallbackContext> {
        Ok(*self.callback_context.lock().map_err(|_|anyhow::anyhow!("Lua context lock poisoned"))?)
    }
    ///278bec/278b48: numbers are entity names, not indices. Other Lua values
    /// supply pointer identities; only a live entity handle resolves at123a38.
    pub fn resolve_native_entity_argument(
        &self,
        lua: &Lua,
        value: Value,
    ) -> mlua::Result<Option<usize>> {
        if matches!(
            value,
            Value::String(_) | Value::Integer(_) | Value::Number(_)
        ) {
            let text = lua
                .coerce_string(value)?
                .ok_or_else(|| mlua::Error::external("Native entity-name conversion failed"))?;
            let text = text.to_str()?;
            return Ok(self.resolve_name(text.split('\0').next().unwrap_or("")));
        }
        if let Value::LightUserData(token) = value {
            return Ok(self.resolve_token(token.0 as usize));
        }
        Ok(None)
    }
    /// REL184a08/184b74 read only the last optional argument through278bec.
    /// 278b48 uses lua_isstring (numbers included), resolving names to entity
    /// tokens. Other values use lua_topointer; nil/booleans yield null. The
    /// resulting word8 is opaque: do not reinterpret it as an integer option.
    pub fn action_event_header(
        &self,
        lua: &Lua,
        args: &mlua::MultiValue,
    ) -> mlua::Result<Option<usize>> {
        let Some(value) = args.back() else {
            return Ok(None);
        };
        if matches!(
            value,
            Value::String(_) | Value::Integer(_) | Value::Number(_)
        ) {
            let text = lua
                .coerce_string(value.clone())?
                .ok_or_else(|| mlua::Error::external("Native entity-name conversion failed"))?;
            let text = text.to_str()?;
            let name = text.split('\0').next().unwrap_or("");
            return self
                .resolve_name(name)
                .map(|entity| {
                    self.entity_token(entity).map_err(mlua::Error::external)
                })
                .transpose();
        }
        let pointer = value.to_pointer() as usize;
        Ok((pointer != 0).then_some(pointer))
    }
    pub(super) fn current_player_entity(&self) -> Result<Option<usize>> {
        Ok(*self.player.lock().map_err(|_|anyhow::anyhow!("Player identity lock poisoned"))?)
    }
    pub(super) fn set_em2_companion(&mut self,entity:usize,ai:bool)->Result<()> {
        ensure!(self.has_entity(entity),"Companion entity is not registered");
        *self.companion.lock().map_err(|_|anyhow::anyhow!("Companion lock poisoned"))?=Some(CompanionBinding {entity,ai});
        Ok(())
    }
    /// PlayerManager clears its slot before the old world entity is destroyed.
    /// Cached entity handles must remain valid until deferred world removal.
    pub(super) fn clear_em2_player_slot(&mut self, index: usize) -> Result<()> {
        match index {
            0 => *self.player.lock().map_err(|_|anyhow::anyhow!("Player lock poisoned"))?=None,
            1 => *self.companion.lock().map_err(|_|anyhow::anyhow!("Companion lock poisoned"))?=None,
            _ => anyhow::bail!("Invalid player slot"),
        }
        Ok(())
    }
    pub(super) fn companion_entity(&self,ai:Option<bool>)->Result<Option<usize>> {
        Ok(self.companion.lock().map_err(|_|anyhow::anyhow!("Companion lock poisoned"))?
            .filter(|binding|ai.is_none_or(|ai|binding.ai==ai)).map(|binding|binding.entity))
    }
    pub fn set_player(&mut self, entity: usize) -> Result<()> {
        ensure!(
            self.has_entity(entity),
            "Player entity is not registered with Lua"
        );
        *self
            .player
            .lock()
            .map_err(|_| anyhow::anyhow!("Player identity lock poisoned"))? = Some(entity);
        Ok(())
    }
    pub(super) fn resolve_token(&self, token:usize) -> Option<usize> {
        self.entities.lock().expect("Entity registry lock poisoned").handles.resolve(token)
    }
    pub(super) fn entity_token(&self, entity:usize) -> Result<usize> {
        Ok(handle(&self.entities,entity)?.0 as usize)
    }
    pub(super) fn entity_value(&self, entity:usize) -> mlua::Result<Value> {
        Ok(Value::LightUserData(handle(&self.entities,entity).map_err(mlua::Error::external)?))
    }
    pub fn load(&mut self, path: &str, bytes: &[u8]) -> Result<()> {
        ensure!(
            !self.scheduler_detached,
            "Script loading requires the execution owner"
        );
        let bytes = crate::lua_cook::prepare_host_chunk(&self.lua, bytes)?;
        let function = self
            .lua
            .load(&bytes)
            .set_name(path)
            .into_function()
            .with_context(|| format!("Loading original script {path}"))?;
        let thread = self.lua.create_thread(function)?;
        self.loaded_files
            .lock()
            .map_err(|_| anyhow::anyhow!("Script module registry lock poisoned"))?
            .insert(path.replace('\\', "/").to_ascii_lowercase());
        self.tasks.push(Task {
            name: path.into(),
            thread,
            wake: self.time,
            context: CallbackContext::default(),
        });
        Ok(())
    }
    pub fn import_source(&mut self, path: &str, bytes: &[u8]) -> Result<()> {
        self.source_files
            .lock()
            .map_err(|_| anyhow::anyhow!("Script resource lock poisoned"))?
            .entry(script_resource_key(path)?)
            .or_insert_with(|| Arc::from(bytes));
        Ok(())
    }
    pub(super) fn imported_source(&self, path: &str) -> Result<Option<Arc<[u8]>>> {
        Ok(self.source_files.lock()
            .map_err(|_| anyhow::anyhow!("Script resource lock poisoned"))?
            .get(&script_resource_key(path)?).cloned())
    }
    /// Lookup is safe on a nested World facade; execution remains owned by
    /// the active scheduler. Clones must not reload an already-live module.
    pub(super) fn module_loaded(&self, path: &str) -> Result<bool> {
        Ok(self.loaded_files.lock()
            .map_err(|_| anyhow::anyhow!("Script module registry lock poisoned"))?
            .contains(&script_resource_key(path)?))
    }
    /// REL2f07b0/278ee8 compiles the payload as Lua source, then schedules it
    /// with the originating entity context. It is not a function-name lookup
    /// or an initialization module, and repeated animation markers must run.
    pub fn schedule_animation_source(&mut self, source: &str, entity: usize) -> Result<()> {
        self.schedule_entity_source(source, entity, "animation")
    }
    pub(super) fn schedule_story_source(&mut self, source: &str, entity: usize) -> Result<()> {
        self.schedule_entity_source(source, entity, "BSQ")
    }
    pub(super) fn report_story_source_error(&mut self, error: anyhow::Error) -> Result<()> {
        //This facade may be detached during a World callout. Shared pending
        //diagnostics belong to the execution owner, never facade.errors.
        self.spawned
            .lock()
            .map_err(|_| anyhow::anyhow!("Spawned task queue poisoned"))?
            .errors
            .push(format!("BSQ Lua: {error:#}"));
        Ok(())
    }
    pub(super) fn schedule_checkpoint_callback(&mut self, callback: &str, entity: usize) -> Result<()> {
        self.lua.globals().get::<mlua::Function>(callback)
            .with_context(|| format!("Checkpoint callback {callback} is missing"))?;
        let quoted = serde_json::to_string(callback)?;
        self.schedule_entity_source(&format!("_G[{quoted}]()"), entity, "checkpoint")
    }
    fn schedule_entity_source(&mut self, source: &str, entity: usize, origin: &str) -> Result<()> {
        ensure!(
            self.has_entity(entity),
            "{origin} script owner is not registered"
        );
        let name = format!("@{origin} {}", self.entity_name(entity).unwrap());
        let function = self
            .lua
            .load(source)
            .set_name(&name)
            .into_function()
            .with_context(|| format!("Compile {origin} script for entity {entity}"))?;
        let thread = self.lua.create_thread(function)?;
        let mut pending = self
            .spawned
            .lock()
            .map_err(|_| anyhow::anyhow!("Spawned task queue poisoned"))?;
        let wake = pending.time;
        pending.tasks.push(Task {
            name,
            thread,
            wake,
            context: CallbackContext {
                entity: Some(entity),
                activator: None,
            },
        });
        Ok(())
    }
    /// Publish a newly encountered prefab's actual module before its spawn IO.
    /// Execute only this module, without advancing unrelated waiting scripts.
    pub fn ensure_module(&mut self, path: &str, bytes: &[u8], entity: usize) -> Result<()> {
        self.ensure_module_context(
            path,
            bytes,
            CallbackContext {
                entity: Some(entity),
                activator: None,
            },
        )
    }
    /// Definition files must execute before map-load callbacks reference them.
    /// Keep their global initialization context separate from entity callbacks.
    pub fn ensure_global_module(&mut self, path: &str, bytes: &[u8]) -> Result<()> {
        self.ensure_module_context(path, bytes, CallbackContext::default())
    }
    fn ensure_module_context(
        &mut self,
        path: &str,
        bytes: &[u8],
        context: CallbackContext,
    ) -> Result<()> {
        ensure!(
            !self.scheduler_detached,
            "Module loading requires the execution owner"
        );
        let key = script_resource_key(path)?;
        if self
            .loaded_files
            .lock()
            .map_err(|_| anyhow::anyhow!("Script module registry lock poisoned"))?
            .contains(&key)
        {
            return Ok(());
        }
        let bytes = crate::lua_cook::prepare_host_chunk(&self.lua, bytes)?;
        let function = self.lua.load(&bytes).set_name(path).into_function()?;
        let thread = self.lua.create_thread(function)?;
        let values = resume_context(&thread, &self.callback_context, context, ())?;
        if key.rsplit('/').next() == Some("gv_zonefx.lua") {
            // On a repeat return the authored callback skips the cinematic
            // that normally releases the projector's input restriction.
            // Select the projector's own non-cinematic exit for that case.
            self.lua.load(r#"
                local enter = GV_BigBen_IntroStart
                if enter then
                    function GV_BigBen_IntroStart(...)
                        if GV_BigBen_Intro ~= 0 then
                            local projector = GetPrefabData("GV_ZoneF_BigBenExitMarker")
                            if projector then projector.ToFlyThrough = "False" end
                        end
                        return enter(...)
                    end
                end
            "#).set_name("@zone-f-repeat-projector-return").exec()?;
        }
        if thread.status() == ThreadStatus::Resumable {
            self.tasks.push(Task {
                name: path.into(),
                thread,
                wake: self.time + delay(&values)?,
                context,
            });
        }
        self.loaded_files
            .lock()
            .map_err(|_| anyhow::anyhow!("Script module registry lock poisoned"))?
            .insert(key);
        Ok(())
    }
    pub fn evaluate(&mut self, source: &str) -> Result<()> {
        self.load("@runtime animation console", source.as_bytes())
    }
    /// PC af6170/90d4b0: preflight the exact global before the first '(';
    /// compile parenthesized source, otherwise pass the IGC instance's handle
    /// as one argument. Both branches start a coroutine immediately, with an
    /// empty execution context, then retain a yielded thread in this scheduler.
    pub(super) fn invoke_igc_callback(&mut self, source:&str, origin_token:usize)->Result<bool> {
        ensure!(!self.scheduler_detached,"IGC callbacks require the execution owner");
        let source=source.split('\0').next().unwrap_or_default();
        if source.is_empty(){return Ok(false);}
        let parenthesized=source.find('(');
        let name=parenthesized.map(|i|&source[..i]).unwrap_or(source);
        let Value::Function(global)=self.lua.globals().get::<Value>(name)? else{return Ok(false)};
        let (function,arguments)=if parenthesized.is_some() {
            (self.lua.load(source).set_name("@IGC Lua callback").into_function()?,mlua::MultiValue::new())
        }else{
            let origin=if origin_token==0 {Value::Nil}else{Value::LightUserData(LightUserData(origin_token as *mut std::ffi::c_void))};
            (global,mlua::MultiValue::from_vec(vec![origin]))
        };
        let thread=self.lua.create_thread(function)?;
        let context=CallbackContext::default();
        let values=resume_context(&thread,&self.callback_context,context,arguments)?;
        if thread.status()==ThreadStatus::Resumable {
            self.tasks.push(Task {name:format!("IGC Lua: {source}"),thread,wake:self.time+delay(&values)?,context});
        }
        Ok(true)
    }
    pub(super) fn bind_native_animation_constants(
        &self,
        catalog: &super::animation_events::Catalog,
    ) -> Result<()> {
        for (id, name) in catalog.entries() {
            self.lua.globals().set(name, id)?;
        }
        for (id, name) in catalog.variable_entries() {
            self.lua.globals().set(name, id)?;
        }
        Ok(())
    }
    /// Isolated native callback fixture; does not schedule or advance unrelated
    /// world coroutines. Gameplay always uses the normal scheduler.
    #[cfg(any(test, debug_assertions))]
    pub(super) fn evaluate_callback_fixture(&self, source: &str) -> Result<()> {
        self.lua
            .load(source)
            .set_name("@native callback fixture")
            .exec()?;
        Ok(())
    }
    /// Event dispatchers invoke an authored script function with the owning
    /// entity handle. Missing functions fail explicitly rather than becoming
    /// successful no-ops. Additional event payload ABI is not inferred here.
    pub fn call(&mut self, name: &str, entity: usize) -> Result<()> {
        self.call_input(name, entity, None)
    }
    pub fn call_input(&mut self, name: &str, entity: usize, parameter: Option<&str>) -> Result<()> {
        let parameter = parameter
            .map(|value| self.lua.create_string(value).map(Value::String))
            .transpose()?;
        self.call_context(
            name,
            CallbackContext {
                entity: Some(entity),
                activator: None,
            },
            parameter,
        )
    }
    pub fn call_io(&mut self, name: &str, context: CallbackContext, parameter: &str) -> Result<()> {
        self.call_context_target(name, context, self.io_parameter(context, parameter)?, false)
    }
    /// REL328e28..328e64 pushes nil when the snapshotted name is not in the
    /// entity map. The input still runs; its native binding owns null admission.
    pub fn call_missing_io(
        &mut self,
        name: &str,
        context: CallbackContext,
        parameter: &str,
    ) -> Result<()> {
        self.call_context_target(name, context, self.io_parameter(context, parameter)?, true)
    }
    pub(super) fn call_io_from(
        &mut self, name: &str, context: CallbackContext, parameter: &str,
        owner: usize, nil_target: bool, payload: Option<&super::actor_io::EventPayload>,
    ) -> Result<()> {
        let parameters = self.io_parameter_from(context, parameter, Some(owner), payload)?;
        self.call_context_target(name, context, parameters, nil_target)
    }
    fn io_parameter(&self, context: CallbackContext, parameter: &str) -> Result<Vec<Value>> {
        self.io_parameter_from(context, parameter, context.entity, None)
    }
    fn io_parameter_from(
        &self, context: CallbackContext, parameter: &str, owner: Option<usize>,
        payload: Option<&super::actor_io::EventPayload>,
    ) -> Result<Vec<Value>> {
        let mut event = None;
        super::actor_io::parameters(parameter).into_iter().map(|parameter| {
            Ok(match parameter.as_str() {
                "_activator" => match context.activator {
                    Some(entity) => Value::LightUserData(handle(&self.entities,entity)?),
                    None => Value::Nil,
                },
                "_self" => match owner {
                    Some(entity) => Value::LightUserData(handle(&self.entities,entity)?),
                    None => Value::Nil,
                },
                "_player" => match *self.player.lock().map_err(|_|anyhow::anyhow!("Player identity lock poisoned"))? {
                    Some(entity) => Value::LightUserData(handle(&self.entities,entity)?),
                    None => Value::Nil,
                },
                "_event" => {
                    if event.is_none() {
                        let payload = payload.context("IO _event has no supported payload")?;
                        event = Some(self.lua.create_userdata(payload.clone())?);
                    }
                    Value::UserData(event.clone().unwrap())
                }
                _ => Value::String(self.lua.create_string(parameter)?),
            })
        }).collect()
    }
    pub fn call_context(
        &mut self,
        name: &str,
        context: CallbackContext,
        parameter: Option<Value>,
    ) -> Result<()> {
        self.call_context_target(name, context, parameter.into_iter().collect(), false)
    }
    fn call_context_target(
        &mut self,
        name: &str,
        context: CallbackContext,
        mut parameters: Vec<Value>,
        nil_target: bool,
    ) -> Result<()> {
        ensure!(
            !self.scheduler_detached,
            "Script callback requires the execution owner"
        );
        let entity = context.entity.context("Callback needs an owning entity")?;
        let function: mlua::Function = self
            .lua
            .globals()
            .get(name)
            .with_context(|| format!("Missing authored script callback {name}"))?;
        let thread = self.lua.create_thread(function)?;
        ensure!(
            self.has_entity(entity),
            "Callback target is not registered with Lua"
        );
        let target = if nil_target {
            Value::Nil
        } else {
            Value::LightUserData(handle(&self.entities,entity)?)
        };
        parameters.insert(0,target);
        let result=resume_context(&thread,&self.callback_context,context,mlua::MultiValue::from_vec(parameters))?;
        if thread.status() == ThreadStatus::Resumable {
            self.tasks.push(Task {
                name: name.into(),
                thread,
                wake: self.time + delay(&result)?,
                context,
            });
        }
        Ok(())
    }
    pub fn advance(&mut self, time: f32) -> Result<Vec<Command>> {
        self.advance_active(time, |_| true)
    }
    pub fn advance_active(
        &mut self,
        time: f32,
        mut entity_active: impl FnMut(usize) -> bool,
    ) -> Result<Vec<Command>> {
        let time=self.time_origin+time;
        ensure!(!self.scheduler_detached, "Script scheduler is detached");
        ensure!(
            time.is_finite() && time >= self.time,
            "Script time must be finite and monotonic"
        );
        {
            let mut pending = self
                .spawned
                .lock()
                .map_err(|_| anyhow::anyhow!("Spawned task queue poisoned"))?;
            // A child appended during the update receives this update's delta
            // on its first visit (native275ba4). Its absolute deadline is
            // therefore relative to the previous update, not the new time.
            pending.time = self.time;
            self.tasks.append(&mut pending.tasks);
            self.errors.append(&mut pending.errors);
        }
        self.time = time;
        // Native275cdc rereads the live count after every iteration.275db0
        // swaps the last task into a retired slot; the loop still increments
        // its index, leaving that replacement for the next update.
        let mut index = 0;
        while index < self.tasks.len() {
            if self.tasks[index]
                .context
                .entity
                .is_some_and(|entity| !entity_active(entity))
            {
                // Scene suspension pauses the coroutine's wait clock. It will
                // resume on the first scheduler tick after its scene returns.
                self.tasks[index].wake = time;
                index += 1;
                continue;
            }
            if time < self.tasks[index].wake {
                index += 1;
                continue;
            }
            let task = &self.tasks[index];
            let result = resume_context(&task.thread, &self.callback_context, task.context, ());
            {
                let mut pending = self
                    .spawned
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Spawned task queue poisoned"))?;
                self.tasks.append(&mut pending.tasks);
                self.errors.append(&mut pending.errors);
            }
            let name = &self.tasks[index].name;
            let wait = match result {
                Ok(values) if self.tasks[index].thread.status() == ThreadStatus::Resumable => {
                    match delay(&values) {
                        Ok(delay) => Some(delay),
                        Err(error) => {
                            self.errors.push(format!("{name}: {error:#}"));
                            None
                        }
                    }
                }
                Ok(_) => None,
                Err(error) => {
                    self.errors.push(format!("{name}: {error}"));
                    None
                }
            };
            if let Some(wait) = wait {
                self.tasks[index].wake = time + wait;
            } else {
                self.tasks.swap_remove(index);
            }
            index += 1;
        }
        {
            let mut pending = self
                .spawned
                .lock()
                .map_err(|_| anyhow::anyhow!("Spawned task queue poisoned"))?;
            pending.time = time;
            self.errors.append(&mut pending.errors);
        }
        Ok(std::mem::take(&mut *self.commands.lock().map_err(
            |_| anyhow::anyhow!("Script command queue poisoned"),
        )?))
    }
    /// Compact scheduler state for progression failures and runtime diagnostics.
    /// Waiting coroutines otherwise look indistinguishable from callbacks that
    /// never started, which makes authored cutscene stalls hard to localize.
    pub fn task_snapshot(&self) -> serde_json::Value {
        fn describe(task: &Task, time: f32) -> serde_json::Value {
            serde_json::json!({"name":task.name,"wake_in":(task.wake-time).max(0.),
                "status":format!("{:?}",task.thread.status()),"entity":task.context.entity,
                "activator":task.context.activator})
        }
        let spawned = self.spawned.lock().ok();
        let errors=self.errors.iter().chain(spawned.as_ref().into_iter().flat_map(|state|state.errors.iter())).cloned().collect::<Vec<_>>();
        serde_json::json!({"time":self.time,
            "active":self.tasks.iter().map(|entry|describe(entry,self.time)).collect::<Vec<_>>(),
            "deferred":spawned.as_ref().map(|state|state.tasks.iter().map(|entry|describe(entry,self.time)).collect::<Vec<_>>()).unwrap_or_default(),
            "errors":errors})
    }
}

/// REL278564 numeric conversion precedes string true/false comparison and
/// Lua truthiness. It differs from Lua truthiness for zero and "false".
pub(super) fn native_boolean(lua: &Lua, value: Value) -> mlua::Result<bool> {
    if let Some(number) = lua.coerce_number(value.clone())? {
        return Ok(number != 0.0);
    }
    if let Value::String(text) = &value {
        let bytes = text.as_bytes();
        let text = bytes.split(|b| *b == 0).next().unwrap_or(&[]);
        if text.eq_ignore_ascii_case(b"true") {
            return Ok(true);
        }
        if text.eq_ignore_ascii_case(b"false") {
            return Ok(false);
        }
    }
    Ok(!matches!(value, Value::Nil | Value::Boolean(false)))
}
/// Tokens are opaque registry identities, never addresses to dereference.
/// Null is reserved for absent entities; no caller may decode a world index.
fn handle(bindings: &Arc<Mutex<EntityBindings>>, entity: usize) -> Result<LightUserData> {
    let token=bindings.lock().map_err(|_|anyhow::anyhow!("Entity registry lock poisoned"))?
        .handles.token(entity).context("Entity handle is not live")?;
    Ok(LightUserData(token as *mut std::ffi::c_void))
}
fn prefab_assignment(row: &str) -> Result<(String, String)> {
    // Retail scanner accepts ASCII letters/digits/underscore, spaces/tabs
    // before/after the key and '=', then preserves the value's trailing bytes.
    let row = row.trim_start_matches([' ', '\t']);
    let length = row
        .bytes()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == b'_')
        .count();
    ensure!(length > 0, "Malformed prefab setting {row:?}: missing key");
    let key = &row[..length];
    let rest = row[length..].trim_start_matches([' ', '\t']);
    let expression = rest
        .strip_prefix('=')
        .context("Malformed prefab setting: expected '='")?
        .trim_start_matches([' ', '\t']);
    ensure!(
        !expression.is_empty() && !expression.starts_with('='),
        "Malformed prefab value for {key}"
    );
    Ok((key.into(), expression.into()))
}
fn delay(values: &mlua::MultiValue) -> Result<f32> {
    let value = match values.front() {
        Some(Value::Number(n)) => *n as f32,
        Some(Value::Integer(n)) => *n as f32,
        _ => anyhow::bail!("Unsupported coroutine yield; expected wait duration"),
    };
    ensure!(
        value.is_finite() && value >= 0.,
        "Invalid coroutine wait duration"
    );
    Ok(value)
}
fn script_resource_key(path: &str) -> Result<String> {
    Ok(super::resource_path::canonical(path)?.to_ascii_lowercase())
}
#[cfg(test)]
mod tests {
    #[test]
    fn streamed_player_alias_reuses_the_existing_owner_without_issuing_a_handle()->anyhow::Result<()> {
        use super::*;
        let mut scripts=Scripts::new(&[(0,"Mickey".into()),(4,"NPC".into())])?;
        scripts.register_existing_alias(0,"incoming.Player 01")?;
        assert_eq!(scripts.resolve_name("incoming.Player 01"),Some(0));
        assert_eq!(scripts.resolve_name("Mickey"),Some(0));
        scripts.register_entity(5,"New NPC",&[])?;
        assert!(scripts.register_existing_alias(100,"invalid").is_err());
        assert_eq!(scripts.resolve_name("invalid"),None);
        Ok(())
    }

    #[test]
    fn native_entity_argument_uses_names_for_numbers_and_only_live_handles_for_pointers(
    ) -> anyhow::Result<()> {
        use super::*;
        let scripts = Scripts::new(&[(0, "Mickey".into()), (5, "2".into())])?;
        let lua = Lua::new();
        assert_eq!(
            scripts.resolve_native_entity_argument(&lua, Value::Integer(2))?,
            Some(5)
        );
        assert_eq!(
            scripts.resolve_native_entity_argument(&lua, Value::Integer(5))?,
            None
        );
        assert_eq!(
            scripts.resolve_native_entity_argument(
                &lua,
                Value::String(lua.create_string("Mickey\0tail")?)
            )?,
            Some(0)
        );
        assert_eq!(
            scripts.resolve_native_entity_argument(
                &lua,
                Value::LightUserData(LightUserData(1 as *mut std::ffi::c_void))
            )?,
            Some(0)
        );
        for value in [
            Value::Nil,
            Value::Boolean(true),
            Value::Table(lua.create_table()?),
            Value::LightUserData(LightUserData(999 as *mut std::ffi::c_void)),
        ] {
            assert_eq!(scripts.resolve_native_entity_argument(&lua, value)?, None);
        }
        Ok(())
    }
    #[test]
    fn resumed_coroutine_errors_remain_visible_after_scheduler_restoration()->anyhow::Result<()> {
        let mut scripts=Scripts::new(&[(0,"Actor".into())])?;
        scripts.ensure_global_module("failure.lua",b"function Broken() Wait(0.1); error('retained coroutine failure') end")?;
        scripts.call("Broken",0)?;
        let mut execution=scripts.detach_scheduler()?;
        execution.advance(0.2)?;
        scripts.restore_scheduler(execution)?;
        let snapshot=scripts.task_snapshot();
        assert!(snapshot["active"].as_array().unwrap().is_empty());
        assert_eq!(snapshot["errors"].as_array().unwrap().len(),1);
        assert!(snapshot["errors"][0].as_str().unwrap().contains("retained coroutine failure"));
        Ok(())
    }
    #[test]
    fn cached_sequence_calls_use_current_world_host_synchronously_across_yields(
    ) -> anyhow::Result<()> {
        use super::*;
        let mut scripts = Scripts::new(&[(0, "Actor".into())])?;
        scripts.evaluate("local sequence=AnimGBSequence; sequence('Actor', 'Rest'); assert(ImmediateCalls==1); Wait(0.1); sequence('Actor', 'Spawn'); assert(ImmediateCalls==2)")?;
        let mut calls = vec![];
        for time in [0., 0.1] {
            let commands = scripts.with_world_host(
                |lua, name, args| {
                    assert_eq!(name, "AnimGBSequence");
                    calls.push(args[1].as_string().unwrap().to_str()?.to_owned());
                    lua.globals().set("ImmediateCalls", calls.len())?;
                    Ok(mlua::MultiValue::new())
                },
                |scripts| scripts.advance(time),
            )?;
            assert!(
                commands.is_empty(),
                "Live requests went through the standalone recorder"
            );
        }
        assert_eq!(calls, ["Rest", "Spawn"]);
        assert!(scripts.errors.is_empty(), "{:?}", scripts.errors);
        Ok(())
    }
    #[test]
    fn action_header_resolves_names_and_preserves_pointer_identity_instead_of_integer_conversion(
    ) -> anyhow::Result<()> {
        use super::*;
        let scripts = Scripts::new(&[(0, "Mickey".into()), (4, "2".into())])?;
        let lua = Lua::new();
        let args = |value| mlua::MultiValue::from_vec(vec![Value::Integer(999), value]);
        assert_eq!(
            scripts.action_event_header(&lua, &mlua::MultiValue::new())?,
            None
        );
        assert_eq!(
            scripts.action_event_header(&lua, &args(Value::Boolean(true)))?,
            None
        );
        assert_eq!(
            scripts.action_event_header(
                &lua,
                &args(Value::String(lua.create_string("Mickey\0ignored")?))
            )?,
            Some(1)
        );
        assert_eq!(
            scripts.action_event_header(&lua, &args(Value::Integer(2)))?,
            Some(5)
        );
        assert_eq!(
            scripts.action_event_header(&lua, &args(Value::Integer(999)))?,
            None
        );
        assert_eq!(
            scripts.action_event_header(
                &lua,
                &args(Value::LightUserData(LightUserData(
                    123 as *mut std::ffi::c_void
                )))
            )?,
            Some(123)
        );
        let table = Value::Table(lua.create_table()?);
        assert_eq!(
            scripts.action_event_header(&lua, &args(table.clone()))?,
            Some(table.to_pointer() as usize)
        );
        Ok(())
    }
    #[test]
    fn openfile_executes_imported_resources_in_order_with_inherited_context() -> anyhow::Result<()>
    {
        use super::*;
        let mut scripts = Scripts::new(&[(0, "owner".into())])?;
        scripts.import_source(
            "Scripts/Child.lua",
            b"assert(BuildConfig()=='FINAL'); OpenVisits=OpenVisits+1; OpenOwner=GetName(_self())",
        )?;
        scripts.ensure_global_module("@openfile-definitions",b"OpenVisits=0; function OpenTest(self) Openfile('./scripts/CHILD.lua'); Openfile('Scripts\\\\Child.lua'); Openfile('Scripts/Sub/../Child.lua'); assert(OpenVisits==3); assert(OpenOwner==GetName(self)) end")?;
        scripts.ensure_global_module("Scripts/Child.lua", b"ModuleVisits=1")?;
        scripts.ensure_global_module(
            "Palettes/../Scripts/Child.lua",
            b"error('Duplicate module alias executed')",
        )?;
        scripts.call("OpenTest", 0)?;
        assert!(scripts
            .ensure_global_module("@missing-openfile", b"Openfile('missing.lua')")
            .is_err());
        assert!(scripts.import_source("../outside.lua", b"").is_err());
        Ok(())
    }
    #[test]
    fn animation_source_is_scheduled_repeatedly_with_owner_context_across_waits(
    ) -> anyhow::Result<()> {
        use super::*;
        let mut scripts = Scripts::new(&[(0, "first".into()), (1, "second".into())])?;
        scripts.ensure_global_module("@animation-source-test",b"AnimationVisits={}; function ObserveMarker() local owner=GetName(_self()); table.insert(AnimationVisits,owner); Wait(0); assert(GetName(_self())==owner); table.insert(AnimationVisits,owner..':done') end")?;
        scripts.schedule_animation_source("ObserveMarker()", 0)?;
        scripts.schedule_animation_source("ObserveMarker()", 1)?;
        scripts.ensure_global_module(
            "@before-animation-scheduler",
            b"assert(#AnimationVisits==0)",
        )?;
        scripts.advance(0.)?;
        scripts.advance(0.)?;
        scripts.advance(0.)?;
        scripts.ensure_global_module("@after-animation-scheduler",b"assert(#AnimationVisits==4); assert(AnimationVisits[1]=='first'); assert(AnimationVisits[2]=='second')")?;
        assert!(scripts
            .schedule_animation_source("not valid Lua !", 0)
            .is_err());
        assert!(scripts
            .schedule_animation_source("ObserveMarker()", 3)
            .is_err());
        assert!(scripts.errors.is_empty(), "{:?}", scripts.errors);
        Ok(())
    }
    #[test]
    fn story_compile_errors_belong_to_execution_owner_during_world_callouts() -> anyhow::Result<()>
    {
        let mut facade = Scripts::new(&[(0, "owner".into())])?;
        facade.bind_world_api("QueueStoryCompileFailure")?;
        let mut execution = facade.detach_scheduler()?;
        execution.with_world_host(
            |_, name, _| {
                assert_eq!(name, "QueueStoryCompileFailure");
                let error = facade
                    .schedule_story_source("not valid Lua !", 0)
                    .unwrap_err();
                facade
                    .report_story_source_error(error)
                    .map_err(mlua::Error::external)?;
                Ok(mlua::MultiValue::new())
            },
            |scripts| scripts.evaluate_callback_fixture("QueueStoryCompileFailure()"),
        )?;
        assert!(facade.errors.is_empty());
        facade.restore_scheduler(execution)?;
        facade.advance(0.)?;
        assert_eq!(facade.errors.len(), 1);
        assert!(facade.errors[0].contains("BSQ Lua"));
        Ok(())
    }
    #[test]
    fn native_callouts_allow_reentrant_world_reads_before_parent_continuations_and_resume_waits(
    ) -> anyhow::Result<()> {
        use super::*;
        let mut scripts = Scripts::new(&[(0, "owner".into()), (1, "activator".into())])?;
        for name in ["ProbeOuter", "ProbeRead"] {
            scripts.bind_world_api(name)?;
        }
        let queue = scripts.callouts.clone();
        let mut noise = 0;
        let mut trace = Vec::new();
        scripts.with_world_host(|lua,name,_| {
            match name {
                "ProbeOuter"=>{
                    trace.push("outer");
                    let mut work=queue.lock().unwrap();
                    work.push(Callout::Script{name:"BwxNestedObserver".into(),context:CallbackContext{entity:Some(0),activator:Some(1)},parameters:vec![Value::String(lua.create_string("Variant4")?)],nil_target:false});
                    work.push(Callout::Native{name:"#probe_noise".into(),args:mlua::MultiValue::new()});
                },
                "ProbeRead"=>{trace.push("read");return Ok(mlua::MultiValue::from_vec(vec![Value::Integer(noise)]));},
                "#probe_noise"=>{trace.push("noise");noise+=1;},
                _=>return Err(mlua::Error::external("Unknown test host operation")),
            }
            Ok(mlua::MultiValue::new())
        },|scripts| {
            scripts.ensure_global_module("@test/reentrant-callout",b"function BwxNestedObserver(self, variant) assert(GetName(self)=='owner'); assert(_self()==self); assert(variant=='Variant4'); assert(GetName(_activator())=='activator'); assert(ProbeRead()==0); Wait(0); assert(ProbeRead()==1); assert(_self()==self); assert(GetName(_activator())=='activator'); BwxNestedDone=true end; ProbeOuter()")?;
            assert!(matches!(scripts.lua.globals().get::<Value>("BwxNestedDone")?,Value::Nil));
            scripts.advance(0.)?;
            assert!(scripts.lua.globals().get::<bool>("BwxNestedDone")?);
            assert!(scripts.errors.is_empty(),"{:?}",scripts.errors);Ok(())
        })?;
        assert_eq!(trace, ["outer", "read", "noise", "read"]);
        assert_eq!(noise, 1);
        Ok(())
    }
    #[test]
    fn prefab_module_loads_once_without_advancing_unrelated_waits() -> anyhow::Result<()> {
        let mut scripts = super::Scripts::new(&[(0, "anvil".into()), (1, "other".into())])?;
        scripts.evaluate("waiting=0; wait(0); waiting=waiting+1")?;
        scripts.advance(0.)?;
        let module=b"assert(GetName(_self())=='anvil'); moduleLoads=(moduleLoads or 0)+1; function moduleCallback(e) assert(e==_self()) end";
        scripts.ensure_module("Scripts/Sketch.lua", module, 0)?;
        scripts.ensure_module("scripts\\sketch.lua", module, 1)?;
        assert_eq!(scripts.lua.globals().get::<i32>("moduleLoads")?, 1);
        assert_eq!(scripts.lua.globals().get::<i32>("waiting")?, 0);
        scripts.call_input("moduleCallback", 1, None)?;
        scripts.advance(0.)?;
        assert_eq!(scripts.lua.globals().get::<i32>("waiting")?, 1);
        anyhow::ensure!(
            scripts.errors.is_empty(),
            "Module import disturbed script execution: {:?}",
            scripts.errors
        );
        Ok(())
    }
    #[test]
    fn callback_owner_and_activator_survive_interleaved_waits_and_errors() -> anyhow::Result<()> {
        let mut scripts = super::Scripts::new(&[(0, "left".into()), (1, "right".into())])?;
        scripts.evaluate("assert(_self()==nil and _activator()==nil); completed=0; function observe(e,a) assert(e==_self() and a==_activator()); local self=_self; local act=_activator; wait(0); assert(self()==e and act()==a); completed=completed+1 end; function bad(e) assert(e==_self()); error('expected') end")?;
        scripts.advance(0.)?;
        for (entity, activator) in [(0, 1), (1, 0)] {
            scripts.call_io(
                "observe",
                super::CallbackContext {
                    entity: Some(entity),
                    activator: Some(activator),
                },
                "_activator",
            )?;
        }
        assert!(scripts
            .call_io(
                "bad",
                super::CallbackContext {
                    entity: Some(0),
                    activator: Some(1)
                },
                ""
            )
            .is_err());
        scripts.evaluate("assert(_self()==nil and _activator()==nil)")?;
        scripts.advance(0.)?;
        scripts.evaluate("assert(completed==2); assert(_self()==nil and _activator()==nil)")?;
        scripts.advance(0.)?;
        anyhow::ensure!(
            scripts.errors.is_empty(),
            "Context leaked between coroutines: {:?}",
            scripts.errors
        );
        Ok(())
    }
    #[test]
    fn waiting_lua_observes_spawns_retirement_and_replacement_without_rebinding(
    ) -> anyhow::Result<()> {
        let mut scripts =
            super::Scripts::with_aliases(&[(0, "mickey".into())], &[(0, "actors".into())])?;
        scripts.set_player(0)?;
        scripts.evaluate("local lookup=GetEntity; assert(lookup('anvil')==nil); wait(1); saved=lookup('anvil'); assert(saved~=nil and GetName(saved)=='anvil'); assert(GetPrefabData(saved).tag=='original'); AnimGBSequence(saved,'Spawn'); wait(1); assert(lookup('anvil')==nil); assert(GetName(saved)==nil and GetPrefabData(saved)==nil); assert(not pcall(AnimGBReset,saved)); wait(1); assert(lookup('anvil')~=nil and lookup('anvil')~=saved); assert(GetName(saved)==nil); AnimGBReset(lookup('anvil'))")?;
        assert!(scripts.advance(0.)?.is_empty());
        // Register through the World-side facade while coroutine ownership is
        // detached, as synchronous factory calls from game Lua will do.
        let execution = scripts.detach_scheduler()?;
        scripts.register_entity(1, "anvil", &["actors".into()])?;
        scripts.prepare_prefab_data(1, &["tag='original'".into()], &[])?;
        scripts.restore_scheduler(execution)?;
        assert_eq!(scripts.resolve_name("actors"), Some(0));
        let commands = scripts.advance(1.)?;
        assert!(
            matches!(commands.as_slice(),[super::Command::Sequence{entity:1,name,..}] if name=="Spawn")
        );
        scripts.unregister_entity(1)?;
        assert!(scripts.register_entity(1, "replacement", &[]).is_err());
        assert!(scripts.advance(2.)?.is_empty());
        scripts.register_entity(2, "anvil", &["actors".into()])?;
        scripts.unregister_entity(0)?;
        assert_eq!(scripts.resolve_name("actors"), Some(2));
        let commands = scripts.advance(3.)?;
        assert!(matches!(
            commands.as_slice(),
            [super::Command::Reset { entity: 2 }]
        ));
        assert!(scripts.errors.is_empty(), "{:?}", scripts.errors);
        scripts.evaluate("assert(GetPlayer()==nil)")?;
        scripts.advance(3.)?;
        assert!(scripts.errors.is_empty(), "{:?}", scripts.errors);
        Ok(())
    }
    #[test]
    fn move_to_entity_decodes_native_stack_flags_player_override_and_nulls() -> anyhow::Result<()> {
        let mut scripts = super::Scripts::new(&[
            (0, "mickey".into()),
            (1, "pickup".into()),
            (2, "marker".into()),
        ])?;
        scripts.set_player(0)?;
        let request =
            |text: &str| -> anyhow::Result<Option<super::super::entity_relocation::Request>> {
                let args = scripts.lua.load(text).eval::<mlua::MultiValue>()?;
                Ok(scripts.move_to_entity_request(&scripts.lua, args)?)
            };
        use super::super::entity_relocation::Request;
        assert_eq!(
            request("return 'pickup','marker'")?,
            Some(Request {
                entity: 1,
                target: 2,
                probe: true,
                reset: false
            })
        );
        assert_eq!(
            request("return 'pickup','marker',false,true")?,
            Some(Request {
                entity: 1,
                target: 2,
                probe: false,
                reset: true
            })
        );
        assert_eq!(
            request("return GetPlayer(),'marker',false,false")?,
            Some(Request {
                entity: 0,
                target: 2,
                probe: true,
                reset: true
            })
        );
        assert_eq!(request("return nil,'marker'")?, None);
        assert_eq!(request("return 'pickup','missing'")?, None);
        assert_eq!(
            request("return 'ignored','pickup','marker',false,true")?,
            Some(Request {
                entity: 1,
                target: 2,
                probe: false,
                reset: true
            })
        );
        assert!(request("return 'pickup'").is_err());
        for (value, expected) in [
            ("0", false),
            ("-0.0", false),
            ("'0'", false),
            ("'1'", true),
            ("'FALSE'", false),
            ("'TrUe'", true),
            ("nil", false),
            ("false", false),
            ("{}", true),
            ("''", true),
            ("'false\\0suffix'", false),
            ("0/0", true),
        ] {
            let value = scripts
                .lua
                .load(format!("return {value}"))
                .eval::<mlua::Value>()?;
            assert_eq!(super::native_boolean(&scripts.lua, value)?, expected);
        }
        // Cached world function remains usable across scheduler boundaries.
        scripts.evaluate("local move=MoveToEntity; move(nil,'marker'); wait(0); move('pickup','marker',false,true)")?;
        let mut calls = 0;
        for time in [0., 1.] {
            scripts.with_world_host(
                |_, name, _| {
                    assert_eq!(name, "MoveToEntity");
                    calls += 1;
                    Ok(mlua::MultiValue::new())
                },
                |scripts| {
                    scripts.advance(time)?;
                    Ok(())
                },
            )?;
        }
        assert_eq!(calls, 2);
        assert!(scripts.errors.is_empty(), "{:?}", scripts.errors);
        Ok(())
    }
    #[test]
    fn scheduler_transfer_retains_vm_coroutines_and_blocks_second_executor() -> anyhow::Result<()> {
        let mut original = super::Scripts::new(&[(0, "owner".into())])?;
        original.evaluate("counter=1; wait(.5); counter=counter+1; AnimGBReset('owner')")?;
        let mut executing = original.detach_scheduler()?;
        assert!(original.advance(0.).is_err());
        assert!(original.detach_scheduler().is_err());
        assert_eq!(original.resolve_name("owner"), Some(0));
        assert!(executing.advance(0.)?.is_empty());
        original.restore_scheduler(executing)?;
        assert_eq!(original.lua.globals().get::<i32>("counter")?, 1);
        assert!(original.advance(0.25)?.is_empty());
        assert!(matches!(
            original.advance(0.5)?.as_slice(),
            [super::Command::Reset { entity: 0 }]
        ));
        assert_eq!(original.lua.globals().get::<i32>("counter")?, 2);
        Ok(())
    }
    #[test]
    fn world_host_is_released_after_handler_error_run_error_and_panic() -> anyhow::Result<()> {
        let mut scripts = super::Scripts::new(&[])?;
        scripts.bind_world_api("ProbeGet")?;
        scripts.lua.load("cached_probe=ProbeGet").exec()?;
        let error = scripts.with_world_host(
            |_, _, _| Err(mlua::Error::external("Unsupported native operation")),
            |s| {
                s.lua.load("cached_probe()").exec()?;
                Ok(())
            },
        );
        assert!(error.is_err());
        assert!(scripts.world_host.lock().unwrap().is_none());
        let error: anyhow::Result<()> = scripts.with_world_host(
            |_, _, _| Ok(mlua::MultiValue::new()),
            |_| anyhow::bail!("Execution failed"),
        );
        assert!(error.is_err());
        assert!(scripts.world_host.lock().unwrap().is_none());
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _: anyhow::Result<()> = scripts.with_world_host(
                |_, _, _| Ok(mlua::MultiValue::new()),
                |_| panic!("Execution panic"),
            );
        }));
        assert!(panic.is_err());
        assert!(scripts.world_host.lock().unwrap().is_none());
        scripts.with_world_host(
            |_, _, _| Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Integer(11)])),
            |s| {
                s.lua.load("assert(cached_probe()==11)").exec()?;
                Ok(())
            },
        )?;
        assert!(scripts.world_host.lock().unwrap().is_none());
        Ok(())
    }
    #[test]
    fn overlapping_world_host_rejection_preserves_outer_host() -> anyhow::Result<()> {
        let mut scripts = super::Scripts::new(&[])?;
        scripts.bind_world_api("ProbeGet")?;
        assert!(scripts.bind_world_api("ProbeGet").is_err());
        scripts.with_world_host(
            |_, _, _| Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Integer(5)])),
            |s| {
                let nested: anyhow::Result<()> =
                    s.with_world_host(|_, _, _| panic!("Rejected host"), |_| Ok(()));
                assert!(nested.is_err());
                s.lua.load("assert(ProbeGet()==5)").exec()?;
                Ok(())
            },
        )?;
        Ok(())
    }
    #[test]
    fn persistent_world_api_observes_synchronous_mutations_across_waits() -> anyhow::Result<()> {
        let mut scripts = super::Scripts::new(&[])?;
        scripts.bind_world_api("ProbeSet")?;
        scripts.bind_world_api("ProbeGet")?;
        scripts.evaluate("local set=ProbeSet; local get=ProbeGet; set(7); assert(get()==7); wait(0); set(get()+2); assert(get()==9)")?;
        let mut state = 0i64;
        for time in [0., 1.] {
            scripts.with_world_host(
                |_, name, args| {
                    if name == "ProbeSet" {
                        let Some(mlua::Value::Integer(value)) = args.front() else {
                            return Err(mlua::Error::external("Expected integer"));
                        };
                        state = *value;
                        Ok(mlua::MultiValue::new())
                    } else {
                        Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Integer(
                            state,
                        )]))
                    }
                },
                |scripts| {
                    scripts.advance(time)?;
                    Ok(())
                },
            )?;
            assert!(scripts.errors.is_empty(), "{:?}", scripts.errors);
        }
        assert_eq!(state, 9);
        scripts.evaluate("ProbeGet()")?;
        scripts.advance(2.)?;
        assert!(scripts
            .errors
            .iter()
            .any(|e| e.contains("world host is not active")));
        Ok(())
    }
    use super::*;
    #[test]
    fn prefab_data_preserves_lua_types_overrides_and_shared_script_writes() -> Result<()> {
        let mut scripts = Scripts::with_aliases(
            &[
                (0, "projector".into()),
                (1, "marker".into()),
                (2, "child".into()),
            ],
            &[(2, "ProjectorStart".into())],
        )?;
        scripts.prepare_prefab_data(
            0,
            &[
                "MapLoad_SetupPosition = \"False\"".into(),
                "transitionTime = 4".into(),
                "flag = false".into(),
                "nested = { enabled = true }".into(),
                "obsolete = 2".into(),
                "multi = 3, 7".into(),
            ],
            &["transitionTime = .8".into(), "obsolete = nil".into()],
        )?;
        scripts.bind_prefab_member(2, 0)?;
        let chunk=crate::lua_cook::compile("local data = GetPrefabData('ProjectorStart'); assert(data == GetPrefabData(GetEntity('projector'))); assert(data.MapLoad_SetupPosition == 'False'); assert(type(data.transitionTime) == 'number' and data.transitionTime == .8); assert(data.flag == false and data.nested.enabled); assert(data.obsolete == nil and data.multi == 3); data.MapLoad_SetupPosition = 'True'; wait(.1); assert(GetPrefabData('projector').MapLoad_SetupPosition == 'True'); assert(GetPrefabData('marker') == nil and GetPrefabData('missing') == nil); AnimGBSequence('projector','Open')")?;
        scripts.load("original-prefab-semantics.lua", &chunk)?;
        assert!(scripts.advance(0.)?.is_empty());
        assert!(
            matches!(&scripts.advance(0.1)?[..],[Command::Sequence{entity:0,name,..}] if name=="Open")
        );
        assert!(scripts.errors.is_empty(), "{:?}", scripts.errors);
        Ok(())
    }
    #[test]
    fn named_group_selects_first_registered_member_in_native_bytecode() -> Result<()> {
        let mut scripts = Scripts::with_aliases(
            &[(17, "door-a".into()), (2, "door-b".into())],
            &[
                (17, "doors".into()),
                (2, "doors".into()),
                (17, "doors".into()),
            ],
        )?;
        let chunk = crate::lua_cook::compile(
            "assert(GetEntity('doors') == GetEntity('door-a')); AnimGBSequence('doors', 'Open')",
        )?;
        scripts.load("ordered-groups.lua", &chunk)?;
        assert!(
            matches!(&scripts.advance(0.)?[..],[Command::Sequence{entity:17,name,..}] if name=="Open")
        );
        assert!(scripts.errors.is_empty());
        assert_eq!(scripts.resolve_name("doors"), Some(17));
        Ok(())
    }
    #[test]
    fn io_group_name_snapshot_is_separate_from_live_registry_iteration() -> Result<()> {
        let mut scripts = Scripts::with_aliases(
            &[(17, "door-a".into()), (2, "door-b".into())],
            &[
                (17, "doors".into()),
                (2, "doors".into()),
                (17, "doors".into()),
            ],
        )?;
        let snapshot = scripts.group_entity_names("doors")?;
        assert_eq!(snapshot, ["door-a", "door-b"]);
        scripts.unregister_entity(2)?;
        scripts.register_entity(18, "door-c", &["doors".into()])?;
        assert_eq!(snapshot, ["door-a", "door-b"]);
        assert_eq!(scripts.resolve_name(&snapshot[1]), None);
        assert_eq!(scripts.group_entity_names("doors")?, ["door-a", "door-c"]);
        assert!(scripts.group_entity_names("absent").unwrap().is_empty());
        Ok(())
    }
    #[test]
    fn native_group_callbacks_keep_order_arguments_context_and_live_appends() -> Result<()> {
        let mut scripts = Scripts::with_aliases(
            &[(17, "door-a".into()), (2, "door-b".into())],
            &[
                (17, "doors".into()),
                (2, "doors".into()),
                (17, "doors".into()),
            ],
        )?;
        let bindings = scripts.entities.clone();
        scripts.lua.globals().set(
            "AppendDoor",
            scripts.lua.create_function(move |_, ()| {
                let mut bindings = bindings
                    .lock()
                    .map_err(|_| mlua::Error::external("Registry poisoned"))?;
                bindings.handles.register(18).map_err(mlua::Error::external)?;
                bindings.names.insert(18, "door-c".into());
                bindings.registry.register("door-c", 18);
                bindings.registry.register("doors", 18);
                bindings.highest_issued = Some(18);
                Ok(())
            })?,
        )?;
        let chunk = crate::lua_cook::compile(
            r#"
            local names={}
            function VisitDoors()
                local count=select('#',ForEachEntityInGroup(function(entity,a,b,c)
                    assert(a=='extra' and b==nil and c==42)
                    assert(GetName(_self())=='door-a')
                    names[#names+1]=GetName(entity)
                    if #names==1 then AppendDoor() end
                    FireUser1(entity)
                    return 'ignored'
                end,'doors','extra',nil,42))
                assert(count==0 and table.concat(names,',')=='door-a,door-b,door-c')
                ForEachEntityInGroup(function() error('must not run') end,'missing')
                ForEachEntityInGroup(nil,'missing')
                local visited=0
                local ok=pcall(ForEachEntityInGroup,function(entity)
                    visited=visited+1;error('stop traversal')
                end,'doors')
                assert(not ok and visited==1)
            end
        "#,
        )?;
        scripts.load("group-callbacks.lua", &chunk)?;
        scripts.advance(0.)?;
        scripts.call("VisitDoors", 17)?;
        let commands = scripts.advance(0.)?;
        assert_eq!(commands.len(), 3);
        for (command, expected) in commands.iter().zip([17, 2, 18]) {
            assert!(
                matches!(command,Command::Output{entity,name} if *entity==expected && name=="OnUser1")
            );
        }
        assert!(scripts.errors.is_empty(), "{:?}", scripts.errors);
        Ok(())
    }
    #[test]
    fn fire_thread_runs_immediately_inherits_context_waits_and_isolates_errors() -> Result<()> {
        let mut scripts = Scripts::new(&[(0, "owner".into()), (1, "argument".into())])?;
        let chunk = crate::lua_cook::compile(
            r#"
            order={}
            function Parent(entity)
                order[#order+1]='parent-start'
                assert(select('#',FireThread(function(a,b,c)
                    assert(a==GetEntity('argument') and b==nil and c==42)
                    assert(_self()==entity)
                    order[#order+1]='child-start'
                    FireThread(function()
                        assert(_self()==entity);order[#order+1]='nested-start'
                        Wait(0);order[#order+1]='nested-end'
                    end)
                    Wait(.25)
                    assert(_self()==entity);order[#order+1]='child-end';FireUser1(a)
                end,GetEntity('argument'),nil,42))==0)
                assert(table.concat(order,',')=='parent-start,child-start,nested-start')
                FireThread(function() error('intentional child failure') end)
                FireUser2(entity)
                order[#order+1]='parent-end'
            end
        "#,
        )?;
        scripts.load("thread-source.lua", &chunk)?;
        scripts.advance(0.)?;
        scripts.call("Parent", 0)?;
        // Also exercise the live-world scheduler transfer used by API calls.
        let mut execution = scripts.detach_scheduler()?;
        let commands = execution.advance(0.)?;
        assert!(matches!(&commands[..],[Command::Output{entity:0,name}] if name=="OnUser2"));
        assert_eq!(execution.errors.len(), 1);
        assert!(execution.errors[0].contains("intentional child failure"));
        let order: mlua::Table = execution.lua.globals().get("order")?;
        assert_eq!(
            order
                .sequence_values::<String>()
                .collect::<mlua::Result<Vec<_>>>()?,
            [
                "parent-start",
                "child-start",
                "nested-start",
                "parent-end",
                "nested-end"
            ]
        );
        assert!(execution.advance(0.24)?.is_empty());
        let commands = execution.advance(0.25)?;
        assert!(matches!(&commands[..],[Command::Output{entity:1,name}] if name=="OnUser1"));
        scripts.restore_scheduler(execution)?;
        assert!(scripts.tasks.is_empty());
        Ok(())
    }
    #[test]
    fn fire_thread_pending_children_are_retired_with_the_world() -> Result<()> {
        let scripts = Scripts::new(&[])?;
        scripts
            .lua
            .load("FireThread(function() Wait(100) end)")
            .exec()?;
        let pending = scripts.spawned.clone();
        assert_eq!(pending.lock().unwrap().tasks.len(), 1);
        drop(scripts);
        assert!(pending.lock().unwrap().tasks.is_empty());
        Ok(())
    }
    #[test]
    fn fire_thread_update_visits_live_appends_and_skips_swapped_replacement() -> Result<()> {
        let mut scripts = Scripts::new(&[])?;
        scripts.load(
            "append-during-update.lua",
            br#"
            order={}
            FireThread(function() order[#order+1]='a-start';Wait(.1);order[#order+1]='a-end' end)
            FireThread(function() order[#order+1]='b-start';Wait(.1);order[#order+1]='b-end' end)
        "#,
        )?;
        scripts.advance(0.1)?;
        let order: mlua::Table = scripts.lua.globals().get("order")?;
        assert_eq!(
            order
                .sequence_values::<String>()
                .collect::<mlua::Result<Vec<_>>>()?,
            ["a-start", "b-start", "a-end"]
        );
        scripts.advance(0.1)?;
        let order: mlua::Table = scripts.lua.globals().get("order")?;
        assert_eq!(
            order
                .sequence_values::<String>()
                .collect::<mlua::Result<Vec<_>>>()?,
            ["a-start", "b-start", "a-end", "b-end"]
        );
        assert!(scripts.tasks.is_empty() && scripts.errors.is_empty());
        Ok(())
    }
    #[test]
    fn retail_entity_handles_have_userdata_identity_and_nil_lookup() -> Result<()> {
        let mut scripts = Scripts::new(&[(0, "mickey".into()), (17, "pipe".into())])?;
        scripts.set_player(0)?;
        let bytecode=crate::lua_cook::compile("assert(type(GetPlayer()) == 'userdata'); assert(GetPlayer() == _player()); assert(GetPlayer() == GetEntity('mickey')); assert(GetName(GetPlayer()) == 'mickey'); assert(GetEntity('missing') == nil); AnimGBSequence(GetEntity('pipe'), 'Spawn')")?;
        scripts.load("entity-abi.lua", &bytecode)?;
        assert!(
            matches!(&scripts.advance(0.)?[..],[Command::Sequence{entity:17,name,..}] if name=="Spawn")
        );
        assert!(scripts.errors.is_empty(), "{:?}", scripts.errors);
        Ok(())
    }
    #[test]
    fn numeric_indices_are_not_native_entity_handles() -> Result<()> {
        let mut scripts = Scripts::new(&[(0, "mickey".into())])?;
        scripts.evaluate("AnimGBSequence(0, 'Idle')")?;
        assert!(scripts.advance(0.)?.is_empty());
        assert_eq!(scripts.errors.len(), 1);
        Ok(())
    }
    #[test]
    fn missing_entity_name_is_nil_without_aborting_original_scripts() -> Result<()> {
        let mut scripts = Scripts::new(&[(0, "mickey".into())])?;
        let chunk=crate::lua_cook::compile("assert(GetName(GetEntity('missing')) == nil); assert(GetName('missing') == nil); assert(GetName(GetEntity('mickey')) == 'mickey'); AnimGBSequence('mickey', 'Idle')")?;
        scripts.load("nullable-name.lua", &chunk)?;
        assert!(
            matches!(&scripts.advance(0.)?[..],[Command::Sequence{entity:0,name,..}] if name=="Idle")
        );
        assert!(scripts.errors.is_empty(), "{:?}", scripts.errors);
        Ok(())
    }
    #[test]
    fn original_bytecode_runs_and_waits_without_decompilation() -> Result<()> {
        let mut scripts = Scripts::new(&[(0, "pipe".into())])?;
        let bytecode = crate::lua_cook::compile(
            "AnimGBSequence(GetEntity('pipe'), 'Rest'); wait(0.1); AnimGBSequence('pipe', 'Spawn')",
        )?;
        scripts.load("game.lua", &bytecode)?;
        assert!(matches!(&scripts.advance(0.)?[..],[Command::Sequence{name,..}] if name=="Rest"));
        assert!(scripts.advance(0.05)?.is_empty());
        assert!(matches!(&scripts.advance(0.1)?[..],[Command::Sequence{name,..}] if name=="Spawn"));
        assert!(scripts.errors.is_empty());
        Ok(())
    }
    #[test]
    fn unsupported_native_calls_are_reported() -> Result<()> {
        let mut scripts = Scripts::new(&[])?;
        scripts.evaluate("MissingGameAPI()")?;
        scripts.advance(0.)?;
        assert_eq!(scripts.errors.len(), 1);
        Ok(())
    }
    #[test]
    #[ignore = "requires original game files in BLOTWORX_TEST_DATA"]
    fn original_zonea_callback_reaches_animation_bridge() -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let pack = crate::pack_io::read_filtered(
            std::fs::read(std::path::Path::new(&root).join("packfiles/gremlinvillage_zonea.pak"))?,
            |_, path| path.to_lowercase().ends_with("/gv_zonea.lua"),
        )?;
        let file = pack
            .pak
            .files
            .iter()
            .find(|file| !file.data.is_empty())
            .context("Missing original GV_ZoneA.lua")?;
        let mut scripts = Scripts::new(&[(0, "gremlinvillage_zonea.Steam_Blocker_01 01".into())])?;
        scripts.load(&file.path, &file.data)?;
        assert!(scripts.advance(0.)?.is_empty());
        scripts.call("GV_ZoneA_SteamFixing01", 0)?;
        assert!(
            matches!(&scripts.advance(0.)?[..],[Command::Sequence{entity:0,name,..}] if name=="rest")
        );
        assert!(scripts.errors.is_empty());
        Ok(())
    }
}

#[cfg(test)]
mod native_io_arguments_tests {
    use super::*;
    #[test]
    fn all_callbacks_receive_split_parameters_without_demo_overloads() -> Result<()> {
        let mut facade=Scripts::new(&[(0,"owner".into()),(1,"target".into()),(2,"player".into())])?;
        facade.set_player(2)?;
        facade.bind_world_api("QueueParameters")?;
        facade.load("io-parameters.lua",br#"
            seen=0
            function Observe(target,a,b,c,d,e)
              assert(GetName(target)=='target')
              assert(a=='literal,comma' and b==nil and GetName(c)=='owner')
              assert(GetName(d)=='player' and e==' tail ')
              assert(select('#',target,a,b,c,d,e)==6)
              seen=seen+1
              Wait(0); assert(GetName(_self())=='target'); seen=seen+1
            end
            function Direct(target,value,...)
              assert(GetName(target)=='target' and value=='literal,comma')
              assert(select('#',...)==0)
            end
            function Invoke() QueueParameters() end
        "#)?;
        facade.advance(0.)?;
        facade.call_input("Direct",1,Some("literal,comma"))?;
        let context=CallbackContext{entity:Some(1),activator:None};
        let parameters=r"literal\,comma,_activator,_self,_player, tail ";
        facade.call_io_from("Observe",context,parameters,0,false,None)?;
        facade.advance(0.1)?;
        facade.evaluate_callback_fixture("assert(seen==2)")?;
        let mut execution=facade.detach_scheduler()?;
        execution.with_world_host(|_,name,_| {
            assert_eq!(name,"QueueParameters");
            facade.queue_io_from("Observe",context,parameters,0,false,None).map_err(mlua::Error::external)?;
            Ok(mlua::MultiValue::new())
        },|scripts|scripts.call("Invoke",0))?;
        execution.advance(0.2)?;
        facade.restore_scheduler(execution)?;
        facade.evaluate_callback_fixture("assert(seen==4)")?;
        assert!(facade.errors.is_empty());
        Ok(())
    }
}
