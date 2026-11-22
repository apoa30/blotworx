//! Retail3655d0/3658a0 query the same scalar state as gameplay/materials.
use mlua::{Lua,Value,MultiValue};
impl super::assets::World {
    /// REL365484 ->365444/3653d0 ->364f58/364fdc. This is a reset
    /// to the initialization anchor, not a paint hit or health/death reset.
    pub(super) fn counter_reset_lua(&mut self,lua:&Lua,mut args:MultiValue)->mlua::Result<MultiValue> {
        if args.len()!=1 {self.report.warnings.push("PaintThinnerCounterReset: expected one argument; preserving native stack order".into());}
        let Some(owner)=self.scripts.resolve_native_entity_argument(lua,args.pop_back().unwrap_or(Value::Nil))?
            .filter(|i|!self.removed_entities.contains(i)) else {return Ok(MultiValue::new());};
        if !self.entities[owner].components.iter().any(|c|c.class_name=="PaintThinnerCounterComponent") {return Ok(MultiValue::new());}
        self.reset_paint_counter(owner).map_err(mlua::Error::external)?;
        Ok(MultiValue::new())
    }
    fn reset_paint_counter(&mut self,owner:usize)->anyhow::Result<()> {
        use anyhow::Context;
        let anchor=self.paint_counters.get(&owner).context("Counter reset owner lacks live scalar state")?.anchor;
        self.set_entity_float_property(owner,"CounterStart",anchor)?;
        let counter=&self.paint_counters[&owner];
        let next=counter.status();
        if let Some(value)=counter.publication_value(next) {
            if value!=counter.value {self.set_entity_float_property(owner,"CounterStart",value)?;}
            let token=self.scripts.entity_token(owner)?;
            self.enqueue_counter_status(owner,next,Some(token),0,None,&mut |_,_|anyhow::bail!("Null reset references must not invoke virtual equality"))?;
        }
        // The shared queue retains deferred ordering. Complete native receiver
        // dispatch is still separate; the existing preview state bridge runs
        // on its normal next update, not synchronously inside this callback.
        let limitation="Counter reset restores its initialization value/material and queues native status75 before cached-state publication. Complete native status receivers, animation/body transitions and scheduler parity remain incomplete.";
        if !self.report.unsupported.iter().any(|v|v==limitation) {self.report.unsupported.push(limitation.into());}
        Ok(())
    }

    pub(super) fn counter_query_lua(&mut self,lua:&Lua,api:&str,mut args:MultiValue)->mlua::Result<MultiValue> {
        if args.len()!=1 {self.report.warnings.push(format!("{api}: expected one argument; preserving native stack-pop order"));}
        let entity=self.scripts.resolve_native_entity_argument(lua,args.pop_back().unwrap_or(Value::Nil))?
            .filter(|i|!self.removed_entities.contains(i));
        let counter=if let Some(entity)=entity {
            if self.entities[entity].components.iter().any(|c|c.class_name=="PaintThinnerCounterComponent") {
                Some(self.paint_counters.get(&entity).ok_or_else(||mlua::Error::external("Paint/thinner counter owner lacks its live scalar state"))?)
            } else {None}
        } else {None};
        if api=="PaintThinnerCounter_GetStatus" {
            Ok(MultiValue::from_vec(counter.map(|c|Value::Integer(c.status() as mlua::Integer)).into_iter().collect()))
        } else {
            Ok(MultiValue::from_vec(vec![Value::Number(counter.map_or(0.,|c|c.painted_percentage()) as f64)]))
        }
    }
}
