//! PC version72 audio catalog. Loads authored level-bank references and
//! resolves original event/action records. Voice execution is not yet wired.
use super::audio_bank_framing::{Bank, Format};
use anyhow::{ensure, Context, Result};
use std::{collections::{BTreeMap, BTreeSet}, path::{Path, PathBuf}};

#[derive(serde::Serialize)]
struct BankInfo { name: String, id: u32, language: u32, objects: usize, media: usize, bytes: usize }
#[derive(Clone, serde::Serialize)]
pub struct Event { pub bank: String, pub actions: Vec<u32> }
#[derive(Default)]
pub struct Catalog {
    banks: Vec<BankInfo>,
    events: BTreeMap<u32, Vec<Event>>,
    errors: Vec<String>,
    paths: BTreeMap<u32,Vec<PathBuf>>,
    nodes:BTreeMap<u32,Vec<NodeRecord>>,
}
#[derive(Clone,Debug,serde::Serialize)]
pub struct NodeRecord {pub id:u32,pub kind:u32,pub definition:super::audio_node72::Node,pub source_banks:Vec<String>}
#[derive(serde::Serialize)]
pub struct DirectPlan {
    pub action:u32,pub sound:u32,pub source:super::audio_codec::Source,
    pub play_flags:u8,
    pub source_record:serde_json::Value,pub action_record:serde_json::Value,
    pub ancestry:Vec<NodeRecord>,
}
pub fn event_id(name: &str) -> Result<u32> {
    ensure!(name.is_ascii() && !name.contains('\0'), "PC non-ASCII audio-name conversion is not reconstructed");
    Ok(name.bytes().fold(0x811c9dc5u32, |hash, b| hash.wrapping_mul(0x01000193) ^ b.to_ascii_lowercase() as u32))
}
fn file(root: &Path, name: &str) -> Result<Option<PathBuf>> {
    if !root.is_dir() {return Ok(None);}
    for entry in std::fs::read_dir(root)? {
        let entry=entry?;
        if entry.file_name().to_string_lossy().eq_ignore_ascii_case(name) {return Ok(Some(entry.path()));}
    }
    Ok(None)
}
impl Catalog {
    pub fn load(root: &Path, language: &str, names: &[String]) -> Self {
        let mut result = Self::default();
        let mut seen = BTreeSet::new();
        // PC a7d576..a7d5a2 loads wide-string Init.bnk and waits for its
        // completion before level-bank admission. Mixer initialization is
        // separate from making the original bank definitions available.
        // MainMenu.lvlc declares Global.bnk; PC AudioAssetManager::UnloadBank
        // b13b81..b13b91 explicitly protects it across level transitions.
        // Direct level bootstraps must retain that persistent boot bank too.
        // global_localized.bnk (Localize/<language>) holds Mickey's and Oswald's voice sounds
        // (jump grunt, double jump, fall yell...) and is resident like Global.bnk.
        let initial=["Init.bnk".to_owned(),"Global.bnk".to_owned(),"global_localized.bnk".to_owned()];
        for name in initial.iter().chain(names) {
            if !seen.insert(name.to_ascii_lowercase()) {continue;}
            let loaded = (|| -> Result<()> {
                ensure!(!name.contains(['/', '\\', ':']) && name != "..", "Unsafe audio bank name");
                ensure!(!language.contains(['/', '\\', ':']) && language != "..", "Unsafe audio language");
                let directory=root.join("Audio/Win32");
                let path=file(&directory,name)?.or(file(&directory.join("Localize").join(language),name)?)
                    .with_context(||format!("Missing authored PC bank {name} ({language})"))?;
                let bytes=std::fs::read(&path)?;
                let bank=Bank::parse(&bytes,Format::Pc72)?;
                let mut entries=Vec::new();
                let mut node_entries=Vec::new();
                for object in &bank.objects {
                    if object.kind == 4 {
                        entries.push((object.id,Event {bank:name.clone(),actions:bank.event_actions(object.id)?}));
                    }
                    if [2,5,6,7,9].contains(&object.kind) {
                        let bytes=if object.kind==2 {super::audio_source72::Source::parse(object.payload)?.node_parameters}else{object.payload};
                        let (definition,remaining)=super::audio_node72::Node::parse(bytes,Format::Pc72.word(bank.header,12)?&1!=0)?;
                        ensure!(object.kind!=2 || remaining.is_empty(),"PC sound has unparsed trailing node bytes");
                        node_entries.push(NodeRecord {id:object.id,kind:object.kind,definition,source_banks:vec![name.clone()]});
                    }
                }
                // Admit atomically after every event record validates. Bank
                // media is not copied or retained while voice decoding is absent.
                result.banks.push(BankInfo {name:name.clone(),id:Format::Pc72.word(bank.header,4)?,language:Format::Pc72.word(bank.header,8)?,objects:bank.objects.len(),media:bank.media.len(),bytes:bytes.len()});
                result.paths.entry(Format::Pc72.word(bank.header,4)?).or_default().push(path);
                for (id,event) in entries {result.events.entry(id).or_default().push(event);}
                for mut node in node_entries {
                    let variants=result.nodes.entry(node.id).or_default();
                    if let Some(previous)=variants.iter_mut().find(|p|p.kind==node.kind && p.definition==node.definition) {previous.source_banks.append(&mut node.source_banks);}
                    else {variants.push(node);}
                }
                Ok(())
            })();
            if let Err(error)=loaded {result.errors.push(format!("{name}: {error:#}"));}
        }
        result
    }
    pub fn has_event(&self,id:u32)->bool {self.events.contains_key(&id)}
    /// Paths of every mounted bank, in load order (Init, Global, then the level's banks).
    pub fn bank_files(&self)->Vec<PathBuf> {
        self.banks.iter().filter_map(|b|self.paths.get(&b.id).and_then(|p|p.first().cloned())).collect()
    }
    pub fn event(&self, name: &str) -> Result<&[Event]> {
        let id=event_id(name)?;
        self.events.get(&id).map(Vec::as_slice).with_context(||format!("PC audio event {name} ({id:08x}) is absent from mounted level banks"))
    }
    fn bank_path(&self,id:u32)->Result<&Path> {
        let paths=self.paths.get(&id).with_context(||format!("PC source bank {id:08x} is not mounted"))?;
        ensure!(paths.len()==1,"Ambiguous PC source bank {id:08x}");Ok(&paths[0])
    }
    fn ancestry(&self,mut id:u32)->Result<Vec<NodeRecord>> {
        let mut seen=BTreeSet::new();let mut nodes=Vec::new();
        while id!=0 {
            ensure!(seen.insert(id),"Cycle in PC audio parameter ancestry at {id:08x}");
            let variants=self.nodes.get(&id).with_context(||format!("PC audio parent {id:08x} is not mounted"))?;
            ensure!(variants.len()==1,"PC audio parent {id:08x} has conflicting loaded definitions");
            let node=&variants[0];id=node.definition.parent;nodes.push(node.clone());
        }
        Ok(nodes)
    }
    /// Resolve direct Play -> Sound -> embedded media without executing an
    /// action or bypassing its property bundles, bus chain or spatial state.
    pub fn prepare_direct(&self,name:&str)->Result<Vec<DirectPlan>> {
        let events=self.event(name)?;ensure!(events.len()==1,"PC duplicate event registration is not reconstructed");
        let event=&events[0];
        let info=self.banks.iter().find(|b|b.name==event.bank).context("Event bank disappeared")?;
        let bytes=std::fs::read(self.bank_path(info.id)?)?;
        let bank=Bank::parse(&bytes,Format::Pc72)?;
        let mut plans=Vec::new();
        for &action_id in &event.actions {
            let mut objects=bank.objects.iter().filter(|o|o.id==action_id && o.kind==3);
            let object=objects.next().context("PC event action is not in its source bank")?;
            ensure!(objects.next().is_none(),"Ambiguous PC action identity");
            let action=super::audio_source72::Action::parse(object.payload)?;
            let source_path=self.bank_path(action.play_bank()?)?;
            let source_bytes=std::fs::read(source_path)?;
            let source_bank=Bank::parse(&source_bytes,Format::Pc72)?;
            let mut sounds=source_bank.objects.iter().filter(|o|o.id==action.target && o.kind==2);
            let sound=sounds.next().with_context(||format!("PC action target {:08x} requires container/node execution",action.target))?;
            ensure!(sounds.next().is_none(),"Ambiguous PC sound identity");
            let source=super::audio_source72::Source::parse(sound.payload)?;
            ensure!(source.stream_type==0,"PC streamed/prefetched media requires the original stream resolver");
            ensure!(source.file_id==action.play_bank()?,"PC sound source crosses an unresolved bank boundary");
            let mut media=source_bank.media.iter().enumerate().filter(|(_,m)|m.id==source.media_id);
            let (index,media_record)=media.next().context("PC embedded source media is missing")?;
            ensure!(media.next().is_none(),"Ambiguous PC embedded media identity");
            let offset=media_record.bytes.as_ptr() as usize-source_bytes.as_ptr() as usize;
            ensure!(source.memory==Some([u32::try_from(offset)?,u32::try_from(media_record.bytes.len())?]),"PC source range differs from original media index");
            plans.push(DirectPlan {action:action_id,sound:action.target,play_flags:action.specific[0],source:super::audio_codec::Source {bank:source_path.into(),subsong:u32::try_from(index+1)?,media_id:source.media_id},
                source_record:serde_json::to_value(&source)?,action_record:serde_json::to_value(&action)?,ancestry:self.ancestry(action.target)?});
        }
        Ok(plans)
    }
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({"banks":self.banks,"events":self.events.len(),"parameter_nodes":self.nodes.len(),"load_errors":self.errors,
            "scope":"Init/level bank metadata, event references and parameter ancestry; action execution, spatialization, mixing and global mixer initialization remain unconnected",
            "encoded_media_retained":false,"playback_connected":false})
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn ascii_event_identity_matches_original_ambience_bank_and_preserves_native_case_fold() {
        assert_eq!(super::event_id("Play_sfx_MHv1_clock_tick").unwrap(),779377991);
        assert_eq!(super::event_id("PLAY_SFX_MHV1_CLOCK_TICK").unwrap(),779377991);
        assert!(super::event_id("bad\0suffix").is_err());
        assert!(super::event_id("é").is_err());
    }
}
