//! Bounded CPU effect-program decoding. Unlike the native dependency's fixed
//! operand array this retains the original six/eight-input dot instructions.
//! Microsoft D3DX runtime captures are the numerical reference.
use anyhow::{ensure, Context, Result};
use std::collections::BTreeMap;
use super::d3d9_constants::Table;
#[derive(Debug)]
pub(super) struct Register {pub table:u32,pub component:u32}
#[derive(Debug)]
pub(super) struct Operand {pub index:Option<Register>,pub register:Register}
#[derive(Debug)]
pub(super) struct Instruction {
    pub opcode:u16,pub components:usize,pub scalar:bool,
    pub inputs:Vec<Operand>,pub output:Operand,
}
#[derive(Debug)]
pub(super) struct Program {
    pub version:u32,pub constants:Table,pub literals:Vec<f64>,
    pub instructions:Vec<Instruction>,pub output_map:Vec<u32>,pub trailer:Vec<u32>,
}
struct Reader<'a> {bytes:&'a [u8],cursor:usize}
impl<'a> Reader<'a> {
    fn take(&mut self,n:usize)->Result<&'a [u8]> {
        let end=self.cursor.checked_add(n).context("CPU shader range overflow")?;
        let bytes=self.bytes.get(self.cursor..end).context("Truncated CPU shader")?;
        self.cursor=end;Ok(bytes)
    }
    fn word(&mut self)->Result<u32>{Ok(u32::from_le_bytes(self.take(4)?.try_into()?))}
    fn register(&mut self)->Result<Register>{
        let table=self.word()?;let mut component=self.word()?;
        ensure!((1..=7).contains(&table),"Unknown CPU shader register table {table}");
        if table==5 {ensure!(component%4==0,"Unaligned CPU boolean register");component/=4;}
        Ok(Register {table,component})
    }
    fn operand(&mut self)->Result<Operand>{
        let relative=self.word()?;ensure!(relative<=1,"Unknown CPU shader relative-address flag");
        let index=if relative==1 {Some(self.register()?)}else{None};
        Ok(Operand {index,register:self.register()?})
    }
}
impl Program {
    /// Input is the exact outer COMMENT token, including its PRES identifier.
    pub fn parse(bytes:&[u8])->Result<Self>{
        ensure!(bytes.len()%4==0,"Unaligned CPU shader");
        let mut outer=Reader {bytes,cursor:0};let token=outer.word()?;
        ensure!(token&0xffff==0xfffe,"CPU shader is not a comment");
        ensure!(((token>>16)&0x7fff) as usize*4==bytes.len()-4,"CPU shader comment size mismatch");
        ensure!(outer.take(4)?==b"PRES","Missing CPU shader identifier");
        let version=outer.word()?;ensure!(matches!(version,0x46580200|0x46580201),"Unsupported CPU shader version");
        let mut sections=BTreeMap::new();
        while outer.cursor<bytes.len(){
            let token=outer.word()?;
            if token==0xffff {ensure!(outer.cursor==bytes.len(),"Data after CPU shader terminator");break;}
            ensure!(token&0xffff==0xfffe,"Invalid CPU shader section");
            let size=((token>>16)&0x7fff) as usize*4;
            let section=outer.take(size)?;let id=section.get(..4).context("Empty CPU shader section")?;
            ensure!(matches!(id,b"CTAB"|b"CLIT"|b"FXLC"|b"PRSI"),"Unknown CPU shader section");
            ensure!(sections.insert(id,&section[4..]).is_none(),"Duplicate CPU shader section");
        }
        let constants=Table::parse(sections.get(b"CTAB".as_slice()).context("CPU shader lacks CTAB")?,version)?;
        let mut clit=Reader {bytes:sections.get(b"CLIT".as_slice()).context("CPU shader lacks literals")?,cursor:0};
        let count=clit.word()? as usize;
        let literal_bytes=clit.take(count.checked_mul(8).context("CPU literal count overflow")?)?;
        let literals=literal_bytes.chunks_exact(8).map(|b|f64::from_bits(u64::from_le_bytes(b.try_into().unwrap()))).collect();
        ensure!(clit.cursor==clit.bytes.len(),"Trailing CPU literal data");
        let mut fxlc=Reader {bytes:sections.get(b"FXLC".as_slice()).context("CPU shader lacks instructions")?,cursor:0};
        let count=fxlc.word()? as usize;
        ensure!(count<=(fxlc.bytes.len()-fxlc.cursor)/8,"CPU instruction count exceeds section");
        let mut instructions=Vec::with_capacity(count);
        for _ in 0..count {
            let token=fxlc.word()?;let components=(token&0xffff) as usize;
            ensure!((1..=4).contains(&components),"Invalid CPU instruction component count");
            let opcode=((token>>20)&0x7ff) as u16;let count=fxlc.word()? as usize;
            ensure!(count.checked_add(1).context("CPU operand count overflow")?<=(fxlc.bytes.len()-fxlc.cursor)/12,"CPU operands exceed section");
            let mut inputs=Vec::with_capacity(count);
            for _ in 0..count {inputs.push(fxlc.operand()?);}
            let output=fxlc.operand()?;
            instructions.push(Instruction {opcode,components,scalar:token&0x80000000!=0,inputs,output});
        }
        let trailer=fxlc.bytes[fxlc.cursor..].chunks_exact(4).map(|b|u32::from_le_bytes(b.try_into().unwrap())).collect::<Vec<_>>();
        ensure!(trailer.is_empty()||trailer==[0xf0f0f0f0,0x0f0f0f0f],"Unrecognized CPU instruction trailer");
        let output_map=sections.get(b"PRSI".as_slice()).map(|bytes|bytes.chunks_exact(4).map(|b|u32::from_le_bytes(b.try_into().unwrap())).collect()).unwrap_or_default();
        Ok(Self {version,constants,literals,instructions,output_map,trailer})
    }
}

/// Numeric effect values are packed through CTAB dimensions/register indices.
/// This boundary intentionally rejects unsupported casts or structures.
pub(super) fn bind_float_constants(table:&Table,effect:&super::d3d9_effect::Effect,overrides:&super::d3d9_draw::Params)->Result<Vec<f32>> {
    let count=table.constants.iter().filter(|c|c.register_set==2).map(|c|(c.register as usize+c.registers as usize)*4).max().unwrap_or(0);
    let mut registers=vec![0.;count];
    for constant in &table.constants {
        // Textures are bound separately; they do not occupy float registers.
        if constant.register_set==3 {continue;}
        ensure!(constant.register_set==2&&constant.ty.kind==3,"CPU constant {} needs its typed register adapter",constant.name);
        ensure!(constant.ty.members.is_empty(),"CPU struct constant requires member bindings");
        let value=&effect.parameters.iter().find(|p|p.value.name.as_deref()==Some(&constant.name)).with_context(||format!("Missing effect parameter {}",constant.name))?.value;
        ensure!(value.kind==3&&value.class<=3,"CPU effect parameter {} is not float numeric",constant.name);
        let super::d3d9_effect::Data::Words(defaults)=&value.data else{anyhow::bail!("CPU effect parameter has no numeric data")};
        let words:&[u32]=overrides.get(constant.name.as_str()).map(|w|&w[..]).unwrap_or(&defaults[..]);
        let rows=constant.ty.rows as usize;let columns=constant.ty.columns as usize;let elements=constant.ty.elements.max(1) as usize;
        ensure!(rows==value.rows as usize&&columns==value.columns as usize&&elements<=value.elements.max(1) as usize,"CPU constant dimensions differ from effect parameter");
        ensure!(words.len()==defaults.len(),"CPU parameter override length mismatch");
        let column_major=constant.ty.class==3;let source_column_major=value.class==3;
        let major=if column_major {columns}else{rows};let minor=if column_major {rows}else{columns};
        ensure!(minor<=4,"CPU constant exceeds register width");
        let first=constant.register as usize*4;let end=first+constant.registers as usize*4;
        for element in 0..elements {for axis in 0..major {for component in 0..minor {
            let dest=first+(element*major+axis)*4+component;
            // The compiler may retain only a prefix of an array's registers.
            if dest>=end {continue;}
            let (row,column)=if column_major {(component,axis)}else{(axis,component)};
            let source=element*rows*columns+if source_column_major {column*rows+row}else{row*columns+column};
            registers[dest]=f32::from_bits(*words.get(source).context("Truncated CPU parameter value")?);
        }}} 
    }
    Ok(registers)
}
#[derive(Clone,Copy,Debug)]
pub(super) enum TemporaryPrecision {Float,Double}
impl Program {
    /// Output banks are supplied by the owning effect pass; existing lanes are
    /// retained. Precision is explicit until the platform's VM is verified.
    pub fn evaluate(&self,constants:&[f32],floats:&mut [f32],booleans:&mut [u32],precision:TemporaryPrecision)->Result<()> {
        let mut temporaries=BTreeMap::<u32,f64>::new();
        for instruction in &self.instructions {
            ensure!(instruction.output.index.is_none(),"CPU relative output requires its addressing implementation");
            let n=instruction.components;let opcode=instruction.opcode;
            let expected=match opcode {0x100|0x101|0x103..=0x10c=>1,0x200..=0x206|0x208|0x500=>2,0x300=>3,0x70e=>instruction.inputs.len(),_=>anyhow::bail!("Unsupported CPU shader opcode {opcode:03x}")};
            ensure!(instruction.inputs.len()==expected&& (opcode!=0x70e||matches!(expected,6|8)),"Invalid CPU shader operand count");
            let mut inputs=Vec::with_capacity(expected);
            for (i,operand) in instruction.inputs.iter().enumerate(){
                ensure!(operand.index.is_none(),"CPU relative input requires its addressing implementation");
                let mut components=Vec::with_capacity(n);
                for lane in 0..n {
                    let lane=if instruction.scalar&&i==0 {0}else{lane};
                    let index=operand.register.component.checked_add(lane as u32).context("CPU input address overflow")?;
                    let at=index as usize;
                    let value=match operand.register.table {
                        1=>*self.literals.get(at).context("CPU literal outside table")?,
                        2=>*constants.get(at).context("CPU constant outside table")? as f64,
                        4=>*floats.get(at).context("CPU float input outside bank")? as f64,
                        5=>*booleans.get(at).context("CPU boolean input outside bank")? as f64,
                        7=>temporaries.get(&index).copied().unwrap_or(0.),
                        table=>anyhow::bail!("CPU input table {table} requires its typed bank"),
                    };
                    components.push(value);
                }
                inputs.push(components);
            }
            let mut result=Vec::with_capacity(n);
            for lane in 0..if opcode==0x500 {1}else{n}{
                let a=inputs[0][lane];
                let b=inputs.get(1).map(|v|v[lane]).unwrap_or(0.);
                let c=inputs.get(2).map(|v|v[lane]).unwrap_or(0.);
                let value=match opcode {
                    0x100=>a,0x101=>-a,0x103=>1./a,0x104=>a-a.floor(),0x105=>a.exp2(),
                    0x106=>if a==0. {0.}else{a.abs().log2()},0x107=>1./a.abs().sqrt(),
                    0x108=>a.sin(),0x109=>a.cos(),0x10a=>a.asin(),0x10b=>a.acos(),0x10c=>a.atan(),
                    0x200=>a.min(b),0x201=>a.max(b),0x202=>(a<b) as u8 as f64,0x203=>(a>=b) as u8 as f64,
                    0x204=>a+b,0x205=>a*b,0x206=>a.atan2(b),
                    // Reserved DIV behavior must be established before use.
                    0x208=>anyhow::bail!("CPU DIV needs native numerical verification"),
                    0x300=>if a>=0. {b}else{c},
                    0x500=>(0..n).fold(0.,|sum,j|sum+inputs[0][j]*inputs[1][j]),
                    0x70e=>{let half=expected/2;(0..half).fold(0.,|sum,j|sum+inputs[j][lane]*inputs[j+half][lane])},
                    _=>unreachable!(),
                };
                result.push(value);
            }
            for (lane,value) in result.into_iter().enumerate(){
                let index=instruction.output.register.component.checked_add(lane as u32).context("CPU output address overflow")?;
                match instruction.output.register.table {
                    4=>*floats.get_mut(index as usize).context("CPU float output outside bank")?=value as f32,
                    5=>*booleans.get_mut(index as usize).context("CPU boolean output outside bank")?=(value!=0.) as u32,
                    7=>{temporaries.insert(index,match precision {TemporaryPrecision::Float=>value as f32 as f64,TemporaryPrecision::Double=>value});},
                    table=>anyhow::bail!("CPU output table {table} requires its typed bank"),
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn empty()->Program {Program {version:0x46580201,constants:Table {creator:String::new(),target:String::new(),flags:0,constants:vec![]},literals:vec![],instructions:vec![],output_map:vec![],trailer:vec![]}}
    fn operand(table:u32,component:u32)->Operand {Operand {index:None,register:Register {table,component}}}
    #[test]
    fn wide_dot_and_dot_write_only_the_authored_lanes()->Result<()> {
        let mut p=empty();p.literals=vec![1.,2.,3.,4.,5.,6.];
        p.instructions.push(Instruction {opcode:0x70e,components:1,scalar:false,inputs:(0..6).map(|i|operand(1,i)).collect(),output:operand(7,0)});
        p.instructions.push(Instruction {opcode:0x100,components:1,scalar:false,inputs:vec![operand(7,0)],output:operand(4,0)});
        p.instructions.push(Instruction {opcode:0x500,components:3,scalar:false,inputs:vec![operand(1,0),operand(1,3)],output:operand(4,2)});
        let mut floats=vec![-99.;4];p.evaluate(&[],&mut floats,&mut [],TemporaryPrecision::Double)?;
        assert_eq!(floats,[32.,-99.,32.,-99.]);Ok(())
    }
    #[test]
    fn scalar_input_is_broadcast_and_boolean_output_is_normalized()->Result<()> {
        let mut p=empty();p.literals=vec![2.,3.,4.,5.];
        p.instructions.push(Instruction {opcode:0x205,components:3,scalar:true,inputs:vec![operand(1,0),operand(1,1)],output:operand(4,0)});
        p.instructions.push(Instruction {opcode:0x202,components:1,scalar:false,inputs:vec![operand(1,0),operand(1,1)],output:operand(5,0)});
        let mut floats=vec![0.;3];let mut booleans=vec![81];p.evaluate(&[],&mut floats,&mut booleans,TemporaryPrecision::Double)?;
        assert_eq!(floats,[6.,8.,10.]);assert_eq!(booleans,[1]);Ok(())
    }
    #[test]
    fn malformed_banks_and_instructions_fail_without_large_allocations() {
        let mut p=empty();p.literals=vec![1.];
        p.instructions.push(Instruction {opcode:0x100,components:1,scalar:false,inputs:vec![operand(1,0)],output:operand(4,u32::MAX)});
        assert!(p.evaluate(&[],&mut [0.;4],&mut [],TemporaryPrecision::Double).is_err());
        p.instructions[0].inputs.clear();assert!(p.evaluate(&[],&mut [0.;4],&mut [],TemporaryPrecision::Double).is_err());
        for bytes in [vec![],vec![0;4],[0x7fffffffu32.to_le_bytes(),*b"PRES",0x46580201u32.to_le_bytes()].concat()]{assert!(Program::parse(&bytes).is_err());}
    }
}
