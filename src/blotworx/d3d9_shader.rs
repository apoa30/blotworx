//! Platform shader resources shared by native game adapters. This discovers
//! bounded SM2/SM3 programs inside original effect resources; effect technique
//! evaluation and GPU execution must not be inferred from discovery alone.
use anyhow::{ensure, Context, Result};
use std::{collections::BTreeMap, ops::Range, path::{Path,PathBuf}, sync::Arc};

#[derive(Debug,serde::Serialize)]
pub(super) struct Program {
    pub stage:&'static str,
    pub offset:usize,
    pub bytes:usize,
    pub instructions:usize,
    pub opcodes:BTreeMap<u16,usize>,
    pub constant_count:usize,
    #[serde(skip_serializing)]
    pub constants:Option<super::d3d9_constants::Table>,
}
pub(super) struct Effect { pub data:Arc<Vec<u8>>, pub programs:Vec<Program>,pub layout:Result<super::d3d9_effect::Effect,String>,pub binding_annotations:Vec<super::d3d9_bindings::Binding> }
#[derive(Debug,Clone,serde::Serialize)]
pub(super) struct TechniqueBinding {
    pub technique:usize,pub name:String,pub shader_name:String,pub implementation:u32,
    pub uses_ni_render_state:bool,pub uses_ni_light_state:bool,
}
pub(super) struct MaterialEffect {pub effect:Arc<Effect>,pub binding:TechniqueBinding}
fn technique_bindings(data:&[u8],layout:&super::d3d9_effect::Effect)->Result<Vec<TechniqueBinding>> {
    use super::d3d9_effect::Data;
    layout.techniques.iter().enumerate().map(|(index,t)|{
        let mut shader_name=None;let mut implementation=0;let mut render=false;let mut lights=false;
        for value in &t.annotations {
            match value.name.as_deref().map(str::to_ascii_lowercase).as_deref() {
                Some("shadername")=>{ensure!(value.kind==4,"Technique ShaderName is not text");shader_name=super::d3d9_bindings::string(data,layout,value)?;},
                Some(field @ ("implementation"|"usesnirenderstate"|"usesnilightstate"))=>{
                    let Data::Words(words)=&value.data else{anyhow::bail!("Technique {field} is not numeric")};
                    ensure!(words.len()==1,"Technique {field} is not scalar");
                    if field=="implementation" {ensure!(value.kind==2,"Technique implementation is not an integer");implementation=words[0];}
                    else {ensure!(matches!(value.kind,1|2),"Technique {field} requires its boolean conversion");if field=="usesnirenderstate" {render=words[0]!=0;}else{lights=words[0]!=0;}}
                },
                _=>{},
            }
        }
        let name=t.name.clone().context("Original effect technique is unnamed")?;
        Ok(TechniqueBinding {technique:index,shader_name:shader_name.unwrap_or_else(||name.clone()),name,implementation,uses_ni_render_state:render,uses_ni_light_state:lights})
    }).collect()
}
fn word(data:&[u8],offset:usize)->Result<u32> {
    Ok(u32::from_le_bytes(data.get(offset..offset.checked_add(4).context("Shader offset overflow")?).context("Truncated shader word")?.try_into()?))
}
fn supported(version:u32)->bool {matches!(version,0xfffe0200|0xfffe0201|0xffff0200|0xffff0201|0xfffe0300|0xffff0300)}
fn program(data:&[u8],start:usize)->Result<Program> {
    let version=word(data,start)?;ensure!(supported(version),"Not a supported SM2/SM3 program");
    let stage=if version>>16==0xfffe {"vertex"}else{"pixel"};
    let mut cursor=start+4;let mut instructions=0;let mut opcodes=BTreeMap::new();let mut constants=None;
    loop {
        let token=word(data,cursor)?;let opcode=(token&0xffff) as u16;
        cursor+=4;
        if opcode==0xffff {
            ensure!(token==0x0000ffff,"Malformed shader terminator");
            ensure!(instructions>0,"Empty shader instruction stream");
            return Ok(Program {stage,offset:start,bytes:cursor-start,instructions,opcodes,
                constant_count:constants.as_ref().map_or(0,|table:&super::d3d9_constants::Table|table.constants.len()),constants});
        }
        let parameters=if opcode==0xfffe {(token>>16)&0x7fff}else{
            ensure!(opcode<=96,"Unknown SM3 opcode {opcode}");
            instructions+=1;*opcodes.entry(opcode).or_insert(0)+=1;
            (token>>24)&15
        };
        let end=cursor.checked_add(parameters as usize*4).context("Shader instruction overflow")?;
        if opcode==0xfffe && data.get(cursor..cursor+4)==Some(b"CTAB") {
            ensure!(constants.is_none(),"Multiple CTAB blocks in shader");
            constants=Some(super::d3d9_constants::Table::parse(data.get(cursor+4..end).context("Truncated CTAB comment")?,word(data,start)?)?);
        }
        cursor=end;
        ensure!(cursor<=data.len(),"Truncated SM3 instruction/comment");
    }
}
impl Effect {
    pub fn technique_bindings(&self)->Result<Vec<TechniqueBinding>> {
        technique_bindings(&self.data,self.layout.as_ref().map_err(|e|anyhow::anyhow!("{e}"))?)
    }
    pub fn resolve_textures<R:super::d3d9_bindings::Textures>(&self,receiver:&mut R)->Result<BTreeMap<usize,Option<R::Texture>>> {
        super::d3d9_bindings::resolve_textures(self.layout.as_ref().map_err(|error|anyhow::anyhow!("{error}"))?,&self.binding_annotations,receiver)
    }
    pub fn render_states(&self,pass:&TranslatedPass)->Result<Vec<super::d3d9_state::State>> {
        super::d3d9_state::resolve(self.layout.as_ref().map_err(|error|anyhow::anyhow!("{error}"))?,pass.technique,pass.pass)
    }
    pub fn sampler_bindings(&self,pass:&TranslatedPass)->Result<PassSamplers> {
        let layout=self.layout.as_ref().map_err(|error|anyhow::anyhow!("{error}"))?;
        let stage=|index:usize|->Result<Vec<super::d3d9_sampler::Sampler>> {
            let program=self.programs.get(index).context("Pass sampler shader is absent")?;
            super::d3d9_sampler::resolve(&self.data,layout,program.constants.as_ref(),&self.binding_annotations)
        };
        Ok(PassSamplers {vertex:stage(pass.vertex_program)?,pixel:stage(pass.pixel_program)?})
    }
    /// Resolve the authored pass explicitly. Technique zero is often a depth
    /// prepass, so treating it as the visible material would be incorrect.
    pub fn translate_pass(&self,technique:&str,pass:&str)->Result<TranslatedPass> {
        let layout=self.layout.as_ref().map_err(|error|anyhow::anyhow!("{error}"))?;
        let mut techniques=layout.techniques.iter().enumerate().filter(|(_,t)|t.name.as_deref()==Some(technique));
        let (ti,t)=techniques.next().context("Original effect technique is absent")?;
        ensure!(techniques.next().is_none(),"Ambiguous original effect technique");
        let mut passes=t.passes.iter().enumerate().filter(|(_,p)|p.name.as_deref()==Some(pass));
        let (pi,_)=passes.next().context("Original effect pass is absent")?;
        ensure!(passes.next().is_none(),"Ambiguous original effect pass");
        let pair=layout.shader_pair(&self.data,ti,pi)?;
        let translate=|shader:Option<super::d3d9_effect::Shader>,stage:&str|->Result<(usize,super::d3d9_translate::Translation)> {
            let range=match shader {
                Some(super::d3d9_effect::Shader::Program {version,payload})=>{
                    ensure!(supported(version),"Original {stage} shader model requires its legacy decoder");payload
                },
                Some(super::d3d9_effect::Shader::Selector {..})=>anyhow::bail!("Original {stage} shader requires its effect selector evaluation"),
                _=>anyhow::bail!("Original {stage} shader requires inherited or fixed-function state"),
            };
            let index=self.programs.iter().position(|p|p.offset==range.start&&p.bytes==range.len()).context("Authored shader differs from discovered program")?;
            Ok((index,super::d3d9_translate::Translation::parse(self.data.get(range).context("Shader outside effect")?)?))
        };
        let (vertex_program,vertex)=translate(pair.vertex,"vertex")?;
        let (pixel_program,pixel)=translate(pair.pixel,"pixel")?;
        Ok(TranslatedPass {technique:ti,pass:pi,vertex_program,pixel_program,vertex,pixel})
    }
    pub fn numeric_inputs<'a>(&'a self,pass:&'a TranslatedPass)->Result<std::collections::BTreeSet<&'a str>> {
        let mut names=std::collections::BTreeSet::new();
        for index in [pass.vertex_program,pass.pixel_program] {
            if let Some(table)=&self.programs[index].constants {for constant in &table.constants {if constant.register_set!=3 {names.insert(constant.name.as_str());}}}
        }
        for translation in [&pass.vertex,&pass.pixel] {for cpu in &translation.cpu_programs {
            let cpu=cpu.as_ref().map_err(|e|anyhow::anyhow!("{e}"))?;
            for constant in &cpu.constants.constants {if constant.register_set!=3 {names.insert(constant.name.as_str());}}
        }}Ok(names)
    }
    pub fn requires_mesh_time(&self,technique:usize)->Result<bool> {
        let layout=self.layout.as_ref().map_err(|error|anyhow::anyhow!("{error}"))?;
        let mut times=Vec::new();
        for binding in &self.binding_annotations {
            if binding.text("VarType")?.is_some_and(|v|v.eq_ignore_ascii_case("Predefined"))&&binding.text("DefinedMapping")?.is_some_and(|v|["time","sin_time","cos_time","tan_time","time_sin_cos_tan"].iter().any(|name|v.eq_ignore_ascii_case(name))) {
                times.push(layout.parameters[binding.parameter].value.name.as_deref().context("Time parameter is unnamed")?);
            }
        }
        if times.is_empty(){return Ok(false)}
        let technique=layout.techniques.get(technique).context("Time technique is absent")?;
        for pass in &technique.passes {
            let source=self.translate_pass(technique.name.as_deref().context("Time technique is unnamed")?,pass.name.as_deref().context("Time pass is unnamed")?)?;
            let inputs=self.numeric_inputs(&source)?;
            if times.iter().any(|name|inputs.contains(name)){return Ok(true)}
        }Ok(false)
    }
    /// Resolve both direct GPU constants and CPU expressions from the same
    /// effect parameter set. Shader-defined DEF constants remain in GPU code.
    pub fn constant_banks(&self,pass:&TranslatedPass,overrides:&super::d3d9_draw::Params,precision:super::d3d9_preshader::TemporaryPrecision)->Result<PassConstants> {
        let layout=self.layout.as_ref().map_err(|error|anyhow::anyhow!("{error}"))?;
        let stage=|index:usize,translation:&super::d3d9_translate::Translation,limit:usize|->Result<StageConstants>{
            let program=self.programs.get(index).context("Pass shader program is absent")?;
            let mut floats=if let Some(table)=&program.constants {super::d3d9_preshader::bind_float_constants(table,layout,overrides)?}else{Vec::new()};
            ensure!(floats.len()<=limit*4,"Effect constants exceed SM3 stage register bank");
            floats.resize(limit*4,0.);let mut booleans=vec![0;16];
            for cpu in &translation.cpu_programs {
                let cpu=cpu.as_ref().map_err(|error|anyhow::anyhow!("{error}"))?;
                let inputs=super::d3d9_preshader::bind_float_constants(&cpu.constants,layout,overrides)?;
                cpu.evaluate(&inputs,&mut floats,&mut booleans,precision)?;
            }
            Ok(StageConstants {floats,integers:vec![0;16*4],booleans})
        };
        Ok(PassConstants {vertex:stage(pass.vertex_program,&pass.vertex,256)?,pixel:stage(pass.pixel_program,&pass.pixel,224)?})
    }
    pub fn discover(data:Vec<u8>)->Result<Self> {
        ensure!(word(&data,0)?==0xfeff0901,"Unsupported compiled D3D9 effect version");
        let layout=super::d3d9_effect::Effect::parse(&data)?;
        let mut programs=Vec::new();
        for object in &layout.objects {
            if object.payload.len()<4||object.source_kind==Some(2) {continue;}
            if supported(word(&data,object.payload.start)?) {
                let p=program(&data,object.payload.start)?;
                ensure!(p.bytes==object.payload.len(),"Shader stream does not fill its effect object");
                programs.push(p);
            }
        }
        programs.sort_by_key(|p|p.offset);programs.dedup_by_key(|p|p.offset);
        ensure!(!programs.is_empty(),"Effect contains no authored SM2/SM3 programs");
        let binding_annotations=super::d3d9_bindings::inspect(&data,&layout)?;
        let layout=Ok(layout);
        Ok(Self {data:Arc::new(data),programs,layout,binding_annotations})
    }
    pub fn bytecode(&self,program:&Program)->Result<&[u8]> {
        let range:Range<usize>=program.offset..program.offset.checked_add(program.bytes).context("Shader range overflow")?;
        self.data.get(range).context("Shader program is outside its effect")
    }
    pub fn translate(&self,program:&Program)->Result<super::d3d9_translate::Translation> {
        super::d3d9_translate::Translation::parse(self.bytecode(program)?)
    }
}
/// CPU programs remain attached to each stage. A renderer must evaluate them,
/// apply the original pass states and bind resources before submitting a draw.
pub(super) struct TranslatedPass {
    pub technique:usize,pub pass:usize,
    pub vertex_program:usize,pub pixel_program:usize,
    pub vertex:super::d3d9_translate::Translation,
    pub pixel:super::d3d9_translate::Translation,
}
pub(super) struct StageConstants {pub floats:Vec<f32>,pub integers:Vec<i32>,pub booleans:Vec<u32>}
pub(super) struct PassConstants {pub vertex:StageConstants,pub pixel:StageConstants}
pub(super) struct PassSamplers {pub vertex:Vec<super::d3d9_sampler::Sampler>,pub pixel:Vec<super::d3d9_sampler::Sampler>}
pub(super) struct Catalog {
    archive:PathBuf,
    entries:BTreeMap<String,crate::pack_io::Member>,
    loaded:BTreeMap<String,Result<Arc<Effect>,String>>,
    aliases:Option<Result<BTreeMap<String,Vec<(String,TechniqueBinding)>>,String>>,
}
fn key(name:&str)->String {
    let normalized=name.replace('\\',"/");
    let leaf=normalized.rsplit('/').next().unwrap_or(&normalized).to_ascii_lowercase();
    leaf.strip_suffix(".fxo").unwrap_or(&leaf).to_owned()
}
impl Catalog {
    pub fn open(archive:&Path)->Result<Self> {
        let mut entries=BTreeMap::new();
        for member in crate::pack_io::index(archive)? {
            if member.path.to_ascii_lowercase().ends_with(".fxo") {
                ensure!(entries.insert(key(&member.path),member).is_none(),"Ambiguous compiled effect name");
            }
        }
        Ok(Self {archive:archive.to_owned(),entries,loaded:BTreeMap::new(),aliases:None})
    }
    /// NIF material names are registered ShaderName values, not FXO filenames.
    /// Retain implementation/technique identity; never select technique zero
    /// merely because its containing effect happened to match a filename.
    pub fn get_material(&mut self,name:&str,implementation:Option<u32>)->Result<MaterialEffect,String> {
        if self.aliases.is_none() {
            let result=(||->Result<BTreeMap<String,Vec<(String,TechniqueBinding)>>> {
                let mut aliases:BTreeMap<String,Vec<(String,TechniqueBinding)>>=BTreeMap::new();
                for (file,member) in &self.entries {
                    let data=member.read(&self.archive)?;let layout=super::d3d9_effect::Effect::parse(&data)?;
                    for binding in technique_bindings(&data,&layout)? {aliases.entry(binding.shader_name.to_ascii_lowercase()).or_default().push((file.clone(),binding));}
                }
                Ok(aliases)
            })().map_err(|e|format!("Original technique registry: {e:#}"));
            self.aliases=Some(result);
        }
        let aliases=self.aliases.as_ref().unwrap().as_ref().map_err(Clone::clone)?;
        let candidates=aliases.get(&name.to_ascii_lowercase()).ok_or_else(||format!("Missing registered original shader {name}"))?;
        let mut selected=candidates.iter().filter(|(_,binding)|implementation.is_none_or(|i|binding.implementation==i));
        let (file,binding)=selected.next().ok_or_else(||format!("Original shader {name} has no requested implementation {implementation:?}"))?;
        ensure_unique_material(selected.next().is_none(),name)?;
        let (file,binding)=(file.clone(),binding.clone());
        Ok(MaterialEffect {effect:self.get(&file)?,binding})
    }
    pub fn get(&mut self,name:&str)->Result<Arc<Effect>,String> {
        let name=key(name);
        if !self.loaded.contains_key(&name) {
            let result=(||->Result<Arc<Effect>> {
                let member=self.entries.get(&name).with_context(||format!("Missing original compiled effect {name}"))?;
                Ok(Arc::new(Effect::discover(member.read(&self.archive)?)?))
            })().map_err(|e|format!("{name}: {e:#}"));
            self.loaded.insert(name.clone(),result);
        }
        self.loaded[&name].clone()
    }
    pub fn summary(&self)->serde_json::Value {
        let loaded=self.loaded.iter().map(|(name,effect)|match effect {
            Ok(effect)=>serde_json::json!({"name":name,"resource_bytes":effect.data.len(),"programs":effect.programs,
                "techniques":effect.layout.as_ref().ok().map(|layout|layout.techniques.iter().map(|t|serde_json::json!({"name":t.name,"passes":t.passes.len()})).collect::<Vec<_>>()),
                "layout_error":effect.layout.as_ref().err()}),
            Err(error)=>serde_json::json!({"name":name,"error":error}),
        }).collect::<Vec<_>>();
        serde_json::json!({"available_effects":self.entries.len(),"loaded":loaded,"original_effect_execution":false,
            "scope":"Lazy original effect objects and bounded SM2/SM3 programs. Explicit preparation APIs resolve numeric bindings, samplers and ordered states. Live original-effect draws remain pending."})
    }
}
fn ensure_unique_material(unique:bool,name:&str)->Result<(),String> {if unique {Ok(())}else{Err(format!("Original shader {name} has multiple implementations; its selection owner is required"))}}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registered_shader_name_selects_identity_independently_of_technique_name()->Result<()> {
        use super::super::d3d9_effect::{Effect as Layout,Technique,Value,Data,Object};
        let value=|kind,name:&str,data|Value {kind,class:0,name:Some(name.into()),semantic:None,elements:0,columns:1,rows:1,data};
        let layout=Layout {parameters:vec![],techniques:vec![Technique {name:Some("Depth".into()),annotations:vec![],passes:vec![]},Technique {name:Some("Color".into()),annotations:vec![value(4,"ShaderName",Data::Objects(vec![1])),value(2,"Implementation",Data::Words(vec![7])),value(1,"UsesNiRenderState",Data::Words(vec![1]))],passes:vec![]}],objects:vec![Object {id:1,payload:0..7,source_kind:None}],object_slots:2,trailing_bytes:0};
        let bindings=technique_bindings(b"Actual\0",&layout)?;
        assert_eq!(bindings[0].shader_name,"Depth");assert_eq!(bindings[0].implementation,0);
        assert_eq!(bindings[1].shader_name,"Actual");assert_eq!(bindings[1].technique,1);assert_eq!(bindings[1].implementation,7);
        assert!(bindings[1].uses_ni_render_state);assert!(!bindings[1].uses_ni_light_state);Ok(())
    }
    fn effect(words:&[u32])->Vec<u8> { words.iter().flat_map(|v|v.to_le_bytes()).collect() }
    #[test]
    fn comment_payload_is_not_a_shader_terminator() -> Result<()> {
        let bytes=effect(&[0xfeff0901,0,0xffff0300,0x0002fffe,0x0000ffff,0xfffe0300,
            0x02000001,0x800f0000,0x90e40000,0x0000ffff]);
        let p=program(&bytes,8)?;
        assert_eq!(p.stage,"pixel");assert_eq!(p.instructions,1);assert_eq!(p.bytes,32);Ok(())
    }
    #[test]
    fn truncated_stream_and_other_versions_are_diagnosed() {
        for data in [effect(&[0xffff0300,0x0f000001]),effect(&[0xffff0200,0x0000ffff]),effect(&[0xffff0101,0x0000ffff])] {
            assert!(program(&data,0).is_err());
        }
    }
    #[test]
    fn model_two_uses_the_same_bounded_instruction_encoding()->Result<()> {
        for version in [0xfffe0200,0xffff0200,0xffff0201] {
            let data=effect(&[version,0x02000001,0x800f0000,0x90e40000,0x0000ffff]);
            let p=program(&data,0)?;assert_eq!(p.instructions,1);assert_eq!(p.bytes,data.len());
        }
        Ok(())
    }
}
