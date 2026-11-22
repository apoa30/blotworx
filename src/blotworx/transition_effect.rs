//! Authored transition definitions and active-child selection recovered from
//! DOL 0x8021de70 and 0x8021eddc. Child flags feed the graph scheduler; selecting
//! children does not itself clone, activate or deactivate their generators.
use super::{
    behavior::Document,
    state_machine::Reference,
    transition_timing::{BlendingClock, GeneratorClock, Stage},
};
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Blend {
    pub variable_binding_set: Option<Reference>,
    pub user_data: u64,
    pub name: String,
    pub self_transition_mode: u8,
    pub event_mode: u8,
    pub duration: f32,
    pub to_generator_start_time_fraction: f32,
    pub flags: u16,
    pub end_mode: u8,
    pub blend_curve: u8,
    #[serde(skip)]
    pub duration_variable: Option<usize>,
}
impl Blend {
    pub(super) fn duration_for(&self, words: &[u32]) -> Result<f32> {
        self.duration_variable.map(|i| words.get(i).copied().map(f32::from_bits)
            .context("Missing transition duration variable")).transpose()
            .map(|bound| bound.unwrap_or(self.duration))
    }
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Generator {
    pub variable_binding_set: Option<Reference>,
    pub user_data: u64,
    pub name: String,
    pub self_transition_mode: u8,
    pub event_mode: u8,
    pub transition_generator: Reference,
    pub blend_in_duration: f32,
    pub blend_out_duration: f32,
    pub sync_to_generator_start_time: bool,
}
pub enum Definition {
    Blend(Blend),
    Generator(Generator),
}
pub fn compile(graph: &Document) -> Result<BTreeMap<String, Definition>> {
    let mut result = BTreeMap::new();
    for (id, object) in &graph.objects {
        let definition = match object.klass.as_str() {
            "hkbBlendingTransitionEffect" => {
                let mut fields = object.fields.clone();
                if let Some(bone) = fields.as_object_mut().unwrap().remove("alignmentBone") {
                    ensure!(
                        bone.as_i64() == Some(-1),
                        "Transition bone alignment is not implemented: {id}"
                    );
                }
                let mut d: Blend = serde_json::from_value(fields)?;
                ensure!(
                    d.duration.is_finite()
                        && d.duration >= 0.
                        && d.to_generator_start_time_fraction.is_finite()
                        && d.self_transition_mode <= 3
                        && d.event_mode <= 3
                        && d.end_mode <= 2
                        && d.blend_curve <= 3
                        && d.flags & !3 == 0,
                    "Invalid blending transition {id}"
                );
                if let Some(reference) = &d.variable_binding_set {
                    let set = graph.objects.get(&reference.id).context("Missing transition binding set")?;
                    ensure!(set.klass == "hkbVariableBindingSet" && set.fields["indexOfBindingToEnable"] == -1,
                        "Unsupported transition binding set: {id}");
                    let bindings = set.fields["bindings"].as_array().context("Invalid transition binding list")?;
                    for binding in bindings {
                        ensure!(binding["memberPath"] == "duration" && binding["bindingType"] == 0 && binding["bitIndex"] == -1,
                            "Unsupported transition field binding: {binding}");
                        let index = binding["variableIndex"].as_u64().context("Invalid transition variable index")? as usize;
                        ensure!(d.duration_variable.replace(index).is_none(), "Duplicate duration binding: {id}");
                    }
                }
                Definition::Blend(d)
            }
            "hkbGeneratorTransitionEffect" => {
                let mut d: Generator = serde_json::from_value(object.fields.clone())?;
                if !d.blend_in_duration.is_finite() || d.blend_in_duration < 0. {
                    d.blend_in_duration = 0.;
                }
                if !d.blend_out_duration.is_finite() || d.blend_out_duration < 0. {
                    d.blend_out_duration = 0.;
                }
                d.self_transition_mode = d.self_transition_mode.min(3);
                d.event_mode = d.event_mode.min(3);
                let child = graph
                    .objects
                    .get(&d.transition_generator.id)
                    .context("Missing transition generator")?;
                ensure!(
                    child.klass.ends_with("Generator") || child.klass == "hkbStateMachine",
                    "Invalid transition generator class {}",
                    child.klass
                );
                Definition::Generator(d)
            }
            _ => continue,
        };
        result.insert(id.clone(), definition);
    }
    Ok(result)
}

/// Original child-info bytes +4 through +7, in authored enumeration order.
#[derive(Debug, PartialEq, Eq)]
pub struct Child<'a> {
    pub generator: &'a str,
    pub discard_events: bool,
    pub reactivate: bool,
    pub synchronize: bool,
    /// Byte +7: target child requested before it contributes to the output.
    /// Keep this separate from synchronization and event suppression.
    pub flag_7: bool,
}
impl<'a> Child<'a> {
    fn new(generator: &'a str) -> Self {
        Self {
            generator,
            discard_events: false,
            reactivate: false,
            synchronize: false,
            flag_7: false,
        }
    }
}
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Children<'a> {
    pub nodes: Vec<Child<'a>>,
    pub discard_events: bool,
    pub reactivate: bool,
    pub synchronize: bool,
    pub flag_7: bool,
}
fn event_mode(authored: u8, global: u8) -> Result<u8> {
    let mode = if authored == 0 { global } else { authored };
    ensure!(
        (1..=3).contains(&mode),
        "Uninitialized/unknown global transition event mode"
    );
    Ok(mode)
}

pub struct Blending {
    pub clock: BlendingClock,
    pub duration: f32,
    pub target_was_active: bool,
}
impl Blending {
    /// 0x8021d410: query target activity before graph topology is changed.
    pub fn activate(d: &Blend, target_was_active: bool) -> Result<Self> {
        Self::activate_with_variables(d, &[], target_was_active)
    }
    pub(super) fn activate_with_variables(d: &Blend, words: &[u32], target_was_active: bool) -> Result<Self> {
        let duration = d.duration_for(words)?;
        Ok(Self {
            clock: BlendingClock::new(duration)?,
            duration,
            target_was_active,
        })
    }
    /// Retail unsynchronized/same-generator branches of 0x8021d570. The
    /// synchronized two-child branch needs marker matching before it can run.
    pub fn synchronize(
        &mut self,
        d: &Blend,
        from: &super::animation_sync::Sync,
        to: &super::animation_sync::Sync,
        same_generator: bool,
        mut apply: impl FnMut(Operation) -> Result<()>,
    ) -> Result<super::animation_sync::Sync> {
        ensure!(
            same_generator || d.flags & 2 == 0,
            "Synchronized blending-transition marker matching is not implemented"
        );
        ensure!(
            [from.frequency, from.time, to.frequency, to.time]
                .into_iter()
                .all(f32::is_finite),
            "Invalid blending-transition child synchronization"
        );
        if self.target_was_active {
            apply(Operation::TargetSelfTransition)?;
            self.target_was_active = false;
        }
        let output = if same_generator {
            to.clone()
        } else {
            if self.clock.time == 0.
                && d.to_generator_start_time_fraction != 0.
                && to.frequency != 0.
            {
                let time = d.to_generator_start_time_fraction * (1. / to.frequency);
                ensure!(time.is_finite(), "Invalid blending-transition target seek");
                apply(Operation::TargetSeek(time))?;
            }
            super::animation_sync::Sync {
                points: vec![],
                frequency: 0.,
                time: 0.,
                field_48: 1.,
                mapped_events: false,
            }
        };
        // The same-generator branch supplies copied output sync; the ordinary
        // branch supplies source sync (virtual +0x70 -> 0x8021dbbc).
        let end = if same_generator { &output } else { from };
        self.clock.remaining = match d.end_mode {
            0 => self.duration - self.clock.time,
            1 => {
                if end.frequency != 0. {
                    1. / end.frequency - end.time
                } else {
                    0.
                }
            }
            2 => {
                if end.frequency != 0. {
                    (self.duration - self.clock.time).min(1. / end.frequency - end.time)
                } else {
                    0.
                }
            }
            _ => anyhow::bail!("Unknown blending-transition end mode"),
        };
        ensure!(
            self.clock.remaining.is_finite(),
            "Invalid blending-transition remaining time"
        );
        Ok(output)
    }
    /// 0x8021dd28 updates the output sync clock before the independent elapsed
    /// and remaining fields. Preserve the strict wrap comparison and FMA.
    pub fn update(&mut self, sync: &mut super::animation_sync::Sync, dt: f32) -> Result<()> {
        ensure!(
            [sync.time, sync.frequency, dt]
                .into_iter()
                .all(f32::is_finite)
                && (sync.time + dt).is_finite(),
            "Invalid blending-transition timestep"
        );
        sync.time += dt;
        if sync.frequency != 0. {
            let period = 1. / sync.frequency;
            ensure!(
                period.is_finite() && period > 0.,
                "Invalid blending-transition period"
            );
            if sync.time > period {
                let cycles = (sync.time / period).trunc();
                ensure!(
                    cycles.is_finite() && cycles < 2147483648.,
                    "Blending-transition cycle overflow"
                );
                sync.time = (-cycles).mul_add(period, sync.time);
            }
        }
        self.clock.advance(dt)
    }
    pub fn children<'a>(
        &mut self,
        d: &Blend,
        from: &'a str,
        to: &'a str,
        active: bool,
        global_event_mode: u8,
    ) -> Result<Children<'a>> {
        let mut output = Children::default();
        if !active {
            return Ok(output);
        }
        if self.target_was_active && d.self_transition_mode == 2 {
            output.reactivate = true;
            self.target_was_active = false;
        }
        let mut target = Child::new(to);
        target.reactivate = output.reactivate;
        output.nodes.push(target);
        if from != to {
            output.nodes.push(Child::new(from));
            if d.flags & 2 != 0 {
                output.synchronize = true;
                for child in &mut output.nodes {
                    child.synchronize = true;
                }
            }
        }
        let mode = event_mode(d.event_mode, global_event_mode)?;
        if mode == 2 || mode == 3 {
            output.discard_events = true;
            if from != to {
                output.nodes[0].discard_events = mode == 3;
                output.nodes[1].discard_events = mode == 2;
            }
        }
        Ok(output)
    }
}

/// Runtime byte +0x39. The sync pass advances Requested to one of the ready
/// states after resolving the target's frequency/start-time requirements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetPhase {
    Waiting,
    Requested,
    ReadyAtZero,
    ReadyAfterSeek,
}
/// Required graph operations, kept in call order. The owner must apply these
/// to the child graph; callers cannot silently skip reactivation or seeking.
#[derive(Debug, PartialEq)]
pub enum Operation {
    TransitionEcho(f32),
    TargetSelfTransition,
    TargetSeek(f32),
}
pub struct Generated {
    pub clock: GeneratorClock,
    pub target_phase: TargetPhase,
    pub transition_was_active: bool,
    pub initialize_sync: bool,
    pub stage: Stage,
}
impl Generated {
    /// Use the retained stage for both pose and attributes, even if the sync
    /// pass changed duration before update commits the next stage.
    pub fn output(
        &self,
        from: &super::generator_output::Output,
        to: &super::generator_output::Output,
        transition: &super::generator_output::Output,
    ) -> Result<super::generator_output::Output> {
        self.clock.output_at_stage(self.stage, from, to, transition)
    }
    pub fn pose(
        &self,
        from: &[Option<epic_mickey_lib_rs::ffi_anim::FfiTransform>],
        to: &[Option<epic_mickey_lib_rs::ffi_anim::FfiTransform>],
        transition: &[Option<epic_mickey_lib_rs::ffi_anim::FfiTransform>],
    ) -> Result<Vec<Option<epic_mickey_lib_rs::ffi_anim::FfiTransform>>> {
        self.clock.pose_at_stage(self.stage, from, to, transition)
    }
    /// 0x8021e644 sets duration to zero until the synchronization pass obtains
    /// the transition generator's duration. Do not use its raw clip duration.
    pub fn activate(d: &Generator, transition_was_active: bool) -> Result<Self> {
        let clock = GeneratorClock::new(0., d.blend_in_duration, d.blend_out_duration)?;
        let stage = clock.stage();
        Ok(Self {
            clock,
            stage,
            target_phase: TargetPhase::Waiting,
            transition_was_active,
            initialize_sync: true,
        })
    }
    fn update_target_phase(
        &mut self,
        d: &Generator,
        apply: &mut impl FnMut(Operation) -> Result<()>,
    ) -> Result<()> {
        if self.target_phase == TargetPhase::Waiting {
            if self.clock.requests_target_start() {
                self.target_phase = TargetPhase::Requested;
            }
        } else if self.target_phase == TargetPhase::ReadyAtZero && d.self_transition_mode != 2 {
            apply(Operation::TargetSelfTransition)?;
        }
        Ok(())
    }
    /// DOL 0x8021e934..0x8021ebac. Duration is the transition child's sync
    /// period, which may differ from the raw resource duration. The stored
    /// stage is not recomputed here: retail does that in the update pass.
    pub fn synchronize(
        &mut self,
        d: &Generator,
        transition: &super::animation_sync::Sync,
        target: &super::animation_sync::Sync,
        mut apply: impl FnMut(Operation) -> Result<()>,
    ) -> Result<super::animation_sync::Sync> {
        ensure!(
            [
                transition.frequency,
                transition.time,
                target.frequency,
                target.time
            ]
            .into_iter()
            .all(f32::is_finite),
            "Invalid transition child synchronization"
        );
        if self.initialize_sync {
            self.initialize_sync = false;
            if transition.frequency != 0. {
                self.clock.duration = 1. / transition.frequency;
                ensure!(
                    self.clock.duration.is_finite() && self.clock.duration >= 0.,
                    "Invalid synchronized transition duration"
                );
            }
            if self.transition_was_active {
                apply(Operation::TransitionEcho(
                    self.clock.duration - transition.time,
                ))?;
            }
            self.update_target_phase(d, &mut apply)?;
        }
        if self.target_phase == TargetPhase::Requested {
            let blend_out = self.clock.effective_blends().1;
            let seek = if blend_out > 0.
                && target.frequency != 0.
                && target.mapped_events
                && d.sync_to_generator_start_time
            {
                (1. / target.frequency - blend_out).max(0.)
            } else {
                0.
            };
            ensure!(seek.is_finite(), "Invalid synchronized target seek");
            if seek == 0. {
                self.target_phase = TargetPhase::ReadyAtZero;
            } else {
                apply(Operation::TargetSeek(seek))?;
                self.target_phase = TargetPhase::ReadyAfterSeek;
            }
        }
        // The remaining copied sync fields retain the transition child values.
        // Native byte +0x4e is cleared separately by the active-node owner.
        Ok(transition.clone())
    }
    /// DOL 0x8021ebdc: phase requests and self-transition work run after the
    /// clock increment, but before storing the new stage/topology notification.
    pub fn update(
        &mut self,
        d: &Generator,
        dt: f32,
        mut apply: impl FnMut(Operation) -> Result<()>,
    ) -> Result<bool> {
        self.clock.advance(dt)?;
        self.update_target_phase(d, &mut apply)?;
        let previous = self.stage;
        self.stage = self.clock.stage();
        Ok(previous != self.stage)
    }
    pub fn children<'a>(
        &mut self,
        d: &'a Generator,
        from: &'a str,
        to: &'a str,
        active: bool,
        global_event_mode: u8,
    ) -> Result<Children<'a>> {
        let mut output = Children::default();
        let transition = d.transition_generator.id.as_str();
        if !active || self.stage == Stage::Transition {
            output.nodes.push(Child::new(transition));
            if !active {
                return Ok(output);
            }
        } else {
            let mode = event_mode(d.event_mode, global_event_mode)?;
            if self.stage == Stage::BlendIn {
                output.discard_events = mode == 2;
                let mut source = Child::new(from);
                source.discard_events = mode == 2;
                output.nodes.push(source);
                output.nodes.push(Child::new(transition));
            } else {
                output.discard_events = mode == 2 || mode == 3;
                output.reactivate =
                    self.target_phase == TargetPhase::ReadyAtZero && d.self_transition_mode == 2;
                if output.reactivate {
                    self.target_phase = TargetPhase::ReadyAfterSeek;
                }
                let mut target = Child::new(to);
                target.reactivate = output.reactivate;
                target.discard_events = mode == 3;
                let mut middle = Child::new(transition);
                middle.discard_events = mode == 2;
                output.nodes.extend([target, middle]);
                return Ok(output);
            }
        }
        output.flag_7 = true;
        let mut target = Child::new(to);
        target.flag_7 = true;
        output.nodes.push(target);
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn definition() -> Generator {
        Generator {
            variable_binding_set: None,
            user_data: 0,
            name: "test".into(),
            self_transition_mode: 0,
            event_mode: 0,
            transition_generator: Reference {
                id: "middle".into(),
            },
            blend_in_duration: 0.,
            blend_out_duration: 0.,
            sync_to_generator_start_time: true,
        }
    }
    #[test]
    fn pose_and_children_share_retained_stage_until_update_commits_change() -> Result<()> {
        use epic_mickey_lib_rs::ffi_anim::FfiTransform;
        let mut d = definition();
        d.blend_out_duration = 0.2;
        let pose = |x| {
            vec![Some(FfiTransform {
                translation: [x, 0., 0.],
                rotation: [0., 0., 0., 1.],
                scale: [1.; 3],
            })]
        };
        let (from, to, middle) = (pose(0.), pose(20.), pose(10.));
        let mut runtime = Generated::activate(&d, false)?;
        runtime.update(&d, 0.9, |_| Ok(()))?;
        runtime.synchronize(&d, &sync(1., 0.9, true), &sync(1., 0., true), |_| Ok(()))?;
        assert_eq!(runtime.stage, Stage::Transition);
        assert_eq!(runtime.clock.stage(), Stage::BlendOut);
        assert_eq!(
            runtime.children(&d, "source", "target", true, 2)?.nodes[0].generator,
            "middle"
        );
        assert_eq!(
            runtime.pose(&from, &to, &middle)?[0].unwrap().translation[0],
            10.
        );
        assert!(runtime.update(&d, 0., |_| Ok(()))?);
        assert_eq!(
            runtime.children(&d, "source", "target", true, 2)?.nodes[0].generator,
            "target"
        );
        assert!(runtime.pose(&from, &to, &middle)?[0].unwrap().translation[0] > 10.);
        Ok(())
    }
    fn sync(frequency: f32, time: f32, mapped_events: bool) -> super::super::animation_sync::Sync {
        super::super::animation_sync::Sync {
            points: vec![],
            frequency,
            time,
            field_48: 0.,
            mapped_events,
        }
    }
    #[test]
    fn blending_sync_uses_source_end_clock_and_preserves_operation_order() -> Result<()> {
        let mut d = Blend {
            variable_binding_set: None,
            user_data: 0,
            name: "test".into(),
            self_transition_mode: 0,
            event_mode: 0,
            duration: 0.5,
            to_generator_start_time_fraction: 0.25,
            flags: 0,
            end_mode: 0,
            blend_curve: 1,
            duration_variable: None,
        };
        let mut runtime = Blending::activate(&d, true)?;
        let mut operations = vec![];
        let mut output = runtime.synchronize(
            &d,
            &sync(2., 0.2, true),
            &sync(0.5, 0., true),
            false,
            |op| {
                operations.push(op);
                Ok(())
            },
        )?;
        assert_eq!(
            operations,
            vec![Operation::TargetSelfTransition, Operation::TargetSeek(0.5)]
        );
        assert_eq!(
            (
                output.frequency,
                output.time,
                output.field_48,
                output.mapped_events
            ),
            (0., 0., 1., false)
        );
        runtime.update(&mut output, 0.1)?;
        assert_eq!(runtime.clock.time, 0.1);
        assert_eq!(runtime.clock.remaining, 0.4);
        d.end_mode = 1;
        runtime.synchronize(
            &d,
            &sync(2., 0.2, true),
            &sync(0.5, 0., true),
            false,
            |_| panic!("Repeated operation"),
        )?;
        assert_eq!(runtime.clock.remaining, 0.5 - 0.2);
        d.end_mode = 2;
        runtime.synchronize(
            &d,
            &sync(2., 0.45, true),
            &sync(0.5, 0., true),
            false,
            |_| Ok(()),
        )?;
        assert_eq!(runtime.clock.remaining, 0.5 - 0.45);
        let mut copied =
            runtime.synchronize(&d, &sync(2., 0., true), &sync(0.5, 2., true), true, |_| {
                Ok(())
            })?;
        runtime.update(&mut copied, 0.)?;
        assert_eq!(copied.time, 2.); // Exact endpoint does not wrap.
        runtime.update(&mut copied, 0.25)?;
        assert_eq!(copied.time, 0.25);
        Ok(())
    }
    #[test]
    fn generator_sync_initialization_precedes_target_start_and_retains_strict_boundary(
    ) -> Result<()> {
        let d = definition();
        let mut runtime = Generated::activate(&d, true)?;
        let mut operations = vec![];
        let copied = runtime.synchronize(&d, &sync(2.5, 0.1, true), &sync(1., 0., true), |op| {
            operations.push(op);
            Ok(())
        })?;
        assert_eq!(runtime.clock.duration.to_bits(), 0.4f32.to_bits());
        assert_eq!(copied.frequency, 2.5);
        assert_eq!(operations, vec![Operation::TransitionEcho(0.4 - 0.1)]);
        assert!(!runtime.initialize_sync);
        runtime.clock.time = 0.3999999761581421;
        runtime.update(&d, 0., |_| panic!("Premature target operation"))?;
        assert_eq!(runtime.target_phase, TargetPhase::Waiting);
        assert!(!runtime.clock.finished());
        runtime.update(&d, 1. / 30., |_| panic!("Target must be prepared by sync"))?;
        assert_eq!(runtime.target_phase, TargetPhase::Requested);
        runtime.synchronize(&d, &sync(2.5, 0., true), &sync(1., 0., true), |_| {
            panic!("Unexpected seek")
        })?;
        assert_eq!(runtime.target_phase, TargetPhase::ReadyAtZero);
        operations.clear();
        runtime.update(&d, 0., |op| {
            operations.push(op);
            Ok(())
        })?;
        assert_eq!(operations, vec![Operation::TargetSelfTransition]);
        Ok(())
    }
    #[test]
    fn target_seek_uses_adjusted_blend_out_and_runs_once() -> Result<()> {
        let mut d = definition();
        d.blend_in_duration = 2.;
        d.blend_out_duration = 2.;
        let mut runtime = Generated::activate(&d, false)?;
        runtime.synchronize(&d, &sync(1., 0., true), &sync(0.5, 0., true), |_| Ok(()))?;
        assert_eq!(runtime.clock.effective_blends(), (0.5, 0.5));
        assert!(runtime.update(&d, 0.75, |_| Ok(()))?);
        let mut operations = vec![];
        runtime.synchronize(&d, &sync(1., 0.75, true), &sync(0.5, 0., true), |op| {
            operations.push(op);
            Ok(())
        })?;
        assert_eq!(operations, vec![Operation::TargetSeek(1.5)]);
        assert_eq!(runtime.target_phase, TargetPhase::ReadyAfterSeek);
        runtime.synchronize(&d, &sync(1., 0.75, true), &sync(0.5, 0., true), |_| {
            panic!("Repeated seek")
        })?;
        Ok(())
    }
    #[test]
    #[ignore = "requires original locally extracted Mickey behavior"]
    fn all_effects_compile_and_children_keep_native_order_and_flags() -> Result<()> {
        let graph = Document::load(&std::fs::read(
            "target/managed-player-assets/MickeyBehavior.hkx_wii",
        )?)?;
        let mut effects = compile(&graph)?;
        assert_eq!(effects.len(), 136);
        let Definition::Blend(d) = effects.get_mut("2:1232").unwrap() else {
            unreachable!()
        };
        let mut b = Blending::activate(d, true)?;
        let children = b.children(d, "source", "target", true, 2)?;
        assert_eq!(
            children
                .nodes
                .iter()
                .map(|c| c.generator)
                .collect::<Vec<_>>(),
            ["target", "source"]
        );
        assert!(!children.nodes[0].discard_events);
        assert!(children.nodes[1].discard_events);
        assert!(children.discard_events);
        d.self_transition_mode = 2;
        assert!(b.children(d, "source", "target", true, 2)?.reactivate);
        assert!(!b.children(d, "source", "target", true, 2)?.reactivate);
        assert_eq!(b.children(d, "same", "same", true, 2)?.nodes.len(), 1);
        let Definition::Generator(d) = effects.get("2:3936").unwrap() else {
            unreachable!()
        };
        let mut g = Generated::activate(d, false)?;
        g.clock.duration = 1.;
        g.stage = g.clock.stage();
        let children = g.children(d, "source", "target", true, 2)?;
        assert_eq!(
            children
                .nodes
                .iter()
                .map(|c| c.generator)
                .collect::<Vec<_>>(),
            ["source", d.transition_generator.id.as_str(), "target"]
        );
        assert!(children.nodes[0].discard_events);
        assert!(children.nodes[2].flag_7);
        g.clock.time = 0.5;
        g.stage = g.clock.stage();
        let children = g.children(d, "source", "target", true, 2)?;
        assert_eq!(children.nodes.len(), 2);
        assert_eq!(children.nodes[0].generator, d.transition_generator.id);
        assert!(children.nodes[1].flag_7);
        assert_eq!(g.children(d, "source", "target", false, 2)?.nodes.len(), 1);
        g.clock.blend_out = 0.25;
        g.clock.time = 0.9;
        g.stage = g.clock.stage();
        let children = g.children(d, "source", "target", true, 2)?;
        assert_eq!(children.nodes[0].generator, "target");
        assert!(!children.nodes[0].flag_7);
        assert!(children.nodes[1].discard_events);
        Ok(())
    }
}
