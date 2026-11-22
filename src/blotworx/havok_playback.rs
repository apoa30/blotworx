//! Character-independent Havok graph playback. Uses the same fixed-tick
//! scheduler as Mickey; unsupported callbacks/root motion remain explicit.
use anyhow::{ensure, Context, Result};
use epic_mickey_lib_rs::hkx_file::{HkxBone, HkxSkeleton};
pub struct Playback {
    rig: HkxSkeleton,
    property_words: Vec<u32>,
    pub pose: Vec<HkxBone>,
    pub attributes: super::animation_attributes::Track,
    pub observed_events: u64,
    pub delivered_events: Vec<super::state_machine::Event>,
    external_delivered_events: Vec<super::state_machine::Event>,
    event_flags: Vec<u32>,
    pub(super) pending_events: std::collections::VecDeque<super::state_machine::Event>,
    /// Output from the most recent advance, grouped by fixed update. Incoming
    /// controller commands are excluded. The world consumes this before the
    /// next advance; bulk preview advances retain every update's markers.
    pub emitted_frames: Vec<Vec<super::state_machine::Event>>,
    pending_emitted: Vec<super::state_machine::Event>,
}
impl Playback {
    pub fn initialize(
        graph: &super::behavior::Document,
        prepared: &mut super::generator_plan::Prepared,
        resources: &mut super::animation_resources::Resources,
    ) -> Result<Self> {
        prepared.activate_pending(graph, resources)?;
        let rig = resources.rig.clone();
        let mut tracks = std::collections::BTreeMap::new();
        for node in &prepared.plan.nodes {
            if let Some(definition) = prepared.state.clips.get(&node.object) {
                let resource = resources.clip_binding(&definition.settings.animation_name,definition.settings.animation_binding_index)?;
                let instance = prepared.clips.get(&node.object)?;
                tracks.insert(
                    node.object.clone(),
                    resource.sample_tracks(rig.bones.len(), instance.clock.local_time)?,
                );
            }
        }
        let output = compose(graph, prepared, &rig, &resources.property_words, tracks)?;
        let pose = super::pose_blend::resolve(&rig, &output.transforms)?;
        let event_flags = graph.single("hkbBehaviorGraphData")?["eventInfos"]
            .as_array()
            .context("Missing graph event flags")?
            .iter()
            .map(|v| {
                v["flags"]
                    .as_u64()
                    .context("Invalid graph event flags")
                    .and_then(|n| u32::try_from(n).map_err(Into::into))
            })
            .collect::<Result<_>>()?;
        let initial = std::mem::take(&mut prepared.plan.pending_events);
        Ok(Self {
            rig,
            property_words: resources.property_words.clone(),
            pose,
            attributes: output.attributes,
            observed_events: 0,
            delivered_events: Vec::new(),
            external_delivered_events: Vec::new(),
            event_flags,
            pending_events: initial.clone().into(),
            pending_emitted: initial,
            emitted_frames: Vec::new(),
        })
    }
    pub(super) fn deliver_event(
        &mut self,
        graph: &super::behavior::Document,
        prepared: &mut super::generator_plan::Prepared,
        resources: &mut super::animation_resources::Resources,
        event: super::state_machine::Event,
    ) -> Result<()> {
        let emitted = Self::dispatch_event(graph, prepared, resources, &event)?;
        self.record_delivery(event, emitted);
        Ok(())
    }
    /// Native script callbacks can dispatch while the world owns the graph.
    /// Retain the resulting playback history without dispatching it twice.
    pub(super) fn record_external_delivery(
        &mut self,
        event: super::state_machine::Event,
        emitted: Vec<super::state_machine::Event>,
    ) {
        self.external_delivered_events.push(event.clone());
        self.record_delivery(event, emitted);
    }
    fn record_delivery(
        &mut self,
        event: super::state_machine::Event,
        emitted: Vec<super::state_machine::Event>,
    ) {
        self.pending_emitted.extend(emitted.iter().cloned());
        self.pending_events.extend(emitted);
        self.delivered_events.push(event);
    }
    pub(super) fn dispatch_event(
        graph: &super::behavior::Document,
        prepared: &mut super::generator_plan::Prepared,
        resources: &mut super::animation_resources::Resources,
        event: &super::state_machine::Event,
    ) -> Result<Vec<super::state_machine::Event>> {
        let mut emitted = Vec::new();
        prepared.dispatch_and_rebuild(
            graph,
            &event,
            resources,
            |owner, id| Ok(active_nested_state(graph, owner, id)),
            |_, notice| {
                match notice {
                    super::machine_runtime::Notice::Event(event) => {
                        emitted.push(event.clone());
                    }
                    super::machine_runtime::Notice::SyncVariable { .. }
                    | super::machine_runtime::Notice::Listener { .. } => {}
                }
                Ok(())
            },
        )?;
        let events = std::mem::take(&mut prepared.plan.pending_events);
        emitted.extend(events);
        Ok(emitted)
    }
    pub fn queue_named_event(
        &mut self,
        graph: &super::behavior::Document,
        name: &str,
    ) -> Result<()> {
        let strings = graph.single("hkbBehaviorGraphStringData")?;
        let names = strings["eventNames"]
            .as_array()
            .context("Missing animation events")?;
        let matches: Vec<_> = names
            .iter()
            .enumerate()
            .filter(|(_, n)| n.as_str() == Some(name))
            .collect();
        ensure!(
            matches.len() == 1,
            "Missing or ambiguous animation event {name}"
        );
        self.pending_events.push_back(super::state_machine::Event {
            id: matches[0].0.try_into()?,
            payload: None,
        });
        Ok(())
    }
    pub fn advance_character(
        &mut self,
        graph: &super::behavior::Document,
        prepared: &mut super::generator_plan::Prepared,
        resources: &mut super::animation_resources::Resources,
        ticks: u64,
    ) -> Result<()> {
        self.emitted_frames.clear();
        for _ in 0..ticks {
            self.delivered_events.clear();
            self.delivered_events
                .append(&mut self.external_delivered_events);
            prepared.activate_pending(graph, resources)?;
            let activation = std::mem::take(&mut prepared.plan.pending_events);
            self.pending_emitted.extend(activation.iter().cloned());
            self.pending_events.extend(activation);
            while let Some(event) = self.pending_events.pop_front() {
                self.deliver_event(graph, prepared, resources, event)?;
            }
            // Self-transition operations belong to synchronization, before
            // any clip advances or samples its restarted/echoed playback.
            let effects: Vec<_> = prepared
                .plan
                .nodes
                .iter()
                .filter_map(|n| n.effect)
                .collect();
            for handle in effects {
                prepared.service_blend_self_transition(graph, handle, resources)?;
                prepared.synchronize_generated(graph, handle, resources, &self.event_flags)?;
            }
            let mut events = Vec::new();
            let mut blender_changed = false;
            // Execute clips and expressions in the authored active-node order.
            // A transition may retain its source while activating a new target;
            // each generator owns its clock even when resources are shared.
            let update_nodes = prepared
                .plan
                .nodes
                .iter()
                .map(|node| (node.object.clone(), node.effect, node.flags))
                .collect::<Vec<_>>();
            for (object, effect, flags) in update_nodes {
                let node = super::generator_plan::Node {
                    object,
                    parent: None,
                    event_parent: None,
                    children: vec![],
                    effect,
                    flags,
                    effect_flags: None,
                };
                if let Some(handle) = node.effect {
                    if !prepared.update_generated(graph, handle, resources, super::STEP)? {
                        prepared.transitions.advance_duration_blend(
                            handle,
                            &prepared.state.transition_effects,
                            super::STEP,
                        )?;
                    }
                } else if let Some(children) = prepared.blenders.get_mut(&node.object) {
                    blender_changed |= super::blender::update_authored(
                        graph,
                        &prepared.state,
                        &node.object,
                        children,
                    )?;
                } else if let Some(definition) = prepared.state.clips.get(&node.object) {
                    let resource = resources.clip_binding(&definition.settings.animation_name,definition.settings.animation_binding_index)?;
                    let frame = prepared.clips.update(
                        &node.object,
                        definition,
                        prepared.state.words(),
                        resource,
                        super::STEP,
                        &self.event_flags,
                    )?;
                    // Extracted clip root motion (e.g. ledge grab, blast, hard land)
                    // is accepted without aborting playback; controller physics owns
                    // world-space translation.
                    let _ = frame.motion;
                    if !node.flags.discard_events {
                        events.extend(frame.events);
                    }
                } else if prepared
                    .state
                    .expression_modifiers
                    .contains_key(&node.object)
                {
                    prepared.expressions.update_with_random(
                        graph,
                        &mut prepared.state,
                        &node.object,
                        node.flags.discard_events,
                        &mut events,
                        &mut prepared.random_seed,
                    )?;
                } else if prepared
                    .attachment_modifiers
                    .instances
                    .contains_key(&node.object)
                {
                    // Attachment update is inherited blr; its wrapper still
                    // reconciles enable/activation even on zero-time updates.
                    prepared
                        .attachment_modifiers
                        .synchronize(graph, &node.object)?;
                } else if let Some(runtime) = prepared.machines.get_mut(&node.object) {
                    let definition = &prepared.state.machines[&node.object];
                    runtime.advance_windows(definition, super::STEP)?;
                }
            }
            self.observed_events += events.len() as u64;
            self.pending_emitted.extend(events.iter().cloned());
            self.pending_events.extend(events);
            // The native graph rebuilds after update, before pose generation.
            // Retired sources must not be sampled or modify the final output.
            let mut completed = Vec::new();
            for node in &prepared.plan.nodes {
                if let Some(stack) = prepared.transition_stacks.get(&node.object) {
                    let handles: Vec<_> = stack.records.iter().map(|r| r.effect).collect();
                    for index in (0..handles.len()).rev() {
                        let handle = handles[index];
                        let mut retire = prepared.transitions.finished(handle)?;
                        if !retire && index + 1 < handles.len() {
                            // DOL 0x8021ac6c enumerates the next effect's direct
                            // children and retires this prefix when that source
                            // effect no longer appears, even before its clock ends.
                            let children = prepared.transitions.children(
                                handles[index + 1],
                                &prepared.state.transition_effects,
                                2,
                            )?;
                            retire = !children.nodes.iter().any(|child| {
                                child.binding
                                    == super::transition_instances::Binding::Effect(handle)
                            });
                        }
                        if retire {
                            completed.push((node.object.clone(), index));
                            break;
                        }
                    }
                }
            }
            for (id, index) in &completed {
                prepared.retire_transitions(id, *index, |_, notice| {
                    queue_notice(&mut self.pending_events, &mut self.pending_emitted, notice)
                })?;
            }
            // Native machine update802182d8..80218364 advances time, retires
            // effects, then arbitrates with event=-1. Ordinary graph delivery
            // deliberately filters that sentinel, so it cannot supply this pass.
            let machines = prepared
                .plan
                .nodes
                .iter()
                .filter(|node| {
                    node.effect.is_none() && prepared.state.machines.contains_key(&node.object)
                })
                .map(|node| node.object.clone())
                .collect::<Vec<_>>();
            let mut automatic_changed = false;
            for id in machines {
                automatic_changed |= prepared.service_machine_echo(&id, |_, notice| {
                    queue_notice(&mut self.pending_events, &mut self.pending_emitted, notice)
                })?;
                let nested = active_nested_state(graph, prepared, &id);
                automatic_changed |=
                    prepared.dispatch_machine_event(&id, -1, nested, |_, notice| {
                        queue_notice(&mut self.pending_events, &mut self.pending_emitted, notice)
                    })?;
            }
            if !completed.is_empty()
                || blender_changed
                || automatic_changed
                || prepared.transitions.topology_changed
            {
                let removed = super::generator_plan::rebuild_in_place(graph, prepared)?;
                prepared.deactivate_removed(graph, &removed, |_, notice| {
                    queue_notice(&mut self.pending_events, &mut self.pending_emitted, notice)
                })?;
                prepared.activate_pending(graph, resources)?;
                prepared.transitions.topology_changed = false;
            }
            let mut tracks = std::collections::BTreeMap::new();
            for node in &prepared.plan.nodes {
                if let Some(definition) = prepared.state.clips.get(&node.object) {
                    let resource = resources.clip_binding(&definition.settings.animation_name,definition.settings.animation_binding_index)?;
                    tracks.insert(
                        node.object.clone(),
                        prepared.clips.sample(
                            &node.object,
                            definition,
                            prepared.state.words(),
                            resource,
                            self.rig.bones.len(),
                        )?,
                    );
                }
            }
            let output = compose(graph, prepared, &self.rig, &self.property_words, tracks)?;
            self.pose = super::pose_blend::resolve(&self.rig, &output.transforms)?;
            self.attributes = output.attributes;
            self.emitted_frames
                .push(std::mem::take(&mut self.pending_emitted));
        }
        Ok(())
    }
}

/// Resolve the active child state machine's current state ID for nested-state
/// transitions (transition flag 4096).
fn active_nested_state(
    graph: &super::behavior::Document,
    prepared: &super::generator_plan::Prepared,
    machine_id: &str,
) -> Option<i32> {
    let machine = prepared.state.machines.get(machine_id)?;
    let runtime = prepared.machines.get(machine_id)?;
    let state = &machine.states[*machine.state_indices.get(&runtime.current)?];
    let mut current = state.info.generator.id.as_str();
    for _ in 0..8 {
        if let Some(child_runtime) = prepared.machines.get(current) {
            return Some(child_runtime.current);
        }
        let obj = graph.objects.get(current)?;
        match obj.klass.as_str() {
            "hkbModifierGenerator" => {
                current = obj.fields["generator"]["ref"].as_str()?;
            }
            _ => return None,
        }
    }
    None
}

fn queue_notice(
    events: &mut std::collections::VecDeque<super::state_machine::Event>,
    emitted: &mut Vec<super::state_machine::Event>,
    notice: super::machine_runtime::Notice<'_>,
) -> Result<()> {
    match notice {
        super::machine_runtime::Notice::Event(event) => {
            emitted.push(event.clone());
            events.push_back(event.clone());
        }
        super::machine_runtime::Notice::SyncVariable { .. }
        | super::machine_runtime::Notice::Listener { .. } => {}
    }
    Ok(())
}

fn compose(
    graph: &super::behavior::Document,
    prepared: &mut super::generator_plan::Prepared,
    rig: &HkxSkeleton,
    properties: &[u32],
    mut tracks: std::collections::BTreeMap<
        String,
        Vec<Option<epic_mickey_lib_rs::ffi_anim::FfiTransform>>,
    >,
) -> Result<super::generator_output::Output> {
    // Output retries are stateful. Keep one owned instance per graph node and
    // restore it even when another modifier fails later in the same output.
    let mut attachments = std::mem::take(&mut prepared.attachment_modifiers);
    let result = super::generator_output::compose(
        graph,
        prepared,
        0.,
        |id| {
            tracks
                .remove(id)
                .with_context(|| format!("Missing updated clip output {id}"))
        },
        |id, output| {
            modify_output(
                graph,
                prepared,
                &mut attachments,
                rig,
                properties,
                id,
                output,
            )
        },
    );
    prepared.attachment_modifiers = attachments;
    result
}

fn modify_output(
    graph: &super::behavior::Document,
    prepared: &super::generator_plan::Prepared,
    attachments: &mut super::attachment_modifier::Instances,
    rig: &HkxSkeleton,
    properties: &[u32],
    id: &str,
    output: &mut super::generator_output::Output,
) -> Result<()> {
    let object = graph.objects.get(id).context("Missing preview modifier")?;
    match object.klass.as_str() {
        "hkbEventDrivenModifier" => {
            if object.fields["enable"].as_bool() == Some(true) && prepared.event_modifiers.contains(id) {
                let child = object.fields["modifier"]["ref"].as_str().context("Missing event-driven modifier child")?;
                modify_output(graph, prepared, attachments, rig, properties, child, output)?;
            }
            Ok(())
        }
        "hkbModifierList" => {
            ensure!(
                object.fields["variableBindingSet"].is_null(),
                "Modifier-list output bindings are not implemented"
            );
            if !object.fields["enable"]
                .as_bool()
                .context("Missing modifier-list enable")?
            {
                return Ok(());
            }
            // DOL8020be64 calls each child modifier in serialized order.
            for child in object.fields["modifiers"]
                .as_array()
                .context("Missing modifier list")?
            {
                let id = child["ref"]
                    .as_str()
                    .context("Invalid modifier-list child")?;
                modify_output(graph, prepared, attachments, rig, properties, id, output)?;
            }
            Ok(())
        }
        // Expression side effects run in update, never a second time when
        // composing output. Their event delivery remains outside preview.
        "hkbEvaluateExpressionModifier" => Ok(()),
        "hkbTwistModifier" => super::twist_modifier::modify(
            graph,
            &prepared.state,
            id,
            Some(properties),
            rig,
            &mut output.transforms,
        ),
        "hkbAttachmentModifier" => attachments.modify(graph, id),
        "hkbAttributeModifier" => {
            let modifier = prepared
                .state
                .attribute_modifiers
                .get(id)
                .context("Missing attribute modifier")?;
            if modifier.enabled {
                modifier.modify(Some(&mut output.attributes.values), None)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
