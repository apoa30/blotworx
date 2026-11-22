//! Character-owned animation resources. Decode on first activation and share
//! one decoded resource between generators referencing the same project path.
use super::behavior::CharacterAssets;
use anyhow::{ensure, Context, Result};
use epic_mickey_lib_rs::{
    ffi_anim::{NativeAnimationInfo, NativeHavokPackFile},
    hkx_file::{HkxBone, HkxFile, HkxSkeleton},
};
use std::collections::{BTreeMap, HashSet};

struct Split {chunks:Vec<NativeHavokPackFile>,durations:Vec<f32>,cursor:std::sync::Mutex<super::animation_chunks::Cursor>}
pub struct Clip {
    pub info: NativeAnimationInfo,
    pub has_extracted_motion: bool,
    native: NativeHavokPackFile,
    split: Option<Split>,
}
impl Clip {
    pub fn load(bytes: &[u8], bones: usize) -> Result<Self> {Self::load_binding(bytes,bones,None)}
    /// The original split header supplies the binding separately from the
    /// standalone animation payload. Multi-chunk scheduling is still explicit.
    pub(super) fn load_split(bytes:&[u8],bones:usize)->Result<Self> {
        Self::load_split_with(bytes,bones,|index|anyhow::bail!("Continuation chunk {index} has no source provider"))
    }
    pub(super) fn load_split_with(bytes:&[u8],bones:usize,mut read:impl FnMut(u32)->Result<Vec<u8>>)->Result<Self> {
        let header=super::split_animation::Header::parse(bytes)?;
        ensure!(header.num_chunks>0,"Streamed animation has no chunks");
        ensure!(header.float_to_slot.is_empty(),"Split float-slot remapping is not connected");
        let mut clip=Self::load_binding(header.payload,bones,Some(&header.track_to_bone))?;
        ensure!(clip.info.num_tracks==header.transform_tracks as usize,"Split animation track count differs");
        let mut durations=vec![clip.info.duration];let mut chunks=vec![];let mut frames=clip.info.num_frames;
        for index in 1..header.num_chunks {
            let bytes=read(index)?;let next=Self::load_binding(&bytes,bones,Some(&header.track_to_bone))?;
            ensure!(next.info.num_tracks==clip.info.num_tracks&&next.info.track_to_bone==clip.info.track_to_bone,"Split continuation binding differs");
            frames=frames.checked_add(next.info.num_frames).context("Split frame count overflow")?;
            clip.has_extracted_motion|=next.has_extracted_motion;
            durations.push(next.info.duration);chunks.push(next.native);
        }
        let total=durations.iter().fold(0f32,|sum,&duration|(sum as f64+duration as f64)as f32);
        ensure!(header.duration<=total&&frames==header.num_frames as usize,"Split header/chunk timing or frame count differs");
        clip.info.duration=header.duration;clip.info.num_frames=header.num_frames as usize;clip.info.name=header.name.to_owned();
        if header.num_chunks>1 {clip.split=Some(Split {chunks,durations,cursor:std::sync::Mutex::new(Default::default())});}
        Ok(clip)
    }
    fn load_binding(bytes:&[u8],bones:usize,split_map:Option<&[i16]>)->Result<Self> {
        let file = HkxFile::load_bytes(bytes).map_err(anyhow::Error::msg)?;
        let has_extracted_motion = super::animation_metadata::has_extracted_motion(&file)?;
        let native =
            NativeHavokPackFile::load_memory(bytes).context("Cannot decode animation packfile")?;
        ensure!(
            native.num_animations() == 1,
            "Expected one authored animation per clip resource"
        );
        let mut info = native
            .animation_info(0)
            .context("Cannot decode animation metadata")?;
        ensure!(
            info.duration.is_finite()
                && info.duration > 0.
                && info.frame_duration.is_finite()
                && info.frame_duration > 0.
                && info.num_tracks > 0
                && info.num_frames > 0,
            "Invalid animation timing or dimensions"
        );
        if let Some(map)=split_map {info.track_to_bone=map.to_vec();}
        if info.track_to_bone.is_empty() {
            // hkaSampleAndCombineUtils uses identity track indices when the
            // authored map is null (EM2 decompilation 2874379, 2873251).
            if split_map.is_none() {
                let view = super::animation_metadata::View::new(&file)?;
                let binding = view.object("hkaAnimationBinding")?;
                ensure!(view.word(binding+20)?==0&&view.pointer(binding+16).is_err()&&view.word(binding+16)?==0,
                    "Native animation decoder lost a nonempty authored binding");
            }
            ensure!(
                info.num_tracks <= bones && info.num_tracks <= i16::MAX as usize,
                "Identity animation binding exceeds the skeleton"
            );
            info.track_to_bone = (0..info.num_tracks).map(|i| i as i16).collect();
        }
        ensure!(
            info.track_to_bone.len() == info.num_tracks
                && info
                    .track_to_bone
                    .iter()
                    .all(|&b| b == -1 || (b >= 0 && (b as usize) < bones)),
            "Invalid animation bone binding: tracks {}, binding count {}, bones {}, binding {:?}",
            info.num_tracks,
            info.track_to_bone.len(),
            bones,
            info.track_to_bone
        );
        let mut targets = HashSet::new();
        ensure!(
            info.track_to_bone
                .iter()
                .filter(|&&b| b >= 0)
                .all(|&b| targets.insert(b)),
            "Multiple animation tracks target the same bone"
        );
        Ok(Self {
            info,
            native,
            has_extracted_motion,
            split:None,
        })
    }
    /// Original identity branches of 0x801fc18c. A clip without an authored
    /// motion reference contributes identity even if its root bone animates.
    /// Non-identity curve extraction must be implemented before those clips run.
    pub fn motion_delta(
        &self,
        flags: i8,
        at_end: bool,
        _span: &super::clip_runtime::Span,
    ) -> Result<epic_mickey_lib_rs::ffi_anim::FfiTransform> {
        ensure!(
            flags & 32 != 0 || !self.has_extracted_motion || (at_end && flags & 1 == 0),
            "Authored extracted-motion curve execution is not implemented for {}",
            self.info.name
        );
        Ok(epic_mickey_lib_rs::ffi_anim::FfiTransform {
            translation: [0.; 3],
            rotation: [0., 0., 0., 1.],
            scale: [1.; 3],
        })
    }
    /// Bound transform sampling only. Float tracks, annotations, motion and
    /// skeleton remapping are separate graph stages, not silently applied here.
    pub fn sample_bones(&self, rig: &HkxSkeleton, time: f32) -> Result<Vec<HkxBone>> {
        let tracks = self.sample_tracks(rig.bones.len(), time)?;
        super::pose_blend::resolve(rig, &tracks)
    }
    /// Keep absent tracks absent through graph blending. Filling them with the
    /// reference skeleton here incorrectly attenuates partial animations.
    pub fn sample_tracks(
        &self,
        bones: usize,
        time: f32,
    ) -> Result<Vec<Option<epic_mickey_lib_rs::ffi_anim::FfiTransform>>> {
        ensure!(time.is_finite(), "Non-finite animation sample time");
        let (native,time)=if let Some(split)=&self.split {
            let (index,local)=split.cursor.lock().map_err(|_|anyhow::anyhow!("Split cursor lock poisoned"))?.select(time,&split.durations)?;
            (if index==0{&self.native}else{&split.chunks[index-1]},local)
        }else{(&self.native,time)};
        let transforms = native
            .sample_animation(0, time)
            .context("Compressed animation sampling failed")?;
        ensure!(
            transforms.len() == self.info.track_to_bone.len(),
            "Sample/binding count differs"
        );
        let mut pose = vec![None; bones];
        for (transform, &bone) in transforms.iter().zip(&self.info.track_to_bone) {
            if bone == -1 {
                continue;
            }
            let bone = pose
                .get_mut(bone as usize)
                .context("Animation used with an incompatible skeleton")?;
            *bone = Some(*transform);
        }
        Ok(pose)
    }
}
enum Entry {
    Missing(String),
    /// Untouched player clips stay losslessly compressed until first use.
    Packed {
        zlib: Vec<u8>,
        raw_len: usize,
    },
    Decoded(Clip),
}
pub struct Resources {
    pub property_words: Vec<u32>,
    pub rig: HkxSkeleton,
    pub reference_qs: Vec<super::actor_bone_query::Pose>,
    names: BTreeMap<String, String>,
    entries: BTreeMap<String, Entry>,
    binding_overrides:BTreeMap<i16,String>,
    wii_source: Option<std::sync::Arc<super::streamed_animation_resources::WiiClips>>,
}
fn key(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}
fn compress_clip(bytes: &[u8]) -> Result<Vec<u8>> {
    use std::io::Write;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder
        .write_all(bytes)
        .context("Compress unused player animation")?;
    encoder
        .finish()
        .context("Finish player animation compression")
}
fn restore_clip(zlib: &[u8], raw_len: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut decoder = flate2::read::ZlibDecoder::new(zlib);
    let mut bytes = Vec::with_capacity(raw_len);
    decoder
        .read_to_end(&mut bytes)
        .context("Restore compressed player animation")?;
    ensure!(
        bytes.len() == raw_len,
        "Compressed player animation length changed"
    );
    Ok(bytes)
}
impl Resources {
    /// Takes ownership of pack bytes; no second extracted asset tree or frame
    /// cache. Missing cinematic resources remain explicit activation errors.
    pub fn load(
        contract: &CharacterAssets,
        mut take: impl FnMut(&str) -> Option<Vec<u8>>,
    ) -> Result<Self> {
        let bytes = take(&contract.rig).context("Missing animation rig resource")?;
        let file = HkxFile::load_bytes(&bytes).map_err(anyhow::Error::msg)?;
        let rig = file
            .parse_skeleton()
            .context("Animation rig has no skeleton")?;
        let reference_qs = super::native_rig_pose::read(&file, rig.bones.len())?;
        let mut names = BTreeMap::new();
        let mut entries = BTreeMap::new();
        for (name, path) in &contract.clips {
            let path_key = key(path);
            if let Some(old) = names.insert(key(name), path_key.clone()) {
                ensure!(old == path_key, "Ambiguous animation name {name}");
            }
            if !entries.contains_key(&path_key) {
                let entry = match take(path) {
                    Some(bytes) => {
                        let raw_len = bytes.len();
                        Entry::Packed {
                            zlib: compress_clip(&bytes)?,
                            raw_len,
                        }
                    }
                    None => Entry::Missing(path.clone()),
                };
                entries.insert(path_key, entry);
            }
        }
        Ok(Self {
            rig,
            reference_qs,
            names,
            entries,binding_overrides:BTreeMap::new(),
            wii_source: None,
            property_words: contract.property_words.clone(),
        })
    }
    pub(super) fn set_wii_source(&mut self, source: Option<std::sync::Arc<super::streamed_animation_resources::WiiClips>>) {
        self.wii_source = source;
    }
    pub(super) fn binding_path(&self,index:i16)->Option<&str>{self.binding_overrides.get(&index).map(String::as_str)}
    pub(super) fn bind_dynamic(&mut self,index:i16,path:&str,bytes:&[u8],split:bool)->Result<()> {
        let path=key(path);
        if !matches!(self.entries.get(&path),Some(Entry::Decoded(_))) {
            let clip=if split{Clip::load_split(bytes,self.rig.bones.len())?}else{Clip::load(bytes,self.rig.bones.len())?};
            self.entries.insert(path.clone(),Entry::Decoded(clip));
        }
        self.binding_overrides.insert(index,path);Ok(())
    }
    /// Pre-resident continuation payloads preserve native sample boundaries;
    /// asynchronous preload/readiness and eviction remain separate work.
    pub(super) fn bind_dynamic_streamed(&mut self,index:i16,path:&str,bytes:&[u8],catalog:&super::streamed_animation_resources::Catalog)->Result<()> {
        let path=key(path);
        if !matches!(self.entries.get(&path),Some(Entry::Decoded(_))) {
            let stem=path.strip_suffix(".000.chk").context("Split resource is not its first chunk")?;
            let clip=Clip::load_split_with(bytes,self.rig.bones.len(),|chunk|{
                let name=format!("{stem}.{chunk:03}.chk");catalog.read(&name)?.with_context(||format!("Missing authored animation continuation {name}"))
            })?;
            self.entries.insert(path.clone(),Entry::Decoded(clip));
        }
        self.binding_overrides.insert(index,path);Ok(())
    }
    pub(super) fn clip_binding(&mut self,name:&str,index:i16)->Result<&Clip> {
        if let Some(path)=self.binding_overrides.get(&index) {
            return match self.entries.get(path) {Some(Entry::Decoded(clip))=>Ok(clip),_=>anyhow::bail!("Dynamic animation binding lost its resource")};
        }
        self.clip(name)
    }
    pub fn clip(&mut self, name: &str) -> Result<&Clip> {
        let path = self
            .names
            .get(&key(name))
            .with_context(|| format!("Unknown authored animation {name}"))?;
        let entry = self
            .entries
            .get_mut(path)
            .context("Missing animation resource entry")?;
        match entry {
            Entry::Missing(path) => {
                let bytes = self.wii_source.as_ref().map(|source| source.read(path))
                    .transpose()?.flatten()
                    .with_context(|| format!("Animation {path} requires loading its animation list"))?;
                let clip = Clip::load(&bytes, self.rig.bones.len())
                    .with_context(|| format!("Decode cross-archive animation {path}"))?;
                *entry = Entry::Decoded(clip);
            }
            Entry::Packed { zlib, raw_len } => {
                let bytes = restore_clip(zlib, *raw_len)?;
                let clip = Clip::load(&bytes, self.rig.bones.len())
                    .with_context(|| format!("Decode animation {path}"))?;
                *entry = Entry::Decoded(clip);
            }
            Entry::Decoded(_) => {}
        }
        match entry {
            Entry::Decoded(clip) => Ok(clip),
            _ => unreachable!(),
        }
    }
    /// Stable FNV-1a hash of a normalized clip name for compact multiplayer animation sync.
    pub fn clip_name_hash(name: &str) -> u32 {
        let mut h: u32 = 0x811c9dc5;
        for b in key(name).bytes() {
            h ^= b as u32;
            h = h.wrapping_mul(0x01000193);
        }
        h
    }
    /// Extract the primary active Havok clip name hash and playback time from a live player graph.
    pub fn active_clip_and_time(prepared: &super::generator_plan::Prepared) -> (u32, f32) {
        let mut best: Option<(i32, &str, f32)> = None;
        for node in &prepared.plan.nodes {
            if let Some(definition) = prepared.state.clips.get(&node.object) {
                if let Ok(instance) = prepared.clips.get(&node.object) {
                    let t = if instance.clock.local_time.is_finite() {
                        instance.clock.local_time.max(0.0)
                    } else {
                        0.0
                    };
                    let name = definition.settings.animation_name.as_str();
                    let lower = name.to_ascii_lowercase();
                    // Prefer primary full-body clips over partial upper-body/face/aim overlays
                    let score = if lower.contains("aim")
                        || lower.contains("additive")
                        || lower.contains("face")
                        || lower.contains("blink")
                        || lower.contains("ear")
                    {
                        0
                    } else {
                        10
                    };
                    if best.is_none() || score > best.unwrap().0 {
                        best = Some((score, name, t));
                    }
                }
            }
        }
        if let Some((_, name, t)) = best {
            (Self::clip_name_hash(name), t)
        } else {
            (0, 0.0)
        }
    }
    /// Sample a player animation clip by its FNV-1a name hash at `time` (wrapped to clip duration).
    /// Used to animate each remote multiplayer Mickey independently with real Havok clips.
    pub fn sample_bones_by_hash(&mut self, name_hash: u32, time: f32) -> Option<Vec<HkxBone>> {
        if name_hash == 0 {
            return None;
        }
        let matched_name = self
            .names
            .keys()
            .find(|name| Self::clip_name_hash(name) == name_hash)
            .cloned()?;
        let rig = self.rig.clone();
        let clip = self.clip(&matched_name).ok()?;
        let dur = clip.info.duration.max(0.001);
        let t = if time.is_finite() {
            time.rem_euclid(dur)
        } else {
            0.0
        };
        clip.sample_bones(&rig, t).ok()
    }
    /// Lossless retained-payload accounting for the runtime memory report.
    /// Native Havok allocations owned by decoded clips are opaque because the
    /// native allocator does not expose their sizes.
    pub fn memory_summary(&self) -> serde_json::Value {
        let mut packed_bytes = 0usize;
        let mut original_packed_bytes = 0usize;
        let mut packed_count = 0usize;
        let mut decoded_count = 0usize;
        let mut missing_count = 0usize;
        for entry in self.entries.values() {
            match entry {
                Entry::Packed { zlib, raw_len } => {
                    packed_bytes = packed_bytes.saturating_add(zlib.capacity());
                    original_packed_bytes = original_packed_bytes.saturating_add(*raw_len);
                    packed_count += 1;
                }
                Entry::Decoded(_) => decoded_count += 1,
                Entry::Missing(_) => missing_count += 1,
            }
        }
        serde_json::json!({
            "clip_count": self.entries.len(),
            "compressed_unused_clip_count": packed_count,
            "compressed_unused_clip_bytes": packed_bytes,
            "unused_clip_source_bytes": original_packed_bytes,
            "decoded_clip_count": decoded_count,
            "missing_clip_count": missing_count,
            "lossless_lazy_compression": true,
            "decoded_native_havok_bytes": "not exposed by the native allocator"
        })
    }
}
