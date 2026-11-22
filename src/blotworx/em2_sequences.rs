//! Original ConversationComponent resources and PC FireSequence admission.
//! A prepared request is not a started SequencePlayer or a completed event.
use anyhow::{Context,Result,ensure};
use std::collections::HashMap;
#[derive(serde::Serialize)]
struct Resource {path:String,key:String,sequence:super::sequence44::Sequence}
#[derive(serde::Serialize)]
struct Start {owner:usize,resource:usize,context:Option<usize>,cursor:usize}
#[derive(serde::Serialize)]
struct ActiveDialog {request:Start,event:usize,line:super::em2_dialogue::Line,selection:usize,blocking:bool}
#[derive(Default)]
pub(super) struct Catalog {
    resources:Vec<Resource>,names:HashMap<String,usize>,registrations:HashMap<usize,Vec<usize>>,
    errors:Vec<serde_json::Value>,pending:Option<Start>,dictionary:Option<super::em2_dialogue::Dictionary>,active:Option<ActiveDialog>,playtest_completed:u64,playtest_interrupted:u64,
}
fn filename(path:&str)->Result<&str> {
    // LoadSequence: prefer the last backslash; the end is the first dot.
    let start=path.rfind('\\').or_else(||path.rfind('/')).context("Sequence declaration has no folder separator")?+1;
    let end=path.find('.').unwrap_or(path.len());ensure!(start<end,"Invalid sequence filename range");Ok(&path[start..end])
}
impl Catalog {
    pub fn load(entities:&[super::assets::RuntimeEntity],files:&[epic_mickey_lib_rs::packfile::VirtualFile])->Result<Self> {
        let mut catalog=Self::default();
        let sources=files.iter().map(|f|(f.path.replace('\\',"/").to_ascii_lowercase(),f)).collect::<HashMap<_,_>>();
        for (owner,entity) in entities.iter().enumerate() {
            for component in entity.components.iter().filter(|c|c.class_name=="ConversationComponent") {
                // The native array property defaults empty. Player/archetype
                // components legitimately omit an empty serialized list.
                let Some(paths)=component.properties.iter().find(|p|p.name=="Sequences").map(|p|&p.value) else{continue;};
                for path in super::actor_scripts::authored_paths(paths).with_context(||format!("Conversation sequence declarations on {}",entity.name))? {
                    let result=(||->Result<usize> {
                        let key=filename(&path)?;
                        if let Some(&id)=catalog.names.get(key){return Ok(id);}
                        let source=sources.get(&path.replace('\\',"/").to_ascii_lowercase()).with_context(||format!("Missing sequence asset {path}"))?;
                        let sequence=super::sequence44::load(&source.data).with_context(||format!("Sequence {path}"))?;
                        let id=catalog.resources.len();catalog.names.insert(key.into(),id);
                        catalog.resources.push(Resource {path:source.path.clone(),key:key.into(),sequence});Ok(id)
                    })();
                    match result {Ok(id)=>catalog.registrations.entry(owner).or_default().push(id),
                        Err(error)=>catalog.errors.push(serde_json::json!({"owner":owner,"entity":entity.name,"path":path,"error":format!("{error:#}")})),}
                }
            }
        }Ok(catalog)
    }
    /// a504f0 checks owner registration before CanStart. Null/unknown decline.
    /// No active player exists until presentation start is actually committed.
    pub fn prepare(&mut self,owner:Option<usize>,name:Option<&str>,context:Option<usize>,player_index:Option<usize>,globals:&super::em2_global_state::State)->Result<bool> {
        let Some((owner,name))=owner.zip(name) else{return Ok(false);};
        let Some(&resource)=self.names.get(name) else{return Ok(false);};
        if !self.registrations.get(&owner).is_some_and(|ids|ids.contains(&resource)){return Ok(false);}
        let sequence=&self.resources[resource].sequence;
        if let Some(active)=self.active.as_ref() {
            let current=&self.resources[active.request.resource].sequence;
            // PC a4ff60..a50017: same bound owner/name, blocking,
            // uninterruptible, then important-current vs ordinary-incoming.
            if !active_admits(active.request.owner,&current.name,current.flags,owner,name,sequence.flags) {return Ok(false);}
        }
        // PC b07200/b07230/b07240: forced-only, Mickey-only, Oswald-only.
        if sequence.flags&0x800!=0 {return Ok(false);}
        if sequence.flags&0x4000!=0 && player_index!=Some(0) {return Ok(false);}
        if sequence.flags&0x8000!=0 && player_index!=Some(1) {return Ok(false);}
        // PC b072c0 checks the once-only mask0x8 before the condition tree.
        if sequence.flags&8!=0 {
            let bytes=sequence.name.as_bytes();let mut played=bytes[..bytes.len().min(127)].to_vec();
            played.extend_from_slice(&b"_Played"[..(127-played.len()).min(7)]);
            if globals.get(std::str::from_utf8(&played)?).is_some_and(|value|value!=0){return Ok(false);}
        }
        if !condition(sequence.condition.tag,&sequence.condition.fields,globals)? {return Ok(false);}
        self.pending=Some(Start {owner,resource,context,cursor:0});Ok(true)
    }
    pub fn enable_playtest(&mut self,root:&std::path::Path,language:&str)->Result<()> {
        if self.dictionary.is_none(){self.dictionary=Some(super::em2_dialogue::Dictionary::load(root,language)?);}Ok(())
    }
    pub fn present_prepared(&mut self,globals:&mut super::em2_global_state::State,mut schedule:impl FnMut(&str,usize)->Result<()>)->Result<bool> {
        let Some(dictionary)=self.dictionary.as_ref() else{return Ok(false)};
        let request=self.pending.as_mut().context("No admitted sequence to present")?;
        let sequence=&self.resources[request.resource].sequence;
        if let Some((event,line))=advance_request(sequence,dictionary,request,globals,&mut schedule)? {
            let blocking=sequence.flags&2!=0;
            let incoming=ActiveDialog{request:self.pending.take().unwrap(),event,line,selection:0,blocking};
            // Only this functional panel is owned here: no native camera or
            // voice is acquired. Count replacement separately from completion.
            if self.active.replace(incoming).is_some(){self.playtest_interrupted+=1;}
        }else{
            finish_request(sequence,globals)?;self.pending=None;if self.active.take().is_some(){self.playtest_interrupted+=1;}self.playtest_completed+=1;
        }Ok(true)
    }
    pub fn blocks_playtest_input(&self)->bool {self.active.as_ref().is_some_and(|a|a.blocking)}
    pub fn answer_playtest(&mut self,answer:usize,globals:&mut super::em2_global_state::State,mut schedule:impl FnMut(&str,usize)->Result<()>)->Result<()> {
        let Some(active)=self.active.as_mut() else{return Ok(())};
        ensure!(answer==0,"Choices require their Apprentice44 choice receiver");
        let sequence=&self.resources[active.request.resource].sequence;
        let dictionary=self.dictionary.as_ref().context("Active dialogue dictionary unavailable")?;
        if let Some((event,line))=advance_request(sequence,dictionary,&mut active.request,globals,&mut schedule)? {
            active.event=event;active.line=line;active.selection=0;
        }else{
            finish_request(sequence,globals)?;self.active=None;self.playtest_completed+=1;
        }Ok(())
    }
    pub fn draw_playtest(&mut self,ui:&egui::Ui,pad_edges:u16,globals:&mut super::em2_global_state::State,schedule:impl FnMut(&str,usize)->Result<()>)->Result<()> {
        let answer=self.active.as_mut().and_then(|a|super::dialogue_panel::draw(ui,&a.line.speaker,&a.line.text,Some(&[]),&mut a.selection,pad_edges));
        if let Some(answer)=answer{self.answer_playtest(answer,globals,schedule)?;}Ok(())
    }
    pub fn pending_description(&self)->String {
        let Some(start)=&self.pending else{return "No prepared sequence".into();};
        let resource=&self.resources[start.resource];
        format!("{} ({}, first event {:?}) requires SequencePlayer presentation start; request retained",resource.key,resource.path,resource.sequence.events.first().map(|e|e.tag))
    }
    pub fn summary(&self)->serde_json::Value {
        serde_json::json!({"resources":self.resources.len(),"registered_entities":self.registrations.len(),"load_errors":self.errors,
            "pending_start":self.pending.as_ref().map(|s|serde_json::json!({"owner":s.owner,"context":s.context,"resource":&self.resources[s.resource]})),
            "active":self.active.is_some(),"playtest_dialogue":self.active,"dictionary":self.dictionary.as_ref().map(|d|&d.source),"playtest_completed":self.playtest_completed,"playtest_interrupted":self.playtest_interrupted,"scope":"Native admission; opt-in shared manual text presentation for dialog events. Authored Lua events use the shared entity scheduler. Voice timing, native UI, speaking animation, completion notifications and other event receivers remain pending"})
    }
}
fn active_admits(current_owner:usize,current_name:&str,current_flags:u16,incoming_owner:usize,incoming_name:&str,incoming_flags:u16)->bool {
    !(current_owner==incoming_owner && current_name==incoming_name)
        && current_flags&2==0 && current_flags&0x20!=0
        && (current_flags&0x10==0 || incoming_flags&0x10!=0)
}

/// SequenceEventLuaCall::Execute schedules source on the bound entity through
/// ScriptSystemThread::ExecuteStringOnThread. It does not wait for completion.
/// Advance only after successful admission, so a later unsupported event cannot
/// cause an earlier Lua event to be scheduled twice when retried.
fn advance_request(sequence:&super::sequence44::Sequence,dictionary:&super::em2_dialogue::Dictionary,request:&mut Start,globals:&mut super::em2_global_state::State,schedule:&mut impl FnMut(&str,usize)->Result<()>)->Result<Option<(usize,super::em2_dialogue::Line)>> {
    while let Some(event)=sequence.events.get(request.cursor) {
        match event.tag {
            0=>{let line=dictionary.line(&event.fields)?;let index=request.cursor;request.cursor+=1;return Ok(Some((index,line)));}
            7=>{if let Some(source)=event.fields["lua"].as_str().filter(|s|!s.is_empty()){schedule(source,request.owner)?;}request.cursor+=1;}
            // multi-line dialog: show the first line (selection mode not reproduced)
            1=>{
                let index=request.cursor;request.cursor+=1;
                if let Some(first)=event.fields["dialogs"].as_array().and_then(|d|d.first()) {
                    return Ok(Some((index,dictionary.line(&serde_json::json!({"dialog":first}))?)));
                }
            }
            // global changes (flag treated as "add to current value"; inferred, not reverse engineered)
            9=>{
                if let Some(changes)=event.fields["changes"].as_array() {for change in changes {
                    let (Some(name),Some(value))=(change["name"].as_str(),change["value"].as_i64()) else {continue};
                    let value=if event.fields["flag"].as_bool()==Some(true) {i64::from(globals.get(name).unwrap_or(0))+value}else{value};
                    if globals.set(name,value.clamp(i16::MIN as i64,i16::MAX as i64) as i32).is_none() {log::warn!("Sequence global {name} is not registered");}
                }}
                request.cursor+=1;
            }
            // timed waits play instantly in the playtest
            10=>{request.cursor+=1;}
            tag=>{
                // Other event receivers (animations, inventory, cameras...) aren't built yet: skip the
                // event instead of stopping the level's scripts, and say so once per kind.
                static SEEN:std::sync::Mutex<Option<std::collections::HashSet<u16>>>=std::sync::Mutex::new(None);
                if SEEN.lock().map(|mut s|s.get_or_insert_with(Default::default).insert(tag)).unwrap_or(false) {
                    log::warn!("EM2 world service not implemented (ignored): SequenceEvent{tag}");
                }
                request.cursor+=1;
            }
        }
    }Ok(None)
}
fn finish_request(sequence:&super::sequence44::Sequence,globals:&mut super::em2_global_state::State)->Result<()> {
    if sequence.flags&8!=0 {
        let bytes=sequence.name.as_bytes();let mut key=bytes[..bytes.len().min(127)].to_vec();key.extend_from_slice(&b"_Played"[..(127-key.len()).min(7)]);
        let key=std::str::from_utf8(&key)?;ensure!(globals.set(key,1).is_some(),"Once-only sequence global {key} is not registered");
    }Ok(())
}

fn condition(tag:u16,fields:&serde_json::Value,globals:&super::em2_global_state::State)->Result<bool> {
    Ok(match tag {
        0|1=>{
            let mut value=tag==0;
            for child in fields["children"].as_array().context("Missing sequence conditions")? {
                let next=condition(child["tag"].as_u64().context("Missing condition tag")? as u16,&child["fields"],globals)?;
                if tag==0 && !next {return Ok(false);}if tag==1 && next {return Ok(true);}value=next;
            }value
        },
        2=>{
            let Some(actual)=fields["name"].as_str().and_then(|name|globals.get(name)) else{return Ok(false);};
            let expected=fields["value"].as_i64().context("Missing condition value")? as i16;
            match fields["comparison"].as_u64().context("Missing condition comparison")? {
                0=>actual==expected,1=>actual!=expected,2=>actual>expected,3=>actual>=expected,4=>actual<expected,5=>actual<=expected,_=>false,
            }
        },
        _=>anyhow::bail!("Apprentice44 condition {tag} requires its original mood/inventory/quest service"),
    })
}
