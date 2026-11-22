//! Source-facing prefab documents. Runtime IDs are resolved in each destination;
//! no source scene's numeric pointers are copied into another scene.
use super::*;
use epic_mickey_lib_rs::scene_file::{Entity, ID};
use std::collections::{BTreeSet, HashMap, VecDeque};
pub const INDEX_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PrefabEdit {
    pub identity: String,
    pub name: String,
    pub edits: Vec<FieldEdit>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FieldEdit {
    pub pointer: String,
    pub before: Value,
    pub after: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variable: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Definition {
    pub index: usize,
    pub identity: String,
    #[serde(default)]
    pub assembly_ready: bool,
}

fn symbols(scene: &SceneFile) -> HashMap<u32, Value> {
    let mut out = HashMap::new();
    let mut names = HashMap::<&str, usize>::new();
    for e in &scene.objects {
        *names.entry(&e.name).or_default() += 1;
    }
    for e in &scene.objects {
        // Duplicate names cannot be addressed safely as a shared source asset.
        let duplicate = names[e.name.as_str()] > 1;
        let owner = if duplicate {
            format!("{}#{}", e.name, e.link_id.to_u32())
        } else {
            e.name.clone()
        };
        out.insert(e.link_id.to_u32(), serde_json::json!({"entity":owner}));
        let mut occurrences = BTreeMap::new();
        for c in &e.components {
            let n = occurrences
                .entry((&c.class_name, &c.name))
                .or_insert(0usize);
            out.insert(
                c.link_id.to_u32(),
                serde_json::json!({"entity":owner,"component":[c.class_name,c.name,*n]}),
            );
            *n += 1;
        }
    }
    out
}
fn encode(v: &mut Value, ids: &HashMap<u32, Value>) {
    if let Some(a) = v.as_array_mut() {
        for v in a {
            encode(v, ids);
        }
    } else if let Some(id) = v.as_u64().and_then(|n| u32::try_from(n).ok()) {
        if id != 0 && id != u32::MAX {
            *v = ids
                .get(&id)
                .cloned()
                .unwrap_or_else(|| serde_json::json!({"unresolved":id}));
        }
    }
}
fn document_with(scene: &SceneFile, index: usize, ids: &HashMap<u32, Value>) -> Value {
    let mut doc = scene.objects[index].to_dict(scene.version.clone());
    doc.as_object_mut().unwrap().remove("link_id");
    if let Some(v) = doc.get_mut("master_link_id") {
        encode(v, ids);
    }
    for c in doc["components"].as_array_mut().unwrap() {
        c.as_object_mut().unwrap().remove("link_id");
        if let Some(v) = c.get_mut("master_link_id") {
            encode(v, ids);
        }
        for p in c["properties"].as_array_mut().unwrap() {
            if p["class_name"] == "Entity Pointer" {
                encode(&mut p["value"], ids);
            }
        }
    }
    doc
}
pub fn document(scene: &SceneFile, index: usize) -> Value {
    document_with(scene, index, &symbols(scene))
}

/// Identity includes referenced definition content, so equal names with different
/// defaults or dependency graphs remain variants. Traversal is cycle-safe.
pub fn index(scene: &SceneFile) -> Vec<Definition> {
    let ids = symbols(scene);
    let placed: BTreeSet<_> = scene.scene.iter().map(|v| v.to_u32()).collect();
    let docs: Vec<_> = (0..scene.objects.len())
        .map(|i| document_with(scene, i, &ids))
        .collect();
    let mut owners = HashMap::new();
    for (i, e) in scene.objects.iter().enumerate() {
        owners.insert(e.link_id.to_u32(), i);
        for c in &e.components {
            owners.insert(c.link_id.to_u32(), i);
        }
    }
    fn pointers(v: &Value, out: &mut Vec<u32>) {
        if let Some(a) = v.as_array() {
            for v in a {
                pointers(v, out)
            }
        } else if let Some(n) = v.as_u64().and_then(|n| u32::try_from(n).ok()) {
            out.push(n)
        }
    }
    let mut names: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, e) in scene.objects.iter().enumerate() {
        names.entry(&e.name).or_default().push(i);
    }
    fn named_values(v: &Value, out: &mut Vec<String>) {
        if let Some(a) = v.as_array() {
            for v in a {
                named_values(v, out)
            }
        } else if let Some(s) = v.as_str() {
            out.push(s.into())
        }
    }
    let edges: Vec<Vec<usize>> = scene
        .objects
        .iter()
        .map(|e| {
            let mut refs = vec![e.master_link_id.to_u32()];
            for c in &e.components {
                refs.push(c.master_link_id.to_u32());
                for p in &c.properties {
                    if p.class_name == "Entity Pointer" {
                        pointers(&p.value, &mut refs)
                    }
                    if p.template || (c.class_name == "InputOutputComponent" && p.name == "Target")
                    {
                        let mut values = vec![];
                        named_values(&p.value, &mut values);
                        for name in values {
                            if let Some(indices) = names.get(name.as_str()).filter(|v| v.len() == 1)
                            {
                                refs.push(scene.objects[indices[0]].link_id.to_u32());
                            }
                        }
                    }
                }
            }
            refs.into_iter()
                .filter_map(|id| owners.get(&id).copied())
                .collect()
        })
        .collect();
    let node_hashes: Vec<_> = docs
        .iter()
        .map(|doc| digest(&serde_json::to_vec(doc).unwrap()))
        .collect();
    scene
        .objects
        .iter()
        .enumerate()
        .filter(|(_, e)| !placed.contains(&e.link_id.to_u32()))
        .map(|(index, _)| {
            let mut queue = VecDeque::from([index]);
            let mut seen = BTreeSet::new();
            let mut graph = vec![];
            while let Some(i) = queue.pop_front() {
                if !seen.insert(i) {
                    continue;
                }
                graph.push(&node_hashes[i]);
                queue.extend(edges[i].iter().copied());
            }
            graph.sort();
            let identity = digest(&serde_json::to_vec(&(&node_hashes[index], graph)).unwrap());
            let members = |e: &Entity| {
                e.components
                    .iter()
                    .find(|c| c.class_name == "JPSPrefabComponent")
                    .and_then(|c| c.properties.iter().find(|p| p.name == "Prefab Entities"))
                    .and_then(|p| p.value.as_array())
                    .map(Vec::len)
            };
            let assembly_ready = members(&scene.objects[index]).is_none_or(|n| n > 0)
                || scene.objects.iter().any(|e| {
                    e.master_link_id == scene.objects[index].link_id
                        && members(e).is_some_and(|n| n > 0)
                });
            Definition {
                index,
                identity,
                assembly_ready,
            }
        })
        .collect()
}
fn diff(pointer: &str, a: &Value, b: &Value, out: &mut Vec<FieldEdit>) {
    if a == b {
        return;
    }
    match (a, b) {
        (Value::Object(a), Value::Object(b)) if a.keys().eq(b.keys()) => {
            for (k, v) in a {
                diff(
                    &format!("{pointer}/{}", k.replace('~', "~0").replace('/', "~1")),
                    v,
                    &b[k],
                    out,
                )
            }
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            for (i, (a, b)) in a.iter().zip(b).enumerate() {
                diff(&format!("{pointer}/{i}"), a, b, out)
            }
        }
        _ => out.push(FieldEdit {
            pointer: pointer.into(),
            before: a.clone(),
            after: b.clone(),
            variable: None,
            label: String::new(),
        }),
    }
}
pub fn capture(
    identity: String,
    base: &SceneFile,
    index: usize,
    current: &SceneFile,
    current_index: usize,
) -> PrefabEdit {
    let mut edits = vec![];
    diff(
        "",
        &document(base, index),
        &document(current, current_index),
        &mut edits,
    );
    for edit in &mut edits {
        let parts: Vec<_> = edit.pointer.split('/').collect();
        edit.label = if parts.get(1) == Some(&"components") && parts.get(3) == Some(&"properties") {
            parts
                .get(2)
                .and_then(|v| v.parse::<usize>().ok())
                .and_then(|i| base.objects[index].components.get(i))
                .and_then(|c| {
                    parts
                        .get(4)
                        .and_then(|v| v.parse::<usize>().ok())
                        .and_then(|i| c.properties.get(i))
                        .map(|p| p.name.clone())
                })
                .unwrap_or_else(|| "Component settings".into())
        } else if edit.pointer == "/name" {
            "Name".into()
        } else {
            "Component settings".into()
        };
    }
    PrefabEdit {
        identity,
        name: base.objects[index].name.clone(),
        edits,
    }
}
fn decode(v: &mut Value, ids: &HashMap<String, u32>) -> Result<()> {
    if let Some(a) = v.as_array_mut() {
        for v in a {
            decode(v, ids)?
        }
    } else if v.is_object() {
        let symbol = serde_json::to_string(v)?;
        *v = (*ids.get(&symbol).with_context(|| {
            format!("Prefab reference is not available in this level: {symbol}")
        })?)
        .into();
    }
    Ok(())
}
fn component_id(
    scene: &SceneFile,
    owner: u32,
    class: &str,
    name: &str,
    occurrence: usize,
) -> Result<u32> {
    // Deterministic IDs keep authored references stable as unrelated objects are
    // added to a level. Reserve neither sentinels nor signed-negative IDs.
    let key = serde_json::to_vec(&(owner, class, name, occurrence))?;
    let bytes = crate::sha256(&key);
    let id =
        0x1000_0000 | (u32::from_be_bytes(bytes[..4].try_into().unwrap()) & 0x0fff_ffff);
    ensure!(
        !scene
            .objects
            .iter()
            .any(|e| e.link_id.to_u32() == id
                || e.components.iter().any(|c| c.link_id.to_u32() == id)),
        "New prefab component ID collides with an existing object"
    );
    Ok(id)
}
fn materialize(
    scene: &SceneFile,
    index: usize,
    mut doc: Value,
    _next: &mut u32,
    aliases: &HashMap<u32, Value>,
) -> Result<Entity> {
    let source = &scene.objects[index];
    let mut lookup: HashMap<String, u32> = symbols(scene)
        .into_iter()
        .map(|(id, v)| (serde_json::to_string(&v).unwrap(), id))
        .collect();
    for (id, symbol) in aliases {
        lookup.entry(serde_json::to_string(symbol)?).or_insert(*id);
    }
    doc["link_id"] = source.link_id.to_u32().into();
    let mut used = BTreeSet::new();
    // Allocate before resolving references so components can refer to one another.
    let mut occurrences = BTreeMap::new();
    for c in doc["components"]
        .as_array_mut()
        .context("Prefab components must be a list")?
    {
        let class = c["class_name"]
            .as_str()
            .context("Component needs a class")?
            .to_string();
        let name = c["name"]
            .as_str()
            .context("Component needs a name")?
            .to_string();
        let existing = source
            .components
            .iter()
            .enumerate()
            .find(|(i, v)| !used.contains(i) && v.class_name == class && v.name == name);
        let id = if let Some((i, c)) = existing {
            used.insert(i);
            c.link_id.to_u32()
        } else {
            component_id(
                scene,
                source.link_id.to_u32(),
                &class,
                &name,
                *occurrences
                    .get(&(class.clone(), name.clone()))
                    .unwrap_or(&0),
            )?
        };
        c["link_id"] = id.into();
        let n = occurrences
            .entry((class.clone(), name.clone()))
            .or_insert(0usize);
        lookup.insert(
            serde_json::to_string(
                &serde_json::json!({"entity":source.name,"component":[class,name,*n]}),
            )?,
            id,
        );
        *n += 1;
    }
    if let Some(v) = doc.get_mut("master_link_id") {
        decode(v, &lookup)?
    }
    for c in doc["components"].as_array_mut().unwrap() {
        if let Some(v) = c.get_mut("master_link_id") {
            decode(v, &lookup)?
        }
        for p in c["properties"]
            .as_array_mut()
            .context("Component properties must be a list")?
        {
            if p["class_name"] == "Entity Pointer" {
                decode(&mut p["value"], &lookup)?
            }
        }
    }
    crate::pack_io::guarded(|| Ok(Entity::from_dict(&doc)))
}
/// Apply by base identity even when another project edit changed a definition.
/// Instance-local values are left intact; inherited values come from the master.
pub fn apply(base: &[u8], current: &[u8], edits: &[PrefabEdit]) -> Result<Vec<u8>> {
    apply_internal(base, current, edits, &BTreeMap::new(), false)
}
pub fn apply_linked(
    base: &[u8],
    current: &[u8],
    edits: &[PrefabEdit],
    links: &BTreeMap<u32, String>,
) -> Result<Vec<u8>> {
    apply_internal(base, current, edits, links, true)
}
fn apply_internal(
    base: &[u8],
    current: &[u8],
    edits: &[PrefabEdit],
    links: &BTreeMap<u32, String>,
    only_linked: bool,
) -> Result<Vec<u8>> {
    if edits.is_empty() {
        return Ok(current.to_vec());
    }
    let original = crate::pack_io::guarded(|| Ok(SceneFile::from_binary(base, EndianType::BIG)))?;
    let mut scene =
        crate::pack_io::guarded(|| Ok(SceneFile::from_binary(current, EndianType::BIG)))?;
    let mut next = scene
        .objects
        .iter()
        .flat_map(|e| {
            std::iter::once(e.link_id.to_u32())
                .chain(e.components.iter().map(|c| c.link_id.to_u32()))
        })
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .context("Entity ID space exhausted")?;
    let mut definitions: BTreeMap<u32, String> = if only_linked {
        BTreeMap::new()
    } else {
        index(&original)
            .into_iter()
            .map(|d| (original.objects[d.index].link_id.to_u32(), d.identity))
            .collect()
    };
    definitions.extend(links.iter().map(|(id, identity)| (*id, identity.clone())));
    let aliases = symbols(&original);
    let mut renamed = BTreeMap::new();
    let mut changed = false;
    let mut added_components = BTreeSet::new();
    for (id, identity) in definitions {
        let Some(edit) = edits.iter().find(|e| e.identity == identity) else {
            continue;
        };
        let linked = links.contains_key(&id);
        let id = ID::from_u32(id);
        let Some(i) = scene.objects.iter().position(|e| e.link_id == id) else {
            continue;
        };
        ensure!(
            !scene.scene.contains(&id),
            "Prefab was changed into a placed instance"
        );
        let mut doc = document(&scene, i);
        for field in &edit.edits {
            let value = doc.pointer_mut(&field.pointer).with_context(|| {
                format!(
                    "Prefab {} changed structurally at {}",
                    edit.name, field.pointer
                )
            })?;
            // Higher-priority mods are composed by the caller. Preserve explicit
            // scene overrides when their value differs from the source default.
            ensure!(
                linked || *value == field.before || *value == field.after,
                "Conflicting prefab edit: {} {}",
                edit.name,
                field.pointer
            );
            *value = field.after.clone();
        }
        let previous: BTreeSet<_> = scene.objects[i]
            .components
            .iter()
            .map(|c| c.link_id.to_u32())
            .collect();
        let old_name = scene.objects[i].name.clone();
        scene.objects[i] = materialize(&scene, i, doc, &mut next, &aliases)?;
        if scene.objects[i].name != old_name {
            renamed.insert(old_name, scene.objects[i].name.clone());
        }
        added_components.extend(
            scene.objects[i]
                .components
                .iter()
                .map(|c| c.link_id.to_u32())
                .filter(|id| !previous.contains(id)),
        );
        changed = true;
    }
    if !changed {
        return Ok(current.to_vec());
    }
    fn rename(value: &mut Value, names: &BTreeMap<String, String>) {
        if let Some(a) = value.as_array_mut() {
            for v in a {
                rename(v, names)
            }
        } else if let Some(s) = value.as_str() {
            if let Some(name) = names.get(s) {
                *value = Value::String(name.clone());
            }
        }
    }
    if !renamed.is_empty() {
        for e in &mut scene.objects {
            for c in &mut e.components {
                for p in &mut c.properties {
                    if p.template || (c.class_name == "InputOutputComponent" && p.name == "Target")
                    {
                        rename(&mut p.value, &renamed);
                    }
                }
            }
        }
    }
    // New components need inherited component records on existing consumers.
    for _ in 0..scene.objects.len() {
        let mut added = false;
        for i in 0..scene.objects.len() {
            let Some(master) = scene
                .objects
                .iter()
                .position(|e| e.link_id == scene.objects[i].master_link_id)
            else {
                continue;
            };
            let components = scene.objects[master].components.clone();
            for component in components {
                if !added_components.contains(&component.link_id.to_u32()) {
                    continue;
                }
                if scene.objects[i]
                    .components
                    .iter()
                    .any(|c| c.master_link_id == component.link_id)
                {
                    continue;
                }
                let mut inherited = component;
                inherited.master_link_id = inherited.link_id;
                inherited.link_id = ID::from_u32(component_id(
                    &scene,
                    scene.objects[i].link_id.to_u32(),
                    &inherited.class_name,
                    &inherited.name,
                    0,
                )?);
                inherited.properties.clear();
                added_components.insert(inherited.link_id.to_u32());
                scene.objects[i].components.push(inherited);
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    // Removal must never leave inherited components pointing at removed IDs.
    let old_ids: BTreeSet<_> = original
        .objects
        .iter()
        .flat_map(|e| e.components.iter().map(|c| c.link_id.to_u32()))
        .collect();
    let new_ids: BTreeSet<_> = scene
        .objects
        .iter()
        .flat_map(|e| e.components.iter().map(|c| c.link_id.to_u32()))
        .collect();
    let mut removed: BTreeSet<_> = old_ids.difference(&new_ids).copied().collect();
    loop {
        let mut propagated = false;
        for e in &mut scene.objects {
            for c in &e.components {
                if removed.contains(&c.master_link_id.to_u32()) {
                    ensure!(c.properties.is_empty(),"{} has overrides on a removed component. Revert those overrides before removing the shared component.",e.name);
                }
            }
            e.components.retain(|c| {
                if removed.contains(&c.master_link_id.to_u32()) {
                    removed.insert(c.link_id.to_u32());
                    propagated = true;
                    false
                } else {
                    true
                }
            });
        }
        if !propagated {
            break;
        }
    }
    fn references_removed(value: &Value, removed: &BTreeSet<u32>) -> bool {
        if let Some(a) = value.as_array() {
            a.iter().any(|v| references_removed(v, removed))
        } else {
            value
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .is_some_and(|v| removed.contains(&v))
        }
    }
    for e in &scene.objects {
        for c in &e.components {
            for p in &c.properties {
                if p.class_name == "Entity Pointer" {
                    ensure!(
                        !references_removed(&p.value, &removed),
                        "{} still references a removed component",
                        e.name
                    );
                }
            }
        }
    }
    let bytes = scene.to_binary(EndianType::BIG);
    let check = crate::pack_io::guarded(|| Ok(SceneFile::from_binary(&bytes, EndianType::BIG)))?;
    if !super::equivalent(&scene.to_dict(), &check.to_dict()) {
        let mut differences = vec![];
        super::diff_json("", &scene.to_dict(), &check.to_dict(), &mut differences);
        anyhow::bail!(
            "Prefab serialization changed authored data: {:?}",
            differences.first()
        );
    }
    Ok(bytes)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use epic_mickey_lib_rs::scene_file::{Component, Property, SceneFileVersion};
    pub(crate) fn fixture(offset: u32) -> SceneFile {
        let prop = Property::new(
            "Float".into(),
            "Scale".into(),
            false,
            false,
            false,
            serde_json::json!(0.1),
        );
        let component = Component::new(
            "JPSTransformationComponent".into(),
            "Transform".into(),
            ID::new(1),
            ID::from_u32(offset + 2),
            ID::new(0),
            vec![prop],
        );
        let root = Entity::new(
            "JPSGeneralEntity".into(),
            "Shared crate".into(),
            ID::from_u32(offset + 1),
            ID::new(0),
            0,
            0,
            vec![component.clone()],
        );
        let mut inherited = component;
        inherited.link_id = ID::from_u32(offset + 4);
        inherited.master_link_id = ID::from_u32(offset + 2);
        inherited.properties.clear();
        let instance = Entity::new(
            "JPSGeneralEntity".into(),
            format!("Placed {offset}"),
            ID::from_u32(offset + 3),
            root.link_id,
            0,
            0,
            vec![inherited],
        );
        let scene = SceneFile::new(
            vec![root, instance],
            vec![ID::from_u32(offset + 3)],
            vec![],
            ID::new(0),
            SceneFileVersion::Version1,
        );
        SceneFile::from_binary(&scene.to_binary(EndianType::BIG), EndianType::BIG)
    }
    #[test]
    fn shared_identity_ignores_runtime_ids_and_keeps_actual_variants() {
        let a = fixture(0);
        let mut b = fixture(100);
        assert_eq!(index(&a)[0].identity, index(&b)[0].identity);
        b.objects[0].components[0].properties[0].value = serde_json::json!(0.3);
        assert_ne!(index(&a)[0].identity, index(&b)[0].identity);
        assert_eq!(index(&a).len(), 1);
    }
    #[test]
    fn named_dependency_defaults_are_part_of_shared_identity() {
        let mut scene = fixture(0);
        let dependency_name = scene.objects[1].name.clone();
        scene.objects[0].components[0]
            .properties
            .push(Property::new(
                "String".into(),
                "Prototype".into(),
                false,
                false,
                true,
                serde_json::json!(dependency_name),
            ));
        // Mark as a declared named reference independently of the constructor flags.
        scene.objects[0].components[0]
            .properties
            .last_mut()
            .unwrap()
            .template = true;
        let original = index(&scene)[0].identity.clone();
        scene.objects[1].components[0]
            .properties
            .push(Property::new(
                "Float".into(),
                "Scale".into(),
                false,
                false,
                false,
                serde_json::json!(2.0),
            ));
        assert_ne!(original, index(&scene)[0].identity);
    }
    #[test]
    fn shared_edit_reaches_another_level_without_changing_instance_overrides() {
        let base = fixture(0);
        let mut edited = fixture(0);
        edited.objects[0].components[0].properties[0].value = serde_json::json!(1.0);
        let recipe = capture(index(&base)[0].identity.clone(), &base, 0, &edited, 0);
        let mut other = fixture(100);
        other.objects[1].components[0].properties = vec![Property::new(
            "Float".into(),
            "Scale".into(),
            false,
            false,
            false,
            serde_json::json!(2.0),
        )];
        let bytes = other.to_binary(EndianType::BIG);
        let output = apply(&bytes, &bytes, &[recipe]).unwrap();
        let result = SceneFile::from_binary(&output, EndianType::BIG);
        assert_eq!(
            result.objects[0].components[0].properties[0].value,
            serde_json::json!(1.0)
        );
        assert_eq!(
            result.objects[1].components[0].properties[0].value,
            serde_json::json!(2.0)
        );
        assert_eq!(result.objects[1].master_link_id, other.objects[0].link_id);
        assert_eq!(result.scene, other.scene);
        assert_eq!(apply(&bytes, &bytes, &[]).unwrap(), bytes);
    }
    #[test]
    fn added_components_inherit_without_filling_unrelated_missing_components() {
        let base = fixture(0);
        let mut edited = fixture(0);
        edited.objects[0].components.push(Component::new(
            "JPSHealthComponent".into(),
            "Unknown".into(),
            ID::new(3),
            ID::new(9),
            ID::new(0),
            vec![],
        ));
        let recipe = capture(index(&base)[0].identity.clone(), &base, 0, &edited, 0);
        let mut other = fixture(100);
        other.objects[1].components.clear();
        let bytes = other.to_binary(EndianType::BIG);
        let result =
            SceneFile::from_binary(&apply(&bytes, &bytes, &[recipe]).unwrap(), EndianType::BIG);
        assert_eq!(result.objects[0].components.len(), 2);
        assert_eq!(result.objects[1].components.len(), 1);
        assert_eq!(
            result.objects[1].components[0].class_name,
            "JPSHealthComponent"
        );
        assert_eq!(
            result.objects[1].components[0].master_link_id,
            result.objects[0].components[1].link_id
        );
    }
    #[test]
    fn adding_a_prefab_component_does_not_reuse_an_unrelated_local_component() {
        let base = fixture(0);
        let mut edited = fixture(0);
        let component = Component::new(
            "JPSHealthComponent".into(),
            "Unknown".into(),
            ID::new(3),
            ID::new(9),
            ID::new(0),
            vec![],
        );
        edited.objects[0].components.push(component.clone());
        let recipe = capture(index(&base)[0].identity.clone(), &base, 0, &edited, 0);
        let mut other = fixture(100);
        let mut local = component;
        local.link_id = ID::from_u32(900);
        other.objects[1].components.push(local.clone());
        let bytes = other.to_binary(EndianType::BIG);
        let result =
            SceneFile::from_binary(&apply(&bytes, &bytes, &[recipe]).unwrap(), EndianType::BIG);
        let health: Vec<_> = result.objects[1]
            .components
            .iter()
            .filter(|c| c.class_name == "JPSHealthComponent")
            .collect();
        assert_eq!(health.len(), 2);
        assert!(health
            .iter()
            .any(|c| c.link_id == local.link_id && c.master_link_id.to_u32() == 0));
        assert!(health
            .iter()
            .any(|c| c.master_link_id == result.objects[0].components[1].link_id));
    }
    #[test]
    fn dependency_ids_are_resolved_against_each_destination() {
        let mut base = fixture(0);
        let mut other = fixture(100);
        for scene in [&mut base, &mut other] {
            let id = scene.objects[0].components[0].link_id.to_u32();
            scene.objects[0].components[0]
                .properties
                .push(Property::new(
                    "Entity Pointer".into(),
                    "Target".into(),
                    false,
                    false,
                    false,
                    serde_json::json!(id),
                ));
        }
        assert_eq!(index(&base)[0].identity, index(&other)[0].identity);
        let mut edited = SceneFile::from_binary(&base.to_binary(EndianType::BIG), EndianType::BIG);
        edited.objects[0].components[0].properties[1].value =
            serde_json::json!(base.objects[0].link_id.to_u32());
        let recipe = capture(index(&base)[0].identity.clone(), &base, 0, &edited, 0);
        let bytes = other.to_binary(EndianType::BIG);
        let result =
            SceneFile::from_binary(&apply(&bytes, &bytes, &[recipe]).unwrap(), EndianType::BIG);
        assert_eq!(
            result.objects[0].components[0].properties[1].value,
            serde_json::json!(other.objects[0].link_id.to_u32())
        );
    }
    #[test]
    fn imported_edited_prefab_remains_linked_to_future_source_edits() {
        let base = fixture(0);
        let mut updated = fixture(0);
        updated.objects[0].components[0].properties[0].value = serde_json::json!(3.0);
        let identity = index(&base)[0].identity.clone();
        let edit = capture(identity.clone(), &base, 0, &updated, 0);
        let mut imported = fixture(100);
        imported.objects[0].components[0].properties[0].value = serde_json::json!(1.0);
        let bytes = imported.to_binary(EndianType::BIG);
        let links = BTreeMap::from([(imported.objects[0].link_id.to_u32(), identity)]);
        let result = SceneFile::from_binary(
            &apply_linked(&bytes, &bytes, &[edit], &links).unwrap(),
            EndianType::BIG,
        );
        assert_eq!(
            result.objects[0].components[0].properties[0].value,
            serde_json::json!(3.0)
        );
        assert_eq!(result.scene, imported.scene);
    }
    #[test]
    fn removing_a_component_removes_empty_inherited_copies_and_preserves_overrides() {
        let base = fixture(0);
        let mut updated = fixture(0);
        updated.objects[0].components.clear();
        let edit = capture(index(&base)[0].identity.clone(), &base, 0, &updated, 0);
        let bytes = base.to_binary(EndianType::BIG);
        let result = SceneFile::from_binary(
            &apply(&bytes, &bytes, &[edit.clone()]).unwrap(),
            EndianType::BIG,
        );
        assert!(result.objects[0].components.is_empty());
        assert!(result.objects[1].components.is_empty());
        let mut instance_override = fixture(0);
        instance_override.objects[1].components[0].properties = instance_override.objects[0]
            .components[0]
            .properties
            .clone();
        let bytes = instance_override.to_binary(EndianType::BIG);
        assert!(apply(&bytes, &bytes, &[edit])
            .unwrap_err()
            .to_string()
            .contains("overrides"));
    }
}
