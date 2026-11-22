//! Initial active-node planning. This is not an executing animation graph:
//! clip/modifier activation and event-driven topology changes remain pending.
use super::transition_instances::{Binding, Flags, Handle, Instances};
use super::{
    behavior::Document,
    behavior_state::State,
    blender,
    machine_runtime::{Notice, Runtime},
};
use anyhow::{ensure, Context, Result};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, serde::Serialize)]
pub struct Node {
    pub object: String,
    pub parent: Option<usize>,
    pub event_parent: Option<usize>,
    pub children: Vec<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect: Option<Handle>,
    #[serde(skip_serializing_if = "Flags::is_empty")]
    pub flags: Flags,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect_flags: Option<Flags>,
}
#[derive(serde::Serialize)]
pub struct Plan {
    pub nodes: Vec<Node>,
    pub pending_generator_activation: Vec<String>,
    pub pending_events: Vec<super::state_machine::Event>,
}
/// Retained preparation state owned by the character, not a second diagnostic
/// graph. Pending activations stay explicit until the runtime services them.
pub struct Prepared {
    pub state: State,
    pub plan: Plan,
    pub machines: BTreeMap<String, Runtime>,
    pub blenders: BTreeMap<String, Vec<blender::Child>>,
    pub random_seed: u32,
    pub clips: super::clip_instance::Instances,
    pub expressions: super::expression_instances::Instances,
    pub attachment_modifiers: super::attachment_modifier::Instances,
    pub(super) event_modifiers: BTreeSet<String>,
    pub transitions: super::transition_instances::Instances,
    pub transition_stacks: BTreeMap<String, super::machine_transitions::Stack<Handle>>,
    pub dispatchers: BTreeMap<String, super::machine_arbitration::Dispatcher>,
    pub blender_echoes: BTreeSet<String>,
}

impl Prepared {
    /// DOL 0x801ed060 visits the target and descendants in reverse active-node
    /// order, once per node. Only generators receive virtual +0x4c.
    pub fn echo_subgraph(
        &mut self,
        graph: &Document,
        target: &str,
        duration: f32,
        resources: &mut super::animation_resources::Resources,
    ) -> Result<()> {
        let root = self
            .plan
            .nodes
            .iter()
            .position(|n| n.object == target)
            .context("Inactive echo target")?;
        let mut visited = vec![false; root + 1];
        visited[root] = true;
        for index in (0..=root).rev() {
            if !visited[index] {
                continue;
            }
            let node = &self.plan.nodes[index];
            for &child in &node.children {
                ensure!(child < index, "Echo graph is not in postorder");
                visited[child] = true;
            }
            if node.effect.is_some() {
                continue;
            } // base generator +0x4c is a no-op
            let object = graph
                .objects
                .get(&node.object)
                .context("Missing echo definition")?;
            match object.klass.as_str() {
                "hkbClipGenerator" => {
                    let definition = &self.state.clips[&node.object];
                    let resource = resources.clip_binding(&definition.settings.animation_name,definition.settings.animation_binding_index)?;
                    self.clips.start_echo(
                        &node.object,
                        definition,
                        self.state.words(),
                        resource,
                        duration,
                    )?;
                }
                "hkbStateMachine" => {
                    self.machines
                        .get_mut(&node.object)
                        .context("Inactive echo machine")?
                        .echo_next_update = true
                }
                "hkbBlenderGenerator" => {
                    self.blender_echoes.insert(node.object.clone());
                }
                "hkbModifierGenerator"
                | "hkbModifierList"
                | "hkbEventDrivenModifier"
                | "hkbEvaluateExpressionModifier"
                | "hkbAttributeModifier"
                | "hkbTwistModifier"
                | "hkbAttachmentModifier" => {}
                class => anyhow::bail!("Echo operation for {class} is not implemented"),
            }
        }
        Ok(())
    }

    pub fn service_blend_self_transition(
        &mut self,
        graph: &Document,
        handle: Handle,
        resources: &mut super::animation_resources::Resources,
    ) -> Result<()> {
        let instance = self.transitions.get(handle)?;
        let Some(super::transition_instances::Playback::Blend(playback)) = &instance.playback
        else {
            return Ok(());
        };
        if !playback.target_was_active {
            return Ok(());
        }
        let definition = match &self.state.transition_effects[&instance.definition] {
            super::transition_effect::Definition::Blend(d) => d,
            _ => anyhow::bail!("Transition definition/playback mismatch"),
        };
        let target = instance
            .target
            .as_ref()
            .context("Unbound self-transition target")?
            .clone();
        let mode = if definition.self_transition_mode == 0 {
            if self.cyclic_output(graph, &target)? {
                1
            } else {
                3
            }
        } else {
            definition.self_transition_mode
        };
        let duration = playback.duration; // blending effect virtual +0x60
        match mode {
            1 => {}
            2 => self.echo_subgraph(graph, &target, 0., resources)?,
            3 => self.echo_subgraph(graph, &target, duration, resources)?,
            _ => anyhow::bail!("Invalid self-transition policy"),
        }
        if let Some(super::transition_instances::Playback::Blend(p)) =
            &mut self.transitions.get_mut(handle)?.playback
        {
            p.target_was_active = false;
        }
        Ok(())
    }
    /// The cyclic byte (+0x4d) used by the native default self-transition
    /// policy. This is not a substitute for frequency/marker synchronization.
    pub fn cyclic_output(&self, graph: &Document, id: &str) -> Result<bool> {
        let node = self
            .plan
            .nodes
            .iter()
            .find(|n| n.object == id)
            .with_context(|| format!("Missing active synchronization target {id}"))?;
        if let Some(handle) = node.effect {
            let instance = self.transitions.get(handle)?;
            let target = instance.target.as_deref().context("Unbound sync target")?;
            if instance.source.as_ref() == Some(&Binding::Generator(target.into())) {
                return self.cyclic_output(graph, target);
            }
            match &self.state.transition_effects[&instance.definition] {
                super::transition_effect::Definition::Blend(d) if d.flags & 2 == 0 => {
                    return Ok(false)
                }
                _ => return self.cyclic_output(graph, target),
            }
        }
        let object = graph
            .objects
            .get(id)
            .context("Missing synchronization definition")?;
        match object.klass.as_str() {
            // DOL 0x801fa2b4..0x801fa2c8.
            "hkbClipGenerator" => Ok(self.state.clips[id].resolve(self.state.words())?.mode == 1),
            // DOL 0x80217db0 copies the current generator/effect's sync info.
            "hkbStateMachine" => {
                ensure!(
                    node.children.len() == 1,
                    "Invalid state-machine sync children"
                );
                self.cyclic_output(graph, &self.plan.nodes[node.children[0]].object)
            }
            // DOL 0x801fecbc copies the generator, then permits the modifier
            // to modify synchronization. Only known no-op modifiers qualify.
            "hkbModifierGenerator" => {
                let child = object.fields["generator"]["ref"]
                    .as_str()
                    .context("Missing modifier generator child")?;
                self.cyclic_output(graph, child)
            }
            "hkbBlenderGenerator" => {
                let flags = object.fields["flags"]
                    .as_u64()
                    .context("Missing blender flags")?;
                // 0x801f5a04 / 0x801f5b8c: a synchronizing blender is cyclic
                // exactly when it has active children, regardless of weights.
                if flags & 1 != 0 {
                    return Ok(!node.children.is_empty());
                }
                let master = object.fields["indexOfSyncMasterChild"]
                    .as_i64()
                    .context("Missing blender sync master")?;
                if master < 0 {
                    return Ok(false);
                }
                let children = object.fields["children"]
                    .as_array()
                    .context("Missing blender children")?;
                let reference = children
                    .get(master as usize)
                    .context("Invalid blender sync master")?["ref"]
                    .as_str()
                    .context("Invalid blender child")?;
                let child = graph
                    .objects
                    .get(reference)
                    .context("Missing blender child")?
                    .fields["generator"]["ref"]
                    .as_str()
                    .context("Missing blender generator")?;
                self.cyclic_output(graph, child)
            }
            _ => Ok(false),
        }
    }
}

pub(super) fn bound_float(
    graph: &Document,
    state: &State,
    object: &str,
    member: &str,
) -> Result<f32> {
    state.real_member(graph, object, member)
}

struct Builder<'a> {
    graph: &'a Document,
    state: &'a mut State,
    visiting: BTreeSet<String>,
    visited: BTreeMap<String, usize>,
    plan: Plan,
    seed: u32,
    machines: BTreeMap<String, Runtime>,
    blenders: BTreeMap<String, Vec<blender::Child>>,
    retained_nodes: BTreeSet<String>,
    event_modifiers: &'a mut BTreeSet<String>,
    transitions: Instances,
    transition_stacks: BTreeMap<String, super::machine_transitions::Stack<Handle>>,
    visiting_effects: BTreeSet<Handle>,
}
impl Prepared {
    /// Complete activation before updates or another topology rebuild. Remove
    /// each request only after success so missing resources remain diagnostic.
    pub fn activate_pending(
        &mut self,
        graph: &Document,
        resources: &mut super::animation_resources::Resources,
    ) -> Result<()> {
        while let Some(id) = self.plan.pending_generator_activation.first().cloned() {
            let object = graph.objects.get(&id).context("Missing activation node")?;
            match object.klass.as_str() {
                "hkbClipGenerator" => {
                    let definition = &self.state.clips[&id];
                    let resource = resources.clip_binding(&definition.settings.animation_name,definition.settings.animation_binding_index)?;
                    self.clips
                        .activate(&id, definition, self.state.words(), resource)?;
                }
                "hkbEvaluateExpressionModifier" => {
                    self.expressions.handle_event(graph, &mut self.state, &id)?
                }
                "hkbAttachmentModifier" => self.attachment_modifiers.activate(graph, &id)?,
                // Blender child selection is initialized by Builder::visit.
                // Sync-master selection remains a separate synchronization pass.
                "hkbBlenderGenerator"
                | "hkbModifierGenerator"
                | "hkbModifierList"
                | "hkbAttributeModifier"
                | "hkbTwistModifier" => {}
                _ => {}
            }
            self.plan.pending_generator_activation.remove(0);
        }
        // Mode2 transfers its reset request from the transition to a child
        // edge. Reactivate the complete target subgraph, including nested
        // state machines; resetting only its leaf loses state-entry semantics.
        if self.plan.nodes.iter().any(|node|node.flags.reactivate) {
            let mut reset=vec![false;self.plan.nodes.len()];
            for index in (0..self.plan.nodes.len()).rev() {
                if self.plan.nodes[index].flags.reactivate || reset[index] {
                    reset[index]=true;
                    for &child in &self.plan.nodes[index].children {reset[child]=true;}
                }
            }
            let retired=self.plan.nodes.iter_mut().zip(&reset).filter_map(|(node,&reset)|{
                if reset && node.effect.is_none(){node.flags.reactivate=true;Some(node.object.clone())}else{None}
            }).collect::<Vec<_>>();
            let mut emitted=Vec::new();
            self.deactivate_removed(graph,&retired,|_,notice|{
                match notice {
                    Notice::Event(event)=>emitted.push(event.clone()),
                    Notice::SyncVariable{..} | Notice::Listener{..}=>{},
                }Ok(())
            })?;
            self.plan.pending_events.extend(emitted);
            // Marked nodes must be treated as newly active by Builder, while
            // unrelated retained generators keep their clocks and resources.
            rebuild_in_place(graph,self)?;
            self.activate_pending(graph,resources)?;
        }
        Ok(())
    }
    /// Service old-tree removals in their original child-first order. Merely
    /// deleting plan nodes leaves machines active and reuses stale clip clocks
    /// when locomotion returns to a previously visited state.
    pub fn deactivate_removed(
        &mut self,
        graph: &Document,
        removed: &[String],
        mut notify: impl FnMut(&Runtime, Notice<'_>) -> Result<()>,
    ) -> Result<()> {
        for id in removed {
            let class = &graph.objects.get(id).context("Missing retired node")?.klass;
            match class.as_str() {
                "hkbClipGenerator" => {
                    self.clips.deactivate(id)?;
                }
                "hkbEvaluateExpressionModifier" => self.expressions.deactivate(id),
                "hkbAttachmentModifier" => self.attachment_modifiers.deactivate(id)?,
                "hkbEventDrivenModifier" => { self.event_modifiers.remove(id); }
                "hkbStateMachine" => {
                    if !self
                        .machines
                        .get(id)
                        .context("Missing retired machine")?
                        .active
                    {
                        continue;
                    }
                    if let Some(last) = self
                        .transition_stacks
                        .get(id)
                        .and_then(|s| s.records.len().checked_sub(1))
                    {
                        self.retire_transitions(id, last, &mut notify)?;
                    }
                    self.dispatchers.remove(id);
                    self.machines
                        .get_mut(id)
                        .unwrap()
                        .deactivate(&self.state.machines[id], &mut notify)?;
                }
                // Verified inherited no-op deactivate slots.
                "hkbBlenderGenerator"
                | "hkbModifierGenerator"
                | "hkbModifierList"
                | "hkbAttributeModifier"
                | "hkbTwistModifier" => {}
                _ => {}
            }
        }
        Ok(())
    }
    /// Update event windows, initiate ready pending requests, then arbitrate
    /// ordinary candidates. The owner controls machine visitation/notices.
    pub fn dispatch_machine_event(
        &mut self,
        id: &str,
        event: i32,
        nested_state: Option<i32>,
        mut notify: impl FnMut(&Runtime, Notice<'_>) -> Result<()>,
    ) -> Result<bool> {
        use super::machine_arbitration::{Decision, Identity, Request};
        let runtime = self
            .machines
            .get_mut(id)
            .context("Missing event target machine")?;
        ensure!(
            runtime.active,
            "Cannot dispatch to an inactive state machine"
        );
        // -1 is the native update/arbitration sentinel (80218360), not an
        // incoming event. Treating it as input would open eventless windows.
        if event != -1 {
            runtime.event_windows(&self.state.machines[id], event)?;
        }
        let ready =
            self.dispatchers
                .entry(id.into())
                .or_default()
                .take_ready(false, |identity| {
                    let intervals = if identity.wildcard {
                        &runtime.wildcard_intervals
                    } else {
                        ensure!(
                            identity.state == runtime.current,
                            "Pending transition refers to a retired local state"
                        );
                        &runtime.local_intervals
                    };
                    intervals
                        .get(identity.index)
                        .copied()
                        .context("Missing live pending interval")
                })?;
        let mut changed = false;
        if let Some(request) = ready {
            self.start_transition(id, &request.transition, request.reset_previous, &mut notify)?;
            changed = true;
        }
        let running = self
            .transition_stacks
            .get(id)
            .and_then(|s| s.records.last())
            .map(|r| &r.transition);
        let allow = self.dispatchers[id].allows_selection(running);
        let runtime = &self.machines[id];
        let mut selection = super::transition_select::Selection::default();
        if allow
            && event != -1
            && event == self.state.machines[id].settings.random_transition_event_id
        {
            self.state.select_random_machine_candidates(
                id,
                runtime,
                event,
                nested_state,
                &mut self.random_seed,
                &mut selection,
            )?;
        } else {
            self.state.select_machine_candidates(
                id,
                runtime,
                event,
                nested_state,
                allow,
                &mut selection,
            )?;
        }
        let machine = &self.state.machines[id];
        let current = machine.state_indices[&runtime.current];
        let requests = selection
            .candidates
            .iter()
            .map(|c| {
                let (table, intervals) = if c.wildcard {
                    (&machine.wildcard_transitions, &runtime.wildcard_intervals)
                } else {
                    (
                        &machine.states[current].transitions,
                        &runtime.local_intervals,
                    )
                };
                let transition = table[c.index].clone();
                let reset_previous = event != -1
                    && event == machine.settings.return_to_previous_state_event_id
                    && transition.to_state_id == runtime.previous
                    && runtime.previous != runtime.current
                    && transition.flags & 64 == 0;
                Request {
                    identity: Identity {
                        state: if c.wildcard { 0 } else { runtime.current },
                        wildcard: c.wildcard,
                        index: c.index,
                    },
                    transition,
                    interval: intervals[c.index],
                    reset_previous,
                }
            })
            .collect();
        let running = self
            .transition_stacks
            .get(id)
            .and_then(|s| s.records.last())
            .map(|r| &r.transition);
        match self
            .dispatchers
            .entry(id.into())
            .or_default()
            .submit(requests, running)
        {
            Decision::Start(request) => {
                self.start_transition(id, &request.transition, request.reset_previous, notify)?;
                Ok(true)
            }
            _ => Ok(changed),
        }
    }
    pub fn retire_transitions(
        &mut self,
        machine_id: &str,
        through: usize,
        notify: impl FnMut(&Runtime, Notice<'_>) -> Result<()>,
    ) -> Result<()> {
        let stack = self
            .transition_stacks
            .get_mut(machine_id)
            .context("Missing transition stack")?;
        let runtime = self
            .machines
            .get_mut(machine_id)
            .context("Missing active transition machine")?;
        let machine = self
            .state
            .machines
            .remove(machine_id)
            .context("Missing transition machine definition")?;
        let result = stack.retire_through(
            through,
            &machine,
            runtime,
            &mut super::transition_instances::Owner {
                instances: &mut self.transitions,
                state: &mut self.state,
                notify,
            },
        );
        self.state.machines.insert(machine_id.into(), machine);
        if result.is_ok() {
            self.transitions.topology_changed = true;
        }
        result
    }
    /// Execute an already arbitrated, initiation-ready transition against this
    /// character's retained stack. Event selection remains a separate phase.
    pub fn start_transition(
        &mut self,
        machine_id: &str,
        transition: &super::state_machine::Transition,
        reset_previous: bool,
        notify: impl FnMut(&Runtime, Notice<'_>) -> Result<()>,
    ) -> Result<()> {
        let runtime = self
            .machines
            .get_mut(machine_id)
            .context("Missing active transition machine")?;
        let machine = self
            .state
            .machines
            .remove(machine_id)
            .context("Missing transition machine definition")?;
        let result = self
            .transition_stacks
            .entry(machine_id.into())
            .or_default()
            .start(
                &machine,
                runtime,
                transition,
                reset_previous,
                &mut super::transition_instances::Owner {
                    instances: &mut self.transitions,
                    state: &mut self.state,
                    notify,
                },
            );
        self.state.machines.insert(machine_id.into(), machine);
        if result.is_ok() {
            if let Some(dispatcher) = self.dispatchers.get_mut(machine_id) {
                dispatcher.pending.clear();
            }
            // Route nested destination state (Havok FLAG_TO_NESTED_STATE_ID_IS_VALID = 8192).
            if transition.flags & 8192 != 0 {
                if let Some(target_gen) = self.state.machines.get(machine_id).and_then(|m| {
                    m.state_indices
                        .get(&transition.to_state_id)
                        .map(|&idx| m.states[idx].info.generator.id.clone())
                }) {
                    if let (Some(child_machine), Some(child_runtime)) = (
                        self.state.machines.get(&target_gen),
                        self.machines.get_mut(&target_gen),
                    ) {
                        if child_machine
                            .state_indices
                            .contains_key(&transition.to_nested_state_id)
                        {
                            child_runtime.current = transition.to_nested_state_id;
                        }
                    }
                }
            }
        }
        result
    }
}
impl Builder<'_> {
    /// DOL 0x802243c4..0x80224444 propagates event suppression from a
    /// parent to its owned children, traversing the postorder list backwards.
    fn propagate_event_suppression(&mut self) {
        for index in (0..self.plan.nodes.len()).rev() {
            if let Some(parent) = self.plan.nodes[index].event_parent {
                self.plan.nodes[index].flags.discard_events |=
                    self.plan.nodes[parent].flags.discard_events;
            }
        }
    }
    fn attach(&mut self, parent: usize, child: usize, edge: Flags, owner: Flags) {
        let node = &mut self.plan.nodes[child];
        if node.parent.is_none() || (owner.synchronize && edge.synchronize) {
            node.parent = Some(parent);
        }
        node.event_parent = Some(parent);
        node.flags.discard_events = owner.discard_events && edge.discard_events;
        node.flags.synchronize |= owner.synchronize && edge.synchronize;
        node.flags.reactivate |= owner.reactivate && edge.reactivate;
    }
    fn visit_effect(&mut self, handle: Handle) -> Result<usize> {
        if let Some(index) = self
            .plan
            .nodes
            .iter()
            .position(|n| n.effect == Some(handle))
        {
            return Ok(index);
        }
        ensure!(
            self.visiting_effects.insert(handle),
            "Cycle in active transition instances"
        );
        self.transitions
            .activate_with_variables(handle, &self.state.transition_effects, self.state.words(), |id| {
                self.retained_nodes.contains(id)
            })?;
        // EM1's global event mode is initialized to 2 at DOL 0x8021f540.
        let children = self
            .transitions
            .children(handle, &self.state.transition_effects, 2)?;
        let mut indices = vec![];
        let mut edges = vec![];
        for child in children.nodes {
            let index = match child.binding {
                Binding::Generator(id) => self.visit(&id)?,
                Binding::Effect(h) => self.visit_effect(h)?,
            };
            edges.push(child.flags);
            indices.push(index);
        }
        let index = self.plan.nodes.len();
        for (&child, edge) in indices.iter().zip(edges) {
            self.attach(index, child, edge, children.flags);
        }
        self.plan.nodes.push(Node {
            object: self.transitions.get(handle)?.definition.clone(),
            parent: None,
            event_parent: None,
            children: indices,
            effect: Some(handle),
            flags: Flags::default(),
            effect_flags: Some(children.flags),
        });
        self.visiting_effects.remove(&handle);
        Ok(index)
    }
    fn visit(&mut self, id: &str) -> Result<usize> {
        ensure!(
            !self.visiting.contains(id),
            "Cycle in active generator references: {id}"
        );
        if let Some(&index) = self.visited.get(id) {
            return Ok(index);
        }
        self.visiting.insert(id.into());
        let object = self
            .graph
            .objects
            .get(id)
            .context("Missing active generator")?;
        let fields = &object.fields;
        let reference = |member: &str| -> Result<String> {
            Ok(fields[member]["ref"]
                .as_str()
                .with_context(|| format!("Missing {member} in {id}"))?
                .into())
        };
        let mut transition_child = None;
        let children = match object.klass.as_str() {
            "hkbStateMachine" => {
                let mut runtime = self.machines.remove(id)
                    .unwrap_or_else(||Runtime::from_definition(&self.state.machines[id]));
                self.state
                    .activate_machine(id, &mut runtime, &mut self.seed, |_, notice| {
                        match notice {
                            Notice::Event(event) => self.plan.pending_events.push(event.clone()),
                            Notice::SyncVariable { .. } | Notice::Listener { .. } => {}
                        }
                        Ok(())
                    })?;
                let child = runtime
                    .current_generator(&self.state.machines[id])?
                    .id
                    .clone();
                self.machines.insert(id.into(), runtime);
                if let Some(record) = self
                    .transition_stacks
                    .get(id)
                    .and_then(|s| s.records.last())
                {
                    transition_child = Some(record.effect);
                    vec![]
                } else {
                    vec![child]
                }
            }
            "hkbModifierGenerator" => {
                ensure!(
                    fields["variableBindingSet"].is_null(),
                    "Modifier-generator topology bindings are not implemented"
                );
                let mut children = vec![reference("generator")?];
                if fields["modifier"].is_object() {
                    children.push(reference("modifier")?);
                }
                if !self.retained_nodes.contains(id) {
                    self.plan.pending_generator_activation.push(id.into());
                }
                children
            }
            "hkbBlenderGenerator" => {
                let references = fields["children"]
                    .as_array()
                    .context("Missing blend children")?;
                let mut weights = vec![];
                let mut generators = vec![];
                for child in references {
                    let id = child["ref"].as_str().context("Invalid blend child")?;
                    let child = self.graph.objects.get(id).context("Missing blend child")?;
                    ensure!(
                        child.klass == "hkbBlenderGeneratorChild",
                        "Invalid blend child class"
                    );
                    weights.push(bound_float(self.graph, &self.state, id, "weight")?);
                    generators.push(
                        child.fields["generator"]["ref"]
                            .as_str()
                            .context("Missing blended generator")?
                            .to_owned(),
                    );
                }
                let master = fields["indexOfSyncMasterChild"]
                    .as_i64()
                    .context("Missing sync master")?;
                ensure!(master >= -1, "Invalid sync master");
                let parameters = blender::Parameters {
                    children: &weights,
                    parameter: bound_float(self.graph, &self.state, id, "blendParameter")?,
                    cyclic_min: fields["minCyclicBlendParameter"]
                        .as_f64()
                        .context("Missing cyclic minimum")? as f32,
                    cyclic_max: fields["maxCyclicBlendParameter"]
                        .as_f64()
                        .context("Missing cyclic maximum")? as f32,
                    flags: fields["flags"]
                        .as_u64()
                        .context("Missing blender flags")?
                        .try_into()?,
                    sync_master: (master >= 0).then_some(master as usize),
                };
                let mut children = self.blenders.remove(id).unwrap_or_default();
                if self.retained_nodes.contains(id) {
                    // The active-node update already selected these children.
                    // Re-evaluating here loses newly_active and can observe
                    // variable writes made later in the same scheduling pass.
                    ensure!(
                        children.len() == generators.len(),
                        "Retained blender child topology changed"
                    );
                } else {
                    blender::update(&parameters, true, &mut children)?;
                }
                if !self.retained_nodes.contains(id) {
                    self.plan.pending_generator_activation.push(id.into());
                }
                let active = generators
                    .into_iter()
                    .zip(&children)
                    .filter_map(|(id, c)| c.active.then_some(id))
                    .collect();
                self.blenders.insert(id.into(), children);
                active
            }
            "hkbEventDrivenModifier" => {
                ensure!(fields["variableBindingSet"].is_null(), "Event-driven modifier has topology bindings");
                if !self.retained_nodes.contains(id) {
                    self.event_modifiers.remove(id);
                    if fields["activeByDefault"].as_bool().context("Missing event modifier default")? {
                        self.event_modifiers.insert(id.into());
                    }
                }
                if fields["enable"].as_bool().context("Missing event modifier enable")?
                    && self.event_modifiers.contains(id)
                {
                    vec![reference("modifier")?]
                } else {
                    vec![]
                }
            }
            "hkbModifierList" => {
                // Retail vtable806b5318: child enumeration b9a0/b9c4
                // preserves list order and suppresses children when disabled.
                ensure!(
                    fields["variableBindingSet"].is_null(),
                    "Modifier-list topology bindings are not implemented"
                );
                let children = fields["modifiers"]
                    .as_array()
                    .context("Missing modifier list")?;
                if !fields["enable"]
                    .as_bool()
                    .context("Missing modifier-list enable")?
                {
                    vec![]
                } else {
                    children
                        .iter()
                        .map(|child| {
                            Ok(child["ref"]
                                .as_str()
                                .context("Null/invalid modifier-list child")?
                                .to_owned())
                        })
                        .collect::<Result<Vec<_>>>()?
                }
            }
            "hkbClipGenerator"
            | "hkbEvaluateExpressionModifier"
            | "hkbAttributeModifier"
            | "hkbTwistModifier"
            | "hkbAttachmentModifier" => {
                if !self.retained_nodes.contains(id) {
                    self.plan.pending_generator_activation.push(id.into());
                }
                vec![]
            }
            _ => vec![],
        };
        let mut child_indices = vec![];
        if let Some(effect) = transition_child {
            child_indices.push(self.visit_effect(effect)?);
        }
        for child in children {
            child_indices.push(self.visit(&child)?);
        }
        let index = self.plan.nodes.len();
        for &child in &child_indices {
            self.attach(index, child, Flags::default(), Flags::default());
        }
        self.plan.nodes.push(Node {
            object: id.into(),
            parent: None,
            event_parent: None,
            children: child_indices,
            effect: None,
            flags: Flags::default(),
            effect_flags: None,
        });
        self.visiting.remove(id);
        self.visited.insert(id.into(), index);
        Ok(index)
    }
}

/// Build in isolation; this never mutates the playable character's variables.
pub fn initial(graph: &Document, seed: u32) -> Result<Plan> {
    Ok(prepare(graph, State::new(graph)?, seed)?.plan)
}

/// Consume the character's variable state and preserve start-state choices,
/// interval state, blend child history and RNG advancement for later updates.
pub fn prepare(graph: &Document, mut state: State, seed: u32) -> Result<Prepared> {
    let root = graph.single("hkbBehaviorGraph")?["rootGenerator"]["ref"]
        .as_str()
        .context("Missing root generator")?;
    let mut event_modifiers = BTreeSet::new();
    let mut builder = Builder {
        graph,
        state: &mut state,
        visiting: BTreeSet::new(),
        visited: BTreeMap::new(),
        plan: Plan {
            nodes: vec![],
            pending_generator_activation: vec![],
            pending_events: vec![],
        },
        seed,
        machines: BTreeMap::new(),
        blenders: BTreeMap::new(),
        retained_nodes: BTreeSet::new(),
        event_modifiers: &mut event_modifiers,
        transitions: Default::default(),
        transition_stacks: Default::default(),
        visiting_effects: Default::default(),
    };
    builder.visit(root)?;
    builder.propagate_event_suppression();
    Ok(Prepared {
        plan: builder.plan,
        machines: builder.machines,
        blenders: builder.blenders,
        random_seed: builder.seed,
        clips: Default::default(),
        expressions: Default::default(),
        attachment_modifiers: Default::default(),
        transitions: builder.transitions,
        transition_stacks: builder.transition_stacks,
        dispatchers: Default::default(),
        blender_echoes: Default::default(),
        event_modifiers,
        state,
    })
}

/// Rebuild the state-selected tree without restarting retained active nodes.
/// The owner must service returned removals and pending activations explicitly.
/// Retained transition stacks supply source/destination branches and edge flags.
/// Time advancement, event dispatch and retirement remain scheduler operations.
pub fn rebuild(graph: &Document, mut prepared: Prepared) -> Result<(Prepared, Vec<String>)> {
    let removed = rebuild_in_place(graph, &mut prepared)?;
    Ok((prepared, removed))
}

/// Replan the live owner without recompiling or copying its character state.
/// On failure runtime maps are restored for diagnostics; the caller must stop
/// execution because state-entry callbacks may already have run.
pub fn rebuild_in_place(graph: &Document, prepared: &mut Prepared) -> Result<Vec<String>> {
    let root = graph.single("hkbBehaviorGraph")?["rootGenerator"]["ref"]
        .as_str()
        .context("Missing root generator")?;
    ensure!(
        prepared.plan.pending_generator_activation.is_empty(),
        "Service pending generator activations before rebuilding the graph"
    );
    let retained_nodes = prepared
        .plan
        .nodes
        .iter()
        .filter(|n| n.effect.is_none() && !n.flags.reactivate)
        .map(|n| n.object.clone())
        .collect();
    let mut builder = Builder {
        graph,
        state: &mut prepared.state,
        visiting: BTreeSet::new(),
        visited: BTreeMap::new(),
        plan: Plan {
            nodes: vec![],
            pending_generator_activation: vec![],
            pending_events: std::mem::take(&mut prepared.plan.pending_events),
        },
        seed: prepared.random_seed,
        machines: std::mem::take(&mut prepared.machines),
        blenders: std::mem::take(&mut prepared.blenders),
        retained_nodes,
        event_modifiers: &mut prepared.event_modifiers,
        transitions: std::mem::take(&mut prepared.transitions),
        transition_stacks: std::mem::take(&mut prepared.transition_stacks),
        visiting_effects: BTreeSet::new(),
    };
    let result = builder.visit(root);
    if result.is_ok() {
        builder.propagate_event_suppression();
    }
    prepared.machines = builder.machines;
    prepared.blenders = builder.blenders;
    prepared.transitions = builder.transitions;
    prepared.transition_stacks = builder.transition_stacks;
    prepared.random_seed = builder.seed;
    result?;
    let removed = prepared
        .plan
        .nodes
        .iter()
        .filter(|n| n.effect.is_none() && !builder.visited.contains_key(&n.object))
        .map(|n| n.object.clone())
        .collect();
    prepared.plan = builder.plan;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires original locally extracted Mickey behavior"]
    fn cyclic_policy_respects_blender_sync_instead_of_guessing_from_leaf_clips() -> Result<()> {
        let graph = Document::load(&std::fs::read(
            "target/managed-player-assets/MickeyBehavior.hkx_wii",
        )?)?;
        let prepared = prepare(&graph, State::new(&graph)?, 0)?;
        assert!(prepared.cyclic_output(&graph, "2:6480")?);
        assert!(prepared.cyclic_output(&graph, "2:6176")?);
        // IdleBlend has flags=16 and no sync master. Its output is therefore
        // noncyclic even though its selected idle animation loops.
        assert!(!prepared.cyclic_output(&graph, "2:2576")?);
        assert!(!prepared.cyclic_output(&graph, "2:240")?);
        Ok(())
    }
    #[test]
    #[ignore = "requires original locally extracted Mickey behavior"]
    fn walk_transition_rebuild_keeps_source_and_destination_under_effect() -> Result<()> {
        let graph = Document::load(&std::fs::read(
            "target/managed-player-assets/MickeyBehavior.hkx_wii",
        )?)?;
        let mut prepared = prepare(&graph, State::new(&graph)?, 0)?;
        prepared.plan.pending_generator_activation.clear();
        let machine = &prepared.state.machines["2:240"];
        let source = machine.states[machine.state_indices[&0]]
            .info
            .generator
            .id
            .clone();
        let target = machine.states[machine.state_indices[&5]]
            .info
            .generator
            .id
            .clone();
        assert!(prepared.dispatch_machine_event("2:240", 0, None, |_, _| Ok(()))?);
        assert_eq!(prepared.machines["2:240"].current, 5);
        assert_eq!(prepared.transition_stacks["2:240"].records[0].to_state, 5);
        let handle = prepared.transition_stacks["2:240"].records[0].effect;
        let (mut prepared, removed) = rebuild(&graph, prepared)?;
        assert!(!removed.iter().any(|id| id == "2:6480"));
        assert!(prepared.plan.nodes.iter().any(|n| n.object == "2:6480"));
        let effect = prepared
            .plan
            .nodes
            .iter()
            .position(|n| n.effect == Some(handle))
            .unwrap();
        let children: Vec<_> = prepared
            .plan
            .nodes
            .iter()
            .filter(|n| n.parent == Some(effect))
            .collect();
        assert_eq!(
            children
                .iter()
                .map(|n| n.object.as_str())
                .collect::<Vec<_>>(),
            [target.as_str(), source.as_str()]
        );
        assert!(children[1].flags.discard_events);
        assert!(
            prepared
                .plan
                .nodes
                .iter()
                .find(|n| n.object == "2:6480")
                .unwrap()
                .flags
                .discard_events
        );
        assert!(
            prepared
                .plan
                .nodes
                .iter()
                .find(|n| n.object == "2:6240")
                .unwrap()
                .flags
                .discard_events
        );
        assert!(!children[0].flags.discard_events);
        assert_eq!(
            prepared.plan.nodes[effect].parent,
            Some(prepared.plan.nodes.len() - 1)
        );
        assert_eq!(prepared.plan.nodes.last().unwrap().object, "2:240");
        if let Some(super::super::transition_instances::Playback::Blend(p)) =
            &mut prepared.transitions.get_mut(handle)?.playback
        {
            p.clock.time = 0.125;
        } else {
            anyhow::bail!("Original walk transition is not a blending effect");
        }
        prepared.plan.pending_generator_activation.clear();
        let (mut prepared, removed) = rebuild(&graph, prepared)?;
        assert!(removed.is_empty());
        if let Some(super::super::transition_instances::Playback::Blend(p)) =
            &prepared.transitions.get(handle)?.playback
        {
            assert_eq!(p.clock.time, 0.125);
        }
        prepared.retire_transitions("2:240", 0, |_, _| Ok(()))?;
        let (prepared, removed) = rebuild(&graph, prepared)?;
        assert!(removed.iter().any(|id| id == "2:6480"));
        assert!(!prepared.plan.nodes.iter().any(|n| n.effect.is_some()));
        assert!(prepared.plan.nodes.iter().any(|n| n.object == target));
        assert!(prepared.transitions.get(handle).is_err());
        Ok(())
    }
    #[test]
    #[ignore = "requires original locally extracted Mickey behavior"]
    fn replanning_preserves_active_machine_clocks_and_reports_branch_changes() -> Result<()> {
        let graph = Document::load(&std::fs::read(
            "target/managed-player-assets/MickeyBehavior.hkx_wii",
        )?)?;
        let mut prepared = prepare(&graph, State::new(&graph)?, 17)?;
        prepared.plan.pending_generator_activation.clear();
        for machine in prepared.machines.values_mut() {
            machine.time = 0.75;
        }
        let nodes = serde_json::to_value(&prepared.plan.nodes)?;
        let (mut prepared, removed) = rebuild(&graph, prepared)?;
        assert!(removed.is_empty());
        assert_eq!(serde_json::to_value(&prepared.plan.nodes)?, nodes);
        assert!(prepared.plan.pending_generator_activation.is_empty());
        assert!(prepared.machines.values().all(|m| m.time == 0.75));
        assert_eq!(prepared.random_seed, 17);
        let mut notices = vec![];
        prepared.machines.get_mut("2:240").unwrap().enter(
            &prepared.state.machines["2:240"],
            5,
            false,
            &mut |_, notice| {
                notices.push(format!("{notice:?}"));
                Ok(())
            },
        )?;
        let (prepared, removed) = rebuild(&graph, prepared)?;
        assert!(removed.iter().any(|id| id == "2:6480"));
        assert!(prepared.plan.nodes.iter().any(|n| n.object == "2:35968"));
        assert!(!prepared
            .plan
            .pending_generator_activation
            .iter()
            .any(|id| id == "2:240"));
        assert_eq!(prepared.machines["2:240"].current, 5);
        Ok(())
    }
    #[test]
    #[ignore = "requires original locally extracted Mickey behavior"]
    fn initial_tree_matches_captured_active_nodes_and_parent_order() -> Result<()> {
        let graph = Document::load(&std::fs::read(
            "target/managed-player-assets/MickeyBehavior.hkx_wii",
        )?)?;
        let plan = initial(&graph, 0)?;
        let captured: serde_json::Value = serde_json::from_str(include_str!(
            "../../tools/blotworx/fixtures/initial_generator_tree.json"
        ))?;
        let mut observed = serde_json::to_value(&plan.nodes)?;
        for node in observed.as_array_mut().unwrap() {
            node.as_object_mut().unwrap().remove("children");
            node.as_object_mut().unwrap().remove("event_parent");
        }
        assert_eq!(observed, captured["nodes"]);
        for (parent, node) in plan.nodes.iter().enumerate() {
            for &child in &node.children {
                assert_eq!(plan.nodes[child].parent, Some(parent));
                assert_eq!(plan.nodes[child].event_parent, Some(parent));
            }
        }
        assert_eq!(plan.nodes.len(), 8);
        assert!(!plan.pending_generator_activation.is_empty());
        let prepared = prepare(&graph, State::new(&graph)?, 0)?;
        assert_eq!(prepared.machines.len(), 4);
        assert_eq!(prepared.blenders.len(), 1);
        assert!(prepared.machines.values().all(|m| m.active));
        for (id, runtime) in &prepared.machines {
            let child = runtime.current_generator(&prepared.state.machines[id])?;
            assert!(prepared.plan.nodes.iter().any(|n| n.object == child.id));
        }
        let blend = &prepared.blenders["2:2576"];
        assert_eq!(blend.iter().filter(|c| c.active).count(), 1);
        Ok(())
    }
}
