//! Lossless PC texture submission: authored BC blocks, mip levels and faces.
//! The editor's decoded single-image preview is intentionally not this path.
use anyhow::{ensure,Context,Result};
use glow::{self,HasContext};
use std::{ops::Range,sync::{Arc,Weak}};
use crate::render_support::nif::pixel_data::{self,Encoding,Storage};
use super::d3d9_sampling::ImageView;

#[derive(Debug)]
pub(super) struct Source {pub bytes:Arc<Vec<u8>>,block:Range<usize>,little:bool,version:u32,persistent:bool}
impl Source {
    pub fn discover(bytes:Arc<Vec<u8>>)->Result<Self> {
        use epic_mickey_lib_rs::nif_file::{NifFile,NiObject};
        let nif=NifFile::load_bytes(&bytes).map_err(anyhow::Error::msg)?;
        let mut candidates=Vec::new();
        for (index,block) in nif.blocks.iter().enumerate() {
            let name=nif.block_type_indices.get(index).and_then(|&i|nif.block_types.get(i as usize)).map(String::as_str);
            match (name,block) {
                (Some("NiPixelData"),_)=>candidates.push((index,false)),
                (Some("NiPersistentSrcTextureRendererData"),NiObject::NiWiiUTextureData(data))=>{
                    ensure!(nif.source_is_little_endian&&matches!(data.target_platform,4|8),"Texture is not linear PC persistent data");candidates.push((index,true));
                },
                _=>{},
            }
        }
        ensure!(candidates.len()==1,"Texture NIF requires exactly one supported pixel-storage block (found {})",candidates.len());
        let (index,persistent)=candidates[0];
        let block=crate::nif_save::block_ranges(&bytes)?.get(index).context("Pixel block range is absent")?.clone();
        let result=Self {bytes,block,little:nif.source_is_little_endian,version:nif.version,persistent};
        result.storage()?;Ok(result)
    }
    pub fn storage(&self)->Result<Storage<'_>> {pixel_data::storage(self.bytes.get(self.block.clone()).context("Texture storage range is invalid")?,self.little,self.version,self.persistent)}
}
pub(super) struct Image {pub view:ImageView,source:Weak<Source>}
unsafe fn upload(gl:&glow::Context,source:&Source,srgb:bool)->Result<(glow::Texture,u32)> {
    let storage=source.storage()?;let first=storage.mips.first().context("Texture has no mip levels")?;
    let faces=first.faces.len();ensure!(matches!(faces,1|6),"Unsupported image dimensionality");
    let target=if faces==6 {glow::TEXTURE_CUBE_MAP}else{glow::TEXTURE_2D};
    let limit=gl.get_parameter_i32(if faces==6 {glow::MAX_CUBE_MAP_TEXTURE_SIZE}else{glow::MAX_TEXTURE_SIZE});
    for (index,mip) in storage.mips.iter().enumerate() {
        ensure!(mip.width==(first.width>>index).max(1)&&mip.height==(first.height>>index).max(1),"Noncanonical mip dimensions require their image adapter");
        ensure!(mip.faces.len()==faces&&(faces!=6||mip.width==mip.height),"Invalid cube texture layout");
        ensure!(mip.width<=limit as u32&&mip.height<=limit as u32,"Original image exceeds GPU texture capability");
    }
    let internal=match &storage.encoding {
        Encoding::Bc(fourcc)=>{
            ensure!(gl.supported_extensions().contains("GL_EXT_texture_compression_s3tc"),"Original BC texture requires S3TC support");
            match (fourcc,srgb) {
                (b"DXT1",false)=>0x83f1,(b"DXT3",false)=>0x83f2,(b"DXT5",false)=>0x83f3,
                (b"DXT1",true)=>0x8c4d,(b"DXT3",true)=>0x8c4e,(b"DXT5",true)=>0x8c4f,
                _=>anyhow::bail!("Unknown original block compression"),
            }
        },
        Encoding::Channels {..}=>if srgb {glow::SRGB8_ALPHA8}else{glow::RGBA8},
        Encoding::Bgra4444=>if srgb {glow::SRGB8_ALPHA8}else{glow::RGBA4},
    };
    let previous_texture=gl.get_parameter_texture(if faces==6 {glow::TEXTURE_BINDING_CUBE_MAP}else{glow::TEXTURE_BINDING_2D});
    let previous_buffer=gl.get_parameter_buffer(glow::PIXEL_UNPACK_BUFFER_BINDING);
    let unpack=[glow::UNPACK_ALIGNMENT,glow::UNPACK_ROW_LENGTH,glow::UNPACK_SKIP_PIXELS,glow::UNPACK_SKIP_ROWS,glow::UNPACK_IMAGE_HEIGHT,glow::UNPACK_SKIP_IMAGES,glow::UNPACK_SWAP_BYTES,glow::UNPACK_LSB_FIRST];
    let previous_unpack=unpack.map(|key|gl.get_parameter_i32(key));
    let image=gl.create_texture().map_err(anyhow::Error::msg)?;
    gl.bind_texture(target,Some(image));gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER,None);
    for (i,key) in unpack.iter().enumerate(){gl.pixel_store_i32(*key,if i==0 {1}else{0});}
    let result=(||->Result<()> {
        let mut rgba=Vec::new();
        for (index,mip) in storage.mips.iter().enumerate() {for (face,bytes) in mip.faces.iter().enumerate() {
            let face_target=if faces==6 {glow::TEXTURE_CUBE_MAP_POSITIVE_X+face as u32}else{target};
            match &storage.encoding {
                Encoding::Bc(_)=>gl.compressed_tex_image_2d(face_target,index as i32,internal as i32,mip.width as i32,mip.height as i32,0,i32::try_from(bytes.len())?,bytes),
                Encoding::Channels {order,stride}=>{
                    rgba.clear();for pixel in bytes.chunks_exact(*stride){let mut color=[0,0,0,255];for (&channel,&value) in order.iter().zip(pixel){color[channel]=value;}rgba.extend(color);}
                    gl.tex_image_2d(face_target,index as i32,internal as i32,mip.width as i32,mip.height as i32,0,glow::RGBA,glow::UNSIGNED_BYTE,glow::PixelUnpackData::Slice(Some(&rgba)));
                },
                Encoding::Bgra4444=>{
                    if srgb {
                        pixel_data::expand_bgra4444(bytes,&mut rgba);
                        gl.tex_image_2d(face_target,index as i32,internal as i32,mip.width as i32,mip.height as i32,0,glow::RGBA,glow::UNSIGNED_BYTE,glow::PixelUnpackData::Slice(Some(&rgba)));
                    }else{
                        gl.tex_image_2d(face_target,index as i32,internal as i32,mip.width as i32,mip.height as i32,0,glow::BGRA,glow::UNSIGNED_SHORT_4_4_4_4_REV,glow::PixelUnpackData::Slice(Some(bytes)));
                    }
                },
            }
            let error=gl.get_error();ensure!(error==glow::NO_ERROR,"Original texture upload failed (GL {error:#x}, mip {index}, face {face})");
        }}
        gl.tex_parameter_i32(target,glow::TEXTURE_BASE_LEVEL,0);
        gl.tex_parameter_i32(target,glow::TEXTURE_MAX_LEVEL,storage.mips.len() as i32-1);
        Ok(())
    })();
    for (key,value) in unpack.into_iter().zip(previous_unpack){gl.pixel_store_i32(key,value);}
    gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER,previous_buffer);gl.bind_texture(target,previous_texture);
    if let Err(error)=result {gl.delete_texture(image);return Err(error);}
    Ok((image,target))
}
impl Image {
    pub unsafe fn new(gl:&glow::Context,source:&Arc<Source>)->Result<Self> {
        let (linear,target)=upload(gl,source,false)?;Ok(Self {view:ImageView {target,linear,srgb:None},source:Arc::downgrade(source)})
    }
    pub unsafe fn ensure_srgb(&mut self,gl:&glow::Context)->Result<()> {
        if self.view.srgb.is_none(){let source=self.source.upgrade().context("Texture source was retired before sRGB admission")?;let (image,target)=upload(gl,&source,true)?;if target!=self.view.target {gl.delete_texture(image);anyhow::bail!("sRGB image source dimensionality changed");}self.view.srgb=Some(image);}Ok(())
    }
    pub unsafe fn destroy(self,gl:&glow::Context){gl.delete_texture(self.view.linear);if let Some(image)=self.view.srgb {gl.delete_texture(image);}}
}
