//! Platform material submission at the shared viewport boundary.
//! Geometry/camera ownership stays common; native material resources do not.
use glow;
use super::viewport3d::{GpuDrawCommand,GpuMeshHandle,GpuLight};
use std::sync::{Arc,Mutex};

pub(crate) struct Frame<'a> {
    pub view:&'a [f32;16],pub projection:&'a [f32;16],pub eye:[f32;3],
    pub viewport:[i32;4],pub lights:&'a [GpuLight],
}
pub(crate) type Shared=Arc<Mutex<dyn Backend>>;
pub(crate) trait Backend:std::fmt::Debug+Send {
    /// True only for a proven linear vertex path or an adapter that refuses
    /// native submission and therefore uses the existing linear fallback.
    fn allows_linear_culling(&self,_:&GpuDrawCommand)->bool {false}
    /// CPU-only ownership query. Managed original shaders may displace
    /// vertices, so the legacy linear-transform culler cannot reject them.
    fn handles(&self,draw:&GpuDrawCommand)->bool;
    /// Called on the current GL context, including frames without visible
    /// managed draws, so retired resources can be collected promptly.
    unsafe fn begin_frame(&mut self,gl:&glow::Context);
    /// True means the complete draw was submitted. False means unowned.
    /// Mesh buffers remain owned by the common cache. Retained bindings must
    /// track the draw's source lifetime and never delete those buffers.
    /// Failure must precede any geometry submission; leave shared GL state
    /// compatible with the following viewport draw on both success and error.
    unsafe fn draw(&mut self,gl:&glow::Context,draw:&GpuDrawCommand,mesh:GpuMeshHandle,frame:&Frame<'_>)->Result<bool,String>;
    /// The application calls this on exit while its GL context is current.
    unsafe fn destroy(&mut self,gl:&glow::Context);
}
