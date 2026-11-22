//! PC ba2500: acquire event, increment playing counter, register unconditionally,
//! then queue. Unlike EM1, registration precedes enqueue. Playback is asynchronous.
use anyhow::Result;
#[derive(Clone,Debug,serde::Serialize,PartialEq,Eq)]
pub struct Context {
    pub owner:usize,pub playing_id:u32,pub cookie:u32,pub flags:u32,
    pub callback:bool,pub words:[u32;4],
}
impl Context {
    /// PC baa4b7..baa4ce masks callback-only flags when callback is null.
    pub fn effective_flags(&self)->u32 {if self.callback {self.flags}else{self.flags&0xffff2004}}
}
#[derive(Clone,Debug,serde::Serialize)]
pub struct Command {pub event:u32,pub name:String,pub context:Context}
#[derive(Clone,Debug,serde::Serialize)]
#[serde(tag="kind",content="payload",rename_all="snake_case")]
pub enum Pending {
    Event(Command),
    State {group:u32,state:u32,skip_transition:u8,skip_extension:u8},
    /// Script AudioSetSwitch: per game object switch value.
    Switch {owner:usize,group:u32,value:u32},
    /// Script AudioStopPlaying*: a playing id, an event (by id) on an owner, or everything.
    Stop {owner:Option<usize>,playing_id:Option<u32>,event:Option<u32>},
    /// Script AudioSetParameter: game parameter (RTPC) value, global when owner is None.
    Parameter {owner:Option<usize>,id:u32,value:f32},
}
pub trait Host {
    type Event:Copy;
    fn acquire(&mut self,event:u32)->Result<Option<Self::Event>>;
    fn register(&mut self,event:Self::Event,context:&Context)->Result<u32>;
    fn release(&mut self,event:Self::Event);
    fn enqueue(&mut self,event:Self::Event,context:Context)->Result<()>;
}
#[derive(Default)]
pub struct Producer {pub counter:u32}
impl Producer {
    pub fn post(&mut self,event:u32,mut context:Context,host:&mut impl Host)->Result<u32> {
        let Some(handle)=host.acquire(event)? else{return Ok(0)};
        self.counter=self.counter.wrapping_add(1);context.playing_id=self.counter;
        if host.register(handle,&context)?!=1 {host.release(handle);return Ok(0);}
        host.enqueue(handle,context)?;
        Ok(self.counter)
    }
}
#[derive(Default)]
pub struct State {
    producer:Producer,cookie:u32,unmute_requests:u64,last_unmute_playing_id:Option<u32>,
    pending:std::collections::VecDeque<Pending>,
    registrations:Vec<Context>,references:std::collections::BTreeMap<u32,u64>,
    prepared:Option<serde_json::Value>,error:Option<String>,
}
impl State {
    pub fn post(&mut self,catalog:&super::em2_audio::Catalog,owner:usize,name:&str)->Result<u32> {
        let event=super::em2_audio::event_id(name)?;
        // PC a4d47e increments its wrapper cookie even if native event lookup
        // subsequently fails. Both counters are zero-initialized BSS fields.
        self.cookie=self.cookie.wrapping_add(1);
        let context=Context {owner,playing_id:0,cookie:self.cookie,flags:0x2705,callback:true,words:[0;4]};
        self.post_context(catalog,event,name,context)
    }
    /// PC a7c900 bypasses the scene wrapper: native global object, no cookie,
    /// callback or flags. Actual audio execution remains a separate consumer.
    pub fn unmute(&mut self,catalog:&super::em2_audio::Catalog)->Result<u32> {
        self.unmute_requests+=1;
        let event=super::em2_audio::event_id("UnMute_All")?;
        let id=self.post_context(catalog,event,"UnMute_All",Context{owner:0x7fffffff,playing_id:0,cookie:0,flags:0,callback:false,words:[0;4]})?;
        self.last_unmute_playing_id=Some(id);Ok(id)
    }
    fn post_context(&mut self,catalog:&super::em2_audio::Catalog,event:u32,name:&str,context:Context)->Result<u32> {
        struct Binding<'a> {
            catalog:&'a super::em2_audio::Catalog,name:&'a str,
            pending:&'a mut std::collections::VecDeque<Pending>,registrations:&'a mut Vec<Context>,references:&'a mut std::collections::BTreeMap<u32,u64>,
        }
        impl Host for Binding<'_> {
            type Event=u32;
            fn acquire(&mut self,event:u32)->Result<Option<u32>> {
                if !self.catalog.has_event(event){return Ok(None)}
                *self.references.entry(event).or_default()+=1;Ok(Some(event))
            }
            fn register(&mut self,_:u32,context:&Context)->Result<u32>{if self.registrations.try_reserve(1).is_err(){return Ok(2)}self.registrations.push(context.clone());Ok(1)}
            fn release(&mut self,event:u32){if let Some(count)=self.references.get_mut(&event){*count-=1;}}
            fn enqueue(&mut self,event:u32,context:Context)->Result<()>{self.pending.push_back(Pending::Event(Command {event,name:self.name.into(),context}));Ok(())}
        }
        self.producer.post(event,context,&mut Binding {catalog,name,pending:&mut self.pending,registrations:&mut self.registrations,references:&mut self.references})
    }
    /// PC ba0980 command7 maps the hash of "None" to numeric state0 and
    /// initializes both transition/extension suppression flags to zero.
    pub fn set_state(&mut self,group:u32,state:u32) {
        self.pending.push_back(Pending::State {group,state:if state==0x2ca33bdb {0}else{state},skip_transition:0,skip_extension:0});
    }
    /// Hand every queued command to the EM2 Studio playback layer (em2_audio_player).
    pub fn drain(&mut self)->Vec<Pending> {self.error=None;self.pending.drain(..).collect()}
    pub fn push(&mut self,pending:Pending) {self.pending.push_back(pending);}
    pub fn prepare_pending(&mut self,catalog:&super::em2_audio::Catalog) {
        // diagnostic only (reads bank files from disk); events are played by em2_audio_player
        if std::env::var_os("BLOTWORX_AUDIO_DIAG").is_none() {return}
        if self.error.is_some(){return}
        let Some(Pending::Event(command))=self.pending.front() else{
            if !self.pending.is_empty(){self.error=Some("PC state-message consumer is unconnected; command retained".into());}
            return;
        };
        match catalog.prepare_direct(&command.name) {
            Ok(plans)=>match serde_json::to_value(plans) {
                Ok(value)=>{self.prepared=Some(value);self.error=Some("PC action/node execution, audio object context and source callbacks remain unconnected; command retained".into());}
                Err(error)=>self.error=Some(error.to_string()),
            },
            Err(error)=>self.error=Some(format!("{}: {error:#}",command.name)),
        }
    }
    pub fn summary(&self)->serde_json::Value {
        serde_json::json!({"unmute_requests":self.unmute_requests,"last_unmute_playing_id":self.last_unmute_playing_id,"post_counter":self.producer.counter,"wrapper_cookie":self.cookie,"pending":self.pending,
            "registrations":self.registrations.len(),"event_references":self.references,"prepared_sources":self.prepared,"backend_error":self.error,
            "scope":"verified PC producer admission and retained FIFO; audio consumer, callbacks, object lifecycle and playback remain unconnected",
            "completed_commands":0,"playback_connected":false})
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Control {present:bool,status:u32,calls:Vec<&'static str>,registered:Option<Context>,queued:Option<Context>}
    impl Host for Control {
        type Event=u32;
        fn acquire(&mut self,id:u32)->Result<Option<u32>>{self.calls.push("acquire");Ok(self.present.then_some(id))}
        fn register(&mut self,_:u32,c:&Context)->Result<u32>{self.calls.push("register");self.registered=Some(c.clone());Ok(self.status)}
        fn release(&mut self,_:u32){self.calls.push("release");}
        fn enqueue(&mut self,_:u32,c:Context)->Result<()>{self.calls.push("enqueue");self.queued=Some(c);Ok(())}
    }
    #[test]
    fn pc_registration_precedes_queue_even_without_callback_and_failure_releases_event()->Result<()> {
        for (present,status,expected,calls) in [(false,1,0,vec!["acquire"]),(true,2,0,vec!["acquire","register","release"]),(true,1,1,vec!["acquire","register","enqueue"])] {
            let mut producer=Producer::default();let mut host=Control {present,status,calls:vec![],registered:None,queued:None};
            let context=Context {owner:7,playing_id:0,cookie:9,flags:0xffff,callback:false,words:[1,2,3,4]};
            assert_eq!(producer.post(42,context,&mut host)?,expected);assert_eq!(host.calls,calls);
            assert_eq!(producer.counter,u32::from(present));
            if let Some(c)=host.registered {assert_eq!(c.words,[1,2,3,4]);assert_eq!(c.effective_flags(),0x2004);}
        }
        let mut producer=Producer {counter:u32::MAX};let mut host=Control {present:true,status:1,calls:vec![],registered:None,queued:None};
        assert_eq!(producer.post(42,Context {owner:0,playing_id:99,cookie:0,flags:0,callback:true,words:[0;4]},&mut host)?,0);
        assert_eq!(host.calls,["acquire","register","enqueue"]);
        Ok(())
    }
    #[test]
    fn producer_order_counter_and_context_match_original_pc_instructions()->Result<()> {
        let reference:serde_json::Value=serde_json::from_str(include_str!("../../tools/blotworx/fixtures/em2_audio_post_reference.json"))?;
        for case in reference["cases"].as_array().unwrap() {
            let input=&case["input"];let output=&case["output"];
            let mut producer=Producer {counter:input["counter"].as_u64().unwrap() as u32};
            let words=std::array::from_fn(|i|input["words"][i].as_u64().unwrap() as u32);
            let mut host=Control {present:input["present"].as_bool().unwrap(),status:input["status"].as_u64().unwrap() as u32,calls:vec![],registered:None,queued:None};
            let result=producer.post(0x2e745d47,Context {owner:77,playing_id:0,cookie:9,flags:0x2705,callback:true,words},&mut host)?;
            assert_eq!(result,output["return"].as_u64().unwrap() as u32);
            assert_eq!(producer.counter,output["counter"].as_u64().unwrap() as u32);
            assert_eq!(serde_json::json!(host.calls),output["calls"]);
            if let Some(registered)=host.registered {
                assert_eq!(serde_json::json!(registered.words),output["registered"]["words"]);
                assert_eq!(u64::from(registered.playing_id),output["registered"]["playing_id"].as_u64().unwrap());
            }
            if !output["queued"].is_null() {
                assert_eq!(serde_json::json!(host.queued.as_ref().unwrap().words),output["queued"]["words"]);
                assert_eq!(output["queued"]["playing_id"],output["return"]);
            }
        }
        Ok(())
    }
    #[test]
    fn numeric_state_producer_maps_none_and_does_not_allocate_playing_ids() {
        let mut state=State::default();state.set_state(0xe414d158,0x2ca33bdb);
        state.set_state(0xe414d158,0x811c9dc5);
        assert_eq!(state.producer.counter,0);assert_eq!(state.cookie,0);
        assert!(matches!(state.pending[0],Pending::State {state:0,skip_transition:0,skip_extension:0,..}));
        assert!(matches!(state.pending[1],Pending::State {state:0x811c9dc5,..}));
    }
}
