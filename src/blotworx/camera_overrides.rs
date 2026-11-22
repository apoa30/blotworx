//! Normalized PC camera-override ownership and verified AutoLevel leaves.
//! PC f16ec0/f16e10/f15bc0: unique handles, stable priority ordering and
//! stale-owner pruning. This does not replace the camera generator with orbit math.
use anyhow::{ensure, Context, Result};
use epic_mickey_lib_rs::scene_file::Component;
use std::collections::BTreeMap;
#[derive(Default, Debug, serde::Serialize)]
pub struct Stack {
    pub owners: Vec<usize>,
    pub selected: Option<usize>,
}
impl Stack {
    /// Existing handles do not get promoted or reapplied (f16ed7..f16ef3).
    pub fn push(
        &mut self,
        owner: usize,
        priority: impl FnMut(usize) -> Result<Option<i32>>,
    ) -> Result<bool> {
        if self.owners.contains(&owner) {
            return Ok(false);
        }
        self.owners.insert(0, owner);
        self.refresh(priority)
    }
    pub fn refresh(
        &mut self,
        mut priority: impl FnMut(usize) -> Result<Option<i32>>,
    ) -> Result<bool> {
        let mut live = Vec::new();
        for &owner in &self.owners {
            if let Some(priority) = priority(owner)? {
                live.push((owner, priority));
            }
        }
        // Native insertion sort moves only a strictly lower priority ahead of
        // earlier entries. Equal values retain the current (newest-first) order.
        live.sort_by_key(|(_, priority)| *priority);
        self.owners = live.into_iter().map(|(owner, _)| owner).collect();
        let selected = self.owners.first().copied();
        let changed = self.selected != selected;
        self.selected = selected;
        Ok(changed)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct AutoLevel {
    pub active: bool,
    pub spline: Option<usize>,
}
#[derive(Debug, serde::Serialize)]
pub struct Controller {
    pub data: super::em2_camera_data::Bank,
    pub overrides: Stack,
    pub auto_level: Option<AutoLevel>,
    pub applied_source: Option<usize>,
    pub apply_count: u64,
    pub clear_plan: Option<super::em2_camera_reset::Plan>,
    pub clear_count: u64,
    pub ability_restore: super::em2_ability_disable::CameraRestore,
    pub hints: super::em2_camera_hints::Hints,
    pub focus: [super::em2_camera_focus::Focus; 3],
    pub yaw: super::em2_camera_yaw::State,
    pub pass: super::em2_camera_pass::State,
    configured_auto_level: bool,
    configured_look_ahead: bool,
    configured_ledge_check: bool,
}
fn value<'a>(component: &'a Component, name: &str) -> Option<&'a serde_json::Value> {
    component
        .properties
        .iter()
        .find(|p| p.name == name)
        .map(|p| &p.value)
}
pub fn priority(component: &Component) -> Result<i32> {
    value(component, "OverridePriority")
        .map(|v| {
            v.as_i64()
                .and_then(|v| i32::try_from(v).ok())
                .context("Invalid camera OverridePriority")
        })
        .transpose()
        .map(|v| v.unwrap_or(i32::MAX))
}
impl Controller {
    pub fn new(settings: &BTreeMap<String, String>) -> Result<Self> {
        // PC ef546e uses key 54fcacfe, matching length-seeded lowercase CRC
        // of FreeCamera_AutoLevel_Active. Its registered default is "true".
        let default = settings
            .get("FreeCamera_AutoLevel_Active")
            .map(String::as_str)
            .unwrap_or("true");
        let active = super::config_value::boolean(Some(default)).map_err(anyhow::Error::msg)?;
        let configured = |name:&str| super::config_value::boolean(Some(settings.get(name).map(String::as_str).unwrap_or("true"))).map_err(anyhow::Error::msg);
        Ok(Self {
            data: Default::default(),
            overrides: Stack::default(),
            auto_level: None,
            applied_source: None,
            apply_count: 0,
            clear_plan: None,
            clear_count: 0,
            ability_restore: Default::default(),
            hints: Default::default(),
            focus: Default::default(),
            yaw: super::em2_camera_yaw::State::new(settings)?,
            pass: Default::default(),
            configured_auto_level: active,
            configured_look_ahead: configured("FreeCamera_ScreenLookAhead_Active")?,
            configured_ledge_check: configured("FreeCamera_LedgeCheck_Active")?,
        })
    }
    pub fn apply(
        &mut self,
        owner: usize,
        component: &Component,
        mut spline: impl FnMut(u32) -> Result<Option<usize>>,
    ) -> Result<()> {
        ensure!(self.clear_plan.is_none(), "Camera reset receivers are still pending");
        ensure!(
            component.class_name == "CameraOverrideComponent",
            "Not a camera override component"
        );
        // Other override properties (pivot, side-scroll, distances...) need camera modes the
        // playtest camera doesn't have yet: apply what's supported and say once what isn't.
        static SEEN: std::sync::Mutex<Option<std::collections::HashSet<String>>> = std::sync::Mutex::new(None);
        for property in &component.properties {
            if !matches!(property.name.as_str(), "OverridePriority" | "AutoLevelActive" | "AutoLevelSpline")
                && SEEN.lock().map(|mut s| s.get_or_insert_with(Default::default).insert(property.name.clone())).unwrap_or(false)
            {
                log::warn!("EM2 world service not implemented (ignored): CameraOverride.{}", property.name);
            }
        }
        let activation = value(component, "AutoLevelActive")
            .map(|v| {
                v.as_i64()
                    .and_then(|v| i32::try_from(v).ok())
                    .context("Invalid AutoLevelActive")
            })
            .transpose()?
            .unwrap_or(3);
        // PC ef5412..ef5493: 0=config default; 1=off; 2=on plus spline;
        // 3 and other native integer values leave the existing leaf untouched.
        let next = match activation {
            0 => Some(AutoLevel {
                active: self.configured_auto_level,
                spline: None,
            }),
            1 => Some(AutoLevel {
                active: false,
                spline: None,
            }),
            2 => {
                let id = value(component, "AutoLevelSpline")
                    .map(|v| {
                        v.as_u64()
                            .and_then(|v| u32::try_from(v).ok())
                            .context("Invalid AutoLevelSpline pointer")
                    })
                    .transpose()?
                    .unwrap_or(0);
                Some(AutoLevel {
                    active: true,
                    spline: if id == 0 { None } else { spline(id)? },
                })
            }
            _ => None,
        };
        if let Some(next) = next {
            self.auto_level = Some(next);
        }
        self.applied_source = Some(owner);
        self.apply_count += 1;
        Ok(())
    }

    /// Native list ownership changes precede the default-pass reset calls.
    /// A renderer or world owner must consume every operation; an empty stack
    /// alone is never treated as a completed ClearAllCameraAttributes request.
    pub fn begin_clear(&mut self, mut input: super::em2_camera_reset::Input) -> Result<()> {
        ensure!(self.clear_plan.is_none(), "Camera reset receivers are still pending");
        input.auto_level = self.configured_auto_level;
        input.look_ahead = self.configured_look_ahead;
        input.ledge_check = self.configured_ledge_check;
        let plan = super::em2_camera_reset::Plan::new(input)?;
        self.overrides.owners.clear();
        self.overrides.selected = None;
        self.applied_source = None;
        self.clear_plan = Some(plan);
        Ok(())
    }

    pub fn drain_clear(&mut self, mut receive: impl FnMut(super::em2_camera_reset::Operation) -> Result<()>, mut restore: impl FnMut(&mut super::em2_ability_disable::CameraRestore) -> Result<()>, world: &mut impl super::em2_camera_yaw::World, hint_defaults:Option<[f32;5]>, precision:Option<super::pc_float::Precision>) -> Result<()> {
        let Some(plan) = self.clear_plan.as_mut() else { return Ok(()) };
        let auto_level = &mut self.auto_level;
        let ability_restore = &mut self.ability_restore;
        let hints = &mut self.hints;
        let focus = &mut self.focus;
        let pass = &mut self.pass;
        let mut receivers = super::em2_camera_yaw::Receivers { yaw: &mut self.yaw, world, precision };
        plan.drain(|operation| {
            match operation {
            super::em2_camera_reset::Operation::Hint=>hints.set([f32::MAX;5],&mut receivers)?,
            super::em2_camera_reset::Operation::HintForce=>hints.force([f32::MAX;5],hint_defaults.context("Native forced-hint defaults are not initialized")?,&mut receivers)?,
            super::em2_camera_reset::Operation::HintRange=>hints.ranges([0.;3])?,
            super::em2_camera_reset::Operation::HintSoft=>hints.soft([0.;10],&mut receivers)?,
            super::em2_camera_reset::Operation::FocusTarget=>focus[0].clear(&mut receivers)?,
            super::em2_camera_reset::Operation::FocusSpeed=>focus[0].set_speed(f32::MAX)?,
            super::em2_camera_reset::Operation::SoftPaintingIgnore(value)=>pass.soft_painting_ignore=i32::from(value),
            super::em2_camera_reset::Operation::SoftIgnoresWrapping(value)=>pass.soft_ignores_wrapping=value,
            super::em2_camera_reset::Operation::LookAhead(active)=>pass.look_ahead(active,[f32::MAX;2])?,
            super::em2_camera_reset::Operation::LedgeValues=>pass.ledge([f32::MAX;2])?,
            super::em2_camera_reset::Operation::DeactivateMode(mask)=>pass.deactivate_reset_mode(mask)?,
            super::em2_camera_reset::Operation::RestoreAbilities=>{
                // Resolve the current live component in the world receiver,
                // rather than assuming the earlier reset snapshot is still live.
                restore(ability_restore)?;
            },
            _=>receive(operation)?,
            }
            if let super::em2_camera_reset::Operation::AutoLevel(active) = operation {
                *auto_level = Some(AutoLevel { active, spline: None });
            }
            Ok(())
        })?;
        self.clear_plan = None;
        self.clear_count += 1;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clear_dispatch_owns_leaves_and_retains_unavailable_world_receiver() -> Result<()> {
        struct World;
        impl super::super::em2_camera_yaw::World for World {
            fn check_snap(&mut self,_:usize,_:f32)->Result<()>{Ok(())}
            fn snap(&mut self,_:usize,_:f32)->Result<()>{panic!("Nonzero-duration clear does not force hints")}
            fn target_live(&mut self,_:u32)->Result<bool>{Ok(true)}
            fn yaw_constrained(&mut self)->Result<bool>{Ok(false)}
        }
        use super::super::em2_camera_reset::{Input, Operation};
        let mut c=Controller::new(&BTreeMap::new())?;
        c.hints.hints[1].value=1.;c.focus[0].target=99;
        c.pass.soft_painting_ignore=1;c.pass.modes=0x33;
        c.begin_clear(Input {duration:-1.,player:false,tracker:false,hud2d:false,faded:false,desired_cursor:false,
            look_ahead:true,ledge_check:false,auto_level:false,default_fov:60.})?;
        let mut calls=Vec::new();
        assert!(c.drain_clear(|op| {
            calls.push(op);
            ensure!(!matches!(op,Operation::ReturnToGame(_)),"Default pass transition is not connected");Ok(())
        },|restore|{assert_eq!(restore.mask,0);Ok(())},&mut World,None,Some(super::super::pc_float::Precision::Double)).is_err());
        assert_eq!(c.hints.hints[1].value,f32::MAX);assert_eq!(c.focus[0].target,0);
        assert_eq!(c.yaw.spring.stiffness,0.005);assert_eq!(c.pass.soft_painting_ignore,0);
        assert!(c.pass.look_ahead_active);assert_eq!(c.pass.modes,0x12);
        assert_eq!(c.clear_count,0);
        assert_eq!(c.clear_plan.as_ref().unwrap().next(),Some(Operation::ReturnToGame(0.)));
        let mut resumed=Vec::new();
        c.drain_clear(|op|{resumed.push(op);Ok(())},|_|panic!("Restore must not repeat"),&mut World,None,None)?;
        assert_eq!(resumed.first(),Some(&Operation::ReturnToGame(0.)));assert_eq!(c.clear_count,1);
        Ok(())
    }
    fn source(activation: i32) -> Component {
        use epic_mickey_lib_rs::scene_file::{Property, ID};
        Component::new(
            "CameraOverrideComponent".into(),
            "camera".into(),
            ID::new(0),
            ID::new(1),
            ID::new(0),
            vec![Property::new(
                "int".into(),
                "AutoLevelActive".into(),
                false,
                false,
                false,
                activation.into(),
            )],
        )
    }
    #[test]
    fn source_activation_enum_config_spline_and_keep_rules() -> Result<()> {
        let mut controller = Controller::new(&BTreeMap::from([(
            "FreeCamera_AutoLevel_Active".into(),
            "false".into(),
        )]))?;
        let mut component = source(0);
        controller.apply(1, &component, |_| {
            panic!("Default activation must clear the spline")
        })?;
        assert_eq!(
            controller.auto_level,
            Some(AutoLevel {
                active: false,
                spline: None
            })
        );
        component.properties[0].value = 2.into();
        component
            .properties
            .push(epic_mickey_lib_rs::scene_file::Property::new(
                "entity".into(),
                "AutoLevelSpline".into(),
                false,
                false,
                false,
                77.into(),
            ));
        controller.apply(2, &component, |id| {
            assert_eq!(id, 77);
            Ok(Some(55))
        })?;
        assert_eq!(
            controller.auto_level,
            Some(AutoLevel {
                active: true,
                spline: Some(55)
            })
        );
        for mode in [3, 4, -1] {
            component.properties[0].value = mode.into();
            controller.apply(3, &component, |_| {
                panic!("Unchanged leaf must not resolve the spline")
            })?;
            assert_eq!(
                controller.auto_level,
                Some(AutoLevel {
                    active: true,
                    spline: Some(55)
                })
            );
        }
        component.properties[0].value = 1.into();
        controller.apply(4, &component, |_| panic!())?;
        assert_eq!(
            controller.auto_level,
            Some(AutoLevel {
                active: false,
                spline: None
            })
        );
        let mut registered_default = Controller::new(&BTreeMap::new())?;
        registered_default.apply(0, &source(0), |_| panic!())?;
        assert_eq!(
            registered_default.auto_level,
            Some(AutoLevel {
                active: true,
                spline: None
            })
        );
        Ok(())
    }
    #[test]
    fn unsupported_explicit_leaves_are_not_silently_skipped() -> Result<()> {
        let mut controller = Controller::new(&BTreeMap::new())?;
        let mut component = source(0);
        component
            .properties
            .push(epic_mickey_lib_rs::scene_file::Property::new(
                "int".into(),
                "HintYawActive".into(),
                false,
                false,
                false,
                2.into(),
            ));
        assert!(controller.apply(0, &component, |_| panic!()).is_err());
        assert_eq!(controller.auto_level, None);
        assert_eq!(controller.apply_count, 0);
        Ok(())
    }
    #[test]
    fn source_priority_ties_duplicates_and_stale_owners() -> Result<()> {
        let mut stack = Stack::default();
        let mut values = BTreeMap::from([(1, 5), (2, 2), (3, 2)]);
        assert!(stack.push(1, |id| Ok(values.get(&id).copied()))?);
        assert!(stack.push(2, |id| Ok(values.get(&id).copied()))?);
        assert!(stack.push(3, |id| Ok(values.get(&id).copied()))?);
        assert_eq!(stack.owners, [3, 2, 1]);
        assert!(!stack.push(2, |_| panic!(
            "Duplicate native handle must not refresh the stack"
        ))?);
        values.remove(&3);
        assert!(stack.refresh(|id| Ok(values.get(&id).copied()))?);
        assert_eq!(stack.owners, [2, 1]);
        assert_eq!(stack.selected, Some(2));
        values.insert(1, 0);
        assert!(stack.refresh(|id| Ok(values.get(&id).copied()))?);
        assert_eq!(stack.owners, [1, 2]);
        Ok(())
    }
}
