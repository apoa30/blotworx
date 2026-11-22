//! Selected transition start/retirement, DOL 0x80218a2c and 0x80219a04.
//! Candidate arbitration, pending initiation and nested-state routing precede
//! this phase. Graph owners implement clone/binding/lifecycle operations.
use super::{
    machine_runtime::{Notice, Runtime},
    state_machine::{Machine, Reference, Transition},
};
use anyhow::{ensure, Context, Result};

pub enum Source<'a, H> {
    Generator(&'a str),
    Effect(&'a H),
}
pub trait Executor<H> {
    fn sync_variable(&mut self, index: usize, state: i32) -> Result<()>;
    fn clone_effect(&mut self, definition: &Reference) -> Result<H>;
    fn set_target(&mut self, effect: &H, generator: &str) -> Result<()>;
    fn set_source(&mut self, effect: &H, source: Source<'_, H>) -> Result<()>;
    fn release_effect(&mut self, effect: H) -> Result<()>;
    fn notice(&mut self, runtime: &Runtime, notice: Notice<'_>) -> Result<()>;
    fn changed(&mut self) -> Result<()>;
}
pub struct Record<H> {
    pub effect: H,
    pub transition: Transition,
    pub from_state: i32,
    pub to_state: i32,
    pub reset_previous: bool,
}
pub struct Stack<H> {
    pub records: Vec<Record<H>>,
}
impl<H> Default for Stack<H> {
    fn default() -> Self {
        Self { records: vec![] }
    }
}
fn generator(machine: &Machine, state: i32) -> Result<&str> {
    let index = *machine
        .state_indices
        .get(&state)
        .context("Missing transition state")?;
    Ok(&machine.states[index].info.generator.id)
}
impl<H> Stack<H> {
    /// Retire a prefix, not just one interrupted effect. If another effect
    /// survives, its source becomes the retired prefix's destination generator.
    pub fn retire_through(
        &mut self,
        index: usize,
        machine: &Machine,
        runtime: &mut Runtime,
        executor: &mut impl Executor<H>,
    ) -> Result<()> {
        ensure!(
            index < self.records.len(),
            "Invalid transition retirement index"
        );
        let record = &self.records[index];
        if index + 1 == self.records.len() && record.transition.flags & 16 != 0 {
            runtime.enter(
                machine,
                record.to_state,
                record.reset_previous,
                &mut |r, n| executor.notice(r, n),
            )?;
        }
        if let Some(next) = self.records.get(index + 1) {
            executor.set_source(
                &next.effect,
                Source::Generator(generator(machine, record.to_state)?),
            )?;
        }
        for record in self.records.drain(..=index) {
            executor.release_effect(record.effect)?;
        }
        Ok(())
    }
    /// Called only for an initiation-ready transition selected by arbitration.
    /// Effect activation itself happens when the graph rebuilds its active tree.
    pub fn start(
        &mut self,
        machine: &Machine,
        runtime: &mut Runtime,
        transition: &Transition,
        reset_previous: bool,
        executor: &mut impl Executor<H>,
    ) -> Result<()> {
        ensure!(
            runtime.active,
            "Cannot start transition on an inactive machine"
        );
        let target = generator(machine, transition.to_state_id)?;
        let maximum = machine.settings.max_simultaneous_transitions;
        ensure!(
            transition.transition.is_none() || maximum > 0,
            "Invalid simultaneous transition capacity"
        );
        ensure!(
            self.records.len() <= maximum.max(0) as usize,
            "Transition stack exceeds authored capacity"
        );
        if machine.settings.sync_variable_index != -1 {
            executor.sync_variable(
                machine.settings.sync_variable_index as usize,
                transition.to_state_id,
            )?;
        }
        if let Some(definition) = &transition.transition {
            if self.records.len() == maximum as usize {
                self.retire_through(0, machine, runtime, executor)?;
            }
            let effect = executor.clone_effect(definition)?;
            let from_state = runtime.current;
            self.records.push(Record {
                effect,
                transition: transition.clone(),
                from_state,
                to_state: transition.to_state_id,
                reset_previous,
            });
            let last = self.records.len() - 1;
            executor.set_target(&self.records[last].effect, target)?;
            let source = if last == 0 {
                Source::Generator(generator(machine, from_state)?)
            } else {
                Source::Effect(&self.records[last - 1].effect)
            };
            executor.set_source(&self.records[last].effect, source)?;
            if transition.flags & 16 == 0 {
                runtime.enter(
                    machine,
                    transition.to_state_id,
                    reset_previous,
                    &mut |r, n| executor.notice(r, n),
                )?;
            }
        } else {
            if !self.records.is_empty() {
                self.retire_through(self.records.len() - 1, machine, runtime, executor)?;
            }
            runtime.enter(
                machine,
                transition.to_state_id,
                reset_previous,
                &mut |r, n| executor.notice(r, n),
            )?;
        }
        executor.changed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Probe {
        next: usize,
        calls: Vec<String>,
    }
    impl Executor<usize> for Probe {
        fn sync_variable(&mut self, i: usize, s: i32) -> Result<()> {
            self.calls.push(format!("sync:{i}:{s}"));
            Ok(())
        }
        fn clone_effect(&mut self, r: &Reference) -> Result<usize> {
            self.calls.push(format!("clone:{}", r.id));
            self.next += 1;
            Ok(self.next)
        }
        fn set_target(&mut self, h: &usize, g: &str) -> Result<()> {
            self.calls.push(format!("target:{h}:{g}"));
            Ok(())
        }
        fn set_source(&mut self, h: &usize, s: Source<'_, usize>) -> Result<()> {
            self.calls.push(match s {
                Source::Generator(g) => format!("source:{h}:{g}"),
                Source::Effect(p) => format!("source:{h}:effect{p}"),
            });
            Ok(())
        }
        fn release_effect(&mut self, h: usize) -> Result<()> {
            self.calls.push(format!("release:{h}"));
            Ok(())
        }
        fn notice(&mut self, _: &Runtime, n: Notice<'_>) -> Result<()> {
            self.calls.push(format!("notice:{n:?}"));
            Ok(())
        }
        fn changed(&mut self) -> Result<()> {
            self.calls.push("changed".into());
            Ok(())
        }
    }
    #[test]
    #[ignore = "requires original locally extracted Mickey behavior"]
    fn authored_walk_start_retains_source_and_delayed_entry_waits_for_retirement() -> Result<()> {
        let graph = super::super::behavior::Document::load(&std::fs::read(
            "target/managed-player-assets/MickeyBehavior.hkx_wii",
        )?)?;
        let mut prepared = super::super::generator_plan::prepare(
            &graph,
            super::super::behavior_state::State::new(&graph)?,
            0,
        )?;
        let machine = &prepared.state.machines["2:240"];
        let runtime = prepared.machines.get_mut("2:240").unwrap();
        let walk = machine.states[machine.state_indices[&0]]
            .transitions
            .iter()
            .find(|t| t.event_id == 0 && t.to_state_id == 5)
            .context("Missing original walk transition")?;
        let mut stack = Stack::default();
        let mut probe = Probe::default();
        stack.start(machine, runtime, walk, false, &mut probe)?;
        assert_eq!(runtime.current, 5);
        assert_eq!(stack.records[0].from_state, 0);
        assert_eq!(
            probe.calls[1],
            format!("target:1:{}", generator(machine, 5)?)
        );
        assert_eq!(
            probe.calls[2],
            format!("source:1:{}", generator(machine, 0)?)
        );
        // Lifecycle variant exercises the authored delay flag without claiming
        // this modified transition was observed in the original walk capture.
        let mut delayed = walk.clone();
        delayed.to_state_id = 0;
        delayed.flags |= 16;
        stack.start(machine, runtime, &delayed, false, &mut probe)?;
        assert_eq!(runtime.current, 5);
        assert!(probe.calls.iter().any(|s| s == "source:2:effect1"));
        stack.retire_through(1, machine, runtime, &mut probe)?;
        assert_eq!(runtime.current, 0);
        assert!(stack.records.is_empty());
        assert_eq!(
            &probe.calls[probe.calls.len() - 2..],
            ["release:1", "release:2"]
        );
        Ok(())
    }
}
