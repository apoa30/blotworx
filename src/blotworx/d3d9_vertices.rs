//! Original shader semantic locations bound to common viewport mesh buffers.
//! Explicit mesh/model-space vectors are evaluated only when their pose changes.
use anyhow::{ensure,Context,Result};
use glow::{self,HasContext};
use std::{collections::BTreeMap,sync::{Arc,Weak}};
use crate::nif_vertex_attributes::Source;
use super::d3d9_gpu::Program;
type Uvs=BTreeMap<u32,Arc<Vec<[f32;2]>>>;
enum Input {
    Primary {location:u32,width:i32,offset:i32},
    Direction {location:u32,name:&'static str,index:u32,buffer:Option<glow::Buffer>},
    Uv {location:u32,index:u32,buffer:Option<glow::Buffer>,source:Weak<Vec<[f32;2]>>},
}
enum Data {Direction(Vec<[f32;4]>,i32),Uv(Arc<Vec<[f32;2]>>)}
pub(super) struct Binding {pub vao:glow::VertexArray,inputs:Vec<Input>,source:Weak<Source>,mesh_space:bool}
impl Binding {
    /// Primary/index buffers remain owned by the common mesh cache. This owns
    /// only its semantic VAO and auxiliary buffers; destroy on a current context.
    pub unsafe fn new(gl:&glow::Context,program:&Program,primary:glow::Buffer,indices:glow::Buffer,vertices:usize,source:Option<&Arc<Source>>,uvs:Option<&Uvs>)->Result<Self> {
        Self::create(gl,program,primary,indices,vertices,source,uvs,false)
    }
    /// Original PC shaders consume mesh-local vectors. The common primary
    /// buffer is model-space, so bind retained native streams for those inputs.
    pub unsafe fn new_mesh_space(gl:&glow::Context,program:&Program,primary:glow::Buffer,indices:glow::Buffer,vertices:usize,source:Option<&Arc<Source>>,uvs:Option<&Uvs>)->Result<Self> {
        Self::create(gl,program,primary,indices,vertices,source,uvs,true)
    }
    unsafe fn create(gl:&glow::Context,program:&Program,primary:glow::Buffer,indices:glow::Buffer,vertices:usize,source:Option<&Arc<Source>>,uvs:Option<&Uvs>,mesh_space:bool)->Result<Self> {
        let mut inputs=Vec::new();
        for attribute in &program.attributes {
            let index=u32::try_from(attribute.index).context("Negative vertex semantic index")?;
            let location=attribute.location;
            let input=match (attribute.usage,index) {
                (0,0) if mesh_space=>Input::Direction {location,name:"POSITION",index:0,buffer:None},
                (3,0) if mesh_space=>Input::Direction {location,name:"NORMAL",index:0,buffer:None},
                (0,0)=>Input::Primary {location,width:3,offset:0},
                (3,0)=>Input::Primary {location,width:3,offset:12},
                (5,0)=>Input::Primary {location,width:2,offset:24},
                (10,0)=>Input::Primary {location,width:4,offset:32},
                (5,index)=>Input::Uv {location,index,buffer:None,source:Weak::new()},
                (6,index)=>Input::Direction {location,name:"TANGENT",index,buffer:None},
                (7,index)=>Input::Direction {location,name:"BINORMAL",index,buffer:None},
                (usage,index)=>anyhow::bail!("Original vertex semantic {usage}:{index} requires its stream adapter"),
            };
            inputs.push(input);
        }
        let vao=gl.create_vertex_array().map_err(anyhow::Error::msg)?;
        let mut binding=Self {vao,inputs,source:Weak::new(),mesh_space};
        if let Err(error)=binding.refresh(gl,primary,indices,vertices,source,uvs){binding.destroy(gl);return Err(error);}
        Ok(binding)
    }
    pub unsafe fn refresh(&mut self,gl:&glow::Context,primary:glow::Buffer,indices:glow::Buffer,vertices:usize,source:Option<&Arc<Source>>,uvs:Option<&Uvs>)->Result<()> {
        let unchanged=source.zip(self.source.upgrade().as_ref()).is_some_and(|(a,b)|Arc::ptr_eq(a,b));
        let mut pending=Vec::new();
        for (slot,input) in self.inputs.iter().enumerate(){
            match input {
                Input::Direction {name,index,buffer,..} if !unchanged||buffer.is_none()=>{
                    let source=source.context("Original shader requires retained mesh-space vertex data")?;
                    ensure!(source.vertices==vertices,"Native vertex count differs from common mesh");
                    let stream=source.streams.iter().find(|s|s.semantic==*name&&s.index==*index).with_context(||format!("Required original {name}{index} stream is absent"))?;
                    let width=((stream.format>>16)&255) as i32;
                    let values=if self.mesh_space {source.mesh_vectors(name,*index)?}else{source.model_vectors(name,*index)?};
                    pending.push((slot,Data::Direction(values,width)));
                },
                Input::Uv {index,buffer,source:previous,..}=>{
                    let values=uvs.and_then(|uvs|uvs.get(index)).context("Original shader requires missing secondary UVs")?;
                    ensure!(values.len()==vertices,"Secondary UV vertex count differs from common mesh");
                    if buffer.is_none()||!previous.upgrade().is_some_and(|old|Arc::ptr_eq(&old,values)){pending.push((slot,Data::Uv(values.clone())));}
                },
                _=>{},
            }
        }
        // All CPU validation precedes GL changes. On a later allocation error
        // the old source receipt remains, so retry rewrites the changed data.
        gl.bind_vertex_array(Some(self.vao));gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER,Some(indices));
        for input in &self.inputs {if let Input::Primary {location,width,offset}=input {
            gl.bind_buffer(glow::ARRAY_BUFFER,Some(primary));gl.enable_vertex_attrib_array(*location);
            gl.vertex_attrib_pointer_f32(*location,*width,glow::FLOAT,false,48,*offset);
        }}
        for (slot,data) in pending {
            let (buffer,location,width,stride)=match &mut self.inputs[slot] {
                Input::Direction {buffer,location,..}=>{let width=match &data {Data::Direction(_,width)=>*width,_=>unreachable!()};(buffer,*location,width,16)},
                Input::Uv {buffer,location,..}=>(buffer,*location,2,8),
                _=>unreachable!(),
            };
            if buffer.is_none(){*buffer=Some(gl.create_buffer().map_err(anyhow::Error::msg)?);}
            gl.bind_buffer(glow::ARRAY_BUFFER,*buffer);
            let (bytes,usage)=match &data {
                Data::Direction(values,_)=>(std::slice::from_raw_parts(values.as_ptr().cast::<u8>(),values.len()*16),glow::DYNAMIC_DRAW),
                Data::Uv(values)=>(std::slice::from_raw_parts(values.as_ptr().cast::<u8>(),values.len()*8),glow::STATIC_DRAW),
            };
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER,bytes,usage);
            gl.enable_vertex_attrib_array(location);gl.vertex_attrib_pointer_f32(location,width,glow::FLOAT,false,stride,0);
            if let (Input::Uv {source,..},Data::Uv(values))=(&mut self.inputs[slot],data){*source=Arc::downgrade(&values);}
        }
        self.source=source.map(Arc::downgrade).unwrap_or_default();
        gl.bind_vertex_array(None);gl.bind_buffer(glow::ARRAY_BUFFER,None);Ok(())
    }
    pub unsafe fn destroy(self,gl:&glow::Context){
        gl.delete_vertex_array(self.vao);
        for input in self.inputs {match input {Input::Direction {buffer:Some(buffer),..}|Input::Uv {buffer:Some(buffer),..}=>gl.delete_buffer(buffer),_=>{}}}
    }
}
