//! Portable mod recipes. Game data never contains configuration placeholders.
use anyhow::{ensure, Context, Result};
use epic_mickey_lib_rs::{file_manipulator::EndianType, scene_file::SceneFile};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

// Kept here instead of reaching through the application crate root so that
// the standalone asset-audit binaries can compile this module directly.
const SUBTITLE_IDENTIFIER_BYTES: usize = 68;
#[path = "mod_project/prefabs.rs"]
pub mod prefabs;
#[path = "mod_project/source.rs"]
pub mod source;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Variable {
    pub id: String,
    pub label: String,
    pub description: String,
    pub default: f64,
    pub min: f64,
    pub max: f64,
    pub step: f64,
}
impl Default for Variable {
    fn default() -> Self {
        Self {
            id: "mickeyScale".into(),
            label: "Mickey Scale".into(),
            description: String::new(),
            default: 0.1,
            min: 0.01,
            max: 1.,
            step: 0.01,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Operand {
    Literal(Value),
    Variable(String),
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct JsonEdit {
    pub pointer: String,
    pub expected: Value,
    pub replacement: Operand,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub operation: JsonOperation,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum JsonOperation {
    #[default]
    Replace,
    Add,
    Remove,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ByteEdit {
    pub offset: usize,
    pub expected: Vec<u8>,
    pub replacement: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Change {
    Scene {
        base_sha256: String,
        edits: Vec<JsonEdit>,
    },
    LuaSource {
        base_sha256: String,
        source_sha256: String,
        source_version: String,
        edits: Vec<source::TextEdit>,
    },
    Binary {
        base_sha256: String,
        base_size: usize,
        edits: Vec<ByteEdit>,
    },
    New {
        data: Vec<u8>,
        tag: String,
        compress: bool,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AssetChange {
    /// Relative to the unified asset root (or files/ for loose files).
    pub path: String,
    #[serde(default)]
    pub loose: bool,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub change: Change,
}

/// Display text and optional voice audio linked to a fixed-record `.sub` cue.
/// The retail subtitle file stores only the cue identifier and timing, so this
/// metadata is kept in the portable project recipe.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SubtitleLocalization {
    pub identifier: String,
    #[serde(default = "default_locale")]
    pub locale: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_path: Option<String>,
}

fn default_locale() -> String {
    "en-US".into()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ModProject {
    pub format: u32,
    pub id: String,
    pub name: String,
    pub author: String,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub variables: Vec<Variable>,
    #[serde(default)]
    pub changes: Vec<AssetChange>,
    /// One source edit, expanded into matching cooked definitions at build time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prefabs: Vec<prefabs::PrefabEdit>,
    /// Source identities for definitions imported into authored levels.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub prefab_links: BTreeMap<String, BTreeMap<u32, String>>,
    #[serde(default)]
    pub dolphin_textures: BTreeMap<String, Vec<u8>>,
    /// Explicit roots for collections loaded by script or engine convention.
    #[serde(default)]
    pub collections: BTreeMap<String, Vec<String>>,
    /// A highest-priority replacement for an archive's inferred root recipe.
    #[serde(default)]
    pub pack_roots: BTreeMap<String, Vec<String>>,
    /// New scenes can inherit measured compatibility rules from a retail scene.
    #[serde(default)]
    pub scene_sources: BTreeMap<String, String>,
    /// Localized display text and optional voice assets keyed by subtitle cue.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subtitle_localizations: Vec<SubtitleLocalization>,
}
impl Default for ModProject {
    fn default() -> Self {
        Self {
            format: 1,
            id: "my_mod".into(),
            name: "Untitled mod".into(),
            author: String::new(),
            version: "1.0.0".into(),
            description: String::new(),
            variables: vec![],
            changes: vec![],
            prefabs: vec![],
            prefab_links: BTreeMap::new(),
            dolphin_textures: BTreeMap::new(),
            collections: BTreeMap::new(),
            pack_roots: BTreeMap::new(),
            scene_sources: BTreeMap::new(),
            subtitle_localizations: Vec::new(),
        }
    }
}
pub fn key(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}
pub fn valid_path(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty() && !path.contains(['\\', ':', '\0']) && !path.starts_with('/'),
        "Use a relative game path with forward slashes"
    );
    for part in path.split('/') {
        ensure!(
            !part.is_empty() && part != "." && part != ".." && !part.ends_with([' ', '.']),
            "Invalid game path: {path}"
        );
    }
    Ok(())
}
pub fn digest(data: &[u8]) -> String {
    crate::sha256(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
impl ModProject {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            matches!(self.format, 1 | 2 | 3),
            "Unsupported mod format {}",
            self.format
        );
        ensure!(
            !self.id.is_empty()
                && self.id.len() <= 80
                && self
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
            "Mod ID must contain letters, numbers, underscores or hyphens"
        );
        ensure!(!self.name.trim().is_empty(), "A mod needs a name");
        let mut prefab_ids = std::collections::BTreeSet::new();
        for (path, links) in &self.prefab_links {
            valid_path(path)?;
            ensure!(path.ends_with(".bin"), "Prefab links require a scene");
            for (id, identity) in links {
                ensure!(
                    *id != 0
                        && *id != u32::MAX
                        && identity.len() == 64
                        && identity.bytes().all(|c| c.is_ascii_hexdigit()),
                    "Invalid prefab link"
                );
            }
        }
        for prefab in &self.prefabs {
            ensure!(
                prefab.identity.len() == 64
                    && prefab.identity.bytes().all(|c| c.is_ascii_hexdigit())
                    && prefab_ids.insert(&prefab.identity),
                "Invalid or duplicate prefab identity"
            );
            ensure!(!prefab.name.is_empty(), "A prefab needs a name");
            for field in &prefab.edits {
                if let Some(id) = &field.variable {
                    ensure!(
                        self.variables.iter().any(|v| &v.id == id)
                            && field.before.is_number()
                            && field.after.is_number(),
                        "Prefab option {id} must reference a numeric field and a defined option"
                    );
                }
            }
        }
        for (archive, roots) in self.collections.iter().chain(self.pack_roots.iter()) {
            valid_path(archive)?;
            ensure!(
                key(archive).starts_with("packfiles/") && key(archive).ends_with(".pak"),
                "Collection archive must be a packfiles/*.pak path"
            );
            for root in roots {
                valid_path(root)?;
            }
        }
        for (scene, source) in &self.scene_sources {
            valid_path(scene)?;
            valid_path(source)?;
            ensure!(
                key(scene).ends_with(".bin") && key(source).ends_with(".bin"),
                "Scene sources must refer to scene .bin paths"
            );
            ensure!(
                self.changes.iter().any(|c| !c.loose
                    && key(&c.path) == key(scene)
                    && matches!(c.change, Change::New { .. })),
                "A scene source needs a new scene asset: {scene}"
            );
        }
        let mut subtitle_keys = std::collections::BTreeSet::new();
        for localization in &self.subtitle_localizations {
            ensure!(
                !localization.identifier.is_empty()
                    && localization.identifier.len() <= SUBTITLE_IDENTIFIER_BYTES
                    && localization
                        .identifier
                        .bytes()
                        .all(|b| b.is_ascii_graphic() && b != b' '),
                "Invalid subtitle identifier"
            );
            ensure!(
                !localization.locale.trim().is_empty()
                    && localization.locale.len() <= 32
                    && localization.locale.is_ascii(),
                "Invalid subtitle locale"
            );
            ensure!(
                !localization.text.trim().is_empty(),
                "Subtitle text cannot be empty for {}",
                localization.identifier
            );
            ensure!(
                subtitle_keys.insert((
                    localization.identifier.to_ascii_lowercase(),
                    localization.locale.to_ascii_lowercase(),
                )),
                "Duplicate subtitle localization"
            );
            if let Some(path) = &localization.audio_path {
                valid_path(path)?;
            }
        }
        for (name, data) in &self.dolphin_textures {
            ensure!(
                name.starts_with("tex1_")
                    && name.ends_with(".png")
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.'),
                "Invalid Dolphin texture filename"
            );
            ensure!(
                data.starts_with(b"\x89PNG\r\n\x1a\n"),
                "Dolphin replacement texture must be PNG"
            );
        }
        let mut ids = std::collections::BTreeSet::new();
        for v in &self.variables {
            ensure!(
                !v.id.is_empty() && ids.insert(&v.id),
                "Duplicate or empty variable ID"
            );
            ensure!(
                [v.default, v.min, v.max, v.step]
                    .iter()
                    .all(|x| x.is_finite())
                    && v.min <= v.default
                    && v.default <= v.max
                    && v.step > 0.,
                "Invalid range for {}",
                v.id
            );
        }
        let mut paths = std::collections::BTreeSet::new();
        for c in &self.changes {
            valid_path(&c.path)?;
            ensure!(
                !c.loose || !key(&c.path).starts_with("packfiles/"),
                "Loose-file changes cannot overwrite compiled packfiles"
            );
            ensure!(
                paths.insert((c.loose, key(&c.path))),
                "Duplicate asset change: {}",
                c.path
            );
            for d in &c.dependencies {
                valid_path(d)?;
            }
            match &c.change {
                Change::Scene { edits, .. } => {
                    for e in edits {
                        ensure!(
                            e.pointer.starts_with('/'),
                            "Scene patch requires a JSON pointer"
                        );
                        if let Operand::Variable(id) = &e.replacement {
                            ensure!(ids.contains(id), "Undefined variable {id}");
                            ensure!(
                                e.expected.is_number(),
                                "Numeric sliders can only replace numeric values"
                            );
                        }
                    }
                }
                Change::LuaSource {
                    source_version,
                    edits,
                    ..
                } => {
                    ensure!(
                        key(&c.path).ends_with(".lua"),
                        "Lua source changes need a .lua path"
                    );
                    ensure!(
                        source::supported(source_version),
                        "Unsupported Lua source baseline"
                    );
                    source::validate(edits)?;
                }
                Change::New { tag, .. } => {
                    ensure!(tag.len() <= 4 && tag.is_ascii(), "Invalid packfile tag")
                }
                Change::Binary {
                    base_size, edits, ..
                } => {
                    let mut end = 0;
                    for e in edits {
                        ensure!(
                            e.offset >= end
                                && e.offset <= *base_size
                                && e.expected.len() <= base_size - e.offset,
                            "Overlapping or invalid byte patch"
                        );
                        end = e.offset + e.expected.len();
                    }
                }
            }
        }
        Ok(())
    }
    pub fn prefab_values(
        &self,
        configured: &BTreeMap<String, f64>,
    ) -> Result<Vec<prefabs::PrefabEdit>> {
        let values = self.values(configured)?;
        let mut prefabs = self.prefabs.clone();
        for prefab in &mut prefabs {
            for field in &mut prefab.edits {
                if let Some(id) = &field.variable {
                    field.after = serde_json::json!(values
                        .get(id)
                        .with_context(|| format!("Undefined prefab option {id}"))?);
                }
            }
        }
        Ok(prefabs)
    }
    pub fn values(&self, configured: &BTreeMap<String, f64>) -> Result<BTreeMap<String, f64>> {
        self.validate()?;
        let mut result = BTreeMap::new();
        for v in &self.variables {
            let value = *configured.get(&v.id).unwrap_or(&v.default);
            ensure!(
                value.is_finite() && value >= v.min && value <= v.max,
                "{} must be between {} and {}",
                v.label,
                v.min,
                v.max
            );
            result.insert(v.id.clone(), value);
        }
        Ok(result)
    }
    pub fn load(path: &Path) -> Result<Self> {
        ensure!(
            std::fs::metadata(path)?.len() <= 256 * 1024 * 1024,
            "Mod package exceeds 256 MiB"
        );
        let p: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        p.validate()?;
        Ok(p)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let mut stored = self.clone();
        if !stored.prefabs.is_empty() || !stored.prefab_links.is_empty() {
            stored.format = 2;
        }
        if stored.changes.iter().any(|c| matches!(c.change, Change::LuaSource { .. }) || matches!(&c.change, Change::New { data, .. } if key(&c.path).ends_with(".lua") && !data.starts_with(b"\x1bLua"))) { stored.format = 3; }
        let data = serde_json::to_vec_pretty(&stored)?;
        atomic_save(path, &data)
    }
}
pub fn atomic_save(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = (|| -> Result<()> {
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        if path.exists() {
            std::fs::copy(path, path.with_extension("btbackup"))?;
        }
        std::fs::rename(&temp, path)?;
        Ok(())
    })();
    // The closure has closed the file on every exit, including write failures.
    // Only clean up a temporary file this invocation successfully created.
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

pub fn capture(path: &str, before: &[u8], after: &[u8]) -> Result<Change> {
    if key(path).ends_with(".lua") && !after.starts_with(b"\x1bLua") {
        return source::capture(before, std::str::from_utf8(after)?);
    }
    if path.to_lowercase().ends_with(".bin") {
        let a = crate::pack_io::guarded(|| {
            Ok(SceneFile::from_binary(before, EndianType::BIG).to_dict())
        })?;
        let b = crate::pack_io::guarded(|| {
            Ok(SceneFile::from_binary(after, EndianType::BIG).to_dict())
        })?;
        let mut edits = Vec::new();
        diff_json("", &a, &b, &mut edits);
        for e in &mut edits {
            let parts: Vec<_> = e.pointer.split('/').collect();
            if parts.len() >= 7
                && parts[1] == "objects"
                && parts[3] == "components"
                && parts[5] == "properties"
            {
                let o = a.pointer(&format!("/objects/{}", parts[2]));
                let c = a.pointer(&format!("/objects/{}/components/{}", parts[2], parts[4]));
                let p = a.pointer(&format!(
                    "/objects/{}/components/{}/properties/{}",
                    parts[2], parts[4], parts[6]
                ));
                e.label = format!(
                    "{} · {} · {}",
                    o.and_then(|v| v["name"].as_str()).unwrap_or("Entity"),
                    c.and_then(|v| v["class_name"].as_str())
                        .unwrap_or("Component"),
                    p.and_then(|v| v["name"].as_str()).unwrap_or("Property")
                );
            }
        }
        return Ok(Change::Scene {
            base_sha256: digest(before),
            edits,
        });
    }
    let mut edits = Vec::new();
    if before.len() == after.len() {
        let mut i = 0;
        while i < before.len() {
            if before[i] == after[i] {
                i += 1;
                continue;
            }
            let start = i;
            while i < before.len() && before[i] != after[i] {
                i += 1;
            }
            edits.push(ByteEdit {
                offset: start,
                expected: before[start..i].to_vec(),
                replacement: after[start..i].to_vec(),
            });
        }
    } else {
        let start = before.iter().zip(after).take_while(|(a, b)| a == b).count();
        let tail = before[start..]
            .iter()
            .rev()
            .zip(after[start..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        edits.push(ByteEdit {
            offset: start,
            expected: before[start..before.len() - tail].to_vec(),
            replacement: after[start..after.len() - tail].to_vec(),
        });
    }
    Ok(Change::Binary {
        base_sha256: digest(before),
        base_size: before.len(),
        edits,
    })
}
fn diff_json(path: &str, a: &Value, b: &Value, out: &mut Vec<JsonEdit>) {
    if a == b {
        return;
    }
    match (a, b) {
        (Value::Object(a), Value::Object(b)) if a.keys().eq(b.keys()) => {
            for (k, v) in a {
                diff_json(
                    &format!("{path}/{}", k.replace('~', "~0").replace('/', "~1")),
                    v,
                    &b[k],
                    out,
                );
            }
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            for (i, (a, b)) in a.iter().zip(b).enumerate() {
                diff_json(&format!("{path}/{i}"), a, b, out);
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            let prefix = a.iter().zip(b).take_while(|(a, b)| a == b).count();
            let suffix = a[prefix..]
                .iter()
                .rev()
                .zip(b[prefix..].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            for i in (prefix..a.len() - suffix).rev() {
                out.push(JsonEdit {
                    pointer: format!("{path}/{i}"),
                    expected: a[i].clone(),
                    replacement: Operand::Literal(Value::Null),
                    label: String::new(),
                    operation: JsonOperation::Remove,
                });
            }
            for i in prefix..b.len() - suffix {
                out.push(JsonEdit {
                    pointer: format!("{path}/{i}"),
                    expected: Value::Null,
                    replacement: Operand::Literal(b[i].clone()),
                    label: String::new(),
                    operation: JsonOperation::Add,
                });
            }
        }
        _ => out.push(JsonEdit {
            pointer: path.into(),
            expected: a.clone(),
            replacement: Operand::Literal(b.clone()),
            label: String::new(),
            operation: JsonOperation::Replace,
        }),
    }
}
pub fn apply_scene(
    data: &[u8],
    edits: &[JsonEdit],
    values: &BTreeMap<String, f64>,
) -> Result<Vec<u8>> {
    let mut doc =
        crate::pack_io::guarded(|| Ok(SceneFile::from_binary(data, EndianType::BIG).to_dict()))?;
    for e in edits {
        let replacement = match &e.replacement {
            Operand::Literal(v) => v.clone(),
            Operand::Variable(id) => serde_json::json!(values
                .get(id)
                .with_context(|| format!("Undefined variable {id}"))?),
        };
        if e.operation != JsonOperation::Replace {
            let (parent, leaf) = e
                .pointer
                .rsplit_once('/')
                .context("Invalid scene patch pointer")?;
            let array = doc
                .pointer_mut(parent)
                .and_then(Value::as_array_mut)
                .context("Scene insertion/removal target is not an array")?;
            let index: usize = leaf.parse()?;
            if e.operation == JsonOperation::Add {
                ensure!(
                    index <= array.len(),
                    "Scene insertion index is out of range"
                );
                array.insert(index, replacement);
            } else {
                ensure!(
                    array.get(index) == Some(&e.expected),
                    "Scene removal conflict at {}",
                    e.pointer
                );
                array.remove(index);
            }
            continue;
        }
        let current = doc
            .pointer_mut(&e.pointer)
            .with_context(|| format!("Scene target no longer exists: {}", e.pointer))?;
        ensure!(
            *current == e.expected || *current == replacement,
            "Conflicting scene edit at {}: expected {}, found {}",
            e.pointer,
            e.expected,
            current
        );
        ensure!(
            same_type(&e.expected, &replacement),
            "Value type mismatch at {}",
            e.pointer
        );
        *current = replacement;
    }
    let result =
        crate::pack_io::guarded(|| Ok(SceneFile::from_dict(&doc).to_binary(EndianType::BIG)))?;
    let check = crate::pack_io::guarded(|| {
        Ok(SceneFile::from_binary(&result, EndianType::BIG).to_dict())
    })?;
    // EM1 stores f32: accept only the expected float quantization.
    ensure!(
        equivalent(&doc, &check),
        "Scene serializer did not preserve the requested edit"
    );
    Ok(result)
}
fn same_type(a: &Value, b: &Value) -> bool {
    matches!(
        (a, b),
        (Value::Null, Value::Null)
            | (Value::Bool(_), Value::Bool(_))
            | (Value::Number(_), Value::Number(_))
            | (Value::String(_), Value::String(_))
            | (Value::Array(_), Value::Array(_))
            | (Value::Object(_), Value::Object(_))
    )
}
fn equivalent(a: &Value, b: &Value) -> bool {
    if a == b {
        return true;
    }
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a
            .as_f64()
            .zip(b.as_f64())
            .is_some_and(|(a, b)| a as f32 == b as f32),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equivalent(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, a)| b.get(k).is_some_and(|b| equivalent(a, b)))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_atomic_save_keeps_original_and_allows_retry() {
        let root = std::env::temp_dir().join(format!(
            "bt-save-retry-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("project.json");
        let temp = path.with_extension(format!("{}.tmp", std::process::id()));
        let backup = path.with_extension("btbackup");
        std::fs::write(&path, b"original").unwrap();
        std::fs::create_dir(&backup).unwrap(); // Force failure after the temporary file was written.
        assert!(atomic_save(&path, b"edited").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert!(
            !temp.exists(),
            "A failed save must not leave its own temporary file blocking retries"
        );
        std::fs::remove_dir(&backup).unwrap();
        std::fs::write(&temp, b"another operation").unwrap();
        assert!(atomic_save(&path, b"edited").is_err());
        assert_eq!(
            std::fs::read(&temp).unwrap(),
            b"another operation",
            "Do not remove a pre-existing temporary file"
        );
        std::fs::remove_file(&temp).unwrap();
        atomic_save(&path, b"edited").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"edited");
        assert_eq!(std::fs::read(&backup).unwrap(), b"original");
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(backup).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn byte_delta_contains_only_changed_runs() {
        let c = capture("a.nif", b"abcdefghi", b"abXdeYghi").unwrap();
        if let Change::Binary { edits, .. } = c {
            assert_eq!(edits.len(), 2);
            assert_eq!(edits[0].replacement, b"X");
        } else {
            panic!();
        }
    }
    #[test]
    fn path_and_variable_validation() {
        assert!(valid_path("../a").is_err());
        assert!(valid_path("C:/x").is_err());
        let mut p = ModProject::default();
        p.variables.push(Variable::default());
        assert_eq!(p.values(&BTreeMap::new()).unwrap()["mickeyScale"], 0.1);
        assert!(p
            .values(&BTreeMap::from([("mickeyScale".into(), 2.)]))
            .is_err());
    }
    #[test]
    fn subtitle_localization_round_trips_and_validates() {
        let mut p = ModProject::default();
        p.subtitle_localizations.push(SubtitleLocalization {
            identifier: "Movie_Line01".into(),
            locale: "fr-FR".into(),
            text: "Bonjour Mickey".into(),
            audio_path: Some("audio/subtitles/movie_line01.wem".into()),
        });
        p.validate().unwrap();
        let encoded = serde_json::to_vec(&p).unwrap();
        let decoded: ModProject = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, p);

        let mut duplicate = p.clone();
        duplicate.subtitle_localizations.push(SubtitleLocalization {
            identifier: "movie_line01".into(),
            locale: "FR-fr".into(),
            text: "Encore".into(),
            audio_path: None,
        });
        assert!(duplicate.validate().is_err());

        let mut bad_path = p;
        bad_path.subtitle_localizations[0].audio_path = Some("../voice.wem".into());
        assert!(bad_path.validate().is_err());
    }
    #[test]
    fn scene_variable_is_cooked_as_an_engine_float_and_merges_disjoint_edits() {
        use epic_mickey_lib_rs::scene_file::{Component, Entity, Property, SceneFileVersion, ID};
        let e = Entity::new(
            "JPSGeneralEntity".into(),
            "Player".into(),
            ID::from_u32(1),
            ID::new(0),
            0,
            0,
            vec![Component::new(
                "JPSTransformationComponent".into(),
                "Transformation".into(),
                ID::from_u32(4),
                ID::from_u32(2),
                ID::new(0),
                vec![
                    Property::new(
                        "Float".into(),
                        "Scale".into(),
                        false,
                        false,
                        false,
                        serde_json::json!(0.1),
                    ),
                    Property::new(
                        "Float".into(),
                        "Other".into(),
                        false,
                        false,
                        false,
                        serde_json::json!(1.),
                    ),
                ],
            )],
        );
        let scene = SceneFile::new(
            vec![e],
            vec![ID::from_u32(1)],
            vec![],
            ID::new(10),
            SceneFileVersion::Version1,
        );
        let bytes = scene.to_binary(EndianType::BIG);
        let mut edited = SceneFile::from_binary(&bytes, EndianType::BIG);
        edited.objects[0].components[0].properties[0].value = serde_json::json!(0.2);
        let Change::Scene { mut edits, .. } =
            capture("scene.bin", &bytes, &edited.to_binary(EndianType::BIG)).unwrap()
        else {
            panic!()
        };
        assert_eq!(edits.len(), 1);
        assert!(edits[0].label.contains("Player"));
        edits[0].replacement = Operand::Variable("mickeyScale".into());
        let result = apply_scene(
            &bytes,
            &edits,
            &BTreeMap::from([("mickeyScale".into(), 0.35)]),
        )
        .unwrap();
        assert!(!result.windows(11).any(|w| w == b"mickeyScale"));
        let parsed = SceneFile::from_binary(&result, EndianType::BIG);
        assert_eq!(
            parsed.objects[0].components[0].properties[0]
                .value
                .as_f64()
                .unwrap() as f32,
            0.35f32
        );
        assert!(apply_scene(
            &result,
            &edits,
            &BTreeMap::from([("mickeyScale".into(), 0.6)])
        )
        .is_err());
    }
}
