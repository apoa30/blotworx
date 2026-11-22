//! Deterministic prototype character motion against imported static Havok triangles.
//! This is deliberately not a Havok behavioral replacement.
use std::{ffi::c_void, ptr::NonNull, sync::Mutex};
static BOX3D: Mutex<()> = Mutex::new(());
// Box3D material callbacks have no context argument. All collision operations
// are serialized by BOX3D; retain invalid-combination admission until the next
// checked simulation boundary instead of silently using a fallback material.
static MATERIAL_MIX_FAILED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
unsafe extern "C" fn mixed_friction(a: f32, _a: u64, b: f32, _b: u64) -> f32 {
    mixed_material(a, b, super::body_material::Property::Friction)
}
unsafe extern "C" fn mixed_restitution(a: f32, _a: u64, b: f32, _b: u64) -> f32 {
    mixed_material(a, b, super::body_material::Property::Restitution)
}
fn mixed_material(a: f32, b: f32, kind: super::body_material::Property) -> f32 {
    match super::body_material::combine(a, b, kind) {
        Ok(value) => value,
        Err(_) => {
            MATERIAL_MIX_FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
            0.
        }
    }
}
unsafe extern "C" {
    fn b3GetByteCount() -> i64;
    fn bw_collision_material_policy(
        handle: *mut c_void,
        friction: unsafe extern "C" fn(f32, u64, f32, u64) -> f32,
        restitution: unsafe extern "C" fn(f32, u64, f32, u64) -> f32,
    ) -> i32;
    fn bw_collision_refresh_filter(
        handle: *mut c_void,
        shape: i32,
        filter: unsafe extern "C" fn(i32, i32, *mut c_void) -> i32,
        context: *mut c_void,
        removed: *mut i32,
    ) -> i32;
    #[cfg(test)]
    fn bw_collision_pair_ids(
        handle: *mut c_void,
        shape: i32,
        output: *mut i32,
        capacity: i32,
    ) -> i32;
    fn bw_collision_body_velocities(
        handle: *mut c_void,
        shape: i32,
        linear: *const f32,
        angular: *const f32,
    ) -> i32;
    fn bw_collision_velocity_policy(
        handle: *mut c_void,
        policy: Option<
            unsafe extern "C" fn(f32, *const f32, *mut f32, *mut f32, *mut c_void) -> i32,
        >,
        context: *mut c_void,
    ) -> i32;
    fn bw_collision_contact_events(handle: *mut c_void, shape: i32) -> i32;
    fn bw_collision_touching_contacts(handle:*mut c_void,shape:i32,
        sink:unsafe extern "C" fn(i32,i32,*mut c_void)->i32,context:*mut c_void)->i32;
    fn bw_collision_begin_contacts(
        handle: *mut c_void,
        sink: unsafe extern "C" fn(i32, i32, *mut c_void) -> i32,
        context: *mut c_void,
    ) -> i32;
    fn bw_collision_release_shape(handle: *mut c_void, index: i32) -> i32;
    fn bw_collision_shape_is_live(handle: *mut c_void, index: i32) -> i32;
    #[cfg(test)]
    fn bw_collision_test_rounded_cache()->i32;
    #[cfg(test)]
    fn bw_collision_shape_payload_released(handle: *mut c_void, index: i32) -> i32;
    fn bw_collision_body_enabled(handle: *mut c_void, index: i32) -> i32;
    fn bw_collision_set_body_enabled(handle: *mut c_void, index: i32, enabled: i32) -> i32;
    fn bw_collision_character_mass_policy(
        handle: *mut c_void,
        shape: i32,
        policy: Option<
            unsafe extern "C" fn(i32, i32, *const f32, f32, *mut f32, *mut c_void) -> i32,
        >,
        context: *mut c_void,
    ) -> i32;
    fn bw_collision_linear_impulse(handle: *mut c_void, index: i32, impulse: *const f32) -> i32;
    fn bw_collision_body_velocity(handle: *mut c_void, index: i32, velocity: *const f32) -> i32;
    fn bw_collision_query_ignore(handle: *mut c_void, index: i32) -> i32;
    #[cfg(test)]
    fn bw_collision_test_mass_step(
        handle: *mut c_void,
        dt: f32,
        gravity: *const f32,
        filter: unsafe extern "C" fn(i32, i32, *mut c_void) -> i32,
        context: *mut c_void,
        shape: i32,
        scale: f32,
        output: *mut f32,
    ) -> i32;
    #[cfg(test)]
    fn bw_collision_skin_pair(
        a: *const f32,
        ac: i32,
        b: *const f32,
        bc: i32,
        position: *const f32,
        ra: f32,
        rb: f32,
        output: *mut f32,
    ) -> i32;
    fn bw_collision_character_contact_policy(
        handle: *mut c_void,
        callback: Option<
            unsafe extern "C" fn(
                i32,
                i32,
                *const f32,
                *const f32,
                f32,
                *mut f32,
                *mut c_void,
            ) -> i32,
        >,
        context: *mut c_void,
    );
    fn bw_collision_body_parameters(
        handle: *mut c_void,
        index: i32,
        mode: i32,
        values: *const f32,
        awake: i32,
    ) -> i32;
    fn bw_collision_body_snapshot(handle: *mut c_void, index: i32, output: *mut f32) -> i32;
    fn bw_collision_wake_body(handle: *mut c_void, index: i32) -> i32;
    fn bw_collision_body_supported(handle: *mut c_void, index: i32, slope_cos: f32) -> i32;
    fn bw_collision_step(
        handle: *mut c_void,
        dt: f32,
        gravity: *const f32,
        filter: unsafe extern "C" fn(i32, i32, *mut c_void) -> i32,
        context: *mut c_void,
    ) -> i32;
    fn bw_collision_native_mass(
        handle: *mut c_void,
        index: i32,
        inverse_mass: f32,
        inverse_inertia: *const f32,
        center: *const f32,
    ) -> i32;
    #[cfg(test)]
    fn bw_collision_native_mass_values(handle: *mut c_void, index: i32, output: *mut f32) -> i32;
    #[cfg(test)]
    fn bw_collision_angular_impulse(
        handle: *mut c_void,
        index: i32,
        impulse: *const f32,
        output: *mut f32,
    ) -> i32;
    fn bw_collision_group_shapes(handle: *mut c_void, indices: *const i32, count: i32) -> i32;
    #[cfg(test)]
    fn bw_collision_same_body(handle: *mut c_void, a: i32, b: i32) -> i32;
    fn bw_collision_sphere_filtered(
        handle: *mut c_void,
        origin: *const f32,
        translation: *const f32,
        radius: f32,
        sink: unsafe extern "C" fn(*const RawRayHit, *mut c_void) -> i32,
        filter: unsafe extern "C" fn(i32, i32, *mut c_void) -> i32,
        context: *mut c_void,
    ) -> i32;
    fn bw_collision_sphere_all(
        handle: *mut c_void,
        origin: *const f32,
        translation: *const f32,
        radius: f32,
        sink: unsafe extern "C" fn(*const RawRayHit, *mut c_void) -> i32,
        context: *mut c_void,
    ) -> i32;
    fn bw_collision_overlap_candidates(
        handle: *mut c_void,
        lower: *const f32,
        upper: *const f32,
        sink: unsafe extern "C" fn(i32, *mut c_void) -> i32,
        context: *mut c_void,
    ) -> i32;
    fn bw_collision_overlap_body_bounds(
        handle: *mut c_void,
        lower: *const f32,
        upper: *const f32,
        sink: unsafe extern "C" fn(i32, *mut c_void) -> i32,
        context: *mut c_void,
    ) -> i32;
    fn bw_collision_overlap_sphere(
        handle: *mut c_void,
        center: *const f32,
        radius: f32,
        sink: unsafe extern "C" fn(i32, *mut c_void) -> i32,
        context: *mut c_void,
    ) -> i32;
    fn bw_collision_overlap_sphere_all(
        handle: *mut c_void,
        center: *const f32,
        radius: f32,
        sink: unsafe extern "C" fn(i32, i32, *mut c_void) -> i32,
        context: *mut c_void,
    ) -> i32;
    fn bw_collision_set_transform(
        handle: *mut c_void,
        index: i32,
        position: *const f32,
        rotation: *const f32,
    ) -> i32;
    fn bw_collision_shape_min_y(handle: *mut c_void, index: i32) -> f32;
    fn bw_collision_capsule_half_extents(handle: *mut c_void, shape: i32, half: *mut f32) -> i32;
    fn bw_collision_box_contact_height(
        handle: *mut c_void, shape: i32, center: *const f32, half: *const f32,
        radius: f32, height: *mut f32,
    ) -> i32;
    #[cfg(test)]
    fn bw_collision_hull_edges(handle: *mut c_void, index: i32) -> i32;
    fn bw_collision_set_solid(handle: *mut c_void, index: i32, solid: i32) -> i32;
    fn bw_collision_ray_all(
        handle: *mut c_void,
        origin: *const f32,
        translation: *const f32,
        sink: unsafe extern "C" fn(*const RawRayHit, *mut c_void) -> i32,
        context: *mut c_void,
    ) -> i32;
    fn bw_collision_sweep_sphere(
        handle: *mut c_void,
        origin: *const f32,
        translation: *const f32,
        radius: f32,
        output: *mut f32,
        source_triangle: *mut i32,
    ) -> i32;
    fn bw_collision_ray_hit(
        handle: *mut c_void,
        origin: *const f32,
        direction: *const f32,
        distance: f32,
        output: *mut f32,
    ) -> i32;
    fn bw_collision_contact_mesh(handle: *mut c_void, index: i32) -> i32;
    fn bw_collision_contact_source_triangle(handle: *mut c_void, index: i32) -> i32;
    fn bw_collision_contact_simplex(handle: *mut c_void, index: i32, output: *mut u8) -> i32;
    fn bw_collision_contact_count(handle: *mut c_void) -> i32;
    fn bw_collision_contact(handle: *mut c_void, index: i32, output: *mut f32) -> i32;
    fn bw_collision_contact_point(handle: *mut c_void, index: i32, output: *mut f32) -> i32;
    fn bw_collision_contact_active(handle: *mut c_void, index: i32) -> i32;
    fn bw_collision_support(
        handle: *mut c_void,
        position: *const f32,
        requested: *const f32,
        dt: f32,
        solved: *mut f32,
    ) -> i32;
    fn bw_collision_create() -> *mut c_void;
    fn bw_collision_hull_capacity() -> i32;
    #[cfg(test)]
    fn bw_collision_hull_vertex_count(handle: *mut c_void, index: i32) -> i32;
    fn bw_collision_add_convex(
        handle: *mut c_void,
        points: *const f32,
        count: i32,
        radius: f32,
    ) -> i32;
    fn bw_collision_add_capsule(handle: *mut c_void, endpoints: *const f32, radius: f32) -> i32;
    fn bw_collision_add_mesh(
        handle: *mut c_void,
        points: *const f32,
        count: i32,
        radius: f32,
        source_ids: *const i32,
    ) -> i32;
    fn bw_collision_add_mesh_range(
        handle: *mut c_void,
        points: *const f32,
        count: i32,
        radius: f32,
        source_start: i32,
    ) -> i32;
    fn bw_collision_destroy(handle: *mut c_void);
    fn bw_collision_set_hull(
        handle: *mut c_void,
        points: *const f32,
        count: i32,
        margin: f32,
    ) -> i32;
    fn bw_collision_shape_penetration(
        handle: *mut c_void,
        shape: i32,
        position: *const f32,
        depth: *mut f32,
    ) -> i32;
    fn bw_collision_shape_penetrations(
        handle: *mut c_void,
        shape: i32,
        position: *const f32,
        sink: unsafe extern "C" fn(i32, f32, *mut c_void) -> i32,
        context: *mut c_void,
    ) -> i32;
    fn bw_collision_ray(
        handle: *mut c_void,
        origin: *const f32,
        direction: *const f32,
        distance: f32,
    ) -> f32;
    fn bw_collision_move(
        handle: *mut c_void,
        position: *mut f32,
        velocity: *mut f32,
        radius: f32,
        height: f32,
        slope: f32,
        dt: f32,
    ) -> i32;
}

pub type V = [f32; 3];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlapCandidate {
    pub shape: usize,
    pub owner: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SimulationContact {
    pub shapes: [usize; 2],
    pub owners: [usize; 2],
    pub motion_types: [u8; 2],
}
#[derive(Default)]
struct ContactPairs {
    pairs: Vec<[usize; 2]>,
    failed: bool,
}
unsafe extern "C" fn collect_contact_pair(a: i32, b: i32, context: *mut c_void) -> i32 {
    let result = unsafe { &mut *(context as *mut ContactPairs) };
    if a < 0 || b < 0 || result.failed || result.pairs.try_reserve(1).is_err() {
        result.failed = true;
        return 0;
    }
    result.pairs.push([a as usize, b as usize]);
    1
}
struct OverlapCollector<'a> {
    hits: Vec<OverlapCandidate>,
    owners: &'a [usize],
    error: Option<&'static str>,
}
unsafe extern "C" fn collect_overlap(shape: i32, context: *mut c_void) -> i32 {
    let collector = unsafe { &mut *(context as *mut OverlapCollector<'_>) };
    let Some(&owner) = usize::try_from(shape)
        .ok()
        .and_then(|i| collector.owners.get(i))
    else {
        collector.error = Some("Overlap returned an unknown collision shape");
        return 0;
    };
    if collector.hits.try_reserve(1).is_err() {
        collector.error = Some("Unable to allocate collision overlap results");
        return 0;
    }
    collector.hits.push(OverlapCandidate {
        shape: shape as usize,
        owner,
    });
    1
}
#[derive(Debug)]
pub struct Penetration {
    pub source_triangle: Option<usize>,
    pub depth: f32,
}
#[derive(Default)]
struct Penetrations {
    values: Vec<Penetration>,
    failed: bool,
}
unsafe extern "C" fn collect_penetration(source: i32, depth: f32, context: *mut c_void) -> i32 {
    let result = unsafe { &mut *(context as *mut Penetrations) };
    if result.failed || result.values.try_reserve(1).is_err() {
        result.failed = true;
        return 0;
    }
    result.values.push(Penetration {
        source_triangle: usize::try_from(source).ok(),
        depth,
    });
    1
}
#[repr(C)]
struct RawRayHit {
    mesh: i32,
    triangle: i32,
    material: i32,
    source_triangle: i32,
    fraction: f32,
    position: V,
    normal: V,
}
/// All-hit static query result. Shape/triangle indices are Box3D identities;
/// mapping back to original Havok keys and collision filters remains separate.
#[derive(Debug)]
pub struct StaticRayHit {
    pub shape: usize,
    pub triangle: Option<usize>,
    pub material: Option<usize>,
    pub owner: usize,
    /// Index in Collision::triangles before grouping/welding/BVH construction.
    pub source_triangle: Option<usize>,
    pub fraction: f32,
    pub position: V,
    pub normal: V,
}
struct RayCollector<'a> {
    hits: Vec<StaticRayHit>,
    owners: &'a [usize],
    error: Option<&'static str>,
}
#[derive(Clone, Copy, Debug)]
pub struct SphereCandidate {
    pub shape: usize,
    pub owner: usize,
    pub source_triangle: Option<usize>,
}
struct SphereOverlaps<'a> {
    values: Vec<SphereCandidate>,
    owners: &'a [usize],
    failed: bool,
}
unsafe extern "C" fn collect_sphere_overlap(shape: i32, source: i32, context: *mut c_void) -> i32 {
    let result = unsafe { &mut *(context as *mut SphereOverlaps<'_>) };
    let Some(&owner) = usize::try_from(shape)
        .ok()
        .and_then(|shape| result.owners.get(shape))
    else {
        result.failed = true;
        return 0;
    };
    if result.values.try_reserve(1).is_err() {
        result.failed = true;
        return 0;
    }
    result.values.push(SphereCandidate {
        shape: shape as usize,
        owner,
        source_triangle: usize::try_from(source).ok(),
    });
    1
}
struct SphereCollector<'a> {
    ray: RayCollector<'a>,
    policy: &'a mut dyn FnMut(SphereCandidate) -> anyhow::Result<bool>,
    error: Option<anyhow::Error>,
}
unsafe extern "C" fn filter_sphere(shape: i32, source: i32, context: *mut c_void) -> i32 {
    let collector = unsafe { &mut *(context as *mut SphereCollector<'_>) };
    let Some(&owner) = usize::try_from(shape)
        .ok()
        .and_then(|i| collector.ray.owners.get(i))
    else {
        collector.error = Some(anyhow::anyhow!("Sphere filter received an unknown shape"));
        return -1;
    };
    let candidate = SphereCandidate {
        shape: shape as usize,
        owner,
        source_triangle: usize::try_from(source).ok(),
    };
    match (collector.policy)(candidate) {
        Ok(admitted) => i32::from(admitted),
        Err(error) => {
            collector.error = Some(error);
            -1
        }
    }
}
unsafe extern "C" fn collect_sphere(hit: *const RawRayHit, context: *mut c_void) -> i32 {
    let collector = unsafe { &mut *(context as *mut SphereCollector<'_>) };
    unsafe { collect_ray(hit, &mut collector.ray as *mut _ as *mut c_void) }
}
unsafe extern "C" fn collect_ray(hit: *const RawRayHit, context: *mut c_void) -> i32 {
    let collector = unsafe { &mut *(context as *mut RayCollector<'_>) };
    let hit = unsafe { &*hit };
    let Some(&owner) = collector.owners.get(hit.mesh as usize) else {
        collector.error = Some("Ray returned an unknown collision shape");
        return 0;
    };
    if collector.hits.try_reserve(1).is_err() {
        collector.error = Some("Unable to allocate collision ray results");
        return 0;
    }
    collector.hits.push(StaticRayHit {
        shape: hit.mesh as usize,
        triangle: usize::try_from(hit.triangle).ok(),
        material: usize::try_from(hit.material).ok(),
        source_triangle: usize::try_from(hit.source_triangle).ok(),
        owner,
        fraction: hit.fraction,
        position: hit.position,
        normal: hit.normal,
    });
    1
}
#[derive(Debug)]
pub struct RayHit {
    pub distance: f32,
    pub position: V,
    pub normal: V,
    pub owner: usize,
}
#[derive(Clone, Debug)]
pub struct SweepHit {
    pub shape: usize,
    pub source_triangle: Option<usize>,
    pub fraction: f32,
    pub position: V,
    pub normal: V,
    pub owner: usize,
}
pub fn add(a: V, b: V) -> V {
    std::array::from_fn(|i| a[i] + b[i])
}
pub fn sub(a: V, b: V) -> V {
    std::array::from_fn(|i| a[i] - b[i])
}
pub fn mul(a: V, s: f32) -> V {
    a.map(|x| x * s)
}
pub fn dot(a: V, b: V) -> f32 {
    (0..3).map(|i| a[i] * b[i]).sum()
}
pub fn cross(a: V, b: V) -> V {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn rotated_center(center: V, q: [f32; 4]) -> V {
    let v = [q[0], q[1], q[2]];
    add(
        center,
        mul(
            add(mul(cross(v, center), q[3]), cross(v, cross(v, center))),
            2.,
        ),
    )
}
fn restore_kinematic_center(center: Option<&V>, values: &mut [f32; 16]) {
    let Some(&center) = center else { return };
    let offset = rotated_center(center, values[3..7].try_into().unwrap());
    let angular = values[10..13].try_into().unwrap();
    let linear = add(values[7..10].try_into().unwrap(), cross(angular, offset));
    for axis in 0..3 {
        values[13 + axis] = values[axis] + offset[axis];
        values[7 + axis] = linear[axis];
    }
}
pub fn length(a: V) -> f32 {
    dot(a, a).sqrt()
}
pub fn unit(a: V) -> V {
    mul(a, 1.0 / length(a).max(1e-8))
}
pub fn point(m: &[f32; 16], p: V) -> V {
    std::array::from_fn(|i| m[i] * p[0] + m[4 + i] * p[1] + m[8 + i] * p[2] + m[12 + i])
}
#[derive(Clone)]
#[repr(C)]
pub struct Triangle {
    pub p: [V; 3],
}
const _: [(); 9 * std::mem::size_of::<f32>()] = [(); std::mem::size_of::<Triangle>()];
/// World-space motion for one collision shape's owning rigid body. Compound
/// bodies publish the same snapshot to every part; entity ownership alone is
/// insufficient because an entity can contain several independently moving bodies.
#[derive(Clone, Copy, Debug)]
pub struct BodyMotion {
    pub center_of_mass: V,
    pub linear_velocity: V,
    pub angular_velocity: V,
    pub motion_type: u8,
}
/// ABI-neutral values after the version-specific source decoder.
struct SimulationBodyParameters {
    motion_type: u8,
    state: super::native_motion_step::MotionState,
    linear: [f32; 4],
    angular: [f32; 4],
    damping: [f32; 2],
    gravity: f32,
    limits: [f32; 2],
}
#[derive(Clone, Copy, Debug)]
pub struct SimulationPose {
    pub position: V,
    pub orientation: [f32; 4],
    pub linear: V,
    pub angular: V,
    pub center: V,
}
/// Havok's estimate-based quaternion math can exceed Box3D's unit-length
/// tolerance. Normalize only the solver/query projection; retain the original
/// quaternion in the native motion record for subsequent game calculations.
fn query_rotation(rotation: [f32; 4]) -> anyhow::Result<[f32; 4]> {
    let squared = rotation.iter().map(|v| v * v).sum::<f32>();
    anyhow::ensure!(
        rotation.iter().all(|v| v.is_finite())
            && squared.is_finite()
            && squared > 1000. * f32::MIN_POSITIVE,
        "Invalid collision rotation {rotation:?}"
    );
    let tolerance = 20. * f32::EPSILON;
    Ok(if squared > 1. - tolerance && squared < 1. + tolerance {
        rotation
    } else {
        let inverse = 1. / squared.sqrt();
        rotation.map(|v| v * inverse)
    })
}
#[derive(Default)]
struct VelocityLimitContext {
    error: Option<String>,
}
unsafe extern "C" fn limit_solver_velocity(
    dt: f32,
    limits: *const f32,
    linear: *mut f32,
    angular: *mut f32,
    context: *mut c_void,
) -> i32 {
    let context = unsafe { &mut *(context as *mut VelocityLimitContext) };
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> anyhow::Result<()> {
            let limits: [f32; 2] = unsafe { std::slice::from_raw_parts(limits, 2) }.try_into()?;
            let lv = unsafe { std::slice::from_raw_parts(linear, 3) };
            let av = unsafe { std::slice::from_raw_parts(angular, 3) };
            let (lv, av, _, _) = super::native_motion_step::limit_velocity(
                [lv[0], lv[1], lv[2], 0.],
                [av[0], av[1], av[2], 0.],
                dt,
                limits,
            )?;
            unsafe { std::slice::from_raw_parts_mut(linear, 3) }.copy_from_slice(&lv[..3]);
            unsafe { std::slice::from_raw_parts_mut(angular, 3) }.copy_from_slice(&av[..3]);
            Ok(())
        }));
    match result {
        Ok(Ok(())) => 1,
        Ok(Err(error)) => {
            context.error = Some(format!("{error:#}"));
            -1
        }
        Err(_) => {
            context.error = Some("Velocity limit callback panicked".into());
            -1
        }
    }
}
struct CharacterMassContext<'a> {
    motions: &'a std::collections::HashMap<usize, BodyMotion>,
    velocity: V,
    limit: f32,
    error: Option<String>,
    count: u64,
}
unsafe extern "C" fn character_mass(
    first: i32,
    other: i32,
    normal: *const f32,
    inverse_mass: f32,
    factor: *mut f32,
    context: *mut c_void,
) -> i32 {
    let context = unsafe { &mut *(context as *mut CharacterMassContext<'_>) };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
        || -> anyhow::Result<bool> {
            let other = usize::try_from(other)?;
            let mode = context
                .motions
                .get(&other)
                .map_or(5, |motion| motion.motion_type);
            let normal: [f32; 4] = unsafe { std::slice::from_raw_parts(normal, 4) }.try_into()?;
            anyhow::ensure!(context.limit>0.,"Character response needs its owned native fallback factors for a nonpositive limit");
            let velocity = [
                context.velocity[0],
                context.velocity[1],
                context.velocity[2],
                0.,
            ];
            let Some(factors) = super::character_contact_response::mass_factors(
                mode,
                first != 0,
                &[normal],
                velocity,
                inverse_mass,
                context.limit,
                [1.; 4],
            )?
            else {
                return Ok(false);
            };
            unsafe { *factor = factors[3] };
            context.count += 1;
            Ok(true)
        },
    ));
    match result {
        Ok(Ok(changed)) => i32::from(changed),
        Ok(Err(error)) => {
            context.error = Some(format!("{error:#}"));
            -1
        }
        Err(_) => {
            context.error = Some("Character mass callback panicked".into());
            -1
        }
    }
}
struct SimulationFilter<'a> {
    filter: &'a super::physics_filter::GameFilter,
    words: &'a std::collections::HashMap<usize, u32>,
    invalid: bool,
}
unsafe extern "C" fn filter_simulation(a: i32, b: i32, context: *mut c_void) -> i32 {
    let context = unsafe { &mut *(context as *mut SimulationFilter<'_>) };
    let words = usize::try_from(a)
        .ok()
        .and_then(|a| context.words.get(&a))
        .zip(usize::try_from(b).ok().and_then(|b| context.words.get(&b)));
    let Some((&a, &b)) = words else {
        context.invalid = true;
        return 0;
    };
    i32::from(context.filter.collision_info(a, b) >> 24 != 0)
}
pub struct Collision {
    character_response: Option<(V, f32)>,
    pub character_mass_modifications: u64,
    pub(super) player_body: Option<usize>,
    query_excluded_body: std::cell::Cell<Option<usize>>,
    native_character_contacts: Option<std::cell::RefCell<super::character_contacts::Resources>>,
    character_contact_error: std::cell::RefCell<Option<String>>,
    last_support_body: std::cell::Cell<super::motion_ground::SupportBody>,
    /// Original contact thresholds; normal correction still requires the
    /// native feature/contact pipeline before applying results to Box3D.
    pub contact_welding: super::shape_welding::ContactTable,
    /// Monotonic global contact-source ID allocator. Box3D owns the actual
    /// collision triangles after admission; duplicate Rust world-space copies
    /// are retained only in test builds for geometry assertions.
    triangle_count: usize,
    #[cfg(test)]
    pub triangles: Vec<Triangle>,
    mesh_owners: Vec<usize>,
    pub planar_convex_count: usize,
    shape_min_y: Vec<f32>,
    owner_transforms: std::collections::HashMap<usize, (V, [f32; 4])>,
    shape_motion: std::collections::HashMap<usize, BodyMotion>,
    // Box3D kinematic bodies have their center at the origin; Havok retains
    // the authored local COM. Convert velocities/snapshots at this boundary.
    kinematic_centers: std::collections::HashMap<usize, V>,
    /// Fixed imported bodies retain the original hkVector4, including W.
    /// Live motion publication takes precedence over this immutable source.
    imported_fixed_linear_velocity: std::collections::HashMap<usize, [f32; 4]>,
    simulation_filters: std::collections::HashMap<usize, u32>,
    shape_groups: std::collections::HashMap<usize, std::sync::Arc<Vec<usize>>>,
    non_solid_owners: std::collections::HashSet<usize>,
    handle: NonNull<c_void>,
    pub min_y: f32,
    /// Provisional recovery boundary from environment geometry. Falling
    /// dynamic objects must not drag the player's recovery plane into the void.
    pub recovery_min_y: f32,
}

/// Live Box3D allocations tracked by Box3D's own process-wide atomic counter.
/// This is allocator payload, not allocator overhead or a process RSS estimate.
pub(super) fn box3d_allocated_bytes() -> usize {
    // SAFETY: Box3D exposes this read-only atomic counter as part of its C API.
    unsafe { b3GetByteCount().max(0) as usize }
}

// All access to Box3D, including its global world registry, is serialized.
unsafe impl Send for Collision {}
impl Drop for Collision {
    fn drop(&mut self) {
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { bw_collision_destroy(self.handle.as_ptr()) }
    }
}
impl Collision {
    /// Identity of one body, not its entity owner. Multiple door halves can
    /// share an owner while compound shapes share a single body.
    pub(super) fn query_body_identity(&self, shape: usize) -> anyhow::Result<usize> {
        anyhow::ensure!(shape < self.mesh_owners.len(), "Unknown query body shape");
        Ok(self
            .shape_groups
            .get(&shape)
            .and_then(|parts| parts.first())
            .copied()
            .unwrap_or(shape))
    }
    pub fn simulation_filter_word(&self, shape: usize) -> anyhow::Result<u32> {
        self.simulation_filters
            .get(&shape)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("Collision shape {shape} has no live body filter"))
    }
    pub fn set_character_response(&mut self, velocity: V, limit: f32) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.player_body.is_some()
                && velocity.iter().all(|v| v.is_finite())
                && limit.is_finite(),
            "Invalid character response command"
        );
        self.character_response = Some((velocity, limit));
        Ok(())
    }
    pub fn next_owner(&self) -> anyhow::Result<usize> {
        self.mesh_owners
            .iter()
            .copied()
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Collision owner namespace exhausted"))
    }
    pub fn maximum_filter_group(&self) -> u32 {
        self.simulation_filters
            .values()
            .map(|word| word >> 17)
            .max()
            .unwrap_or(0)
    }
    /// REL 37f5f0 applies a central impulse once per rigid body, excluding
    /// keyframed/fixed motion (4/5). Compound shapes share that single body.
    pub fn apply_owner_impulse(&mut self, owner: usize, impulse: V) -> anyhow::Result<()> {
        anyhow::ensure!(
            impulse.iter().all(|v| v.is_finite()),
            "Invalid combat impulse"
        );
        let shapes = self.owner_shapes(owner).collect::<Vec<_>>();
        let mut applied = std::collections::HashSet::new();
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        for shape in shapes {
            let Some(motion) = self.shape_motion.get(&shape) else {
                continue;
            };
            if matches!(motion.motion_type, 4 | 5) {
                continue;
            }
            let body = self
                .shape_groups
                .get(&shape)
                .and_then(|parts| parts.iter().min())
                .copied()
                .unwrap_or(shape);
            if applied.insert(body) {
                anyhow::ensure!(
                    unsafe {
                        bw_collision_linear_impulse(
                            self.handle.as_ptr(),
                            shape as i32,
                            impulse.as_ptr(),
                        )
                    } != 0,
                    "Combat impulse failed"
                );
            }
        }
        Ok(())
    }
    pub fn set_simulation_velocity(&mut self, shape: usize, velocity: V) -> anyhow::Result<()> {
        anyhow::ensure!(
            shape < self.mesh_owners.len() && velocity.iter().all(|v| v.is_finite()),
            "Invalid commanded body velocity"
        );
        if self.kinematic_centers.contains_key(&shape) {
            let angular = self.simulation_pose(shape)?.angular;
            let shapes = self
                .shape_groups
                .get(&shape)
                .map(|group| group.as_ref().clone())
                .unwrap_or_else(|| vec![shape]);
            return self.set_simulation_velocities(&shapes, velocity, angular);
        }
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        anyhow::ensure!(
            unsafe {
                bw_collision_body_velocity(self.handle.as_ptr(), shape as i32, velocity.as_ptr())
            } != 0,
            "Body velocity publication failed"
        );
        Ok(())
    }
    pub fn set_simulation_velocities(
        &mut self,
        shapes: &[usize],
        linear: V,
        angular: V,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            !shapes.is_empty() && linear.iter().chain(angular.iter()).all(|v| v.is_finite()),
            "Invalid body velocity pair"
        );
        anyhow::ensure!(
            shapes
                .iter()
                .all(|shape| self.shape_motion.contains_key(shape)),
            "Velocity pair has an unadmitted shape"
        );
        if let Some(group) = self.shape_groups.get(&shapes[0]) {
            anyhow::ensure!(
                group.len() == shapes.len() && group.iter().all(|s| shapes.contains(s)),
                "Partial compound velocity publication"
            );
        } else {
            anyhow::ensure!(shapes.len() == 1, "Ungrouped body velocity pair");
        }
        let solver_linear = if let Some(&center) = self.kinematic_centers.get(&shapes[0]) {
            sub(
                linear,
                cross(
                    angular,
                    rotated_center(center, self.simulation_pose(shapes[0])?.orientation),
                ),
            )
        } else {
            linear
        };
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        anyhow::ensure!(
            unsafe {
                bw_collision_body_velocities(
                    self.handle.as_ptr(),
                    i32::try_from(shapes[0])?,
                    solver_linear.as_ptr(),
                    angular.as_ptr(),
                )
            } != 0,
            "Body velocity pair publication failed"
        );
        for shape in shapes {
            let motion = self.shape_motion.get_mut(shape).unwrap();
            motion.linear_velocity = linear;
            motion.angular_velocity = angular;
        }
        Ok(())
    }
    pub(super) fn with_player_queries<R>(&self, action: impl FnOnce() -> R) -> R {
        if let Some(shape) = self.player_body {
            self.with_query_body_excluded(shape, |_| action())
                .expect("Missing attached player body")
        } else {
            action()
        }
    }
    /// Ignore one body's grouped shapes for this caller's queries only. Does
    /// not alter solver filtering, paint state or queries from other actors.
    pub fn with_query_body_excluded<R>(
        &self,
        shape: usize,
        action: impl FnOnce(&Self) -> R,
    ) -> anyhow::Result<R> {
        anyhow::ensure!(shape < self.mesh_owners.len(), "Unknown query source body");
        struct Reset<'a>(&'a std::cell::Cell<Option<usize>>, Option<usize>);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.set(self.1);
            }
        }
        let reset = Reset(
            &self.query_excluded_body,
            self.query_excluded_body.replace(Some(shape)),
        );
        let result = action(self);
        drop(reset);
        Ok(result)
    }
    // Caller holds BOX3D. Install only around an actual query, never a solver
    // step. Scopes restore on unwind and do not hold the global mutex across
    // Rust caller code (which would deadlock nested public queries).
    fn with_query_policy<R>(&self, action: impl FnOnce() -> R) -> R {
        let Some(shape) = self.query_excluded_body.get() else {
            return action();
        };
        let old = unsafe { bw_collision_query_ignore(self.handle.as_ptr(), shape as i32) };
        assert!(old >= -1, "Invalid backend query source body");
        struct Reset(*mut c_void, i32);
        impl Drop for Reset {
            fn drop(&mut self) {
                unsafe { bw_collision_query_ignore(self.0, self.1) };
            }
        }
        let reset = Reset(self.handle.as_ptr(), old);
        let result = action();
        drop(reset);
        result
    }
    pub fn install_native_static_character_contacts(
        &mut self,
        support: &super::native_gsk::ConvexSupport,
        radius: f32,
        sources: Vec<super::character_contacts::Source>,
    ) -> anyhow::Result<()> {
        self.native_character_contacts = Some(std::cell::RefCell::new(
            super::character_contacts::Resources::new(support, radius, sources)?,
        ));
        Ok(())
    }
    pub fn append_native_static_character_contacts(
        &mut self,
        sources: Vec<super::character_contacts::Source>,
    ) -> anyhow::Result<()> {
        self.native_character_contacts
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Native character contact registry absent"))?
            .borrow_mut()
            .append(sources)
    }
    pub fn take_character_contact_error(&self) -> Option<String> {
        self.character_contact_error.borrow_mut().take()
    }
    pub fn native_character_contact_counts(&self) -> Option<[u64; 3]> {
        self.native_character_contacts
            .as_ref()
            .map(|r| r.borrow().counts)
    }
    // Caller holds BOX3D. Native queries make no recursive backend calls.
    fn with_character_contacts<R>(&self, action: impl FnOnce() -> R) -> R {
        self.with_query_policy(|| self.with_native_character_contacts(action))
    }
    fn with_native_character_contacts<R>(&self, action: impl FnOnce() -> R) -> R {
        let Some(resources) = &self.native_character_contacts else {
            return action();
        };
        let mut resources = resources.borrow_mut();
        let mut context = super::character_contacts::ContextState {
            resources: &mut resources,
            owners: &self.mesh_owners,
            table: &self.contact_welding,
            error: None,
        };
        struct Reset(*mut c_void);
        impl Drop for Reset {
            fn drop(&mut self) {
                unsafe {
                    bw_collision_character_contact_policy(self.0, None, std::ptr::null_mut())
                };
            }
        }
        unsafe {
            bw_collision_character_contact_policy(
                self.handle.as_ptr(),
                Some(super::character_contacts::resolve),
                &mut context as *mut _ as *mut c_void,
            )
        };
        let reset = Reset(self.handle.as_ptr());
        let result = action();
        drop(reset);
        if let Some(error) = context.error {
            self.character_contact_error
                .borrow_mut()
                .get_or_insert(error);
        }
        result
    }
    fn refresh_height_bounds(&mut self) {
        let mut all = f32::INFINITY;
        let mut environment = f32::INFINITY;
        for (shape, &height) in self.shape_min_y.iter().enumerate() {
            all = all.min(height);
            if self
                .shape_motion
                .get(&shape)
                .is_none_or(|motion| motion.motion_type == 5)
            {
                environment = environment.min(height);
            }
        }
        self.min_y = all;
        self.recovery_min_y = environment;
    }
    /// Explicit native body-filter words for solver pairs. Per-triangle child
    /// routing remains distinct; never reinterpret these as query categories.
    pub fn set_simulation_filter(&mut self, shape: usize, word: u32) -> anyhow::Result<()> {
        anyhow::ensure!(shape < self.mesh_owners.len(), "Unknown simulation shape");
        self.simulation_filters.insert(shape, word);
        Ok(())
    }
    pub fn simulation_filter(&self, shape: usize) -> Option<u32> {
        self.simulation_filters.get(&shape).copied()
    }
    /// Immediate Box3D compatibility adapter for native body refresh(0,1).
    /// Preserve still-valid contacts and discover newly permitted pairs before
    /// the next step. Havok agent/manifold ordering is not asserted by this.
    pub fn refresh_simulation_filter(
        &mut self,
        shape: usize,
        filter: &super::physics_filter::GameFilter,
    ) -> anyhow::Result<usize> {
        anyhow::ensure!(
            shape < self.mesh_owners.len(),
            "Unknown body for filter refresh"
        );
        anyhow::ensure!(
            (0..self.mesh_owners.len()).all(|shape| {
                (unsafe { bw_collision_shape_is_live(self.handle.as_ptr(), shape as i32) }) == 0
                    || self.simulation_filters.contains_key(&shape)
            }),
            "Filter refresh requires all live body filter words"
        );
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let mut context = SimulationFilter {
            filter,
            words: &self.simulation_filters,
            invalid: false,
        };
        anyhow::ensure!(
            !MATERIAL_MIX_FAILED.swap(false, std::sync::atomic::Ordering::Relaxed),
            "Unsupported native contact material combination before simulation"
        );
        let mut removed = 0;
        let result = unsafe {
            bw_collision_refresh_filter(
                self.handle.as_ptr(),
                i32::try_from(shape)?,
                filter_simulation,
                &mut context as *mut _ as *mut c_void,
                &mut removed,
            )
        };
        anyhow::ensure!(
            result != 0 && !context.invalid && removed >= 0,
            "Installed body filter refresh failed or world is locked"
        );
        Ok(removed as usize)
    }
    #[cfg(test)]
    fn raw_pair_ids(&self, shape: usize) -> anyhow::Result<Vec<i32>> {
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let count = unsafe {
            bw_collision_pair_ids(
                self.handle.as_ptr(),
                i32::try_from(shape)?,
                std::ptr::null_mut(),
                0,
            )
        };
        anyhow::ensure!(count >= 0, "Cannot enumerate body pairs");
        let mut ids = vec![0; count as usize];
        let written = unsafe {
            bw_collision_pair_ids(
                self.handle.as_ptr(),
                i32::try_from(shape)?,
                ids.as_mut_ptr(),
                count,
            )
        };
        anyhow::ensure!(written == count, "Pair list changed outside simulation");
        ids.sort_unstable();
        Ok(ids)
    }
    /// Box3D world admission parameters, after source mass/placement. Negative
    /// native material/damping values need a compatibility path, not clamping.
    pub fn set_simulation_body(
        &mut self,
        shapes: &[usize],
        motion: &super::native_motion_record::MotionRecord,
        material: super::body_material::Coefficients,
        awake: bool,
    ) -> anyhow::Result<()> {
        let scalar = |offset| f32::from_be_bytes(motion.0[offset..offset + 4].try_into().unwrap());
        let parameters = SimulationBodyParameters {
            motion_type: motion.motion_type(),
            state: motion.swept_state(),
            linear: motion.linear_velocity(),
            angular: motion.angular_velocity(),
            damping: [scalar(0xb4), scalar(0xb8)],
            gravity: f32::from_bits(
                u32::from(u16::from_be_bytes(
                    motion.0[0x11e..0x120].try_into().unwrap(),
                )) << 16,
            ),
            limits: [
                super::native_motion_limits::decode(motion.0[0xbc]),
                super::native_motion_limits::decode(motion.0[0xbd]),
            ],
        };
        self.set_simulation_body_parameters(shapes, &parameters, material, awake)
    }
    /// Admit resolved PC motion through the same solver path without recoding
    /// its compressed limits through the older Wii lookup table. Placement,
    /// inverse mass, activation, filters and scene lifecycle remain caller-owned.
    pub fn set_simulation_body_2012(
        &mut self,
        shapes: &[usize],
        motion: &super::havok_2012_motion::Motion,
        material: super::body_material::Coefficients,
        awake: bool,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            motion.state_halfword_b8 == 0x3f80,
            "Non-default Havok 2012 packed state requires compatibility support"
        );
        let parameters = SimulationBodyParameters {
            motion_type: motion.motion_type,
            state: super::native_motion_step::MotionState {
                transform: motion.transform,
                previous_center: motion.previous_center,
                center: motion.center,
                previous_orientation: motion.previous_orientation,
                orientation: motion.orientation,
                local_center: motion.local_center,
                delta_angle: motion.delta_angle,
            },
            linear: motion.linear_velocity,
            angular: motion.angular_velocity,
            damping: [motion.linear_damping, motion.angular_damping],
            gravity: motion.gravity_factor,
            limits: motion.velocity_limits,
        };
        self.set_simulation_body_parameters(shapes, &parameters, material, awake)
    }
    fn set_simulation_body_parameters(
        &mut self,
        shapes: &[usize],
        motion: &SimulationBodyParameters,
        material: super::body_material::Coefficients,
        awake: bool,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(!shapes.is_empty(), "Empty simulation body");
        let first = shapes[0];
        anyhow::ensure!(first < self.mesh_owners.len(), "Unknown simulation body");
        if let Some(group) = self.shape_groups.get(&first) {
            anyhow::ensure!(
                group.len() == shapes.len() && group.iter().all(|part| shapes.contains(part)),
                "Partial simulation body"
            );
        } else {
            anyhow::ensure!(shapes.len() == 1, "Ungrouped simulation body");
        }
        let linear = motion.linear;
        let angular = motion.angular;
        let state = &motion.state;
        let center: V = state.local_center[..3].try_into().unwrap();
        let solver_linear = if motion.motion_type == 4 {
            sub(
                linear[..3].try_into().unwrap(),
                cross(
                    angular[..3].try_into().unwrap(),
                    rotated_center(center, state.orientation),
                ),
            )
        } else {
            linear[..3].try_into().unwrap()
        };
        let values = [
            solver_linear[0],
            solver_linear[1],
            solver_linear[2],
            angular[0],
            angular[1],
            angular[2],
            motion.damping[0],
            motion.damping[1],
            motion.gravity,
            material.friction,
            material.restitution,
            motion.limits[0],
            motion.limits[1],
        ];
        anyhow::ensure!(
            values.iter().all(|v| v.is_finite())
                && values[6] >= 0.
                && values[7] >= 0.
                && material.friction >= 0.
                && material.restitution >= 0.,
            "Body coefficients require a native compatibility path"
        );
        anyhow::ensure!(
            matches!(motion.motion_type, 2 | 3 | 4 | 5 | 6 | 7),
            "Unresolved simulation motion mode"
        );
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        anyhow::ensure!(
            unsafe {
                bw_collision_body_parameters(
                    self.handle.as_ptr(),
                    i32::try_from(first)?,
                    i32::from(motion.motion_type),
                    values.as_ptr(),
                    i32::from(awake),
                )
            } != 0,
            "Body simulation admission failed"
        );
        let snapshot = BodyMotion {
            center_of_mass: state.center[..3].try_into().unwrap(),
            linear_velocity: linear[..3].try_into().unwrap(),
            angular_velocity: angular[..3].try_into().unwrap(),
            motion_type: motion.motion_type,
        };
        for &shape in shapes {
            if motion.motion_type == 4 {
                self.kinematic_centers.insert(shape, center);
            } else {
                self.kinematic_centers.remove(&shape);
            }
            self.shape_motion.insert(shape, snapshot);
        }
        self.refresh_height_bounds();
        Ok(())
    }
    pub fn simulation_pose(&self, shape: usize) -> anyhow::Result<SimulationPose> {
        anyhow::ensure!(
            shape < self.mesh_owners.len(),
            "Unknown simulation body pose"
        );
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let mut values = [0.; 16];
        anyhow::ensure!(
            unsafe {
                bw_collision_body_snapshot(
                    self.handle.as_ptr(),
                    i32::try_from(shape)?,
                    values.as_mut_ptr(),
                )
            } != 0
                && values.iter().all(|v| v.is_finite()),
            "Invalid simulated body pose"
        );
        restore_kinematic_center(self.kinematic_centers.get(&shape), &mut values);
        Ok(SimulationPose {
            position: values[..3].try_into().unwrap(),
            orientation: values[3..7].try_into().unwrap(),
            linear: values[7..10].try_into().unwrap(),
            angular: values[10..13].try_into().unwrap(),
            center: values[13..16].try_into().unwrap(),
        })
    }
    pub fn wake_simulation_body(&mut self, shape: usize) -> anyhow::Result<()> {
        anyhow::ensure!(shape < self.mesh_owners.len(), "Unknown body to activate");
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        anyhow::ensure!(
            unsafe { bw_collision_wake_body(self.handle.as_ptr(), i32::try_from(shape)?) } != 0,
            "Body activation failed"
        );
        Ok(())
    }
    pub fn simulation_body_supported(&self, shape: usize, slope_cos: f32) -> anyhow::Result<bool> {
        anyhow::ensure!(
            shape < self.mesh_owners.len()
                && slope_cos.is_finite()
                && (0.0..=1.0).contains(&slope_cos),
            "Invalid body support query"
        );
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let result = unsafe {
            bw_collision_body_supported(self.handle.as_ptr(), i32::try_from(shape)?, slope_cos)
        };
        anyhow::ensure!(result >= 0, "Body support query failed");
        Ok(result != 0)
    }
    pub fn enable_simulation_contact_events(&mut self, shape: usize) -> anyhow::Result<()> {
        anyhow::ensure!(
            shape < self.mesh_owners.len(),
            "Unknown contact-event shape"
        );
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        anyhow::ensure!(
            unsafe { bw_collision_contact_events(self.handle.as_ptr(), i32::try_from(shape)?) }
                != 0,
            "Contact-event activation failed"
        );
        Ok(())
    }
    /// Persistent solver contacts, including resting contacts after begin events
    /// have expired. Returns owned indices; native manifold pointers never escape.
    pub(super) fn touching_shape_pairs(&self,shape:usize)->anyhow::Result<Vec<[usize;2]>> {
        let _guard=BOX3D.lock().unwrap_or_else(|e|e.into_inner());
        let mut collected=ContactPairs::default();
        anyhow::ensure!(unsafe {bw_collision_touching_contacts(self.handle.as_ptr(),i32::try_from(shape)?,
            collect_contact_pair,&mut collected as *mut _ as *mut c_void)}!=0 && !collected.failed,
            "Persistent contact collection failed");
        Ok(collected.pairs)
    }
    /// Copy transient begin-touch identities before modifying any world bodies.
    /// Box3D touch timing is not a verified Havok contact-point callback match.
    pub fn simulation_begin_contacts(&self) -> anyhow::Result<Vec<SimulationContact>> {
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let mut collected = ContactPairs::default();
        anyhow::ensure!(
            unsafe {
                bw_collision_begin_contacts(
                    self.handle.as_ptr(),
                    collect_contact_pair,
                    &mut collected as *mut _ as *mut c_void,
                )
            } != 0
                && !collected.failed,
            "Contact-event collection failed"
        );
        collected
            .pairs
            .into_iter()
            .map(|shapes| {
                let owners = [
                    *self
                        .mesh_owners
                        .get(shapes[0])
                        .ok_or_else(|| anyhow::anyhow!("Unknown contact shape A"))?,
                    *self
                        .mesh_owners
                        .get(shapes[1])
                        .ok_or_else(|| anyhow::anyhow!("Unknown contact shape B"))?,
                ];
                let motion_types = shapes.map(|shape| {
                    self.shape_motion
                        .get(&shape)
                        .map_or(5, |motion| motion.motion_type)
                });
                Ok(SimulationContact {
                    shapes,
                    owners,
                    motion_types,
                })
            })
            .collect()
    }
    /// One Box3D solver step, explicitly separate from Havok fidelity. All
    /// shapes need resolved native filter words before any solver mutation.
    pub fn step_simulation(
        &mut self,
        dt: f32,
        gravity: V,
        filter: &super::physics_filter::GameFilter,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            dt.is_finite() && dt > 0. && gravity.iter().all(|v| v.is_finite()),
            "Invalid simulation step"
        );
        anyhow::ensure!(
            (0..self.mesh_owners.len()).all(|shape| {
                (unsafe { bw_collision_shape_is_live(self.handle.as_ptr(), shape as i32) }) == 0
                    || self.simulation_filters.contains_key(&shape)
            }),
            "Simulation requires every body's resolved collision filter"
        );
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let mut context = SimulationFilter {
            filter,
            words: &self.simulation_filters,
            invalid: false,
        };
        let mut velocity_limits = VelocityLimitContext::default();
        struct ResetVelocity(*mut c_void);
        impl Drop for ResetVelocity {
            fn drop(&mut self) {
                unsafe { bw_collision_velocity_policy(self.0, None, std::ptr::null_mut()) };
            }
        }
        anyhow::ensure!(
            unsafe {
                bw_collision_velocity_policy(
                    self.handle.as_ptr(),
                    Some(limit_solver_velocity),
                    &mut velocity_limits as *mut _ as *mut c_void,
                )
            } != 0,
            "Velocity limit policy registration failed"
        );
        let reset_velocity = ResetVelocity(self.handle.as_ptr());
        let response = self.character_response;
        let mut mass = CharacterMassContext {
            motions: &self.shape_motion,
            velocity: response.map_or([0.; 3], |r| r.0),
            limit: response.map_or(1000., |r| r.1),
            error: None,
            count: 0,
        };
        struct Reset(*mut c_void);
        impl Drop for Reset {
            fn drop(&mut self) {
                unsafe {
                    bw_collision_character_mass_policy(self.0, -1, None, std::ptr::null_mut())
                };
            }
        }
        let reset = if response.is_some() {
            let shape = self
                .player_body
                .ok_or_else(|| anyhow::anyhow!("Response body disappeared"))?;
            anyhow::ensure!(
                unsafe {
                    bw_collision_character_mass_policy(
                        self.handle.as_ptr(),
                        shape as i32,
                        Some(character_mass),
                        &mut mass as *mut _ as *mut c_void,
                    )
                } != 0,
                "Character mass policy registration failed"
            );
            Some(Reset(self.handle.as_ptr()))
        } else {
            None
        };
        let success = unsafe {
            bw_collision_step(
                self.handle.as_ptr(),
                dt,
                gravity.as_ptr(),
                filter_simulation,
                &mut context as *mut _ as *mut c_void,
            )
        } != 0;
        drop(reset);
        drop(reset_velocity);
        anyhow::ensure!(
            !MATERIAL_MIX_FAILED.swap(false, std::sync::atomic::Ordering::Relaxed),
            "Unsupported native contact material combination during simulation"
        );
        if let Some(error) = velocity_limits.error {
            anyhow::bail!(error);
        }
        if let Some(error) = mass.error {
            anyhow::bail!(error);
        }
        self.character_mass_modifications += mass.count;
        anyhow::ensure!(
            success && !context.invalid,
            "Shared simulation/filter/contact policy failed"
        );
        for (&shape, motion) in &mut self.shape_motion {
            if motion.motion_type == 5 {
                continue;
            }
            let mut values = [0.; 16];
            anyhow::ensure!(
                unsafe {
                    bw_collision_body_snapshot(
                        self.handle.as_ptr(),
                        shape as i32,
                        values.as_mut_ptr(),
                    )
                } != 0,
                "Simulated body disappeared"
            );
            restore_kinematic_center(self.kinematic_centers.get(&shape), &mut values);
            motion.linear_velocity = values[7..10].try_into().unwrap();
            motion.angular_velocity = values[10..13].try_into().unwrap();
            motion.center_of_mass = values[13..16].try_into().unwrap();
            self.shape_min_y[shape] =
                unsafe { bw_collision_shape_min_y(self.handle.as_ptr(), shape as i32) };
        }
        self.refresh_height_bounds();
        Ok(())
    }
    /// Shared native query override lifecycle around the real Box3D sweep.
    /// The evaluator must resolve actual body/child records and return native
    /// register bits. A low-byte-only value is not an admitted collision.
    pub fn sphere_hits_game_query(
        &self,
        origin: V,
        translation: V,
        radius: f32,
        filter: &mut super::physics_filter::GameFilter,
        flags: Option<super::physics_filter::QueryFlags>,
        mut evaluate: impl FnMut(
            &mut super::physics_filter::GameFilter,
            SphereCandidate,
        ) -> anyhow::Result<u32>,
    ) -> anyhow::Result<Vec<StaticRayHit>> {
        filter.query(flags, |shared| {
            self.sphere_hits_filtered(origin, translation, radius, |candidate| {
                Ok(evaluate(shared, candidate)? >> 24 != 0)
            })
        })
    }
    /// Filter each BVH triangle/convex candidate before GJK intersection. Native
    /// body/child filter routing and key reconstruction are caller obligations.
    /// Callback must not panic or reenter Collision: the Box3D lock is held.
    pub fn sphere_hits_filtered(
        &self,
        origin: V,
        translation: V,
        radius: f32,
        mut policy: impl FnMut(SphereCandidate) -> anyhow::Result<bool>,
    ) -> anyhow::Result<Vec<StaticRayHit>> {
        anyhow::ensure!(
            origin
                .into_iter()
                .chain(translation)
                .chain(add(origin, translation))
                .all(f32::is_finite)
                && radius.is_finite()
                && radius >= 0.,
            "Invalid filtered sphere sweep"
        );
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let mut collector = SphereCollector {
            ray: RayCollector {
                hits: Vec::new(),
                owners: &self.mesh_owners,
                error: None,
            },
            policy: &mut policy,
            error: None,
        };
        let success = self.with_query_policy(|| unsafe {
            bw_collision_sphere_filtered(
                self.handle.as_ptr(),
                origin.as_ptr(),
                translation.as_ptr(),
                radius,
                collect_sphere,
                filter_sphere,
                &mut collector as *mut _ as *mut c_void,
            )
        });
        if let Some(error) = collector.error {
            return Err(error);
        }
        if let Some(error) = collector.ray.error {
            anyhow::bail!(error);
        }
        anyhow::ensure!(success != 0, "Filtered sphere sweep failed");
        Ok(collector.ray.hits)
    }
    /// Box3D all-hit static sphere cast, including non-solid geometry. These
    /// unfiltered results retain Box3D/source identities; Havok tolerances,
    /// keys, filters and delivery order have not been validated as equivalent.
    pub fn sphere_hits(
        &self,
        origin: V,
        translation: V,
        radius: f32,
    ) -> anyhow::Result<Vec<StaticRayHit>> {
        anyhow::ensure!(
            origin
                .into_iter()
                .chain(translation)
                .chain(add(origin, translation))
                .all(f32::is_finite)
                && radius.is_finite()
                && radius >= 0.,
            "Invalid all-hit sphere sweep"
        );
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let mut collector = RayCollector {
            hits: Vec::new(),
            owners: &self.mesh_owners,
            error: None,
        };
        let success = self.with_query_policy(|| unsafe {
            bw_collision_sphere_all(
                self.handle.as_ptr(),
                origin.as_ptr(),
                translation.as_ptr(),
                radius,
                collect_ray,
                &mut collector as *mut _ as *mut c_void,
            )
        });
        if let Some(error) = collector.error {
            anyhow::bail!(error);
        }
        anyhow::ensure!(success != 0, "All-hit sphere sweep failed");
        Ok(collector.hits)
    }
    /// Current Box3D broadphase candidates, including non-solid owners. These
    /// are shape identities, not confirmed Havok phantom overlap events.
    /// Compound parts remain separate; native filter/agent dispatch is required.
    pub fn overlap_candidates(&self, lower: V, upper: V) -> anyhow::Result<Vec<OverlapCandidate>> {
        anyhow::ensure!(
            lower.iter().chain(&upper).all(|v| v.is_finite())
                && (0..3).all(|i| lower[i] <= upper[i]),
            "Invalid collision overlap bounds"
        );
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let mut collector = OverlapCollector {
            hits: Vec::new(),
            owners: &self.mesh_owners,
            error: None,
        };
        let success = self.with_query_policy(|| unsafe {
            bw_collision_overlap_candidates(
                self.handle.as_ptr(),
                lower.as_ptr(),
                upper.as_ptr(),
                collect_overlap,
                &mut collector as *mut _ as *mut c_void,
            )
        });
        if let Some(error) = collector.error {
            anyhow::bail!(error);
        }
        anyhow::ensure!(success != 0, "Collision broadphase overlap query failed");
        Ok(collector.hits)
    }
    /// Phantom-style whole-body union bounds, including gaps between compound
    /// shapes. No native Havok quantization or fat-proxy equivalence is claimed.
    pub(super) fn overlap_body_bounds(
        &self,
        lower: V,
        upper: V,
    ) -> anyhow::Result<Vec<OverlapCandidate>> {
        anyhow::ensure!(
            lower.iter().chain(&upper).all(|v| v.is_finite())
                && (0..3).all(|i| lower[i] <= upper[i]),
            "Invalid body overlap bounds"
        );
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let mut collector = OverlapCollector {
            hits: Vec::new(),
            owners: &self.mesh_owners,
            error: None,
        };
        let success = self.with_query_policy(|| unsafe {
            bw_collision_overlap_body_bounds(
                self.handle.as_ptr(),
                lower.as_ptr(),
                upper.as_ptr(),
                collect_overlap,
                &mut collector as *mut _ as *mut c_void,
            )
        });
        if let Some(error) = collector.error {
            anyhow::bail!(error);
        }
        anyhow::ensure!(success != 0, "Body bounds overlap query failed");
        Ok(collector.hits)
    }
    /// Narrowphase sphere overlap against current body poses, including erased
    /// geometry. Game-layer/owner filtering remains the caller's responsibility.
    pub fn overlap_sphere(&self, center: V, radius: f32) -> anyhow::Result<Vec<OverlapCandidate>> {
        anyhow::ensure!(
            center.into_iter().all(f32::is_finite) && radius.is_finite() && radius >= 0.,
            "Invalid overlap sphere"
        );
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let mut collector = OverlapCollector {
            hits: Vec::new(),
            owners: &self.mesh_owners,
            error: None,
        };
        let success = self.with_query_policy(|| unsafe {
            bw_collision_overlap_sphere(
                self.handle.as_ptr(),
                center.as_ptr(),
                radius,
                collect_overlap,
                &mut collector as *mut _ as *mut c_void,
            )
        });
        if let Some(error) = collector.error {
            anyhow::bail!(error);
        }
        anyhow::ensure!(success != 0, "Collision sphere overlap failed");
        Ok(collector.hits)
    }
    /// Raw Box3D GJK witness pairs (triangle vertex, mover hull vertex), in
    /// simplex order. These are not native Havok features. SAT/convex/capsule
    /// contacts return None; never derive a feature from an overlap cache.
    pub fn contact_simplexes(&self) -> Vec<Option<Vec<[u8; 2]>>> {
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let count = unsafe { bw_collision_contact_count(self.handle.as_ptr()) };
        (0..count)
            .map(|i| {
                let mut pairs = [[0; 2]; 4];
                let n = unsafe {
                    bw_collision_contact_simplex(self.handle.as_ptr(), i, pairs.as_mut_ptr().cast())
                };
                assert!((0..=4).contains(&n), "Invalid Box3D simplex count");
                (n != 0).then(|| pairs[..n as usize].to_vec())
            })
            .collect()
    }
    /// Snapshot identities in contact-plane order. Triangle IDs index the
    /// immutable Collision::triangles array, not Box3D's reordered mesh faces
    /// or a body's local subpart. Convex/capsule paths without IDs return None.
    pub fn contact_sources(&self) -> Vec<(Option<usize>, Option<usize>)> {
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let count = unsafe { bw_collision_contact_count(self.handle.as_ptr()) };
        (0..count)
            .map(|i| unsafe {
                (
                    usize::try_from(bw_collision_contact_mesh(self.handle.as_ptr(), i)).ok(),
                    usize::try_from(bw_collision_contact_source_triangle(
                        self.handle.as_ptr(),
                        i,
                    ))
                    .ok(),
                )
            })
            .collect()
    }
    /// Publish a compound body's stepped pose and support velocity together.
    /// Validate every shape before moving any of them. One lock and one global
    /// bounds reduction per body avoid per-part lock/reduction overhead.
    pub fn set_body_state(
        &mut self,
        shapes: &[usize],
        position: V,
        rotation: [f32; 4],
        motion: BodyMotion,
    ) -> anyhow::Result<()> {
        let rotation = query_rotation(rotation)?;
        anyhow::ensure!(!shapes.is_empty(), "Rigid body has no collision shapes");
        anyhow::ensure!(
            shapes.iter().all(|&i| i < self.mesh_owners.len()),
            "Unknown rigid-body collision shape"
        );
        anyhow::ensure!(
            shapes.iter().all(|shape| self
                .shape_groups
                .get(shape)
                .is_none_or(|group| group.iter().all(|part| shapes.contains(part)))),
            "Partial compound-body state publication"
        );
        anyhow::ensure!(
            (1..=7).contains(&motion.motion_type)
                && motion
                    .center_of_mass
                    .iter()
                    .chain(&motion.linear_velocity)
                    .chain(&motion.angular_velocity)
                    .all(|v| v.is_finite()),
            "Invalid collision body motion"
        );
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        for &shape in shapes {
            // Every index was validated, and every call uses the same pose.
            // Invalid transforms fail on the first call without changing it.
            anyhow::ensure!(unsafe {bw_collision_set_transform(self.handle.as_ptr(),shape as i32,
                position.as_ptr(),rotation.as_ptr())}!=0,"Invalid collision rigid transform: shape={shape}, position={position:?}, rotation={rotation:?}");
            self.shape_min_y[shape] =
                unsafe { bw_collision_shape_min_y(self.handle.as_ptr(), shape as i32) };
            self.shape_motion.insert(shape, motion);
        }
        self.refresh_height_bounds();
        Ok(())
    }
    /// Move one rigid body's collision part independently of other bodies owned
    /// by the same entity. Placement is absolute in that part's imported frame.
    /// Callers apply this to all parts of a compound body, then publish motion.
    pub fn set_shape_transform(
        &mut self,
        shape: usize,
        position: V,
        rotation: [f32; 4],
    ) -> anyhow::Result<()> {
        let rotation = query_rotation(rotation)?;
        anyhow::ensure!(
            shape < self.mesh_owners.len(),
            "Unknown collision shape {shape}"
        );
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        anyhow::ensure!(unsafe {bw_collision_set_transform(self.handle.as_ptr(),shape as i32,
            position.as_ptr(),rotation.as_ptr())}!=0,"Invalid collision rigid transform: shape={shape}, position={position:?}, rotation={rotation:?}");
        if let Some(group) = self.shape_groups.get(&shape) {
            for &part in group.iter() {
                self.shape_min_y[part] =
                    unsafe { bw_collision_shape_min_y(self.handle.as_ptr(), part as i32) };
            }
        } else {
            self.shape_min_y[shape] =
                unsafe { bw_collision_shape_min_y(self.handle.as_ptr(), shape as i32) };
        }
        self.refresh_height_bounds();
        Ok(())
    }
    /// Publish support-query motion only. The physics owner must separately
    /// advance the body and update its world transform at the simulation boundary.
    pub fn set_shape_motion(&mut self, shape: usize, motion: BodyMotion) -> anyhow::Result<()> {
        anyhow::ensure!(
            shape < self.mesh_owners.len(),
            "Unknown collision shape {shape}"
        );
        anyhow::ensure!(
            (1..=7).contains(&motion.motion_type)
                && motion
                    .center_of_mass
                    .iter()
                    .chain(&motion.linear_velocity)
                    .chain(&motion.angular_velocity)
                    .all(|v| v.is_finite()),
            "Invalid collision body motion"
        );
        self.shape_motion.insert(shape, motion);
        Ok(())
    }
    /// Absolute rigid placement relative to the imported geometry frame (xyzw quaternion).
    /// This updates queries and bounds, but supplies no platform velocity or carry policy.
    pub fn set_owner_transform(
        &mut self,
        owner: usize,
        position: V,
        rotation: [f32; 4],
    ) -> anyhow::Result<()> {
        let rotation = query_rotation(rotation)?;
        anyhow::ensure!(
            self.mesh_owners.contains(&owner),
            "Unknown collision owner {owner}"
        );
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        for (index, &candidate) in self.mesh_owners.iter().enumerate() {
            if candidate != owner {
                continue;
            }
            // Every shape receives the same validated transform. Rejection on the
            // first shape leaves the owner unchanged; later calls cannot differ.
            anyhow::ensure!(unsafe {bw_collision_set_transform(self.handle.as_ptr(),index as i32,
                position.as_ptr(),rotation.as_ptr())}!=0,"Invalid collision rigid transform: owner={owner}, shape={index}, position={position:?}, rotation={rotation:?}");
            self.shape_min_y[index] =
                unsafe { bw_collision_shape_min_y(self.handle.as_ptr(), index as i32) };
        }
        self.refresh_height_bounds();
        self.owner_transforms.insert(owner, (position, rotation));
        Ok(())
    }
    pub fn has_owner(&self, owner: usize) -> bool {
        self.mesh_owners.contains(&owner)
    }
    /// Group parts of one initialized query body. Retain shape IDs, imported
    /// geometry, welding/source-triangle identities and per-shape solidity.
    pub fn group_body_shapes(&mut self, shapes: &[usize]) -> anyhow::Result<()> {
        anyhow::ensure!(!shapes.is_empty(), "Empty query body");
        let owner = self
            .shape_owner(shapes[0])
            .ok_or_else(|| anyhow::anyhow!("Unknown query-body part"))?;
        let mut indices = Vec::with_capacity(shapes.len());
        for &shape in shapes {
            anyhow::ensure!(
                self.shape_owner(shape) == Some(owner),
                "Mixed query-body owners"
            );
            anyhow::ensure!(
                !self.shape_groups.contains_key(&shape),
                "Query body already grouped"
            );
            let index = i32::try_from(shape)?;
            anyhow::ensure!(!indices.contains(&index), "Duplicate query-body part");
            indices.push(index);
        }
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        anyhow::ensure!(
            unsafe {
                bw_collision_group_shapes(
                    self.handle.as_ptr(),
                    indices.as_ptr(),
                    i32::try_from(indices.len())?,
                )
            } != 0,
            "Compound query-body construction failed"
        );
        let group = std::sync::Arc::new(shapes.to_vec());
        for &shape in shapes {
            self.shape_groups.insert(shape, group.clone());
        }
        Ok(())
    }
    /// Admit a compound query body with caller-resolved native inverse mass
    /// and diagonal local inertia. No reciprocal reconstruction/axis unlock.
    /// Motion lifecycle, forces, filtering and simulation remain caller-owned.
    pub fn set_native_body_mass(
        &mut self,
        shapes: &[usize],
        inverse_mass: f32,
        inverse_inertia: V,
        center: V,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(!shapes.is_empty(), "Empty native mass body");
        let first = shapes[0];
        anyhow::ensure!(first < self.mesh_owners.len(), "Unknown mass body");
        if let Some(group) = self.shape_groups.get(&first) {
            anyhow::ensure!(
                group.len() == shapes.len() && group.iter().all(|part| shapes.contains(part)),
                "Incomplete native mass body"
            );
        } else {
            anyhow::ensure!(shapes.len() == 1, "Ungrouped native mass parts");
        }
        anyhow::ensure!(
            inverse_mass.is_finite()
                && inverse_mass >= 0.
                && inverse_inertia.iter().all(|v| v.is_finite() && *v >= 0.)
                && center.iter().all(|v| v.is_finite()),
            "Invalid native inverse mass properties"
        );
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        anyhow::ensure!(
            unsafe {
                bw_collision_native_mass(
                    self.handle.as_ptr(),
                    i32::try_from(first)?,
                    inverse_mass,
                    inverse_inertia.as_ptr(),
                    center.as_ptr(),
                )
            } != 0,
            "Native mass admission failed"
        );
        Ok(())
    }
    /// Installed Box3D shape's owner record key; not a native Havok address.
    pub fn shape_owner(&self, shape: usize) -> Option<usize> {
        self.mesh_owners.get(shape).copied()
    }
    pub(super) fn supporting_body(&self) -> super::motion_ground::SupportBody {
        self.last_support_body.get()
    }
    /// Box3D static support probe feeding the recovered retail classifier.
    /// Havok manifold persistence and simplex-solver parity remain unverified.
    pub(super) fn support_at(
        &self,
        position: V,
        dt: f32,
        slope_cos: f32,
    ) -> Option<super::support::State> {
        self.support_at_with_depth(position, dt, slope_cos, None)
    }
    fn support_at_with_depth(
        &self,
        position: V,
        dt: f32,
        slope_cos: f32,
        depth: Option<V>,
    ) -> Option<super::support::State> {
        if !dt.is_finite() || dt <= 0. {
            return None;
        }
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let up = [0., 1., 0.];
        let requested = super::support::probe_velocity(up, dt);
        let mut solved = [0.; 3];
        let count = self.with_character_contacts(|| unsafe {
            bw_collision_support(
                self.handle.as_ptr(),
                position.as_ptr(),
                requested.as_ptr(),
                dt,
                solved.as_mut_ptr(),
            )
        });
        if count < 0 {
            self.last_support_body
                .set(super::motion_ground::SupportBody::Unknown);
            return None;
        }
        // Native ownership visits the contact stream, not just the plane
        // selected by support classification. The bridge already rejects
        // native rejected contacts; Havok manifold/disabled-flag parity is open.
        self.last_support_body
            .set(super::motion_ground::contact_support_body((0..count).map(
                |index| {
                    let raw = unsafe { bw_collision_contact_mesh(self.handle.as_ptr(), index) };
                    let shape = usize::try_from(raw).unwrap_or(usize::MAX);
                    let mode = self
                        .shape_motion
                        .get(&shape)
                        .map(|body| body.motion_type)
                        .or_else(|| {
                            self.imported_fixed_linear_velocity
                                .contains_key(&shape)
                                .then_some(5)
                        });
                    (shape, mode)
                },
            )));
        let contacts = || {
            (0..count).map(|index| {
                let mut plane = [0.; 4];
                unsafe {
                    bw_collision_contact(self.handle.as_ptr(), index, plane.as_mut_ptr());
                }
                ([plane[0], plane[1], plane[2]], unsafe {
                    bw_collision_contact_active(self.handle.as_ptr(), index) != 0
                })
            })
        };
        let candidate = super::support::classify(requested, solved, up, slope_cos);
        let state = super::support::retain_active_support(candidate, up, contacts());
        if let Some(depth) = depth {
            if state == super::support::State::Sliding
                && contacts().any(|(normal, active)| {
                    active && normal[1] > 0.08
                        && super::side_scroller::surface_normal(normal, depth)
                            .is_some_and(|projected| projected[1] >= slope_cos)
                })
            {
                return Some(super::support::State::Supported);
            }
        }
        Some(state)
    }
    /// Latest mover planes, copied only by explicit diagnostics (no frame trace).
    pub fn contact_planes(&self) -> Vec<[f32; 4]> {
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let count = unsafe { bw_collision_contact_count(self.handle.as_ptr()) };
        (0..count)
            .map(|index| {
                let mut plane = [0.; 4];
                let valid = unsafe {
                    bw_collision_contact(self.handle.as_ptr(), index, plane.as_mut_ptr())
                };
                assert_eq!(valid, 1, "Collision contact changed under registry lock");
                plane
            })
            .collect()
    }
    /// World-space query points parallel to the latest contact-plane snapshot.
    /// GJK points are on the core shape; intersecting cores use the retained
    /// deepest SAT contact. These are not yet verified Havok manifold points.
    pub fn contact_points(&self) -> Vec<V> {
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let count = unsafe { bw_collision_contact_count(self.handle.as_ptr()) };
        (0..count)
            .map(|index| {
                let mut point = [0.; 3];
                assert_eq!(
                    unsafe {
                        bw_collision_contact_point(self.handle.as_ptr(), index, point.as_mut_ptr())
                    },
                    1
                );
                point
            })
            .collect()
    }
    /// Source-part identities parallel to the latest contact-plane snapshot.
    /// Capsule fallback contacts currently have no retained mesh identity.
    pub fn contact_owners(&self) -> Vec<Option<usize>> {
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let count = unsafe { bw_collision_contact_count(self.handle.as_ptr()) };
        (0..count)
            .map(|index| {
                let mesh = unsafe { bw_collision_contact_mesh(self.handle.as_ptr(), index) };
                usize::try_from(mesh)
                    .ok()
                    .and_then(|i| self.mesh_owners.get(i).copied())
            })
            .collect()
    }
    /// Aggregates solver-active support contacts and world-space body motion.
    /// Unpublished shapes are the existing fixed world. The bridge's plane
    /// offset is penetration (combined radii minus core separation), whereas
    /// the native surface record stores signed separation. The solver restores
    /// its temporary slop adjustment before exposing these planes.
    pub fn active_support_motion(
        &self,
        include_dynamic_velocity: bool,
    ) -> Option<super::support::Surface> {
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let count = unsafe { bw_collision_contact_count(self.handle.as_ptr()) };
        let contacts = (0..count).filter_map(|index| {
            if unsafe { bw_collision_contact_active(self.handle.as_ptr(), index) } == 0 {
                return None;
            }
            let mut plane = [0.; 4];
            if unsafe { bw_collision_contact(self.handle.as_ptr(), index, plane.as_mut_ptr()) } == 0
                || plane[1] <= 0.08
            {
                return None;
            }
            let shape = unsafe { bw_collision_contact_mesh(self.handle.as_ptr(), index) };
            let motion = usize::try_from(shape)
                .ok()
                .and_then(|i| self.shape_motion.get(&i))
                .copied()
                .unwrap_or(BodyMotion {
                    center_of_mass: [0.; 3],
                    linear_velocity: [0.; 3],
                    angular_velocity: [0.; 3],
                    motion_type: 5,
                });
            let mut position = [0.; 3];
            assert_eq!(
                unsafe {
                    bw_collision_contact_point(self.handle.as_ptr(), index, position.as_mut_ptr())
                },
                1
            );
            Some(super::support::SurfaceContact {
                normal: [plane[0], plane[1], plane[2]],
                distance: -plane[3],
                position,
                center_of_mass: motion.center_of_mass,
                linear_velocity: motion.linear_velocity,
                angular_velocity: motion.angular_velocity,
                motion_type: motion.motion_type,
            })
        });
        super::support::aggregate_surface_iter(contacts, include_dynamic_velocity, 0.)
    }
    fn active_support_normal(&self) -> Option<V> {
        self.active_support_motion(false)
            .map(|surface| surface.normal)
    }
    fn active_support_surface(&self) -> Option<(V, f32)> {
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let count = unsafe { bw_collision_contact_count(self.handle.as_ptr()) };
        let mut best: Option<(V, f32)> = None;
        for index in 0..count {
            if unsafe { bw_collision_contact_active(self.handle.as_ptr(), index) } == 0 {
                continue;
            }
            let mut plane = [0.; 4];
            unsafe {
                bw_collision_contact(self.handle.as_ptr(), index, plane.as_mut_ptr());
            }
            if plane[1] > 0.08 && best.is_none_or(|(normal, _)| plane[1] > normal[1]) {
                best = Some(([plane[0], plane[1], plane[2]], plane[3]));
            }
        }
        best
    }
    pub fn set_player_hull(
        &mut self,
        hull: &super::character_hull::ConvexHull,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            hull.vertices.len() >= 4
                && hull.vertices.len() <= unsafe { bw_collision_hull_capacity() } as usize,
            "Character hull exceeds Box3D's supported vertex count; refusing to simplify it"
        );
        anyhow::ensure!(
            hull.radius.is_finite()
                && hull.radius >= 0.0
                && hull.vertices.iter().flatten().all(|x| x.is_finite()),
            "Invalid character hull"
        );
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        anyhow::ensure!(
            unsafe {
                bw_collision_set_hull(
                    self.handle.as_ptr(),
                    hull.vertices.as_ptr().cast(),
                    hull.vertices.len() as i32,
                    hull.radius,
                )
            } != 0,
            "Box3D could not construct the authored character hull"
        );
        Ok(())
    }
    /// Toggle physical participation without discarding geometry. Brush sphere
    /// sweeps and all-hit ray collection retain non-solid paint targets; ordinary
    /// rays, support queries and character movement only see solid shapes.
    /// This is a bridge capability, not the native Havok filter-policy mapping.
    /// Same erased-surface admission used by movement ground probes.
    pub(super) fn owner_is_solid(&self, owner: usize) -> bool {
        !self.non_solid_owners.contains(&owner)
    }
    pub fn set_owner_solid(&mut self, owner: usize, solid: bool) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.mesh_owners.contains(&owner),
            "Unknown collision owner {owner}"
        );
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        for (index, &candidate) in self.mesh_owners.iter().enumerate() {
            if candidate == owner {
                anyhow::ensure!(
                    unsafe {
                        bw_collision_set_solid(self.handle.as_ptr(), index as i32, solid as i32)
                    } != 0,
                    "Could not update collision shape {index}"
                );
            }
        }
        if solid {
            self.non_solid_owners.remove(&owner);
        } else {
            self.non_solid_owners.insert(owner);
        }
        Ok(())
    }
    /// Remove/re-add owned bodies to the backend world without changing their
    /// geometry, motion mode, velocities or painted/real filter state.
    /// This is the Box3D binding used by native rigid-body visibility hooks;
    /// native world locks, listeners and island rebuild ordering remain separate.
    pub fn set_owner_in_world(&mut self, owner: usize, present: bool) -> anyhow::Result<()> {
        let shapes = self.owner_shapes(owner).collect::<Vec<_>>();
        anyhow::ensure!(!shapes.is_empty(), "Unknown membership owner {owner}");
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        // Validate the whole owner first. A stale owner must never resurrect
        // retired geometry or partially enable one part of a compound body.
        for &shape in &shapes {
            anyhow::ensure!(
                unsafe { bw_collision_body_enabled(self.handle.as_ptr(), shape as i32) } >= 0,
                "Membership requested for retired shape {shape}"
            );
        }
        for shape in shapes {
            anyhow::ensure!(
                unsafe {
                    bw_collision_set_body_enabled(
                        self.handle.as_ptr(),
                        shape as i32,
                        i32::from(present),
                    )
                } != 0,
                "Could not change world membership for shape {shape}"
            );
        }
        Ok(())
    }
    pub fn owner_in_world(&self, owner: usize) -> anyhow::Result<bool> {
        let shapes = self.owner_shapes(owner).collect::<Vec<_>>();
        anyhow::ensure!(!shapes.is_empty(), "Unknown membership owner {owner}");
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let mut result = None;
        for shape in shapes {
            let enabled = unsafe { bw_collision_body_enabled(self.handle.as_ptr(), shape as i32) };
            anyhow::ensure!(enabled >= 0, "Membership queried for retired shape {shape}");
            let enabled = enabled != 0;
            anyhow::ensure!(
                result.is_none_or(|previous| previous == enabled),
                "Mixed membership for owner {owner}"
            );
            result = Some(enabled);
        }
        Ok(result.unwrap())
    }
    /// Remove all participation, including brush queries. Keep stable shape
    /// identities until world teardown so outstanding handles cannot alias.
    pub fn retire_owner(&mut self, owner: usize) -> anyhow::Result<()> {
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let shapes = self.owner_shapes(owner).collect::<Vec<_>>();
        for &index in &shapes {
            anyhow::ensure!(
                unsafe { bw_collision_release_shape(self.handle.as_ptr(), index as i32) } != 0,
                "Could not release retired shape {index}"
            );
        }
        for index in shapes {
            self.shape_motion.remove(&index);
            self.kinematic_centers.remove(&index);
            self.imported_fixed_linear_velocity.remove(&index);
            // Stable query handles remain allocated as tombstones, but their
            // solver filter metadata is no longer needed after native release.
            self.simulation_filters.remove(&index);
            self.shape_groups.remove(&index);
        }
        self.owner_transforms.remove(&owner);
        self.non_solid_owners.insert(owner);
        Ok(())
    }
    pub fn add_static_triangles(
        &mut self,
        triangles: &[Triangle],
        radius: f32,
        owner: usize,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            !triangles.is_empty()
                && triangles.len() <= 1_000_000
                && radius.is_finite()
                && radius >= 0.
                && triangles
                    .iter()
                    .flat_map(|t| t.p.iter().flatten())
                    .all(|v| v.is_finite()),
            "Invalid added collision triangles"
        );
        let start = self.triangle_count;
        let end = start
            .checked_add(triangles.len())
            .ok_or_else(|| anyhow::anyhow!("Collision triangle count overflow"))?;
        anyhow::ensure!(end <= i32::MAX as usize, "Collision source index overflow");
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        anyhow::ensure!(
            unsafe {
                bw_collision_add_mesh_range(
                    self.handle.as_ptr(),
                    triangles.as_ptr().cast(),
                    triangles.len() as i32,
                    radius,
                    start as i32,
                )
            } != 0,
            "Box3D could not add collision triangles"
        );
        self.mesh_owners.push(owner);
        self.triangle_count = end;
        #[cfg(test)]
        self.triangles.extend_from_slice(triangles);
        if let Some((p, q)) = self.owner_transforms.get(&owner) {
            assert_eq!(
                unsafe {
                    bw_collision_set_transform(
                        self.handle.as_ptr(),
                        (self.mesh_owners.len() - 1) as i32,
                        p.as_ptr(),
                        q.as_ptr(),
                    )
                },
                1,
                "Previously validated owner transform rejected"
            );
        }
        let minimum = unsafe {
            bw_collision_shape_min_y(self.handle.as_ptr(), (self.mesh_owners.len() - 1) as i32)
        };
        self.shape_min_y.push(minimum);
        self.min_y = self.min_y.min(minimum);
        self.recovery_min_y = self.recovery_min_y.min(minimum);
        // Adding shapes to an already-thinned owner must not resurrect collision.
        if self.non_solid_owners.contains(&owner) {
            anyhow::ensure!(
                unsafe {
                    bw_collision_set_solid(
                        self.handle.as_ptr(),
                        (self.mesh_owners.len() - 1) as i32,
                        0,
                    )
                } != 0,
                "Cannot retain non-solid owner state"
            );
        }
        Ok(())
    }
    pub fn triangle_count(&self) -> usize {
        self.triangle_count
    }
    pub fn owner_shapes(&self, owner: usize) -> impl Iterator<Item = usize> + '_ {
        self.mesh_owners
            .iter()
            .enumerate()
            .filter_map(move |(shape, &candidate)| (candidate == owner).then_some(shape))
    }
    pub fn add_static_convex(
        &mut self,
        vertices: &[V],
        radius: f32,
        owner: usize,
    ) -> anyhow::Result<usize> {
        anyhow::ensure!((4..=unsafe {bw_collision_hull_capacity()} as usize).contains(&vertices.len()) && radius.is_finite() && radius>=0.
            && vertices.iter().flatten().all(|v|v.is_finite()),
            "Unsupported static convex hull ({} vertices, radius {radius}); refusing to simplify geometry",vertices.len());
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let admission = unsafe {
            bw_collision_add_convex(
                self.handle.as_ptr(),
                vertices.as_ptr().cast(),
                vertices.len() as i32,
                radius,
            )
        };
        anyhow::ensure!(
            admission != 0,
            "Box3D could not construct static convex hull"
        );
        if admission == 2 {
            self.planar_convex_count += 1;
        }
        let shape_index = self.mesh_owners.len();
        self.mesh_owners.push(owner);
        if let Some((p, q)) = self.owner_transforms.get(&owner) {
            assert_eq!(
                unsafe {
                    bw_collision_set_transform(
                        self.handle.as_ptr(),
                        shape_index as i32,
                        p.as_ptr(),
                        q.as_ptr(),
                    )
                },
                1,
                "Previously validated owner transform rejected"
            );
        }
        if self.non_solid_owners.contains(&owner) {
            anyhow::ensure!(
                unsafe {
                    bw_collision_set_solid(
                        self.handle.as_ptr(),
                        (self.mesh_owners.len() - 1) as i32,
                        0,
                    )
                } != 0,
                "Cannot retain non-solid owner state"
            );
        }
        let minimum = unsafe { bw_collision_shape_min_y(self.handle.as_ptr(), shape_index as i32) };
        self.shape_min_y.push(minimum);
        self.min_y = self.min_y.min(minimum);
        self.recovery_min_y = self.recovery_min_y.min(minimum);
        Ok(shape_index)
    }
    /// Analytic segment plus radius, not a tessellated convex approximation.
    pub fn add_static_capsule(
        &mut self,
        endpoints: [V; 2],
        radius: f32,
        owner: usize,
    ) -> anyhow::Result<usize> {
        anyhow::ensure!(
            radius.is_finite() && radius > 0. && endpoints.iter().flatten().all(|v| v.is_finite()),
            "Invalid analytic capsule"
        );
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        anyhow::ensure!(
            unsafe {
                bw_collision_add_capsule(self.handle.as_ptr(), endpoints.as_ptr().cast(), radius)
            } != 0,
            "Box3D could not construct analytic capsule"
        );
        let shape = self.mesh_owners.len();
        self.mesh_owners.push(owner);
        if let Some((p, q)) = self.owner_transforms.get(&owner) {
            anyhow::ensure!(
                unsafe {
                    bw_collision_set_transform(
                        self.handle.as_ptr(),
                        shape as i32,
                        p.as_ptr(),
                        q.as_ptr(),
                    )
                } == 1,
                "Capsule owner transform rejected"
            );
        }
        if self.non_solid_owners.contains(&owner) {
            anyhow::ensure!(
                unsafe { bw_collision_set_solid(self.handle.as_ptr(), shape as i32, 0) } != 0,
                "Capsule solidity rejected"
            );
        }
        let minimum = unsafe { bw_collision_shape_min_y(self.handle.as_ptr(), shape as i32) };
        self.shape_min_y.push(minimum);
        self.min_y = self.min_y.min(minimum);
        self.recovery_min_y = self.recovery_min_y.min(minimum);
        Ok(shape)
    }
    pub fn new(triangles: Vec<Triangle>) -> anyhow::Result<Self> {
        let radii = vec![0.; triangles.len()];
        Self::with_triangle_radii(triangles, radii)
    }
    pub fn with_triangle_radii(triangles: Vec<Triangle>, radii: Vec<f32>) -> anyhow::Result<Self> {
        let owners = vec![0; triangles.len()];
        Self::with_triangle_owners(triangles, radii, owners)
    }
    pub fn with_triangle_owners(
        triangles: Vec<Triangle>,
        radii: Vec<f32>,
        owners: Vec<usize>,
    ) -> anyhow::Result<Self> {
        let contact_welding = super::shape_welding::ContactTable::retail_world()?;
        anyhow::ensure!(
            owners.len() == triangles.len(),
            "Collision owner count does not match triangles"
        );
        anyhow::ensure!(
            radii.len() == triangles.len() && radii.iter().all(|r| r.is_finite() && *r >= 0.),
            "Invalid static collision radii"
        );
        anyhow::ensure!(
            triangles.len() <= 1_000_000,
            "Invalid collision triangle count"
        );
        anyhow::ensure!(
            triangles
                .iter()
                .all(|t| t.p.iter().flatten().all(|v| v.is_finite())),
            "Non-finite collision geometry"
        );
        // Scene import usually emits each body's triangles together. In that
        // case the native bridge can copy the packed Rust triangle slice
        // directly. Keep the original grouping path for interleaved owners so
        // grouping behavior and contact source IDs remain unchanged.
        let radius_bits = |index: usize| {
            if radii[index] == 0. {
                0
            } else {
                radii[index].to_bits()
            }
        };
        let mut spans = Vec::<(usize, u32, usize, usize)>::new();
        let mut seen = std::collections::HashSet::new();
        let mut start = 0;
        let mut contiguous = true;
        while start < triangles.len() {
            let key = (owners[start], radius_bits(start));
            let mut end = start + 1;
            while end < triangles.len() && (owners[end], radius_bits(end)) == key {
                end += 1;
            }
            if !seen.insert(key) {
                contiguous = false;
                break;
            }
            if spans
                .last()
                .is_some_and(|previous| (previous.0, previous.1) >= key)
            {
                contiguous = false;
                break;
            }
            spans.push((key.0, key.1, start, end - start));
            start = end;
        }
        let mut groups = std::collections::BTreeMap::<(usize, u32), (Vec<f32>, Vec<i32>)>::new();
        if !contiguous {
            spans.clear();
            for (index, triangle) in triangles.iter().enumerate() {
                let key = (owners[index], radius_bits(index));
                let (points, ids) = groups.entry(key).or_default();
                points.extend(triangle.p.into_iter().flatten());
                ids.push(index as i32);
            }
        }
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        // The bridge copies this contiguous triangle soup into an owned Box3D mesh.
        let handle = NonNull::new(unsafe { bw_collision_create() })
            .ok_or_else(|| anyhow::anyhow!("Box3D could not create the collision world"))?;
        if unsafe {
            bw_collision_material_policy(handle.as_ptr(), mixed_friction, mixed_restitution)
        } == 0
        {
            unsafe {
                bw_collision_destroy(handle.as_ptr());
            }
            anyhow::bail!("Box3D could not install native material combination");
        }
        let mut mesh_owners = Vec::with_capacity(groups.len());
        let mut shape_min_y = Vec::with_capacity(groups.len());
        if contiguous {
            for (owner, radius, start, count) in spans {
                if unsafe {
                    bw_collision_add_mesh_range(
                        handle.as_ptr(),
                        triangles.as_ptr().add(start).cast(),
                        count as i32,
                        f32::from_bits(radius),
                        start as i32,
                    )
                } == 0
                {
                    unsafe {
                        bw_collision_destroy(handle.as_ptr());
                    }
                    anyhow::bail!("Box3D could not create a static collision mesh");
                }
                mesh_owners.push(owner);
                shape_min_y.push(unsafe {
                    bw_collision_shape_min_y(handle.as_ptr(), (mesh_owners.len() - 1) as i32)
                });
            }
        } else {
            for ((owner, radius), (points, ids)) in groups {
                if unsafe {
                    bw_collision_add_mesh(
                        handle.as_ptr(),
                        points.as_ptr(),
                        (points.len() / 9) as i32,
                        f32::from_bits(radius),
                        ids.as_ptr(),
                    )
                } == 0
                {
                    unsafe {
                        bw_collision_destroy(handle.as_ptr());
                    }
                    anyhow::bail!("Box3D could not create a static collision mesh");
                }
                mesh_owners.push(owner);
                shape_min_y.push(unsafe {
                    bw_collision_shape_min_y(handle.as_ptr(), (mesh_owners.len() - 1) as i32)
                });
            }
        }
        let triangle_count = triangles.len();
        Ok(Self {
            planar_convex_count: 0,
            last_support_body: std::cell::Cell::new(super::motion_ground::SupportBody::Unknown),
            character_response: None,
            character_mass_modifications: 0,
            player_body: None,
            query_excluded_body: Default::default(),
            native_character_contacts: None,
            character_contact_error: Default::default(),
            contact_welding,
            triangle_count,
            #[cfg(test)]
            triangles,
            mesh_owners,
            min_y: shape_min_y.iter().copied().fold(f32::INFINITY, f32::min),
            recovery_min_y: shape_min_y.iter().copied().fold(f32::INFINITY, f32::min),
            shape_min_y,
            owner_transforms: Default::default(),
            shape_motion: Default::default(),
            kinematic_centers: Default::default(),
            imported_fixed_linear_velocity: Default::default(),
            simulation_filters: Default::default(),
            shape_groups: Default::default(),
            non_solid_owners: Default::default(),
            handle,
        })
    }
    pub fn ray(&self, origin: V, dir: V, max: f32) -> Option<f32> {
        if !origin
            .into_iter()
            .chain(dir)
            .chain([max])
            .all(f32::is_finite)
            || max <= 0.0
        {
            return None;
        }
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let hit = self.with_query_policy(|| unsafe {
            bw_collision_ray(self.handle.as_ptr(), origin.as_ptr(), dir.as_ptr(), max)
        });
        (hit >= 0.0).then_some(hit)
    }
    /// Same Box3D closest-hit query as the camera ray, with authored ownership
    /// preserved for aim/interaction routing. Does not mutate mover contacts.
    pub fn ray_hit(&self, origin: V, dir: V, max: f32) -> Option<RayHit> {
        if max <= 0.
            || !origin
                .into_iter()
                .chain(dir)
                .chain([max])
                .all(f32::is_finite)
        {
            return None;
        }
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let mut output = [0.; 7];
        let mesh = self.with_query_policy(|| unsafe {
            bw_collision_ray_hit(
                self.handle.as_ptr(),
                origin.as_ptr(),
                dir.as_ptr(),
                max,
                output.as_mut_ptr(),
            )
        });
        let owner = *self.mesh_owners.get(usize::try_from(mesh).ok()?)?;
        Some(RayHit {
            distance: output[0],
            position: [output[1], output[2], output[3]],
            normal: [output[4], output[5], output[6]],
            owner,
        })
    }
    /// Every intersected static mesh triangle plus convex entry hits, in Box3D
    /// traversal order. No clipping, sorting, deduplication or fixed result cap.
    /// This is not yet the Havok collector: winding, margins, filter and key
    /// mapping, and equal-fraction ordering require compatibility validation.
    pub fn ray_hits(&self, origin: V, translation: V) -> anyhow::Result<Vec<StaticRayHit>> {
        anyhow::ensure!(
            origin
                .into_iter()
                .chain(translation)
                .chain(add(origin, translation))
                .all(f32::is_finite),
            "Invalid all-hit ray segment"
        );
        if translation == [0.; 3] {
            return Ok(Vec::new());
        }
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let mut collector = RayCollector {
            hits: Vec::new(),
            owners: &self.mesh_owners,
            error: None,
        };
        let success = self.with_query_policy(|| unsafe {
            bw_collision_ray_all(
                self.handle.as_ptr(),
                origin.as_ptr(),
                translation.as_ptr(),
                collect_ray,
                &mut collector as *mut _ as *mut c_void,
            )
        });
        if let Some(error) = collector.error {
            anyhow::bail!(error);
        }
        anyhow::ensure!(success != 0, "All-hit collision query failed");
        Ok(collector.hits)
    }
    /// Actual overlap contacts retain each original source triangle. Convex and
    /// capsule hits use None. Include non-solid geometry; filtering is separate.
    /// Box3D's overlap slop/GJK are not a claim of Havok contact equivalence.
    pub fn overlap_sphere_contacts(
        &self,
        center: V,
        radius: f32,
    ) -> anyhow::Result<Vec<SphereCandidate>> {
        anyhow::ensure!(
            center.into_iter().chain([radius]).all(f32::is_finite) && radius >= 0.,
            "Invalid material-aware sphere overlap"
        );
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let mut result = SphereOverlaps {
            values: Vec::new(),
            owners: &self.mesh_owners,
            failed: false,
        };
        let ok = self.with_query_policy(|| unsafe {
            bw_collision_overlap_sphere_all(
                self.handle.as_ptr(),
                center.as_ptr(),
                radius,
                collect_sphere_overlap,
                &mut result as *mut _ as *mut c_void,
            )
        });
        anyhow::ensure!(
            ok != 0 && !result.failed,
            "Material-aware sphere overlap failed"
        );
        Ok(result.values)
    }
    pub(super) fn capsule_half_extents(&self, shape: usize) -> anyhow::Result<V> {
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let mut half = [0.; 3];
        anyhow::ensure!(unsafe {
            bw_collision_capsule_half_extents(self.handle.as_ptr(), i32::try_from(shape)?, half.as_mut_ptr())
        } != 0 && half.iter().all(|x: &f32| x.is_finite()), "Invalid spawn body bounds");
        Ok(half)
    }

    pub(super) fn box_contact_height(
        &self,
        center: V,
        half: V,
        radius: f32,
        mut admits: impl FnMut(usize) -> bool,
    ) -> anyhow::Result<Option<f32>> {
        anyhow::ensure!(half.iter().all(|&v| v.is_finite() && v > 0.) && radius.is_finite() && radius >= 0.,
            "Invalid placement box");
        let lower = std::array::from_fn(|i| center[i] - half[i] - radius);
        let upper = std::array::from_fn(|i| center[i] + half[i] + radius);
        let candidates = self.overlap_candidates(lower, upper)?;
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let mut highest: Option<f32> = None;
        for hit in candidates.into_iter().filter(|hit| admits(hit.shape)) {
            let mut height = 0.;
            if self.with_query_policy(|| unsafe {
                bw_collision_box_contact_height(self.handle.as_ptr(), hit.shape as i32,
                    center.as_ptr(), half.as_ptr(), radius, &mut height)
            }) != 0 {
                highest = Some(highest.map_or(height, |prior| prior.max(height)));
            }
        }
        Ok(highest)
    }

    /// Box3D mover-hull penetration for a single shape, including erased shapes.
    /// Uses the mover's existing one-sided triangle policy. This is not yet
    /// the native restoration collector (per-child keys/filters and ray veto).
    pub fn shape_penetration(&self, shape: usize, position: V) -> anyhow::Result<f32> {
        anyhow::ensure!(
            shape < self.mesh_owners.len() && position.into_iter().all(f32::is_finite),
            "Invalid penetration query"
        );
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let mut depth = 0.;
        anyhow::ensure!(
            self.with_query_policy(|| unsafe {
                bw_collision_shape_penetration(
                    self.handle.as_ptr(),
                    shape as i32,
                    position.as_ptr(),
                    &mut depth,
                )
            }) != 0,
            "Penetration query requires an installed mover hull"
        );
        Ok(depth)
    }
    /// Same provisional Box3D narrowphase as shape_penetration, retaining
    /// source triangle identity for filtering. Does not modify mover contacts.
    pub fn shape_penetrations(
        &self,
        shape: usize,
        position: V,
    ) -> anyhow::Result<Vec<Penetration>> {
        anyhow::ensure!(
            shape < self.mesh_owners.len() && position.into_iter().all(f32::is_finite),
            "Invalid penetration query"
        );
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let mut result = Penetrations::default();
        let success = self.with_query_policy(|| unsafe {
            bw_collision_shape_penetrations(
                self.handle.as_ptr(),
                shape as i32,
                position.as_ptr(),
                collect_penetration,
                &mut result as *mut _ as *mut c_void,
            )
        });
        anyhow::ensure!(!result.failed, "Unable to allocate penetration contacts");
        anyhow::ensure!(
            success != 0,
            "Penetration query requires an installed mover hull"
        );
        Ok(result.values)
    }
    /// Retain source velocity for an imported fixed body's collision parts.
    /// This does not activate, move or assign a guessed center to the body.
    pub(super) fn retain_fixed_source_velocity(
        &mut self,
        owner: usize,
        velocity: [f32; 4],
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            velocity.iter().all(|v| v.is_finite()),
            "Nonfinite imported fixed velocity"
        );
        let shapes = self.owner_shapes(owner).collect::<Vec<_>>();
        anyhow::ensure!(
            !shapes.is_empty(),
            "Imported fixed velocity has no collider owner {owner}"
        );
        for shape in shapes {
            self.imported_fixed_linear_velocity.insert(shape, velocity);
        }
        Ok(())
    }
    pub(super) fn ground_body_linear_velocity(&self, shape: usize) -> Option<[f32; 4]> {
        self.shape_motion
            .get(&shape)
            .map(|motion| {
                [
                    motion.linear_velocity[0],
                    motion.linear_velocity[1],
                    motion.linear_velocity[2],
                    0.,
                ]
            })
            .or_else(|| self.imported_fixed_linear_velocity.get(&shape).copied())
    }
    /// Source-derived record publication over the current Box3D query adapter.
    /// Native filter/shape-key/equal-fraction collector ordering still require
    /// compatibility work; unresolved body metadata stays explicitly unknown.
    pub(super) fn update_motion_ground(
        &self,
        state: &mut super::motion_ground::State,
        input: super::motion_ground::Input,
    ) -> anyhow::Result<()> {
        struct Adapter<'a>(&'a Collision);
        impl super::motion_ground::Query for Adapter<'_> {
            fn hits(
                &mut self,
                start: [f32; 4],
                end: [f32; 4],
            ) -> anyhow::Result<Vec<super::motion_ground::Hit>> {
                let start: V = start[..3].try_into().unwrap();
                let delta = std::array::from_fn(|i| end[i] - start[i]);
                let mut hits = self.0.ray_hits(start, delta)?;
                hits.sort_by(|a, b| a.fraction.total_cmp(&b.fraction));
                Ok(hits
                    .into_iter()
                    .map(|hit| super::motion_ground::Hit {
                        point: [hit.position[0], hit.position[1], hit.position[2], 0.],
                        normal: [hit.normal[0], hit.normal[1], hit.normal[2], 0.],
                        body: if self.0.player_body == Some(hit.shape) {
                            super::motion_ground::Body::Own
                        } else {
                            super::motion_ground::Body::Rigid(hit.shape)
                        },
                    })
                    .collect())
            }
            fn linear_velocity(&mut self, shape: usize) -> anyhow::Result<Option<[f32; 4]>> {
                // Live body state overrides imported fixed-source metadata.
                // An unowned body remains unknown rather than becoming zero.
                Ok(self.0.ground_body_linear_velocity(shape))
            }
        }
        self.with_player_queries(|| state.update(input, &mut Adapter(self)))
    }
    /// Sweep an authored sphere through static meshes including their margins.
    /// Initial overlaps and gameplay layer filtering are separate operations;
    /// this query alone is not a replacement for rigid-body contact callbacks.
    pub fn sweep_sphere(
        &self,
        origin: V,
        translation: V,
        radius: f32,
    ) -> anyhow::Result<Option<SweepHit>> {
        anyhow::ensure!(
            radius > 0.
                && origin
                    .into_iter()
                    .chain(translation)
                    .chain([radius])
                    .all(f32::is_finite),
            "Invalid projectile sphere sweep"
        );
        if translation == [0.; 3] {
            return Ok(None);
        }
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let mut output = [0.; 7];
        let mut source_triangle = -1;
        let mesh = self.with_query_policy(|| unsafe {
            bw_collision_sweep_sphere(
                self.handle.as_ptr(),
                origin.as_ptr(),
                translation.as_ptr(),
                radius,
                output.as_mut_ptr(),
                &mut source_triangle,
            )
        });
        if mesh < 0 {
            return Ok(None);
        }
        let owner = *self
            .mesh_owners
            .get(mesh as usize)
            .ok_or_else(|| anyhow::anyhow!("Sphere sweep returned an unknown collision owner"))?;
        Ok(Some(SweepHit {
            shape: mesh as usize,
            source_triangle: usize::try_from(source_triangle).ok(),
            fraction: output[0],
            position: [output[1], output[2], output[3]],
            normal: [output[4], output[5], output[6]],
            owner,
        }))
    }
    /// Slope-filtered ground hits use the same live geometry as physical queries.
    /// Includes convex bodies; source triangle snapshots stay immutable.
    fn ground_distance(&self, origin: V, maximum: f32, slope_cos: f32) -> f32 {
        let hits = self
            .ray_hits(origin, [0., -maximum, 0.])
            .expect("Ground collision query failed");
        hits.into_iter()
            .filter(|hit| {
                !self.non_solid_owners.contains(&hit.owner)
                    && hit.normal[1] > 0.
                    && hit.normal[1] > slope_cos * length(hit.normal)
            })
            .map(|hit| hit.fraction * maximum)
            .fold(maximum, f32::min)
    }
    fn move_player(&self, position: &mut V, velocity: &mut V, dt: f32, slope_cos: f32) -> bool {
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        self.with_character_contacts(|| unsafe {
            bw_collision_move(
                self.handle.as_ptr(),
                position.as_mut_ptr(),
                velocity.as_mut_ptr(),
                0.3,
                1.5,
                slope_cos,
                dt,
            ) != 0
        })
    }
}
#[derive(Clone, serde::Serialize)]
pub struct Motion {
    pub stairs: super::stairs::Settings,
    pub maintain_ground_velocity: bool,
    pub jump_type: u32,
    pub jump_height_multipliers: Vec<f32>,
    pub air_speed: f32,
    pub air_control_acceleration: f32,
    pub air_side_acceleration: f32,
    pub stationary_turn_rate: f32,
    pub moving_turn_rate: f32,
    pub air_turn_rate: f32,
    pub ground_use_fixed_speeds: bool,
    pub ground_fixed_speeds: Vec<f32>,
    pub ground_stick_values: Vec<f32>,
    pub input_filter_type: u32,
    pub input_linear_velocity: f32,
    pub input_length_velocity: f32,
    pub input_rotation_velocity: f32,
    pub ground_acceleration: f32,
    pub ground_stopping_acceleration: f32,
    pub terminal_speed: f32,
    pub speed: f32,
    pub jump_height: f32,
    pub double_jump_height: f32,
    pub double_jump_min_distance: f32,
    pub max_distance_for_jump: f32,
    pub max_relevant_ground_distance: f32,
    pub ground_probe_start_offset: f32,
    pub max_slope_degrees: f32,
    pub immediate_jump_velocity_multiplier: f32,
    pub gravity_up: f32,
    pub gravity_down: f32,
    pub damage_leap_enabled: bool,
    pub damage_leap_launch_speed: f32,
    pub damage_leap_gravity: f32,
    pub damage_leap_air_speed: f32,
    pub damage_leap_acceleration: f32,
    /// EM2 MickeyPlayerMotionController skid / 180 settings (None = not authored).
    pub em2_moves: Option<super::locomotion_actions::Settings>,
    /// EM2 The180MinAllowedSpeed: extra factor on the "running fast" threshold (360 0x829f6bf0).
    pub turn_min_allowed_speed: f32,
}
impl Default for Motion {
    fn default() -> Self {
        Self {
            stairs: Default::default(),
            maintain_ground_velocity: false,
            jump_type: 2,
            jump_height_multipliers: vec![0.3, 0.3, 0.65, 0.65, 0.65, 1.0],
            air_speed: 4.5,
            air_control_acceleration: 20.0,
            air_side_acceleration: 20.0,
            stationary_turn_rate: 10.0,
            moving_turn_rate: 3.0,
            air_turn_rate: 4.0,
            ground_use_fixed_speeds: true,
            ground_fixed_speeds: vec![0.22, 0.4, 0.6, 1.0],
            ground_stick_values: vec![0.7, 0.71, 0.99, 1.0],
            input_filter_type: 2,
            input_linear_velocity: 2.0,
            input_length_velocity: 30.0,
            input_rotation_velocity: 4.5,
            ground_acceleration: 12.0,
            ground_stopping_acceleration: 25.0,
            terminal_speed: -25.0,
            speed: 5.0,
            jump_height: 2.0,
            double_jump_height: 1.0,
            double_jump_min_distance: 0.9,
            max_distance_for_jump: 0.125,
            max_relevant_ground_distance: 5.0,
            ground_probe_start_offset: 0.,
            max_slope_degrees: 39.0,
            immediate_jump_velocity_multiplier: 0.75,
            gravity_up: 30.0,
            gravity_down: 35.0,
            damage_leap_enabled: false,
            damage_leap_launch_speed: 11.5,
            damage_leap_gravity: -22.,
            damage_leap_air_speed: 5.,
            damage_leap_acceleration: 20.,
            em2_moves: None,
            turn_min_allowed_speed: 1.,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct GroundVectors {
    /// Ground-state +0x40, before surface velocity and collision response.
    pub movement: V,
    /// Ground-state +0x50, the desired movement vector.
    pub desired: V,
}
pub struct Player {
    /// Native motion+2e1; does not imply a collision contact or landing.
    pub stairs_ground_retained: bool,
    /// Ground0+70, retained slope-gravity force, reset outside ground state.
    slope_gravity_force: f32,
    pub native_knockback: bool,
    pub(super) damage_leaping: bool,
    pub(super) damage_leap_landed: bool,
    pub(super) knockback_velocity_override: bool,
    pending_physics: Option<SimulationPose>,
    pending_ground_support: Option<super::support::State>,
    /// Box3D contact-boundary state, not a serialized native controller field.
    /// Retain the supporting rigid body's point velocity across fixed ticks.
    support_carry: Option<(usize, V)>,
    pub support_state: Option<super::support::State>,
    pub motion_ground: Option<super::motion_ground::State>,
    /// Motion controller+e0, published by308a5c..308a68. Unsupported support
    /// retains this vector; a fresh owner has no proven initialized value.
    pub motion_surface_normal: Option<[f32; 4]>,
    /// Motion output before collision clipping, used by animation inputs.
    pub animation_velocity: V,
    pub speed_overrides: super::motion_overrides::Overrides,
    jump_stage: Option<(f32, usize)>,
    active_jump_height: f32,
    double_jump_available: bool,
    ground_jump_available: bool,
    last_ground_y: f32,
    filtered_stick: [f32; 2],
    raw_stick: [f32; 2],
    pub position: V,
    pub previous: V,
    pub velocity: V,
    pub spawn: V,
    pub grounded: bool,
    pub yaw: f32,
    pub respawns: u32,
    /// Mickey controller byte+31, read by the input-value publication path.
    pub script_input_restricted: bool,
    pub side_scroll_plane: Option<(V,V)>,
    pub motion: Motion,
    pub ground_vectors: Option<GroundVectors>,
    /// One-tick notification from the accepted ground-jump branch.
    pub ground_jump_started: bool,
    /// First jump accepted while airborne (for example after leaving a ledge).
    pub air_first_jump_started: bool,
    pub second_jump_started: bool,
    spin_gravity: Option<f32>,
    turn_stopping_acceleration: Option<f32>,
    action_input_lock: bool,
    pub(super) motion_input_flags:u32,
    pub(super) saved_motion_input_flags:u32,
    brush_turn_rate: Option<f32>,
    facing_target: f32,
    movement_direction_override: Option<V>,
}
impl Player {
    pub(super) fn can_spray_below(&self) -> bool {
        self.jump_stage.is_some() && !self.grounded
    }

    pub(super) fn query_support(&self, world: &Collision, dt: f32) -> Option<super::support::State> {
        let slope_cos = self.motion.max_slope_degrees.to_radians().cos();
        match self.side_scroll_plane {
            Some((_, depth)) => world.support_at_with_depth(self.position, dt, slope_cos, Some(depth)),
            None => world.support_at(self.position, dt, slope_cos),
        }
    }

    fn planar_support_normal(&self, normal: V) -> Option<V> {
        match self.side_scroll_plane {
            Some((_, depth)) => super::side_scroller::surface_normal(normal, depth),
            None => Some(normal),
        }
    }

    fn support_normal_walkable(&self, normal: V) -> bool {
        self.planar_support_normal(normal)
            .is_some_and(|normal| normal[1] >= self.motion.max_slope_degrees.to_radians().cos())
    }
    pub(super) fn begin_damage_leap(&mut self) -> bool {
        if !self.motion.damage_leap_enabled || self.native_knockback || self.damage_leaping {
            return false;
        }
        self.damage_leaping = true;
        self.damage_leap_landed = false;
        self.velocity[1] = self.motion.damage_leap_launch_speed;
        self.grounded = false;
        self.stairs_ground_retained = false;
        self.jump_stage = None;
        self.ground_jump_available = false;
        self.double_jump_available = false;
        self.pending_ground_support = None;
        self.support_carry = None;
        self.ground_vectors = None;
        true
    }
    /// Verified motion subset of brush start/stop; other action gates and
    /// CharacterState flags are still owned by the unfinished action runtime.
    pub fn begin_brush_motion(&mut self, speed: f32, stationary_rate: f32) -> anyhow::Result<()> {
        self.speed_overrides.set(1, speed)?;
        self.brush_turn_rate = Some(stationary_rate);
        Ok(())
    }
    pub fn end_brush_motion(&mut self) {
        self.speed_overrides.remove(1);
        self.brush_turn_rate = None;
    }
    /// Input-facing target suppression (motion+0x210) does not freeze yaw.
    pub(super) fn accept_script_facing(&mut self,yaw:f32) {
        self.yaw=yaw;
        self.facing_target=yaw;
    }
    pub fn set_aim_facing_target(&mut self, yaw: f32) {
        self.facing_target = yaw;
    }

    /// REL 0x3069a8, backed by Max Move Speed at motion+0x220.
    pub fn max_move_speed(&self) -> f32 {
        self.speed_overrides.highest().unwrap_or(self.motion.speed)
    }
    /// Brush start calls 0x30bf3c(true); stop reloads CharacterMaintainVelocity.
    pub fn maintains_ground_velocity(&self) -> bool {
        self.brush_turn_rate.is_some() || self.motion.maintain_ground_velocity
    }
    /// REL 0x97ef4 replaces GroundControlAccelStop during a quick turn.
    /// Keep the authored baseline separate so reset cannot retain the override.
    pub fn begin_turn_motion(&mut self, stopping_acceleration: f32) {
        self.turn_stopping_acceleration = Some(stopping_acceleration);
        self.action_input_lock = true;
    }
    pub fn begin_skid_motion(&mut self) {
        self.action_input_lock = true;
    }
    pub fn end_turn_motion(&mut self) {
        self.turn_stopping_acceleration = None;
        self.action_input_lock = false;
        self.movement_direction_override = None;
    }
    /// Native motion+213 is set for skid/turn entry and cleared by reset.
    /// It also controls the controller's idle-timer reset306680..3066a8.
    pub(super) fn reverse_motion_enabled(&self) -> bool {
        self.action_input_lock
    }
    /// Return non-motion effects to their owner; never silently consume an
    /// animation, gameplay event, or history-timer operation here.
    pub fn apply_action_motion(
        &mut self,
        effect: super::locomotion_actions::Effect,
    ) -> Option<super::locomotion_actions::Effect> {
        use super::locomotion_actions::Effect;
        match effect {
            Effect::BeginSkid => self.begin_skid_motion(),
            Effect::BeginTurn {
                direction,
                stopping_acceleration,
            } => {
                self.begin_turn_motion(stopping_acceleration);
                self.movement_direction_override = Some(direction);
            }
            Effect::Facing(direction) => self.apply_turn_facing(direction),
            Effect::ResetMotion => self.end_turn_motion(),
            other => return Some(other),
        }
        None
    }
    /// The action writes both the facing target and entity orientation
    /// directly (REL 0x97bd4..0x97bf0), bypassing the normal yaw-rate limit.
    pub fn apply_turn_facing(&mut self, direction: V) {
        self.yaw = direction[0].atan2(direction[2]);
        self.facing_target = self.yaw;
    }
    /// Spin start zeros vertical velocity and temporarily replaces both gravity
    /// directions (REL 0x0b7c50..0x0b7c80). Config stores signed acceleration.
    pub fn begin_spin_motion(&mut self, gravity: f32) {
        self.velocity[1] = 0.;
        if self.spin_gravity.is_none() {
            self.spin_gravity = Some(gravity);
        }
    }
    pub fn end_spin_motion(&mut self) {
        self.spin_gravity = None;
    }
    pub fn new(spawn: V, motion: Motion) -> Self {
        Self {
            native_knockback: false,
            damage_leaping: false,
            damage_leap_landed: false,
            knockback_velocity_override: false,
            stairs_ground_retained: false,
            slope_gravity_force: 0.,
            pending_physics: None,
            pending_ground_support: None,
            support_carry: None,
            support_state: None,
            motion_ground: None,
            motion_surface_normal: None,
            animation_velocity: [0.; 3],
            speed_overrides: Default::default(),
            jump_stage: None,
            active_jump_height: motion.jump_height,
            double_jump_available: false,
            ground_jump_available: false,
            last_ground_y: spawn[1],
            filtered_stick: [0.; 2],
            raw_stick: [0.; 2],
            position: spawn,
            previous: spawn,
            spawn,
            velocity: [0.; 3],
            grounded: false,
            yaw: 0.,
            respawns: 0,
            script_input_restricted: false,
            side_scroll_plane: None,
            motion,
            ground_vectors: None,
            ground_jump_started: false,
            air_first_jump_started: false,
            second_jump_started: false,
            spin_gravity: None,
            turn_stopping_acceleration: None,
            action_input_lock: false,
            motion_input_flags:3,
            saved_motion_input_flags:3,
            brush_turn_rate: None,
            facing_target: 0.,
            movement_direction_override: None,
        }
    }
    pub fn respawn(&mut self) {
        self.slope_gravity_force = 0.;
        self.native_knockback = false;
        self.damage_leaping = false;
        self.damage_leap_landed = false;
        self.knockback_velocity_override = false;
        self.stairs_ground_retained = false;
        self.support_carry = None;
        self.support_state = None;
        self.motion_ground = None;
        self.pending_ground_support = None;
        self.animation_velocity = [0.; 3];
        self.spin_gravity = None;
        self.turn_stopping_acceleration = None;
        self.action_input_lock = false;
        self.facing_target = self.yaw;
        self.movement_direction_override = None;
        self.ground_jump_started = false;
        self.air_first_jump_started = false;
        self.second_jump_started = false;
        self.jump_stage = None;
        self.double_jump_available = false;
        self.ground_jump_available = false;
        self.last_ground_y = self.spawn[1];
        self.filtered_stick = [0.; 2];
        self.raw_stick = [0.; 2];
        self.position = self.spawn;
        self.previous = self.spawn;
        self.velocity = [0.; 3];
        self.grounded = false;
        self.ground_vectors = None;
        self.respawns += 1;
    }
    /// Supported relocation reset from REL30a788: airborne state2, facing from
    /// the destination, zero commanded motion. This is not death or a checkpoint.
    /// It intentionally preserves input filtering and ability speed overrides.
    pub(super) fn accept_script_relocation(&mut self, position: V, yaw: f32) {
        self.slope_gravity_force = 0.;
        self.native_knockback = false;
        self.damage_leaping = false;
        self.damage_leap_landed = false;
        self.knockback_velocity_override = false;
        self.stairs_ground_retained = false;
        self.support_carry = None;
        self.position = position;
        self.previous = position;
        self.yaw = yaw;
        self.facing_target = yaw;
        self.velocity = [0.; 3];
        self.animation_velocity = [0.; 3];
        self.grounded = false;
        self.support_state = None;
        self.motion_ground = None;
        self.pending_ground_support = None;
        self.ground_vectors = None;
        self.jump_stage = None;
        self.ground_jump_started = false;
        self.air_first_jump_started = false;
        self.second_jump_started = false;
    }
    /// Feed device-space input once per simulation step, before camera conversion.
    /// Original runtime filter overrides and input-enable flags are still pending.
    pub fn filter_stick(&mut self, target: [f32; 2], dt: f32) -> [f32; 2] {
        self.try_filter_stick(target, dt)
            .expect("Native input filtering failed")
    }
    pub fn try_filter_stick(&mut self, target: [f32; 2], dt: f32) -> mlua::Result<[f32; 2]> {
        let magnitude = (target[0] * target[0] + target[1] * target[1]).sqrt();
        let target = if magnitude > 1.0 {
            target.map(|v| v / magnitude)
        } else {
            target
        };
        // REL 0x306df4..0x306e10 preserves raw +0x7c/+0x80 but
        // clears the filter target when action entry clears +0x214 bit 0.
        self.raw_stick = target;
        let target = if self.action_input_lock || self.motion_input_flags & 2 == 0 {
            [0.; 2]
        } else {
            target
        };
        self.filtered_stick = match self.motion.input_filter_type {
            1 => super::ai_controller_input::smooth(
                &super::ai_controller_input::Prepared {
                    stored: self.raw_stick,
                    effective: target,
                    smoothing_mode: 1,
                },
                self.filtered_stick,
                self.motion.input_linear_velocity,
                dt,
            )?,
            2 => super::ai_controller_input::polar(
                self.filtered_stick,
                target,
                self.motion.input_length_velocity,
                self.motion.input_rotation_velocity,
                dt,
            )?,
            _ => target,
        };
        Ok(self.filtered_stick)
    }
    pub fn raw_stick(&self) -> [f32; 2] {
        self.raw_stick
    }
    pub fn step(&mut self, world: &Collision, direction: V, jump: impl Into<JumpInput>, dt: f32) {
        // World-space probes use a fixed +Z reference, but share gameplay's
        // contact-frame solver rather than maintaining a second physics path.
        let direction = if length(direction) > 1. {
            unit(direction)
        } else {
            direction
        };
        self.step_input(world, [direction[0], -direction[2]], [0., 0., 1.], jump, dt);
    }
    /// Preserve controller-space values through the ground solve. Reconstructing
    /// them from a rounded world direction loses the original camera frame.
    pub fn step_input(
        &mut self,
        world: &Collision,
        filtered: [f32; 2],
        reference: V,
        jump: impl Into<JumpInput>,
        dt: f32,
    ) {
        self.prepare_input_step(world, filtered, reference, jump, dt);
        self.finish_input_step(world, dt);
    }
    /// Calculate this tick's commanded velocity from the previous support and
    /// action state, without translating the character. Shared physics belongs
    /// between this stage and finish_input_step.
    pub(super) fn prepare_input_step(
        &mut self,
        world: &Collision,
        filtered: [f32; 2],
        reference: V,
        jump: impl Into<JumpInput>,
        dt: f32,
    ) {
        self.pending_physics = None;
        let jump = jump.into();
        world.with_player_queries(|| {
            self.prepare_step_inner(
                world,
                super::input::movement_direction(filtered, reference),
                jump,
                dt,
                Some((filtered, reference)),
            )
        });
    }
    fn prepare_step_inner(
        &mut self,
        world: &Collision,
        direction: V,
        jump: JumpInput,
        dt: f32,
        input_frame: Option<([f32; 2], V)>,
    ) {
        self.ground_jump_started = false;
        self.air_first_jump_started = false;
        self.second_jump_started = false;
        self.damage_leap_landed = false;
        let mut jump = jump;
        if self.spin_gravity.is_some() || self.damage_leaping {
            jump.pressed = false;
            jump.held = false;
        }
        self.previous = self.position;
        if self.native_knockback || self.knockback_velocity_override {
            self.slope_gravity_force = 0.;
            self.stairs_ground_retained = false;
            self.knockback_velocity_override = false;
            self.jump_stage = None;
            self.pending_ground_support = None;
            self.support_carry = None;
            self.ground_vectors = None;
            self.animation_velocity = self.velocity;
            return;
        }
        let incoming_velocity = self.velocity;
        // World updates (painting, spawning, retirement, other movers) may
        // invalidate or replace the shared query planes after our last tick.
        // A physical character must resolve its own current support before
        // using those planes to remove inward velocity from its next command.
        let published_support = self.pending_ground_support.take();
        if world.player_body.is_some() && self.grounded {
            self.support_state = published_support.or_else(|| {
                self.query_support(world, dt)
            });
            self.grounded = self
                .support_state
                .is_some_and(|state| state != super::support::State::Unsupported);
        }
        let support_surface = if self.grounded {
            world.active_support_motion(false)
        } else {
            None
        };
        let ground_motion = self.grounded || self.stairs_ground_retained;
        //308a5c retains the last aggregate normal when support is0. The
        //stairs flag preserves motion0; it does not invent a new contact.
        let support_normal = support_surface
            .as_ref()
            .map(|surface| surface.normal)
            .or_else(|| {
                self.stairs_ground_retained
                    .then_some(self.motion_surface_normal)
                    .flatten()
                    .map(|v| [v[0], v[1], v[2]])
            })
            .and_then(|normal| self.planar_support_normal(normal));
        let support_velocity = support_surface
            .as_ref()
            .map(|surface| surface.velocity)
            .or_else(|| {
                self.stairs_ground_retained
                    .then_some(self.motion_ground.as_ref().and_then(|s| s.surface_velocity))
                    .flatten()
                    .map(|v| [v[0], v[1], v[2]])
            })
            .unwrap_or([0.; 3]);
        let walkable_support = ground_motion
            && support_normal
                .is_none_or(|normal| normal[1] >= self.motion.max_slope_degrees.to_radians().cos());
        // The recovered ground-frame calculation preserves its incoming
        // normal velocity. Havok supplies a contact-resolved velocity there;
        // the Box3D adapter can instead return residual inward landing speed
        // against the deliberately heavy character. Reissuing that speed on
        // every supported tick continuously crushes/pumps the supporting body.
        // Resolve only inward motion at this boundary, relative to the support;
        // keep tangential motion, outward motion and airborne/jump gravity.
        let mut ground_velocity = self.velocity;
        let carry = if world.player_body.is_some() && walkable_support {
            match world.supporting_body() {
                super::motion_ground::SupportBody::Body(shape)
                    if world
                        .shape_motion
                        .get(&shape)
                        .is_some_and(|motion| motion.motion_type == 4) =>
                {
                    // Compound collision parts share one admitted rigid body.
                    let body = world
                        .shape_groups
                        .get(&shape)
                        .and_then(|parts| parts.first())
                        .copied()
                        .unwrap_or(shape);
                    Some((body, support_velocity))
                }
                _ => None,
            }
        } else {
            None
        };
        if let (Some((body, current)), Some((previous_body, previous)), Some(normal)) =
            (carry, self.support_carry, support_normal)
        {
            if body == previous_body {
                // The recovered ground calculation preserves incoming normal
                // velocity relative to the surface. Box3D can retain the last
                // world-space upward speed when a kinematic platform slows,
                // progressively launching its rider out of the support probe.
                // Transport only the inherited normal component into this
                // body's new velocity frame; retain external relative impulses
                // and leave tangential ground acceleration/jump logic intact.
                let change = dot(sub(current, previous), normal) / dot(normal, normal);
                ground_velocity = add(ground_velocity, mul(normal, change));
            }
        }
        self.support_carry = carry;
        if world.player_body.is_some() && walkable_support {
            if let Some(normal) = support_normal {
                ground_velocity =
                    resolve_inward_support_velocity(ground_velocity, support_velocity, normal);
            }
        }
        let d = if length(direction) > 1.0 {
            unit(direction)
        } else {
            direction
        };
        let desired = if ground_motion && self.motion.ground_use_fixed_speeds {
            fixed_ground_velocity(
                [d[0], d[2]],
                self.max_move_speed(),
                &self.motion.ground_fixed_speeds,
                &self.motion.ground_stick_values,
            )
        } else {
            let speed = if ground_motion {
                self.max_move_speed()
            } else if self.damage_leaping {
                self.motion.damage_leap_air_speed
            } else {
                self.motion.air_speed
            };
            [d[0] * speed, d[2] * speed]
        };
        let mut frame_ground_velocity = None;
        let horizontal = if ground_motion {
            if let Some((stick, reference)) =
                input_frame.filter(|(_, r)| r[0] * r[0] + r[2] * r[2] >= f32::EPSILON)
            {
                let up = [0., 1., 0.];
                let basis = ground_support_basis(up, reference, support_normal.unwrap_or(up));
                let flat_basis = ground_support_basis(up, reference, up);
                // REL222474/222484 subtract supporting-body velocity before
                // transforming into the contact frame.
                let local = ground_to_local(basis, sub(ground_velocity, support_velocity));
                let target = if self.motion.ground_use_fixed_speeds {
                    fixed_ground_velocity(
                        [stick[1], stick[0]],
                        self.max_move_speed(),
                        &self.motion.ground_fixed_speeds,
                        &self.motion.ground_stick_values,
                    )
                } else {
                    [
                        stick[1] * self.max_move_speed(),
                        stick[0] * self.max_move_speed(),
                    ]
                };
                let (sin, cos) = self.yaw.sin_cos();
                let facing = ground_to_local(
                    flat_basis,
                    self.movement_direction_override.unwrap_or([sin, 0., cos]),
                );
                let solved = ground_frame_velocity(
                    local,
                    [target[0], target[1], 0.],
                    facing,
                    self.max_move_speed(),
                    self.motion.ground_acceleration,
                    self.turn_stopping_acceleration
                        .unwrap_or(self.motion.ground_stopping_acceleration),
                    dt,
                    self.maintains_ground_velocity(),
                    self.turn_stopping_acceleration.is_some(),
                );
                // Exported locomotion vectors below remain surface-relative;
                // REL222a64..222a80 adds support motion only to physical output.
                let velocity = ground_from_local(basis, solved, support_velocity);
                // REL+0x222784 bypasses rebuilding stack+0xb0 with the up
                // frame when maintaining velocity. Both exported vectors then
                // retain the contact frame, just like the physical output.
                let vector_basis = if self.maintains_ground_velocity() {
                    basis
                } else {
                    flat_basis
                };
                self.ground_vectors = Some(GroundVectors {
                    movement: ground_from_local(vector_basis, solved, [0.; 3]),
                    desired: ground_from_local(vector_basis, [target[0], target[1], 0.], [0.; 3]),
                });
                frame_ground_velocity = Some(velocity);
                [velocity[0], velocity[2]]
            } else {
                // DOL+0x2f7e84 initializes output from incoming velocity;
                // REL+0x222378 returns before changing output or exported
                // ground vectors when forward x up is below epsilon.
                frame_ground_velocity = Some(ground_velocity);
                [ground_velocity[0], ground_velocity[2]]
            }
        } else {
            let (accel, side_accel) = if self.damage_leaping {
                let accel = self.motion.damage_leap_acceleration;
                (accel, accel)
            } else {
                (self.motion.air_control_acceleration, self.motion.air_side_acceleration)
            };
            if let Some(direction) = self.movement_direction_override {
                air_velocity_step_with_direction(
                    [self.velocity[0], self.velocity[2]],
                    desired,
                    [direction[0], direction[2]],
                    accel,
                    side_accel,
                    dt,
                )
            } else {
                air_velocity_step(
                    [self.velocity[0], self.velocity[2]],
                    desired,
                    self.yaw,
                    accel,
                    side_accel,
                    dt,
                )
            }
        };
        if frame_ground_velocity.is_none() {
            self.ground_vectors = ground_motion.then_some(GroundVectors {
                movement: [horizontal[0], 0., horizontal[1]],
                desired: [desired[0], 0., desired[1]],
            });
        }
        self.velocity[0] = horizontal[0];
        self.velocity[2] = horizontal[1];
        if let Some(velocity) = frame_ground_velocity {
            self.velocity[1] = velocity[1];
        } else if walkable_support {
            // Recompute the vertical component when horizontal speed changes;
            // retaining yesterday's uphill velocity launches the character
            // away from the slope when stopping or reversing. Contact-normal
            // aggregation remains the static Box3D compatibility path.
            if let Some(normal) = support_normal {
                self.velocity[1] =
                    -(normal[0] * horizontal[0] + normal[2] * horizontal[1]) / normal[1];
            } else {
                self.velocity[1] = 0.;
            }
        }
        if !self.action_input_lock && self.brush_turn_rate.is_none() && length(d) > 0.01 {
            self.facing_target = d[0].atan2(d[2]);
        }
        if self.action_input_lock || self.brush_turn_rate.is_some() || length(d) > 0.01 {
            // Turn after computing movement using the incoming facing. Retail
            // also supports scripted direction/rate overrides and history routing.
            let rate = if !ground_motion {
                self.motion.air_turn_rate
            } else {
                // REL309fc8 reads locomotion history, which exports movement
                // before support velocity is added. Platform carry must not
                // select the moving turn rate for a stationary character.
                // Full native history normalization/scheduling remains pending.
                let movement = self
                    .ground_vectors
                    .map_or([horizontal[0], 0., horizontal[1]], |vectors| {
                        vectors.movement
                    });
                let speed_squared = movement[2] * movement[2] + movement[0] * movement[0];
                if speed_squared < 0.01 {
                    self.brush_turn_rate
                        .unwrap_or(self.motion.stationary_turn_rate * std::f32::consts::TAU)
                } else {
                    self.motion.moving_turn_rate * std::f32::consts::TAU
                }
            };
            self.yaw = facing_step(self.yaw, self.facing_target, rate, dt);
        }
        let mut entering_air_jump = false;
        if jump.pressed && walkable_support {
            self.stairs_ground_retained = false;
            self.ground_jump_started = true;
            self.ground_jump_available = false;
            self.active_jump_height = self.motion.jump_height;
            self.jump_stage = Some((self.position[1], 0));
            self.grounded = false;
        } else if jump.pressed && self.jump_stage.is_none() {
            // Production World publishes the source-derived motion record
            // before movement. Standalone mover fixtures retain their explicit
            // provisional query until they admit the complete World owner.
            let clearance = self
                .motion_ground
                .as_ref()
                .and_then(|state| state.distance)
                .unwrap_or_else(|| {
                    world.ground_distance(
                        self.position,
                        self.motion.max_relevant_ground_distance,
                        self.motion.max_slope_degrees.to_radians().cos(),
                    )
                });
            // REL+0x21f090: a first jump remains available after walking off
            // an edge; it is a flag, not an invented coyote-time duration.
            let first_jump =
                clearance < self.motion.max_distance_for_jump || self.ground_jump_available;
            let second_jump = self.double_jump_available
                && (clearance >= self.motion.double_jump_min_distance
                    || (self.position[1] - self.last_ground_y).abs()
                        >= self.motion.double_jump_min_distance);
            if first_jump || second_jump {
                self.air_first_jump_started = first_jump;
                self.second_jump_started = !first_jump;
                self.ground_jump_available = false;
                self.double_jump_available = first_jump;
                self.active_jump_height = if first_jump {
                    self.motion.jump_height
                } else {
                    self.motion.double_jump_height
                };
                self.jump_stage = Some((self.position[1], 0));
                // REL+0x22034c: enter with a velocity blend, then run the
                // first staged impulse on the following controller update.
                self.velocity = jump_entry_velocity(
                    incoming_velocity,
                    [desired[0], 0., desired[1]],
                    self.motion.immediate_jump_velocity_multiplier,
                );
                entering_air_jump = true;
            }
        }
        let impulse = self
            .jump_stage
            .filter(|_| !entering_air_jump)
            .and_then(|(origin, index)| {
                let multiplier = match self.motion.jump_type {
                    0 if index == 0 => Some(1.0),
                    2 if index == 0 || jump.held => {
                        self.motion.jump_height_multipliers.get(index).copied()
                    }
                    _ => None,
                }?;
                let remaining = self.active_jump_height * multiplier - (self.position[1] - origin);
                (remaining > 0.0).then(|| jump_launch_speed(self.motion.gravity_up, remaining))
            });
        if let Some(velocity) = impulse {
            self.velocity[1] = velocity;
            if let Some((_, index)) = &mut self.jump_stage {
                *index += 1;
            }
        } else if !entering_air_jump {
            // REL+0x220180 transitions to the air state without running its
            // gravity update in the same tick. Preserve the outgoing velocity.
            let leaving_jump_state = self.jump_stage.take().is_some();
            // Gravity belongs to the air state. Applying it during ground
            // movement forces the beveled authored hull down into stair edges,
            // cancelling the ground acceleration before it can climb them.
            // Ground support is tested separately with its downward probe.
            if !leaving_jump_state
                && !self.stairs_ground_retained
                && (!ground_motion || !walkable_support)
            {
                self.velocity[1] -= if self.damage_leaping {
                    -self.motion.damage_leap_gravity
                } else if self.velocity[1] > 0.0 {
                    self.spin_gravity.map_or(self.motion.gravity_up, |g| -g)
                } else {
                    self.spin_gravity.map_or(self.motion.gravity_down, |g| -g)
                } * dt;
            }
        }
        if ground_motion && self.jump_stage.is_none() {
            let output = super::stairs::reproject(
                [self.velocity[0], self.velocity[1], self.velocity[2], 0.],
                [0., 1., 0., 0.],
                support_normal.map_or([0., 1., 0., 0.], |n| [n[0], n[1], n[2], 0.]),
                [
                    support_velocity[0],
                    support_velocity[1],
                    support_velocity[2],
                    0.,
                ],
                false,
            );
            self.velocity.copy_from_slice(&output[..3]);
        }
        if ground_motion
            && self.jump_stage.is_none()
            && self.support_state == Some(super::support::State::Sliding)
        {
            let output = super::stairs::sliding_velocity(
                [self.velocity[0], self.velocity[1], self.velocity[2], 0.],
                [0., 1., 0., 0.],
                incoming_velocity[1],
                self.max_move_speed(),
                dt,
                &self.motion.stairs,
                &mut self.slope_gravity_force,
            );
            self.velocity.copy_from_slice(&output[..3]);
        } else if self.stairs_ground_retained && self.jump_stage.is_none() {
            self.slope_gravity_force = 0.;
            let output = super::stairs::above_stairs_velocity(
                [self.velocity[0], self.velocity[1], self.velocity[2], 0.],
                [0., 1., 0., 0.],
                incoming_velocity[1],
                self.motion.stairs.gravity,
                dt,
                true,
            );
            self.velocity.copy_from_slice(&output[..3]);
        } else {
            self.slope_gravity_force = 0.;
        }
        self.velocity[1] = self.velocity[1].max(self.motion.terminal_speed);
        self.animation_velocity = self.velocity;
    }
    /// Existing mover completion. Physical-body pose publication will replace
    /// this translation after admission; support and jump lifecycle remain
    /// downstream of the shared world boundary rather than presentation time.
    pub(super) fn finish_input_step(&mut self, world: &Collision, dt: f32) {
        world.with_player_queries(|| self.finish_step_inner(world, dt));
    }
    /// Consume once, immediately after the pre-movement World stage. Call
    /// only after that stage's collision mutations; never cache across ticks.
    pub(super) fn accept_ground_support(&mut self, support: super::support::State) {
        self.pending_ground_support = Some(support);
    }
    pub(super) fn published_support(&self) -> Option<super::support::State> {
        self.pending_ground_support.or(self.support_state)
    }
    ///802f6ae8..6b18, then308a5c..68. The aggregate callback is not invoked
    ///when unsupported, preserving all four lanes of the previous vector.
    pub(super) fn accept_motion_surface(
        &mut self,
        support: super::support::State,
        aggregate: impl FnOnce() -> anyhow::Result<[f32; 4]>,
    ) -> anyhow::Result<()> {
        if support != super::support::State::Unsupported {
            self.motion_surface_normal = Some(aggregate()?);
        }
        Ok(())
    }
    pub(super) fn accept_physics_pose(&mut self, pose: SimulationPose) {
        self.pending_physics = Some(pose);
    }
    fn finish_step_inner(&mut self, world: &Collision, dt: f32) {
        let physical = self.pending_physics.take();
        if let Some(pose) = physical {
            self.position = pose.position;
            self.velocity = pose.linear;
            self.grounded = false;
        } else {
            self.grounded = world.move_player(
                &mut self.position,
                &mut self.velocity,
                dt,
                self.motion.max_slope_degrees.to_radians().cos(),
            );
        }
        self.support_state = self.query_support(world, dt);
        // Retail ground state retains both sliding and supported results.
        // Keep the provisional jump lifecycle in charge while rising from a
        // deliberate jump; support beneath the hull must not cancel takeoff.
        if !self.ground_jump_started
            && !self.air_first_jump_started
            && !self.second_jump_started
            && self.jump_stage.is_none()
            && (self.ground_vectors.is_some() || self.velocity[1] <= 0.)
        {
            if let Some(support) = self.support_state {
                self.grounded = support != super::support::State::Unsupported;
                if physical.is_none() && self.grounded && self.ground_vectors.is_some() {
                    // The support query can see a surface below the hull before
                    // physical contact. Ground movement must close that gap;
                    // otherwise removing airborne gravity leaves it hovering.
                    if let Some((normal, offset)) = world.active_support_surface() {
                        let correction = offset / normal[1];
                        if self.support_normal_walkable(normal)
                            && correction < 0.
                            && correction >= -0.1
                        {
                            self.position[1] += correction;
                        }
                    }
                }
            }
        }
        // Sliding is contact, not a landing. The provisional lifecycle must
        // not replenish jumps against steep surfaces retained by support_at.
        let landed = self.grounded
            && self.support_state != Some(super::support::State::Sliding)
            && world.active_support_surface().is_none_or(|(normal, _)| {
                self.support_normal_walkable(normal)
            });
        if landed {
            self.damage_leap_landed = std::mem::take(&mut self.damage_leaping);
            self.jump_stage = None;
            self.double_jump_available = true;
            self.ground_jump_available = true;
            self.last_ground_y = self.position[1];
        }
        if (world.recovery_min_y.is_finite() && self.position[1] < world.recovery_min_y - 15.0)
            || !self.position.iter().all(|x| x.is_finite())
        {
            self.respawn()
        }
    }
}
impl Player {
    /// Implemented character-context subset: ground0, jumping1, airborne2.
    /// Other native states require their actual action/lifecycle integration.
    pub fn motion_state(&self) -> i32 {
        if self.native_knockback {
            10
        } else if self.damage_leaping {
            8
        } else if self.jump_stage.is_some() {
            1
        } else if self.grounded || self.stairs_ground_retained {
            0
        } else {
            2
        }
    }
}
/// Controlled collision-free gap check of the production movement bridge.
/// The admission flag is supplied; original support-query parity is separate.
#[cfg(any(test, debug_assertions))]
pub fn validate_stairs_context() -> anyhow::Result<()> {
    let collision = Collision::new(Vec::new())?;
    let mut player = Player::new([0., 2., 0.], Motion::default());
    player.motion.stairs = super::stairs::Settings {
        enabled: true,
        height: 0.3,
        gravity: -50.,
        ..Default::default()
    };
    player.stairs_ground_retained = true;
    player.motion_surface_normal = Some([0., 1., 0., 0.]);
    player.ground_jump_available = false;
    player.double_jump_available = false;
    player.step_input(&collision, [0., 1.], [0., 0., 1.], false, 1. / 30.);
    anyhow::ensure!(
        player.motion_state() == 0 && player.ground_vectors.is_some(),
        "Stairs lost ground motion context"
    );
    anyhow::ensure!(
        !player.grounded && !player.ground_jump_available && !player.double_jump_available,
        "Stairs fabricated physical support or replenished jumps"
    );
    anyhow::ensure!(
        player.position[1] < 2. && player.velocity[1] < 0.,
        "Stairs retained context without downward motion"
    );
    player.stairs_ground_retained = false;
    player.step_input(&collision, [0., 1.], [0., 0., 1.], false, 1. / 30.);
    anyhow::ensure!(
        player.motion_state() == 2 && player.ground_vectors.is_none(),
        "Expired stairs admission retained ground motion"
    );
    player.stairs_ground_retained = true;
    player.step_input(&collision, [0., 1.], [0., 0., 1.], true, 1. / 30.);
    anyhow::ensure!(
        player.ground_jump_started && !player.stairs_ground_retained && player.motion_state() == 1,
        "Jump failed to leave retained stairs context"
    );
    player.stairs_ground_retained = true;
    player.respawn();
    anyhow::ensure!(
        !player.stairs_ground_retained,
        "Respawn retained obsolete stairs admission"
    );
    // The native producer admits support1 as well as0. Do not combine its
    //ground-state above-stairs force with the provisional air/slope gravity.
    let mut sliding = Player::new([0., 2., 0.], Motion::default());
    sliding.motion.stairs.gravity = -50.;
    sliding.stairs_ground_retained = true;
    sliding.motion_surface_normal = Some([0.8, 0.6, 0., 0.]);
    sliding.prepare_input_step(&collision, [0.; 2], [0., 0., 1.], false, 1. / 30.);
    anyhow::ensure!(
        sliding.velocity[1].to_bits() == (-50f32 * (1f32 / 30.)).to_bits(),
        "Retained sliding support applied air gravity in addition to the native ground force"
    );
    let mut rising = Player::new([0., 2., 0.], Motion::default());
    rising.motion.stairs.gravity = -50.;
    rising.stairs_ground_retained = true;
    rising.motion_surface_normal = Some([0., 1., 0., 0.]);
    rising.velocity = [0., 2., 0.];
    rising.prepare_input_step(&collision, [0.; 2], [0., 0., 1.], false, 1. / 30.);
    anyhow::ensure!(
        rising.velocity[1].to_bits() == (-50f32).mul_add(1. / 30., 2.).to_bits(),
        "Ground wrapper doubled incoming upward velocity before adding stairs gravity"
    );
    Ok(())
}
/// Ordinary jump entry, REL+0x22034c..0x220380. Do not clamp this authored
/// configuration value: the original permits extrapolation as well as blending.
fn jump_entry_velocity(previous: V, desired: V, multiplier: f32) -> V {
    std::array::from_fn(|axis| {
        multiplier.mul_add(desired[axis], previous[axis] * (1.0 - multiplier))
    })
}
/// REL21fa88..21fb34, static Y-up gravity specialization. Preserve both
/// reciprocal-root/refinement stages and separate f32 multiply ordering.
/// Alternate launch direction and explicit state speed override remain pending.
fn jump_launch_speed(gravity: f32, remaining: f32) -> f32 {
    assert!(
        gravity.is_finite() && remaining.is_finite() && remaining > 0.,
        "Invalid jump launch input"
    );
    let gravity_squared = gravity * gravity;
    assert!(
        gravity_squared.is_finite(),
        "Jump gravity magnitude overflow"
    );
    // Zero input's retail fsel fallbacks produce zero launch speed.
    if gravity_squared == 0. {
        return 0.;
    }
    let magnitude = gravity_squared * ground_reciprocal_length(gravity_squared);
    let squared = (2. * magnitude) * remaining;
    assert!(squared.is_finite(), "Jump launch speed overflow");
    if squared == 0. {
        0.
    } else {
        squared * ground_reciprocal_length(squared)
    }
}
/// Static Y-up ground path, REL+0x222740..0x222a60. The default redirects
/// the acceleration-limited speed along the incoming facing. Maintaining
/// velocity bypasses redirection. The reverse-motion branch is available in
/// the helper below; sloped/moving support frames are not represented here yet.
fn ground_facing_velocity(
    limited: [f32; 2],
    facing: [f32; 2],
    maximum: f32,
    maintain_velocity: bool,
) -> [f32; 2] {
    ground_facing_velocity_with_reverse(limited, facing, maximum, maintain_velocity, [0.; 2], false)
}
fn ground_facing_velocity_with_reverse(
    limited: [f32; 2],
    facing: [f32; 2],
    maximum: f32,
    maintain_velocity: bool,
    desired: [f32; 2],
    allow_reverse: bool,
) -> [f32; 2] {
    let length = ground_length(limited);
    let limited = if length > maximum {
        let scale = maximum / length;
        limited.map(|x| x * scale)
    } else {
        limited
    };
    if maintain_velocity {
        limited
    } else {
        // Retail recomputes length after the cap, rather than selecting maximum.
        let speed = ground_length(limited);
        let speed = super::locomotion_actions::signed_ground_speed(
            speed,
            facing[0] * desired[0] + facing[1] * desired[1],
            allow_reverse,
        );
        facing.map(|x| x * speed)
    }
}
#[derive(Clone, Copy)]
pub struct JumpInput {
    pub pressed: bool,
    pub held: bool,
}
impl From<bool> for JumpInput {
    fn from(pressed: bool) -> Self {
        Self {
            pressed,
            held: pressed,
        }
    }
}
impl From<(bool, bool)> for JumpInput {
    fn from((pressed, held): (bool, bool)) -> Self {
        Self { pressed, held }
    }
}
/// Static, Y-up specialization of REL+0x221d50..0x222200. Surface velocity
/// remains unimplemented. The direction variant handles user-data+0x40.
fn air_velocity_step(
    current: [f32; 2],
    target: [f32; 2],
    yaw: f32,
    forward_acceleration: f32,
    side_acceleration: f32,
    dt: f32,
) -> [f32; 2] {
    let (sin, cos) = yaw.sin_cos();
    air_velocity_step_with_direction(
        current,
        target,
        [sin, cos],
        forward_acceleration,
        side_acceleration,
        dt,
    )
}
fn air_velocity_step_with_direction(
    current: [f32; 2],
    target: [f32; 2],
    forward: [f32; 2],
    forward_acceleration: f32,
    side_acceleration: f32,
    dt: f32,
) -> [f32; 2] {
    let side = [forward[1], -forward[0]];
    let delta = [target[0] - current[0], target[1] - current[1]];
    let along = (delta[0] * forward[0] + delta[1] * forward[1])
        .clamp(-forward_acceleration * dt, forward_acceleration * dt);
    let across = (delta[0] * side[0] + delta[1] * side[1])
        .clamp(-side_acceleration * dt, side_acceleration * dt);
    [
        current[0] + side[0] * across + forward[0] * along,
        current[1] + side[1] * across + forward[1] * along,
    ]
}
/// REL+0x297cd0 angle difference and +0x30a014..0x30a044 angular limiter.
/// The caller must supply the state-selected rate in radians per second.
fn facing_step(current: f32, target: f32, rate: f32, dt: f32) -> f32 {
    let yaw = current + facing_delta(current,target,rate,dt);
    // The original stores a direction vector; keep our scalar representation
    // canonical so later differences see the same atan2 range.
    yaw.sin().atan2(yaw.cos())
}
pub(super) fn facing_delta(current:f32,target:f32,rate:f32,dt:f32)->f32 {
    let tau = std::f32::consts::TAU;
    // Retail adds 4pi before fmod and maps only results strictly above pi.
    let mut delta = (((target - current) + 2.0 * tau) as f64 % tau as f64) as f32;
    if delta > std::f32::consts::PI {
        delta -= tau;
    }
    let maximum = rate * dt;
    delta.clamp(-maximum,maximum)
}
/// Mickey override at REL+0x99e7c..0x9a0e0. Despite its name, the fixed-speed
/// setting interpolates between table entries, beginning at (0,0).
fn fixed_ground_velocity(
    input: [f32; 2],
    maximum: f32,
    speeds: &[f32],
    thresholds: &[f32],
) -> [f32; 2] {
    let magnitude = (input[0] * input[0] + input[1] * input[1]).sqrt();
    let direction = if magnitude == 0.0 {
        input
    } else {
        input.map(|v| v / magnitude)
    };
    let (mut lower_threshold, mut lower_speed) = (0.0, 0.0);
    let (mut threshold, mut speed) = (1.0, 1.0);
    // Retail uses the shorter table and retains the last entry after exhaustion.
    for (&next_speed, &next_threshold) in speeds.iter().zip(thresholds) {
        threshold = next_threshold;
        speed = next_speed;
        if magnitude < threshold {
            break;
        }
        lower_threshold = threshold;
        lower_speed = speed;
    }
    let width = threshold - lower_threshold;
    let factor = if width == 0.0 {
        speed
    } else {
        ((magnitude - lower_threshold) / width).mul_add(speed - lower_speed, lower_speed)
    };
    let velocity = (factor * maximum).min(maximum);
    direction.map(|v| v * velocity)
}
// Keep the existing input-filter arithmetic separate from the verified ground
// state limiter. Its own instruction sequence needs independent validation.
fn linear_filter_step(current: [f32; 2], target: [f32; 2], rate: f32, dt: f32) -> [f32; 2] {
    let delta = [target[0] - current[0], target[1] - current[1]];
    let squared = delta[0] * delta[0] + delta[1] * delta[1];
    let maximum = rate * dt;
    if squared <= maximum * maximum {
        target
    } else {
        let scale = maximum / squared.sqrt();
        [current[0] + delta[0] * scale, current[1] + delta[1] * scale]
    }
}
/// Flat-ground specialization of the rate selection and vector limiter at
/// EM1 REL+0x2225f4..0x2226e4. Terrain basis is handled separately; keep
/// normalization and multiply/add boundaries in the retail instruction order.
fn ground_velocity_step(
    current: [f32; 2],
    target: [f32; 2],
    acceleration: f32,
    stopping_acceleration: f32,
    dt: f32,
) -> [f32; 2] {
    let squared = |v: [f32; 2]| v[0] * v[0] + v[1] * v[1];
    let rate = if squared(target) < squared(current) {
        stopping_acceleration
    } else {
        acceleration
    };
    let maximum_change = rate * dt;
    let delta = [target[0] - current[0], target[1] - current[1]];
    let distance_squared = squared(delta);
    let delta = if distance_squared > maximum_change * maximum_change {
        let reciprocal = ground_reciprocal_length(distance_squared);
        delta.map(|v| (v * reciprocal) * maximum_change)
    } else {
        delta
    };
    // Even the uncapped path adds the delta back; substituting target changes
    // cancellation rounding (REL+0x2226e4).
    [current[0] + delta[0], current[1] + delta[1]]
}
/// Local ground-state calculation at REL+0x2225f4..0x222a30.
/// Rates are already resolved from the original per-character properties.
fn ground_frame_velocity(
    current: V,
    target: V,
    facing: V,
    maximum: f32,
    acceleration: f32,
    stopping: f32,
    dt: f32,
    maintain: bool,
    reverse: bool,
) -> V {
    let magnitude = |v: V| {
        let squared = v[2].mul_add(v[2], v[0] * v[0]) + v[1] * v[1];
        if squared == 0. {
            0.
        } else {
            squared * ground_reciprocal_length(squared)
        }
    };
    let change = if magnitude(target) < magnitude(current) {
        stopping * dt
    } else {
        acceleration * dt
    };
    let delta = [target[0] - current[0], target[1] - current[1]];
    let squared = delta[0] * delta[0] + delta[1] * delta[1];
    let capped = squared > change * change;
    let delta = if capped {
        let reciprocal = ground_reciprocal_length(squared);
        delta.map(|v| (v * reciprocal) * change)
    } else {
        delta
    };
    // Retail zeroes the vertical delta for the uncapped path, but restores
    // the original vertical delta after normalizing a capped horizontal step.
    let mut result = [
        current[0] + delta[0],
        current[1] + delta[1],
        if capped {
            current[2] + (target[2] - current[2])
        } else {
            current[2]
        },
    ];
    let speed = ground_length([result[0], result[1]]);
    if speed > maximum {
        let scale = maximum / speed;
        result[0] *= scale;
        result[1] *= scale;
    }
    if !maintain {
        let mut speed = ground_length([result[0], result[1]]);
        let dot = facing[2].mul_add(target[2], facing[0] * target[0]) + facing[1] * target[1];
        if reverse && dot < 0. {
            speed *= -1.;
        }
        result[0] = speed * facing[0];
        result[1] = speed * facing[1];
    }
    result
}
/// Retail contact frame, REL+0x222304..0x2224e0. Columns are backward,
/// right, and the supplied support normal (which is not normalized again).
fn ground_support_basis(up: V, forward: V, normal: V) -> [V; 3] {
    let cross = |a: V, b: V| {
        [
            a[1].mul_add(b[2], -(b[1] * a[2])),
            -a[0].mul_add(b[2], -(b[0] * a[2])),
            -a[1].mul_add(b[0], -(b[1] * a[0])),
        ]
    };
    let normalize = |v: V| {
        let squared = v[2].mul_add(v[2], v[0] * v[0]) + v[1] * v[1];
        let reciprocal = ground_reciprocal_length(squared);
        v.map(|x| x * reciprocal)
    };
    let side = normalize(cross(forward, up));
    let backward = normalize(cross(side, normal));
    let right = normalize(cross(backward, normal));
    [backward, right, normal]
}
/// Box3D boundary correction, not a recovered ground-controller operation.
/// Native reciprocal-root normalization does not guarantee exactly unit length.
/// Project using n.n so its rounding error cannot reissue inward motion.
fn resolve_inward_support_velocity(velocity: V, surface_velocity: V, normal: V) -> V {
    let inward = dot(sub(velocity, surface_velocity), normal);
    if inward < 0. {
        sub(velocity, mul(normal, inward / dot(normal, normal)))
    } else {
        velocity
    }
}
fn ground_to_local(basis: [V; 3], velocity: V) -> V {
    basis.map(|axis| {
        axis[2].mul_add(
            velocity[2],
            axis[1].mul_add(velocity[1], axis[0] * velocity[0]),
        )
    })
}
/// DOL 0x8014f070, then REL+0x222a64..0x222a80. Translation from the
/// moving support is added after the three-column transform, not fused into it.
fn ground_from_local(basis: [V; 3], velocity: V, surface_velocity: V) -> V {
    std::array::from_fn(|i| {
        basis[2][i].mul_add(
            velocity[2],
            basis[1][i].mul_add(velocity[1], basis[0][i] * velocity[0]),
        ) + surface_velocity[i]
    })
}
/// REL+0x222680..0x22269c: frsqrte, frsp, one Newton refinement.
/// Called only for positive finite squared lengths from controller vectors.
fn ground_reciprocal_length(squared: f32) -> f32 {
    let estimate = super::pose_blend::estimate(squared);
    let correction = (-squared).mul_add(estimate * estimate, 1.);
    correction.mul_add(0.5 * estimate, estimate)
}
fn ground_length(v: [f32; 2]) -> f32 {
    let squared = v[0] * v[0] + v[1] * v[1];
    if squared == 0. {
        0.
    } else {
        squared * ground_reciprocal_length(squared)
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn retired_shape_drops_filter_without_blocking_live_solver_shapes() -> anyhow::Result<()> {
        let mut world = Collision::new(vec![])?;
        let retired = world.add_static_convex(&test_hull().vertices, 0.01, 88)?;
        let live = world.add_static_convex(&test_hull().vertices, 0.01, 89)?;
        let filter_word = 3 | 0x18000;
        world.set_simulation_filter(retired, filter_word)?;
        world.set_simulation_filter(live, filter_word)?;
        world.retire_owner(88)?;

        assert_eq!(world.simulation_filter(retired), None);
        assert!(!world.owner_shapes(88).any(|candidate| candidate == retired));
        assert_eq!(
            unsafe { bw_collision_shape_is_live(world.handle.as_ptr(), retired as i32) },
            0
        );
        world.step_simulation(
            1. / 30.,
            [0.; 3],
            &super::super::physics_filter::GameFilter::default(),
        )?;
        Ok(())
    }

    #[test]
    fn body_bounds_overlap_preserves_compound_identity_and_empty_gaps() -> anyhow::Result<()> {
        let mut world = Collision::new(vec![])?;
        let cube = |x: f32| {
            test_hull()
                .vertices
                .into_iter()
                .map(|mut p| {
                    p[0] += x;
                    p
                })
                .collect::<Vec<_>>()
        };
        let a = world.add_static_convex(&cube(-4.), 0.05, 88)?;
        let b = world.add_static_convex(&cube(4.), 0.05, 88)?;
        let c = world.add_static_convex(&cube(10.), 0.05, 88)?;
        world.group_body_shapes(&[a, b])?;
        assert_eq!(world.query_body_identity(a)?, world.query_body_identity(b)?);
        assert_ne!(world.query_body_identity(a)?, world.query_body_identity(c)?);
        let bounds = ([-0.1, 0.4, -0.1], [0.1, 0.6, 0.1]);
        let hits = world.overlap_body_bounds(bounds.0, bounds.1)?;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].shape, a);
        world.set_owner_solid(88, false)?;
        assert_eq!(world.overlap_body_bounds(bounds.0, bounds.1)?.len(), 1);
        world.set_owner_in_world(88, false)?;
        assert!(world.overlap_body_bounds(bounds.0, bounds.1)?.is_empty());
        world.set_owner_in_world(88, true)?;
        world.retire_owner(88)?;
        assert!(world.overlap_body_bounds(bounds.0, bounds.1)?.is_empty());
        for shape in [a, b, c] {
            assert_ne!(
                unsafe { bw_collision_shape_payload_released(world.handle.as_ptr(), shape as i32) },
                0,
                "retired grouped collider retained its native mesh/hull payload"
            );
        }
        Ok(())
    }
    #[test]
    fn live_filter_refresh_preserves_valid_pairs_and_recreates_stationary_pairs(
    ) -> anyhow::Result<()> {
        let mut world = Collision::new(vec![])?;
        let a = world.add_static_capsule([[0., 0.5, 0.], [0., 1.5, 0.]], 0.5, 10)?;
        let b = world.add_static_capsule([[0.8, 0.5, 0.], [0.8, 1.5, 0.]], 0.5, 11)?;
        world.set_native_body_mass(&[a], 0.02, [0.; 3], [0.; 3])?;
        let mut motion = super::super::native_motion_record::MotionRecord([0; 0x120]);
        motion.0[8] = 7;
        motion.0[0xbc] = 255;
        motion.0[0xbd] = 255;
        world.set_simulation_body(
            &[a],
            &motion,
            super::super::body_material::Coefficients {
                friction: 0.,
                restitution: 0.,
            },
            false,
        )?;
        let info = |layer: u32| layer | 0x18000u32 | (1u32 << 17);
        world.set_simulation_filter(a, info(2))?;
        world.set_simulation_filter(b, 3 | 0x18000 | (2 << 17))?;
        let mut filter = super::super::physics_filter::GameFilter::default();
        filter.matrix.rows = [0; 32];
        filter.matrix.rows[2] = 1 << 3;
        filter.matrix.rows[3] = (1 << 2) | (1 << 5);
        filter.matrix.rows[5] = 1 << 3;
        let before = world.simulation_pose(a)?;
        assert_eq!(world.refresh_simulation_filter(a, &filter)?, 0);
        let pairs = world.raw_pair_ids(a)?;
        assert_eq!(
            pairs.len(),
            1,
            "Refilter did not create an overlapping pair before integration"
        );
        world.set_simulation_filter(a, info(5))?;
        assert_eq!(
            world.refresh_simulation_filter(a, &filter)?,
            0,
            "Still-valid contact was destroyed"
        );
        assert_eq!(world.raw_pair_ids(a)?, pairs);
        world.set_simulation_filter(a, info(4))?;
        assert_eq!(world.refresh_simulation_filter(a, &filter)?, 1);
        assert!(
            world.raw_pair_ids(a)?.is_empty(),
            "Rejected contact survived the immediate refresh"
        );
        world.set_simulation_filter(a, info(2))?;
        assert_eq!(world.refresh_simulation_filter(a, &filter)?, 0);
        assert_eq!(
            world.raw_pair_ids(a)?.len(),
            1,
            "Stationary filtered pair was never recreated"
        );
        let after = world.simulation_pose(a)?;
        assert_eq!(before.position, after.position);
        assert_eq!(before.orientation, after.orientation);
        assert_eq!(before.linear, after.linear);
        assert_eq!(before.angular, after.angular);
        // Contacts created outside Step must be consumable by the normal
        // solver; an immediate refresh must not leave stack/task state stale.
        world.step_simulation(1. / 30., [0.; 3], &filter)?;
        assert!(!world.raw_pair_ids(a)?.is_empty());
        world.set_owner_in_world(10, false)?;
        assert!(
            world.refresh_simulation_filter(a, &filter).is_err(),
            "A detached body must not be treated as an installed refresh"
        );
        Ok(())
    }
    #[test]
    fn analytic_capsule_queries_preserve_curvature_radius_pose_and_membership() -> anyhow::Result<()>
    {
        let mut world = Collision::new(vec![])?;
        let shape = world.add_static_capsule([[0., 0.5, 0.], [0., 1.5, 0.]], 0.5, 88)?;
        assert_eq!(world.ray_hits([-2., 1., 0.], [4., 0., 0.])?.len(), 1);
        let hits = world.ray_hits([-2., 1., 0.], [4., 0., 0.])?;
        let hit = &hits[0];
        assert!((hit.fraction - 0.375).abs() < 1e-5);
        assert!(hit.source_triangle.is_none());
        // At the upper cap's diagonal a capsule differs from its AABB.
        assert!(world
            .overlap_sphere([0.3, 1.8, 0.], 0.01)?
            .iter()
            .any(|h| h.shape == shape));
        assert!(world.overlap_sphere([0.49, 1.99, 0.], 0.01)?.is_empty());
        let hit = world
            .sweep_sphere([-2., 1., 0.], [4., 0., 0.], 0.1)?
            .unwrap();
        assert!(
            (hit.fraction - 0.35).abs() < 0.002,
            "Capsule radius applied twice: {hit:?}"
        );
        let hits = world.sphere_hits([-2., 1., 0.], [4., 0., 0.], 0.1)?;
        assert_eq!(hits.len(), 1);
        assert!((hits[0].fraction - 0.35).abs() < 0.002);
        world.set_player_hull(&test_hull())?;
        assert!(world.shape_penetration(shape, [0., 0., 0.])? > 0.);
        assert_eq!(world.shape_penetration(shape, [3., 0., 0.])?, 0.);
        assert!(!world.shape_penetrations(shape, [0., 0., 0.])?.is_empty());
        let half = std::f32::consts::FRAC_1_SQRT_2;
        world.set_owner_transform(88, [5., 3., 0.], [0., 0., half, half])?;
        assert!(world
            .ray_hits([4., 3., -2.], [0., 0., 4.])?
            .iter()
            .any(|h| h.owner == 88));
        assert!((world.shape_min_y[shape] - 2.5).abs() < 1e-5);
        world.set_owner_in_world(88, false)?;
        assert!(world.ray_hits([4., 3., -2.], [0., 0., 4.])?.is_empty());
        world.set_owner_in_world(88, true)?;
        assert!(!world.ray_hits([4., 3., -2.], [0., 0., 4.])?.is_empty());
        world.retire_owner(88)?;
        assert!(world.ray_hits([4., 3., -2.], [0., 0., 4.])?.is_empty());
        Ok(())
    }
    #[test]
    fn material_overlap_preserves_triangles_curvature_margins_and_live_pose() -> anyhow::Result<()>
    {
        let triangles = [2., 2.2]
            .map(|h| Triangle {
                p: [[-2., h, -2.], [0., h, 2.], [2., h, -2.]],
            })
            .to_vec();
        let mut world = Collision::with_triangle_owners(triangles, vec![0.; 2], vec![88; 2])?;
        world.set_owner_solid(88, false)?;
        let keys = |hits: Vec<SphereCandidate>| {
            let mut keys = hits
                .into_iter()
                .map(|h| h.source_triangle.unwrap())
                .collect::<Vec<_>>();
            keys.sort_unstable();
            keys
        };
        assert_eq!(
            keys(world.overlap_sphere_contacts([0., 2.1, 0.], 0.2)?),
            vec![0, 1]
        );
        let half = std::f32::consts::FRAC_1_SQRT_2;
        world.set_owner_transform(88, [3., 1., -2.], [0., 0., half, half])?;
        assert_eq!(
            keys(world.overlap_sphere_contacts([0.9, 1., -2.], 0.2)?),
            vec![0, 1]
        );
        assert!(world
            .overlap_sphere_contacts([0., 2.1, 0.], 0.2)?
            .is_empty());
        world.set_owner_in_world(88, false)?;
        assert!(world
            .overlap_sphere_contacts([0.9, 1., -2.], 0.2)?
            .is_empty());
        let mut world = Collision::new(vec![])?;
        let capsule = world.add_static_capsule([[0., 0.5, 0.], [0., 1.5, 0.]], 0.5, 88)?;
        assert!(
            world
                .overlap_sphere_contacts([0.49, 1.99, 0.], 0.05)?
                .is_empty(),
            "Capsule must not become its AABB"
        );
        let contacts = world.overlap_sphere_contacts([0.3, 1.8, 0.], 0.01)?;
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].shape, capsule);
        assert!(contacts[0].source_triangle.is_none());
        world.set_owner_in_world(88, false)?;
        let points = [-0.5, 0.5]
            .into_iter()
            .flat_map(|x| {
                [-0.5, 0.5]
                    .into_iter()
                    .flat_map(move |y| [-0.5, 0.5].map(move |z| [x, y, z]))
            })
            .collect::<Vec<_>>();
        let hull = world.add_static_convex(&points, 0.1, 99)?;
        assert!(
            world
                .overlap_sphere_contacts([0.65, 0., 0.], 0.01)?
                .is_empty(),
            "Convex radius must be applied once"
        );
        let contacts = world.overlap_sphere_contacts([0.65, 0., 0.], 0.07)?;
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].shape, hull);
        assert!(contacts[0].source_triangle.is_none());
        Ok(())
    }
    #[test]
    fn analytic_capsule_character_simulation_and_compound_reparent_keep_mass() -> anyhow::Result<()>
    {
        let mut world = Collision::new(vec![])?;
        let shape = world.add_static_capsule([[0., 0.55, 0.], [0., 0.95, 0.]], 0.5, 88)?;
        let part = world.add_static_capsule([[2., 0.55, 0.], [2., 0.95, 0.]], 0.5, 88)?;
        world.group_body_shapes(&[shape, part])?;
        world.set_native_body_mass(&[shape, part], 0.02, [0.; 3], [0.; 3])?;
        let mut record = super::super::native_motion_record::MotionRecord([0; 0x120]);
        record.0[8] = 7;
        record.0[0xbc] = 255;
        record.0[0xbd] = 255;
        record.set_linear_velocity([3., 0., 0., 0.]);
        world.set_simulation_body(
            &[shape, part],
            &record,
            super::super::body_material::Coefficients {
                friction: 0.,
                restitution: 0.,
            },
            true,
        )?;
        for s in [shape, part] {
            world.set_simulation_filter(s, 20 | 0x18000)?;
        }
        let filter = super::super::physics_filter::GameFilter {
            matrix: super::super::physics_filter::Matrix::retail(),
            ..Default::default()
        };
        world.step_simulation(1. / 30., [0., -9.8, 0.], &filter)?;
        let pose = world.simulation_pose(shape)?;
        assert!((pose.position[0] - 0.1).abs() < 1e-5);
        assert_eq!(pose.linear, [3., 0., 0.]);
        assert_eq!(pose.position[1], 0., "Controller gravity was added twice");
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        let mut values = [0.; 4];
        assert_eq!(
            unsafe {
                bw_collision_native_mass_values(
                    world.handle.as_ptr(),
                    shape as i32,
                    values.as_mut_ptr(),
                )
            },
            1
        );
        assert_eq!(values, [0.02, 0., 0., 0.]);
        Ok(())
    }
    #[test]
    fn native_motion_probe_caches_raised_ray_clearance_and_changes_air_jump_admission(
    ) -> anyhow::Result<()> {
        let triangles = [0., 1.25]
            .map(|h| Triangle {
                p: [[-2., h, -2.], [0., h, 2.], [2., h, -2.]],
            })
            .to_vec();
        let world = Collision::with_triangle_owners(triangles, vec![0.; 2], vec![88; 2])?;
        let input = super::super::motion_ground::Input {
            position: [0., 1.1, 0., 0.],
            radius: 0.4,
            maximum: 5.,
            slope_cosine: 0.75,
            enabled: true,
            support: 0,
            supporting_body: super::super::motion_ground::SupportBody::Unknown,
        };
        let mut state = super::super::motion_ground::State::default();
        world.update_motion_ground(&mut state, input)?;
        let point = state
            .last_hit
            .ok_or_else(|| anyhow::anyhow!("Raised query missed the overhead tread"))?;
        // Box3D intersection points are unverified against Havok. Allow two
        // f32 epsilons in this controlled geometry test, not reference parity.
        assert!((point[1] - 1.25).abs() <= 2. * f32::EPSILON);
        assert!((state.distance.unwrap() - (1.1 - 1.25)).abs() <= 2. * f32::EPSILON);
        assert_eq!(
            state.surface_velocity, None,
            "Missing static-body metadata became a guessed zero velocity"
        );
        assert!(
            world.ground_distance([0., 1.1, 0.], 5., 0.75) > 1.,
            "Legacy ray unexpectedly sees above its origin"
        );
        let motion = Motion {
            max_distance_for_jump: 0.,
            ground_probe_start_offset: 0.4,
            ..Default::default()
        };
        let mut baseline = Player::new([0., 1.1, 0.], motion.clone());
        baseline.prepare_input_step(
            &world,
            [0.; 2],
            [0., 0., 1.],
            JumpInput {
                pressed: true,
                held: true,
            },
            super::super::STEP,
        );
        assert!(!baseline.air_first_jump_started);
        let mut player = Player::new([0., 1.1, 0.], motion);
        player.motion_ground = Some(state);
        player.prepare_input_step(
            &world,
            [0.; 2],
            [0., 0., 1.],
            JumpInput {
                pressed: true,
                held: true,
            },
            super::super::STEP,
        );
        assert!(
            player.air_first_jump_started,
            "Published native clearance never reached the actual jump gate"
        );
        assert!(player.jump_stage.is_some());
        Ok(())
    }
    #[test]
    fn large_retail_hull_capacity_preserves_vertices_and_face_indices() -> anyhow::Result<()> {
        // A sphere cloud gives >256 faces, exercising the widened face indices
        // as well as the old 128-vertex admission limit. No point reduction.
        let count = 142;
        let points = (0..count)
            .map(|i| {
                let y = 1. - 2. * (i as f32 + 0.5) / count as f32;
                let radius = (1. - y * y).sqrt();
                let angle = i as f32 * 2.3999631;
                [radius * angle.cos(), y, radius * angle.sin()]
            })
            .collect::<Vec<_>>();
        let mut world = Collision::new(vec![])?;
        let shape = world.add_static_convex(&points, 0., 90)?;
        {
            let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
            assert_eq!(
                unsafe { bw_collision_hull_vertex_count(world.handle.as_ptr(), shape as i32) },
                count
            );
        }
        // Rays through every extremal point traverse the constructed topology.
        for point in points {
            let origin = mul(point, 3.);
            let direction = mul(point, -1.);
            let hit = world
                .ray(origin, direction, 5.)
                .ok_or_else(|| anyhow::anyhow!("Missing large hull ray"))?;
            assert!((hit - 2.).abs() < 0.002, "Hull ray distance {hit}");
        }
        Ok(())
    }
    #[test]
    fn havok_rotation_projects_to_box3d_without_rejecting_authored_roundoff() -> anyhow::Result<()>
    {
        // Captured failure while loading MJM_ZoneA: Havok's rotation is valid
        // for the game but its squared length exceeds Box3D's tolerance.
        let authored = [2.1839874e-8, 0.9113271, 5.7830896e-8, 0.41168934];
        let projected = super::query_rotation(authored)?;
        assert!((projected.iter().map(|v| v * v).sum::<f32>() - 1.).abs() < 20. * f32::EPSILON);
        let mut collision = super::Collision::new(vec![])?;
        let shape = collision.add_static_convex(
            &[[-1., -1., -1.], [1., -1., -1.], [0., 1., -1.], [0., 0., 1.]],
            0.,
            5,
        )?;
        collision.set_shape_transform(shape, [1., 2., 3.], authored)?;
        collision.set_owner_transform(5, [1., 2., 3.], authored)?;
        assert!(super::query_rotation([0.; 4]).is_err());
        assert!(super::query_rotation([f32::NAN, 0., 0., 1.]).is_err());
        Ok(())
    }
    use super::*;
    #[test]
    fn inward_support_projection_does_not_assume_an_exact_unit_normal() {
        let native_unit = f32::from_bits(0x3f7fffff);
        for y in [native_unit, 1., 2.] {
            let incoming = [2., -0.25 * y, 3.];
            let normal = [0., y, 0.];
            assert_eq!(
                resolve_inward_support_velocity(incoming, [0.; 3], normal),
                [2., 0., 3.]
            );
            let outgoing = [2., 0.25, 3.];
            assert_eq!(
                resolve_inward_support_velocity(outgoing, [0.; 3], normal),
                outgoing
            );
        }
    }
    #[test]
    fn motion_surface_retention_and_lazy_aggregation_match_original() -> anyhow::Result<()> {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tools/blotworx/fixtures/motion_surface.json"
        ))?;
        let floats = |value: &serde_json::Value| {
            std::array::from_fn(|i| f32::from_bits(value[i].as_u64().unwrap() as u32))
        };
        for case in fixture["retention"].as_array().unwrap() {
            let mut player = Player::new([0.; 3], Motion::default());
            player.motion_surface_normal = Some(floats(&case["previous_bits"]));
            let support = match case["state"].as_u64().unwrap() {
                0 => super::super::support::State::Unsupported,
                1 => super::super::support::State::Sliding,
                2 => super::super::support::State::Supported,
                _ => unreachable!(),
            };
            let mut calls = 0;
            let result = player.accept_motion_surface(support, || {
                calls += 1;
                anyhow::ensure!(
                    !case["fail"].as_bool().unwrap(),
                    "supplied aggregate failure"
                );
                Ok(floats(&case["surface_bits"]))
            });
            assert_eq!(result.is_err(), !case["error"].is_null());
            assert_eq!(
                calls,
                case["events"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|v| v.as_str() == Some("aggregate"))
                    .count()
            );
            assert_eq!(
                serde_json::json!(player.motion_surface_normal.unwrap().map(f32::to_bits)),
                case["normal_bits"]
            );
        }
        let mut player = Player::new([0.; 3], Motion::default());
        player.accept_motion_surface(super::super::support::State::Unsupported, || {
            panic!("Unsupported owner aggregated a surface")
        })?;
        assert!(
            player.motion_surface_normal.is_none(),
            "Fresh unknown normal was fabricated as zero/up"
        );
        Ok(())
    }
    #[test]
    fn sphere_game_query_uses_shared_native_filter_overrides_and_high_byte() -> anyhow::Result<()> {
        use super::super::physics_filter::{GameFilter, QueryFlags};
        let triangles = [0., 2.]
            .map(|h| Triangle {
                p: [[-2., h, -2.], [0., h, 2.], [2., h, -2.]],
            })
            .to_vec();
        let world = Collision::with_triangle_owners(triangles, vec![0.; 2], vec![88; 2])?;
        let mut filter = GameFilter::default();
        filter.mode_30 = 7;
        filter.excluded_group_38 = 9;
        let flags = Some(QueryFlags {
            bytes: [1, 1, 1, 0, 1],
            excluded_group: 0,
        });
        let hits = world.sphere_hits_game_query(
            [0., 4., 0.],
            [0., -6., 0.],
            0.1,
            &mut filter,
            flags,
            |shared, c| {
                assert_eq!(
                    (
                        shared.mode_30,
                        shared.mode_31,
                        shared.flag_32,
                        shared.flag_33,
                        shared.flag_34
                    ),
                    (1, 1, 1, 0, 1)
                );
                assert_eq!(shared.excluded_group_38, 0);
                assert_eq!(c.owner, 88);
                let body = match c.source_triangle {
                    Some(0) => 0x18001,
                    Some(1) => 0x18003,
                    _ => anyhow::bail!("Missing test body"),
                };
                Ok(shared.collision_info(0x602003, body))
            },
        )?;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source_triangle, Some(0));
        assert_eq!(
            (
                filter.mode_30,
                filter.mode_31,
                filter.flag_34,
                filter.excluded_group_38
            ),
            (0, 1, 1, 0)
        );
        assert!(world
            .sphere_hits_game_query(
                [0., 4., 0.],
                [0., -6., 0.],
                0.1,
                &mut filter,
                None,
                |_, _| Ok(1)
            )?
            .is_empty());
        assert!(world
            .sphere_hits_game_query(
                [0., 4., 0.],
                [0., -6., 0.],
                0.1,
                &mut filter,
                flags,
                |_, _| anyhow::bail!("Native body missing")
            )
            .is_err());
        assert_eq!(filter.mode_30, 0);
        assert_eq!(filter.mode_31, 1);
        Ok(())
    }
    #[test]
    fn sphere_filtered_hits_keep_farther_accepted_triangle_and_propagate_policy_error(
    ) -> anyhow::Result<()> {
        let triangles = [0., 2.]
            .map(|h| Triangle {
                p: [[-2., h, -2.], [0., h, 2.], [2., h, -2.]],
            })
            .to_vec();
        let world = Collision::with_triangle_owners(triangles, vec![0.; 2], vec![88; 2])?;
        let mut visited = vec![];
        let hits = world.sphere_hits_filtered([0., 4., 0.], [0., -6., 0.], 0.1, |c| {
            assert_eq!(c.owner, 88);
            visited.push(c.source_triangle);
            Ok(c.source_triangle == Some(0))
        })?;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source_triangle, Some(0));
        assert!(visited.contains(&Some(0)) && visited.contains(&Some(1)));
        assert!(world
            .sphere_hits_filtered([0., 4., 0.], [0., -6., 0.], 0.1, |_| Ok(false))?
            .is_empty());
        let error = world
            .sphere_hits_filtered([0., 4., 0.], [0., -6., 0.], 0.1, |_| {
                anyhow::bail!("Filter body missing")
            })
            .unwrap_err();
        assert!(error.to_string().contains("Filter body missing"));
        Ok(())
    }
    #[test]
    fn sphere_all_hits_keeps_multiple_faces_non_solid_geometry_and_live_transform(
    ) -> anyhow::Result<()> {
        let triangles = [0., 2.]
            .map(|h| Triangle {
                p: [[-2., h, -2.], [0., h, 2.], [2., h, -2.]],
            })
            .to_vec();
        let mut world = Collision::with_triangle_owners(triangles, vec![0.; 2], vec![88; 2])?;
        let cast = |w: &Collision, x| w.sphere_hits([x, 4., 0.], [0., -6., 0.], 0.1);
        let hits = cast(&world, 0.)?;
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|h| h.owner == 88 && h.normal[1] > 0.99));
        let mut sources = hits
            .iter()
            .map(|h| h.source_triangle.unwrap())
            .collect::<Vec<_>>();
        sources.sort();
        assert_eq!(sources, vec![0, 1]);
        assert_ne!(hits[0].fraction, hits[1].fraction);
        world.set_owner_solid(88, false)?;
        assert_eq!(cast(&world, 0.)?.len(), 2);
        world.set_owner_transform(88, [20., 0., 0.], [0., 0., 0., 1.])?;
        assert!(cast(&world, 0.)?.is_empty());
        assert_eq!(cast(&world, 20.)?.len(), 2);
        assert!(world.sphere_hits([0.; 3], [0.; 3], -1.).is_err());
        assert!(world.sphere_hits([f32::NAN, 0., 0.], [0.; 3], 0.1).is_err());
        Ok(())
    }
    #[test]
    fn broadphase_overlap_keeps_compound_shapes_non_solid_owners_and_live_poses(
    ) -> anyhow::Result<()> {
        let mut world = floor();
        let hull = test_hull();
        let a = world.add_static_convex(&hull.vertices, 0.05, 88)?;
        let b = world.add_static_convex(&hull.vertices, 0.05, 88)?;
        let hits = world.overlap_candidates([-2.; 3], [2.; 3])?;
        assert!(hits.contains(&OverlapCandidate {
            shape: a,
            owner: 88
        }));
        assert!(hits.contains(&OverlapCandidate {
            shape: b,
            owner: 88
        }));
        world.set_owner_solid(88, false)?;
        let erased = world.overlap_candidates([-2.; 3], [2.; 3])?;
        assert!(erased.contains(&OverlapCandidate {
            shape: a,
            owner: 88
        }));
        assert!(erased.contains(&OverlapCandidate {
            shape: b,
            owner: 88
        }));
        world.set_owner_transform(88, [20., 0., 0.], [0., 0., 0., 1.])?;
        assert!(world
            .overlap_candidates([-2.; 3], [2.; 3])?
            .iter()
            .all(|h| h.owner != 88));
        let moved = world.overlap_candidates([18., -2., -2.], [22., 2., 2.])?;
        assert!(moved.contains(&OverlapCandidate {
            shape: a,
            owner: 88
        }));
        assert!(moved.contains(&OverlapCandidate {
            shape: b,
            owner: 88
        }));
        assert!(world.overlap_candidates([1000.; 3], [1001.; 3])?.is_empty());
        assert!(world
            .overlap_candidates([f32::NAN, 0., 0.], [1.; 3])
            .is_err());
        assert!(world.overlap_candidates([1.; 3], [0.; 3]).is_err());
        Ok(())
    }
    #[test]
    fn world_membership_removes_all_queries_and_preserves_paint_on_reentry() -> anyhow::Result<()> {
        let mut world = floor();
        let hull = test_hull();
        world.set_player_hull(&hull)?;
        let a = world.add_static_convex(&hull.vertices, 0.05, 88)?;
        let b = world.add_static_convex(&hull.vertices, 0.05, 88)?;
        world.set_owner_in_world(88, false)?;
        assert!(
            world.group_body_shapes(&[a, b]).is_err(),
            "Regrouping revived detached geometry"
        );
        assert!(!world.owner_in_world(88)?);
        world.set_owner_in_world(88, true)?;
        world.group_body_shapes(&[a, b])?;
        world.set_owner_solid(88, false)?;
        let bounds = ([-2.; 3], [2.; 3]);
        assert!(world.owner_in_world(88)?);
        assert!(world.shape_penetration(a, [0.; 3])? > 0.);
        assert!(world
            .sphere_hits([0., 2., 0.], [0., -4., 0.], 0.1)?
            .iter()
            .any(|hit| hit.owner == 88));
        for _ in 0..2 {
            world.set_owner_in_world(88, false)?;
        }
        assert!(!world.owner_in_world(88)?);
        assert!(world
            .overlap_candidates(bounds.0, bounds.1)?
            .iter()
            .all(|hit| hit.owner != 88));
        assert!(world
            .ray_hits([0., 2., 0.], [0., -4., 0.])?
            .iter()
            .all(|hit| hit.owner != 88));
        assert!(world
            .sphere_hits([0., 2., 0.], [0., -4., 0.], 0.1)?
            .iter()
            .all(|hit| hit.owner != 88));
        assert_eq!(world.shape_penetration(a, [0.; 3])?, 0.);
        assert!(world.shape_penetrations(b, [0.; 3])?.is_empty());
        // Painting while hidden must not reinsert a body into the world.
        world.set_owner_solid(88, true)?;
        assert!(!world.owner_in_world(88)?);
        world.set_owner_solid(88, false)?;
        for _ in 0..2 {
            world.set_owner_in_world(88, true)?;
        }
        assert!(world.owner_in_world(88)?);
        let hits = world.overlap_candidates(bounds.0, bounds.1)?;
        assert!(hits.iter().any(|hit| hit.shape == a) && hits.iter().any(|hit| hit.shape == b));
        assert!(
            world.non_solid_owners.contains(&88),
            "Unhide repainted an erased body"
        );
        assert!(world.shape_penetration(a, [0.; 3])? > 0.);
        assert!(world.set_owner_in_world(999, true).is_err());
        world.retire_owner(88)?;
        assert!(
            world.set_owner_in_world(88, true).is_err(),
            "Retired body resurrected"
        );
        assert!(world
            .overlap_candidates(bounds.0, bounds.1)?
            .iter()
            .all(|hit| hit.owner != 88));
        Ok(())
    }
    #[test]
    fn world_membership_stops_simulation_without_losing_velocity() -> anyhow::Result<()> {
        let mut world = Collision::new(vec![])?;
        let hull = test_hull();
        let shape = world.add_static_convex(&hull.vertices, 0.01, 88)?;
        let part = world.add_static_convex(&hull.vertices, 0.01, 88)?;
        world.group_body_shapes(&[shape, part])?;
        world.set_native_body_mass(&[shape, part], 1., [1.; 3], [0.; 3])?;
        let mut record = super::super::native_motion_record::MotionRecord([0; 0x120]);
        record.0[8] = 3;
        record.0[0xbc] = 255;
        record.0[0xbd] = 255;
        record.set_linear_velocity([3., 0., 0., 0.]);
        world.set_simulation_body(
            &[shape, part],
            &record,
            super::super::body_material::Coefficients {
                friction: 0.,
                restitution: 0.,
            },
            true,
        )?;
        world.set_simulation_filter(shape, 2 | 0x18000)?;
        world.set_simulation_filter(part, 2 | 0x18000)?;
        let filter = super::super::physics_filter::GameFilter {
            matrix: super::super::physics_filter::Matrix::retail(),
            ..Default::default()
        };
        let before = world.simulation_pose(shape)?;
        world.set_owner_in_world(88, false)?;
        for _ in 0..30 {
            world.step_simulation(1. / 30., [0.; 3], &filter)?;
        }
        let hidden = world.simulation_pose(shape)?;
        assert_eq!(hidden.position, before.position);
        assert_eq!(hidden.linear, before.linear);
        world.set_simulation_velocities(&[shape, part], [4., 0., 0.], [0., 1., 0.])?;
        assert_eq!(world.simulation_pose(part)?.linear, [4., 0., 0.]);
        assert_eq!(world.simulation_pose(part)?.angular, [0., 1., 0.]);
        world.set_owner_in_world(88, true)?;
        world.step_simulation(1. / 30., [0.; 3], &filter)?;
        let resumed = world.simulation_pose(shape)?;
        assert!(resumed.position[0] > before.position[0]);
        assert_eq!(resumed.linear, [4., 0., 0.]);
        assert_eq!(world.simulation_pose(part)?.linear, resumed.linear);
        Ok(())
    }
    #[test]
    fn planar_convex_wall_retains_both_sides_and_source_radius() -> anyhow::Result<()> {
        let mut world = floor();
        let vertices = [[0., 1., -1.], [0., 3., -1.], [0., 3., 1.], [0., 1., 1.]];
        let shape = world.add_static_convex(&vertices, 0.1, 88)?;
        assert_eq!(world.planar_convex_count, 1);
        for sign in [-1., 1.] {
            assert!(world
                .overlap_sphere([sign * 0.15, 2., 0.], 0.1)?
                .iter()
                .any(|hit| hit.shape == shape));
            assert!(!world
                .overlap_sphere([sign * 0.3, 2., 0.], 0.1)?
                .iter()
                .any(|hit| hit.shape == shape));
            let hit = world
                .sweep_sphere([sign * 2., 2., 0.], [-sign * 4., 0., 0.], 0.1)?
                .expect("Flat wall must stop casts from either side");
            assert_eq!(hit.owner, 88);
            assert!(hit.source_triangle.is_none());
            assert!(
                (hit.fraction - 0.45).abs() < 0.002,
                "Authored radii changed: {}",
                hit.fraction
            );
        }
        Ok(())
    }
    #[test]
    fn sphere_overlap_tests_geometry_skin_live_pose_and_self_exclusion() -> anyhow::Result<()> {
        let cube: Vec<V> = [-0.5, 0.5]
            .into_iter()
            .flat_map(|x| {
                [-0.5, 0.5]
                    .into_iter()
                    .flat_map(move |y| [-0.5, 0.5].into_iter().map(move |z| [x, y, z]))
            })
            .collect();
        let mut world = floor();
        let core = world.add_static_convex(&cube, 0., 80)?;
        let rounded = world.add_static_convex(&cube, 0.2, 81)?;
        let hits = world.overlap_sphere([0.65, 0., 0.], 0.05)?;
        assert!(!hits.iter().any(|h| h.shape == core));
        assert!(hits.iter().any(|h| h.shape == rounded));
        // The corner is within the cube AABB but outside its rounded sphere
        // expansion; an AABB-only implementation would produce a false hit.
        assert!(!world
            .overlap_sphere([0.7, 0.7, 0.7], 0.05)?
            .iter()
            .any(|h| h.shape == rounded));
        world.set_owner_solid(81, false)?;
        assert!(world
            .overlap_sphere([0.65, 0., 0.], 0.05)?
            .iter()
            .any(|h| h.shape == rounded));
        assert!(world
            .with_query_body_excluded(rounded, |collision| collision
                .overlap_sphere([0.65, 0., 0.], 0.05))??
            .iter()
            .all(|h| h.shape != rounded));
        world.set_owner_transform(81, [5., 0., 0.], [0., 0., 0., 1.])?;
        assert!(world
            .overlap_sphere([0.65, 0., 0.], 0.05)?
            .iter()
            .all(|h| h.shape != rounded));
        assert!(world
            .overlap_sphere([5.65, 0., 0.], 0.05)?
            .iter()
            .any(|h| h.shape == rounded));
        assert!(world.overlap_sphere([f32::NAN, 0., 0.], 1.).is_err());
        assert!(world.overlap_sphere([0.; 3], -1.).is_err());
        let face = Triangle {
            p: [[-10., 0., -10.], [0., 0., 10.], [10., 0., -10.]],
        };
        let mesh = Collision::with_triangle_owners(vec![face], vec![0.2], vec![90])?;
        assert!(mesh
            .overlap_sphere([0., 0.15, 0.], 0.01)?
            .iter()
            .any(|h| h.owner == 90));
        assert!(mesh.overlap_sphere([0., 0.25, 0.], 0.01)?.is_empty());
        Ok(())
    }
    #[test]
    fn overlap_collector_rejects_missing_shape_identity() {
        let mut collector = OverlapCollector {
            hits: Vec::new(),
            owners: &[88],
            error: None,
        };
        let context = &mut collector as *mut _ as *mut c_void;
        assert_eq!(unsafe { collect_overlap(0, context) }, 1);
        assert_eq!(
            collector.hits,
            vec![OverlapCandidate {
                shape: 0,
                owner: 88
            }]
        );
        assert_eq!(unsafe { collect_overlap(-1, context) }, 0);
        assert!(collector.error.is_some());
    }
    #[test]
    fn owner_transform_is_absolute_and_applies_to_later_parts() {
        let mut world = Collision::new(vec![]).unwrap();
        let hull = test_hull();
        world
            .add_static_convex(&hull.vertices, hull.radius, 42)
            .unwrap();
        let identity = [0., 0., 0., 1.];
        world
            .set_owner_transform(42, [10., 5., 0.], identity)
            .unwrap();
        let floor = Triangle {
            p: [[-2., 0., -2.], [0., 0., 2.], [2., 0., -2.]],
        };
        world.add_static_triangles(&[floor], 0.1, 42).unwrap();
        assert!((world.min_y - 4.9).abs() < 1e-5);
        assert!((world.ground_distance([11., 7., -1.], 4., 0.77) - 2.).abs() < 1e-5);
        world.set_owner_solid(42, false).unwrap();
        world
            .set_owner_transform(42, [20., 8., 0.], identity)
            .unwrap();
        assert_eq!(world.ground_distance([21., 10., -1.], 4., 0.77), 4.);
        assert!(
            world
                .ray_hits([21., 10., -1.], [0., -4., 0.])
                .unwrap()
                .len()
                > 0
        );
        world.set_owner_solid(42, true).unwrap();
        assert!((world.min_y - 7.9).abs() < 1e-5);
        assert!((world.ground_distance([21., 10., -1.], 4., 0.77) - 2.).abs() < 1e-5);
        assert!(world
            .ray_hits([11., 7., -1.], [0., -4., 0.])
            .unwrap()
            .is_empty());
        world.set_owner_transform(42, [0.; 3], identity).unwrap();
        assert!((world.min_y + 0.1).abs() < 1e-5);
        assert!((world.ground_distance([1., 2., -1.], 4., 0.77) - 2.).abs() < 1e-5);
    }
    // Compare live owner placement with independently baked geometry across
    // both bridge queries and Rust ground/bounds consumers.
    #[test]
    fn transformed_bridge_queries_agree_with_baked_geometry() {
        let angle = 0.35_f32;
        let (s, c) = angle.sin_cos();
        let translation = [7., 3., -2.];
        let transform = |p: V| {
            [
                c * p[0] - s * p[1] + 7.,
                s * p[0] + c * p[1] + 3.,
                p[2] - 2.,
            ]
        };
        let vector = |p: V| [c * p[0] - s * p[1], s * p[0] + c * p[1], p[2]];
        let corners: Vec<V> = [-2., 2.]
            .into_iter()
            .flat_map(|x| {
                [-0.5, 0.5]
                    .into_iter()
                    .flat_map(move |y| [-2., 2.].into_iter().map(move |z| [x, y, z]))
            })
            .collect();
        for convex in [false, true] {
            let triangles = vec![
                Triangle {
                    p: [[-2., 0., -2.], [2., 0., 2.], [2., 0., -2.]],
                },
                Triangle {
                    p: [[-2., 0., -2.], [-2., 0., 2.], [2., 0., 2.]],
                },
            ];
            let mut moved =
                Collision::new(if convex { vec![] } else { triangles.clone() }).unwrap();
            let mut baked = Collision::new(if convex {
                vec![]
            } else {
                triangles
                    .iter()
                    .map(|t| Triangle {
                        p: t.p.map(transform),
                    })
                    .collect()
            })
            .unwrap();
            if convex {
                moved.add_static_convex(&corners, 0., 0).unwrap();
                baked
                    .add_static_convex(
                        &corners.iter().copied().map(transform).collect::<Vec<_>>(),
                        0.,
                        0,
                    )
                    .unwrap();
            }
            moved.set_player_hull(&test_hull()).unwrap();
            baked.set_player_hull(&test_hull()).unwrap();
            let q = [0., 0., (angle * 0.5).sin(), (angle * 0.5).cos()];
            moved.set_owner_transform(0, translation, q).unwrap();
            let origin = transform([0., 4., 0.]);
            let delta = vector([0., -8., 0.]);
            let close =
                |a: f32, b: f32| assert!((a - b).abs() < 2e-4, "{a} != {b}, convex={convex}");
            close(moved.min_y, baked.min_y);
            close(
                moved.ground_distance(origin, 8., 0.77),
                baked.ground_distance(origin, 8., 0.77),
            );
            assert_eq!(moved.ground_distance([0., 4., 0.], 8., 0.77), 8.);
            assert!(moved.set_owner_transform(0, [f32::NAN, 0., 0.], q).is_err());
            assert!(moved.set_owner_transform(0, translation, [0.; 4]).is_err());
            assert!(moved.set_owner_transform(99, translation, q).is_err());
            let a = moved.ray_hit(origin, unit(delta), 8.).unwrap();
            let b = baked.ray_hit(origin, unit(delta), 8.).unwrap();
            close(a.distance, b.distance);
            let a = moved.ray_hits(origin, delta).unwrap();
            let b = baked.ray_hits(origin, delta).unwrap();
            assert_eq!(a.len(), b.len());
            assert!(!a.is_empty());
            for (a, b) in a.iter().zip(&b) {
                close(a.fraction, b.fraction);
                for axis in 0..3 {
                    close(a.position[axis], b.position[axis]);
                    close(a.normal[axis], b.normal[axis]);
                }
            }
            let a = moved.sweep_sphere(origin, delta, 0.1).unwrap().unwrap();
            let b = baked.sweep_sphere(origin, delta, 0.1).unwrap().unwrap();
            close(a.fraction, b.fraction);
            for axis in 0..3 {
                close(a.position[axis], b.position[axis]);
                close(a.normal[axis], b.normal[axis]);
            }
            let p = transform([0., if convex { 0.4 } else { -0.1 }, 0.]);
            let a = moved.shape_penetration(0, p).unwrap();
            let b = baked.shape_penetration(0, p).unwrap();
            assert!(a > 0.);
            close(a, b);
            let a = moved.shape_penetrations(0, p).unwrap();
            let b = baked.shape_penetrations(0, p).unwrap();
            assert_eq!(a.len(), b.len());
            for (a, b) in a.iter().zip(&b) {
                close(a.depth, b.depth);
            }
            assert!(moved.ray([0., 4., 0.], [0., -1., 0.], 8.).is_none());
            assert!(moved
                .ray_hits([0., 4., 0.], [0., -8., 0.])
                .unwrap()
                .is_empty());
            let mut a = origin;
            let mut b = origin;
            for _ in 0..60 {
                let mut va = [0., -4., 0.];
                let mut vb = va;
                moved.move_player(&mut a, &mut va, 1. / 30., 0.77);
                baked.move_player(&mut b, &mut vb, 1. / 30., 0.77);
            }
            for axis in 0..3 {
                close(a[axis], b[axis]);
            }
        }
    }
    #[test]
    fn degenerate_ground_reference_preserves_incoming_motion_and_exports() {
        let world = floor();
        let mut player = Player::new([0., 1., 0.], Motion::default());
        for _ in 0..60 {
            player.step(&world, [0.; 3], false, 1. / 30.);
        }
        assert!(player.grounded);
        player.velocity = [2., 0., 0.];
        player.ground_vectors = Some(GroundVectors {
            movement: [1., 2., 3.],
            desired: [4., 5., 6.],
        });
        let start = player.position;
        player.step_input(&world, [1., -1.], [0.; 3], false, 1. / 30.);
        assert_eq!(player.velocity, [2., 0., 0.]);
        let vectors = player.ground_vectors.unwrap();
        assert_eq!(vectors.movement, [1., 2., 3.]);
        assert_eq!(vectors.desired, [4., 5., 6.]);
        assert!(player.position[0] > start[0]);
    }
    #[test]
    fn captured_retail_ground_support_frames() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tools/blotworx/fixtures/controller_ground_frame.json"
        ))
        .unwrap();
        for sample in fixture["samples"].as_array().unwrap() {
            let frame = &sample["ground_frame"];
            let words = |key: &str| {
                frame[key]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| f32::from_bits(v.as_u64().unwrap() as u32))
                    .collect::<Vec<_>>()
            };
            let input = words("input_words");
            let expected = words("basis_words");
            let vector = |offset: usize| [input[offset], input[offset + 1], input[offset + 2]];
            let basis = ground_support_basis(vector(4), vector(8), vector(20));
            for axis in 0..3 {
                for lane in 0..3 {
                    assert_eq!(
                        basis[axis][lane].to_bits(),
                        expected[axis * 4 + lane].to_bits(),
                        "basis axis {axis} lane {lane}"
                    );
                }
            }
            let relative = std::array::from_fn(|i| input[40 + i] - input[24 + i]);
            let local = ground_to_local(basis, relative);
            let expected = words("local_velocity_words");
            for lane in 0..3 {
                assert_eq!(local[lane].to_bits(), expected[lane].to_bits());
            }
            let facing = words("facing_words");
            let facing_basis = ground_support_basis(vector(4), vector(8), vector(4));
            let facing = ground_to_local(facing_basis, [facing[0], facing[1], facing[2]]);
            let target = words("local_target_words");
            let rates = words("rates_words");
            let maximum = f32::from_bits(frame["maximum_word"].as_u64().unwrap() as u32);
            let solved = ground_frame_velocity(
                local,
                [target[0], target[1], target[2]],
                facing,
                maximum,
                rates[1],
                rates[0],
                input[34],
                frame["maintain_velocity"].as_u64().unwrap() != 0,
                false,
            );
            let final_local = words("final_local_words");
            for lane in 0..3 {
                assert_eq!(
                    solved[lane].to_bits(),
                    final_local[lane].to_bits(),
                    "solved local lane {lane}"
                );
            }
            let output = ground_from_local(
                basis,
                [final_local[0], final_local[1], final_local[2]],
                vector(24),
            );
            let expected = words("output_words");
            for lane in 0..3 {
                assert_eq!(
                    output[lane].to_bits(),
                    expected[lane].to_bits(),
                    "output lane {lane}"
                );
            }
        }
    }
    #[test]
    fn dense_mesh_contacts_are_not_truncated() {
        let mut triangles = Vec::new();
        for x in 0..48 {
            for z in 0..48 {
                let a = [x as f32 * 0.004 - 0.096, 0., z as f32 * 0.004 - 0.096];
                let b = [a[0] + 0.004, 0., a[2]];
                let c = [a[0], 0., a[2] + 0.004];
                let d = [b[0], 0., c[2]];
                triangles.push(Triangle { p: [a, c, d] });
                triangles.push(Triangle { p: [a, d, b] });
            }
        }
        let w = Collision::new(triangles).unwrap();
        let mut p = Player::new([0., -0.005, 0.], Motion::default());
        p.step(&w, [0.; 3], false, 1. / 30.);
        let count = {
            let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
            unsafe { bw_collision_contact_count(w.handle.as_ptr()) }
        };
        assert!(count > 128, "Dense mesh yielded only {count} contacts");
        assert!(p.grounded && p.position[1].abs() < 0.02);
        for _ in 0..10 {
            p.step(&w, [0.; 3], false, 1. / 30.);
        }
        assert!(p.grounded && p.position[1].abs() < 0.02);
    }
    #[test]
    fn ground_query_skips_steep_triangle_above_walkable_floor() {
        let mut triangles = floor().triangles.clone();
        triangles.push(Triangle {
            p: [[-1., 0., -2.], [-1., 0., 2.], [1., 4., 0.]],
        });
        let w = Collision::new(triangles).unwrap();
        let origin = [0., 3., 0.];
        assert!((w.ray(origin, [0., -1., 0.], 5.).unwrap() - 1.).abs() < 1e-5);
        assert!((w.ground_distance(origin, 5., 39f32.to_radians().cos()) - 3.).abs() < 1e-5);
        assert!((w.ground_distance(origin, 5., 70f32.to_radians().cos()) - 1.).abs() < 1e-5);
        assert_eq!(w.ground_distance(origin, 0.5, 0.), 0.5);
        assert_eq!(w.ground_distance([200., 3., 0.], 5., 0.), 5.);
    }
    #[test]
    fn ground_query_ignores_back_faces_and_degenerate_triangles() {
        let w = Collision::new(vec![
            Triangle {
                p: [[-1., 0., -1.], [1., 0., -1.], [0., 0., 1.]],
            },
            Triangle {
                p: [[0., 1., 0.]; 3],
            },
        ])
        .unwrap();
        assert_eq!(w.ground_distance([0., 2., 0.], 5., 0.5), 5.);
    }
    #[test]
    fn spin_motion_cancels_vertical_speed_blocks_jump_and_restores_gravity() {
        let w = floor();
        let mut p = Player::new([0., 5., 0.], Motion::default());
        p.velocity = [1., 3., 2.];
        p.begin_spin_motion(-20.);
        assert_eq!(p.velocity, [1., 0., 2.]);
        p.step(&w, [0.; 3], (true, true), 1. / 30.);
        assert!((p.velocity[1] + 20. / 30.).abs() < 1e-6);
        assert!(!p.ground_jump_started && !p.air_first_jump_started && !p.second_jump_started);
        p.end_spin_motion();
        let before = p.velocity[1];
        p.step(&w, [0.; 3], false, 1. / 30.);
        assert!((p.velocity[1] - (before - p.motion.gravity_down / 30.)).abs() < 1e-6);
        p.begin_spin_motion(-20.);
        p.respawn();
        assert!(p.spin_gravity.is_none());
    }
    #[test]
    fn leaving_an_edge_retains_first_jump_then_allows_second() {
        let w = floor();
        let mut p = Player::new([99., 0., 0.], Motion::default());
        for _ in 0..3 {
            p.step(&w, [0.; 3], false, 1. / 30.);
        }
        assert!(p.grounded);
        for _ in 0..22 {
            p.step(&w, [1., 0., 0.], false, 1. / 30.);
        }
        assert!(!p.grounded && p.position[0] > 100.);
        assert!(p.ground_jump_available);
        p.step(&w, [0.; 3], (true, true), 1. / 30.);
        assert!(p.air_first_jump_started && !p.ground_jump_started && !p.second_jump_started);
        assert_eq!(p.active_jump_height, 2.);
        assert!(!p.ground_jump_available && p.double_jump_available);
        for _ in 0..10 {
            p.step(&w, [0.; 3], (false, true), 1. / 30.);
        }
        assert!(!p.air_first_jump_started);
        p.step(&w, [0.; 3], (true, true), 1. / 30.);
        assert_eq!(p.active_jump_height, 1.);
        assert!(!p.double_jump_available);
    }
    #[test]
    fn falling_near_ground_can_begin_first_jump_without_previous_contact() {
        let w = floor();
        let mut p = Player::new([0., 0.1, 0.], Motion::default());
        p.step(&w, [0.; 3], (true, true), 1. / 30.);
        assert_eq!(p.jump_stage, Some((0.1, 0)));
        assert!(p.double_jump_available);
        let mut distant = Player::new([0., 2., 0.], Motion::default());
        distant.step(&w, [0.; 3], (true, true), 1. / 30.);
        assert!(distant.jump_stage.is_none());
    }
    #[test]
    fn double_jump_enters_before_impulse_and_is_consumed() {
        let w = floor();
        let mut p = Player::new([0.; 3], Motion::default());
        for _ in 0..3 {
            p.step(&w, [0.; 3], false, 1. / 30.);
        }
        for i in 0..16 {
            // A press during the first jump's staged state cannot restart it.
            p.step(&w, [0.; 3], (i == 0 || i == 2, true), 1. / 30.);
            assert!(p.double_jump_available);
            assert_eq!(p.ground_jump_started, i == 0);
            assert!(!p.second_jump_started);
        }
        let origin = p.position[1];
        let velocity = p.velocity[1];
        p.step(&w, [0.; 3], (true, true), 1. / 30.);
        assert!(!p.double_jump_available);
        assert_eq!(p.jump_stage, Some((origin, 0)));
        assert!(p.second_jump_started && !p.ground_jump_started);
        assert!((p.velocity[1] - velocity * 0.25).abs() < 1e-6);
        p.step(&w, [0.; 3], (false, true), 1. / 30.);
        assert!(!p.second_jump_started);
        assert!(p.velocity[1] > 0.);
        assert_eq!(p.active_jump_height, 1.);
        for _ in 0..10 {
            p.step(&w, [0.; 3], (false, true), 1. / 30.);
        }
        p.step(&w, [0.; 3], (true, true), 1. / 30.);
        assert!(p.jump_stage.is_none(), "A third jump must not be granted");
        assert!(!p.second_jump_started);
        for _ in 0..90 {
            p.step(&w, [0.; 3], false, 1. / 30.);
        }
        assert!(p.grounded && p.double_jump_available);
        p.respawn();
        assert!(!p.double_jump_available);
    }
    #[test]
    fn jump_entry_blend_matches_observed_downward_transition() {
        let before = 2.317856550216675_f32;
        let previous_y = 2.3658206462860107_f32;
        let velocity = (before - previous_y) * 30.;
        let blended = jump_entry_velocity([0., velocity, 0.], [0.; 3], 0.75);
        assert!((before + blended[1] / 30. - 2.305865526199341).abs() < 0.000001);
        assert_eq!(
            jump_entry_velocity([1., 2., 3.], [4., 5., 6.], 0.),
            [1., 2., 3.]
        );
        assert_eq!(
            jump_entry_velocity([1., 2., 3.], [4., 5., 6.], 1.),
            [4., 5., 6.]
        );
    }
    #[test]
    fn stock_jump_stages_match_observed_early_heights() {
        let w = floor();
        let mut p = Player::new([0.; 3], Motion::default());
        for _ in 0..3 {
            p.step(&w, [0.; 3], false, 1. / 30.);
        }
        let origin = p.position[1];
        // Repeated original VillageTest observations, relative to its recorded
        // jump origin. Limited numerical regression, not synchronized parity.
        let heights = [
            0.199997, 0.363294, 0.613185, 0.827162, 1.004705, 1.262292, 1.519879,
        ];
        for (i, height) in heights.into_iter().enumerate() {
            let previous_velocity = p.velocity[1];
            p.step(&w, [0.; 3], (i == 0, true), 1. / 30.);
            assert!(
                (p.position[1] - origin - height).abs() < 0.00005,
                "stage {}: native {} vs observed {height}",
                i + 1,
                p.position[1] - origin
            );
            if i == 6 {
                assert_eq!(p.velocity[1], previous_velocity);
            }
        }
        let velocity = p.velocity[1];
        p.step(&w, [0.; 3], (false, true), 1. / 30.);
        assert!((p.velocity[1] - (velocity - 1.0)).abs() < 1e-6);
    }
    #[test]
    fn air_steering_limits_independent_facing_axes() {
        assert_eq!(
            air_velocity_step([0., 0.], [4., 4.], 0., 2., 1., 0.5),
            [0.5, 1.]
        );
        let v = air_velocity_step([0., 0.], [4., 4.], std::f32::consts::FRAC_PI_2, 2., 1., 0.5);
        assert!((v[0] - 1.).abs() < 1e-6 && (v[1] - 0.5).abs() < 1e-6);
        assert_eq!(
            air_velocity_step([4., 0.], [0., 0.], 0., 20., 20., 0.1),
            [2., 0.]
        );
        assert_eq!(
            air_velocity_step([0., 0.], [0.1, 0.2], 0., 20., 20., 0.1),
            [0.1, 0.2]
        );
    }
    #[test]
    fn airborne_player_retains_momentum_while_reversing() {
        let mut p = Player::new([0., 10., 0.], Motion::default());
        p.velocity = [4.5, 2., 0.];
        p.step(&floor(), [-1., 0., 0.], false, 0.1);
        assert!((p.velocity[0] - 2.5).abs() < 1e-5);
        assert!(p.position[0] > 0.);
        assert!(!p.grounded);
    }
    #[test]
    fn facing_limits_angular_speed_and_crosses_wrap() {
        assert!((facing_step(0., 1., 3., 0.1) - 0.3).abs() < 1e-6);
        assert!((facing_step(0., -1., 3., 0.1) + 0.3).abs() < 1e-6);
        assert!((facing_step(0., 0.1, 3., 0.1) - 0.1).abs() < 1e-6);
        let a = 179f32.to_radians();
        let b = -179f32.to_radians();
        assert!((facing_step(a, b, 4., 0.1) - b).abs() < 1e-6);
        assert_eq!(facing_step(0., 1., 0., 0.1), 0.);
    }
    #[test]
    fn authored_speed_curve_interpolates_and_saturates() {
        let m = Motion::default();
        let speed = |x| {
            fixed_ground_velocity(
                [x, 0.],
                m.speed,
                &m.ground_fixed_speeds,
                &m.ground_stick_values,
            )[0]
        };
        for (stick, expected) in [
            (0., 0.),
            (0.35, 0.55),
            (0.7, 1.1),
            (0.705, 1.55),
            (0.71, 2.),
            (0.85, 2.5),
            (0.99, 3.),
            (0.995, 4.),
            (1., 5.),
            (1.2, 5.),
        ] {
            assert!(
                (speed(stick) - expected).abs() < 0.0001,
                "stick {stick}: {}",
                speed(stick)
            );
        }
        let v = fixed_ground_velocity(
            [0.21, 0.28],
            5.,
            &m.ground_fixed_speeds,
            &m.ground_stick_values,
        );
        assert!((v[0] - 0.33).abs() < 1e-6 && (v[1] - 0.44).abs() < 1e-6);
        assert_eq!(fixed_ground_velocity([0.5, 0.], 5., &[], &[]), [2.5, 0.]);
        assert_eq!(
            fixed_ground_velocity([1., 0.], 5., &[0.2, 0.8], &[0.5]),
            [1., 0.]
        );
        assert_eq!(
            fixed_ground_velocity([0.5, 0.], 5., &[0.2, 0.8], &[0.5, 0.5]),
            [4., 0.]
        );
    }
    #[test]
    fn controller_filter_uses_authored_mode_and_resets_on_respawn() {
        let mut p = Player::new([0.; 3], Motion::default());
        // Native sqrt/reciprocal normalization preserves this extra ULP.
        assert_eq!(
            p.filter_stick([1., 0.], 1. / 120.),
            [f32::from_bits(0x3e800001), 0.]
        );
        p.respawn();
        assert_eq!(
            p.filter_stick([0., 1.], 1. / 120.),
            [0., f32::from_bits(0x3e800001)]
        );
        p.motion.input_filter_type = 0;
        assert_eq!(p.filter_stick([1., 0.], 1. / 120.), [1., 0.]);
        p.motion.input_filter_type = 1;
        p.motion.input_linear_velocity = 2.;
        assert_eq!(
            p.filter_stick([0., 0.], 0.25),
            [f32::from_bits(0x3efffffe), 0.]
        );
    }
    #[test]
    fn ground_acceleration_limits_vector_change_and_selects_stopping_rate() {
        let v = ground_velocity_step([0., 0.], [3., 4.], 12., 25., 1. / 30.);
        assert!((v[0] - 0.24).abs() < 1e-6 && (v[1] - 0.32).abs() < 1e-6);
        let v = ground_velocity_step([3., 4.], [0., 0.], 12., 25., 1. / 30.);
        assert!((v[0] - 2.5).abs() < 1e-6 && (v[1] - 3.3333333).abs() < 1e-6);
        // Equal-speed reversal uses acceleration, not the stopping rate.
        assert_eq!(
            ground_velocity_step([1., 0.], [-1., 0.], 1., 10., 0.5),
            [0.5, 0.]
        );
        assert_eq!(
            ground_velocity_step([0., 0.], [0.1, 0.], 12., 25., 1. / 30.),
            [0.1, 0.]
        );
        assert_eq!(
            ground_velocity_step([0., 0.], [0., 0.], 12., 25., 0.),
            [0., 0.]
        );
    }
    fn floor() -> Collision {
        Collision::new(vec![
            Triangle {
                p: [[-100., 0., -100.], [100., 0., 100.], [100., 0., -100.]],
            },
            Triangle {
                p: [[-100., 0., -100.], [-100., 0., 100.], [100., 0., 100.]],
            },
        ])
        .unwrap()
    }
    #[test]
    fn convex_hulls_with_more_than_256_half_edges_keep_contacts_and_queries() {
        let mut world = Collision::new(vec![]).unwrap();
        let points = (0..60)
            .map(|i| {
                let y = 1. - 2. * (i as f32 + 0.5) / 60.;
                let r = (1. - y * y).sqrt();
                let angle = i as f32 * 2.3999631;
                [r * angle.cos(), y + 2., r * angle.sin()]
            })
            .collect::<Vec<_>>();
        world.add_static_convex(&points, 0.05, 91).unwrap();
        {
            let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
            assert!(unsafe { bw_collision_hull_edges(world.handle.as_ptr(), 0) } > 256);
        }
        world.set_player_hull(&test_hull()).unwrap();
        for solid in [true, false, true] {
            world.set_owner_solid(91, solid).unwrap();
            assert_eq!(world.ray([0., 5., 0.], [0., -1., 0.], 8.).is_some(), solid);
            let swept = world
                .sweep_sphere([0., 5., 0.], [0., -8., 0.], 0.1)
                .unwrap()
                .unwrap();
            assert_eq!(swept.owner, 91);
            assert_eq!(swept.source_triangle, None);
            let mut position = [0., 4., 0.];
            for _ in 0..60 {
                let mut velocity = [0., -4., 0.];
                world.move_player(&mut position, &mut velocity, 1. / 30., 0.77);
            }
            assert!(position.iter().all(|v| v.is_finite()));
            assert_eq!(
                position[1] > 2.,
                solid,
                "wide hull contact failed: {position:?}"
            );
        }
    }
    #[test]
    fn erased_collision_remains_paint_targetable_and_restores() {
        let mut world = floor();
        world.set_player_hull(&test_hull()).unwrap();
        for solid in [true, false, true] {
            world.set_owner_solid(0, solid).unwrap();
            assert_eq!(world.ray([0., 2., 0.], [0., -1., 0.], 4.).is_some(), solid);
            assert_eq!(
                world.ray_hit([0., 2., 0.], [0., -1., 0.], 4.).is_some(),
                solid
            );
            assert!(!world
                .ray_hits([0., 2., 0.], [0., -4., 0.])
                .unwrap()
                .is_empty());
            assert!(world
                .sweep_sphere([0., 2., 0.], [0., -4., 0.], 0.1)
                .unwrap()
                .is_some());
            assert_eq!(world.ground_distance([0., 2., 0.], 4., 0.77) < 4., solid);
            let mut p = [0., 1., 0.];
            for _ in 0..30 {
                let mut v = [0., -5., 0.];
                world.move_player(&mut p, &mut v, 1. / 30., 0.77);
            }
            assert_eq!(p[1] > -0.2, solid, "unexpected support {p:?}");
        }
        assert!(world.set_owner_solid(99, false).is_err());
    }
    #[test]
    fn compound_convex_children_keep_gap_radius_and_query_ownership() {
        let mut world = floor();
        let cube = |cx: f32| {
            let mut points = vec![];
            for x in [cx - 0.5, cx + 0.5] {
                for y in [1., 2.] {
                    for z in [-0.5, 0.5] {
                        points.push([x, y, z]);
                    }
                }
            }
            points
        };
        world.add_static_convex(&cube(-2.), 0.1, 41).unwrap();
        world.add_static_convex(&cube(2.), 0.2, 42).unwrap();
        assert_eq!(
            world
                .ray_hit([-2., 4., 0.], [0., -1., 0.], 4.)
                .unwrap()
                .owner,
            41
        );
        assert_eq!(
            world
                .ray_hit([2., 4., 0.], [0., -1., 0.], 4.)
                .unwrap()
                .owner,
            42
        );
        // A merged hull would incorrectly close the space between these pillars.
        assert!(world.ray_hit([0., 1.5, -3.], [0., 0., 1.], 6.).is_none());
        world.set_player_hull(&test_hull()).unwrap();
        for (x, top, owner) in [(-2., 2.05, 41), (2., 2.15, 42)] {
            let mut p = [x, 4., 0.];
            for _ in 0..60 {
                let mut v = [0., -10., 0.];
                world.move_player(&mut p, &mut v, 1. / 30., 0.77);
            }
            assert!(
                (p[1] - top).abs() < 0.01,
                "incorrect compound radius/support {p:?}, expected {top}"
            );
            assert!(world.contact_owners().contains(&Some(owner)));
        }
    }
    #[test]
    fn grouped_body_preserves_parts_and_moves_as_one() -> anyhow::Result<()> {
        let mut world = floor();
        let cube = |x: f32| {
            test_hull()
                .vertices
                .into_iter()
                .map(|mut p| {
                    p[0] += x;
                    p
                })
                .collect::<Vec<_>>()
        };
        let a = world.add_static_convex(&cube(-2.), 0.05, 88)?;
        let b = world.add_static_convex(&cube(2.), 0.05, 88)?;
        let other = world.add_static_convex(&cube(8.), 0.05, 89)?;
        assert!(world.group_body_shapes(&[a, other]).is_err());
        assert!(world.group_body_shapes(&[a, a]).is_err());
        world.group_body_shapes(&[a, b])?;
        {
            let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
            assert_eq!(
                unsafe { bw_collision_same_body(world.handle.as_ptr(), a as i32, b as i32) },
                1
            );
            assert_eq!(
                unsafe { bw_collision_same_body(world.handle.as_ptr(), a as i32, other as i32) },
                0
            );
        }
        assert!(world.group_body_shapes(&[a, b]).is_err());
        assert!(
            world.ray_hit([0., 0.5, -3.], [0., 0., 1.], 6.).is_none(),
            "Compound gap closed"
        );
        world.set_shape_transform(a, [5., 4., 0.], [0., 0., 0., 1.])?;
        for x in [3., 7.] {
            assert_eq!(
                world.ray_hit([x, 8., 0.], [0., -1., 0.], 5.).unwrap().owner,
                88
            );
        }
        assert!((world.shape_min_y[a] - world.shape_min_y[b]).abs() < 1e-6);
        assert!(world.ray_hit([5., 4.5, -3.], [0., 0., 1.], 6.).is_none());
        let motion = BodyMotion {
            center_of_mass: [5., 4., 0.],
            linear_velocity: [1., 0., 0.],
            angular_velocity: [0.; 3],
            motion_type: 4,
        };
        assert!(world
            .set_body_state(&[a], [0.; 3], [0., 0., 0., 1.], motion)
            .is_err());
        world.set_body_state(&[a, b], [6., 4., 0.], [0., 0., 0., 1.], motion)?;
        assert_eq!(
            world
                .ray_hit([4., 8., 0.], [0., -1., 0.], 5.)
                .unwrap()
                .owner,
            88
        );
        Ok(())
    }
    #[test]
    fn combat_impulse_uses_mass_and_applies_once_per_compound_body() -> anyhow::Result<()> {
        let mut world = floor();
        let hull = test_hull();
        let a = world.add_static_convex(&hull.vertices, 0.01, 88)?;
        let b = world.add_static_convex(&hull.vertices, 0.01, 88)?;
        world.group_body_shapes(&[a, b])?;
        world.set_native_body_mass(&[a, b], 0.5, [1.; 3], [0.; 3])?;
        let mut record = super::super::native_motion_record::MotionRecord([0; 0x120]);
        record.0[0xbc] = 255;
        record.0[0xbd] = 255;
        record.0[8] = 2;
        world.set_simulation_body(
            &[a, b],
            &record,
            super::super::body_material::Coefficients {
                friction: 1.,
                restitution: 0.,
            },
            false,
        )?;
        world.apply_owner_impulse(88, [4., 0., 0.])?;
        assert_eq!(world.simulation_pose(a)?.linear, [2., 0., 0.]);
        world.apply_owner_impulse(88, [0., 2., 0.])?;
        assert_eq!(world.simulation_pose(b)?.linear, [2., 1., 0.]);
        assert!(world.apply_owner_impulse(88, [f32::NAN, 0., 0.]).is_err());
        record.0[8] = 4;
        world.set_simulation_body(
            &[a, b],
            &record,
            super::super::body_material::Coefficients {
                friction: 1.,
                restitution: 0.,
            },
            true,
        )?;
        let before = world.simulation_pose(a)?.linear;
        world.apply_owner_impulse(88, [100., 0., 0.])?;
        assert_eq!(world.simulation_pose(a)?.linear, before);
        Ok(())
    }
    #[test]
    fn rounded_contact_caches_survive_repeated_core_overlap_and_separation() {
        let _guard=BOX3D.lock().unwrap_or_else(|e|e.into_inner());
        assert_eq!(unsafe {bw_collision_test_rounded_cache()},1);
    }
    #[test]
    fn simulated_body_moves_queries_and_honors_native_paint_filter() -> anyhow::Result<()> {
        let run = |painted: bool| -> anyhow::Result<(V, V)> {
            let mut world = floor();
            let hull = test_hull();
            let shape = world.add_static_convex(&hull.vertices, 0.01, 88)?;
            world.group_body_shapes(&[shape])?;
            world.set_shape_transform(shape, [0., 4., 0.], [0., 0., 0., 1.])?;
            world.set_native_body_mass(&[shape], 1., [1.; 3], [0.; 3])?;
            let mut record = super::super::native_motion_record::MotionRecord([0; 0x120]);
            record.0[0xbc] = 255;
            record.0[0xbd] = 255;
            record.0[8] = 2;
            record.0[0x11e..].copy_from_slice(&0x3f80u16.to_be_bytes());
            let state = super::super::native_motion_step::MotionState {
                transform: super::super::native_hk_transform::from_entity(
                    [1., 0., 0., 0., 1., 0., 0., 0., 1.],
                    [0., 4., 0.],
                ),
                previous_center: [0., 4., 0., 0.],
                center: [0., 4., 0., 0.],
                previous_orientation: [0., 0., 0., 1.],
                orientation: [0., 0., 0., 1.],
                local_center: [0.; 4],
                delta_angle: [0.; 4],
            };
            record.set_swept_state(&state);
            world.set_simulation_body(
                &[shape],
                &record,
                super::super::body_material::Coefficients {
                    friction: 1.,
                    restitution: 0.,
                },
                true,
            )?;
            // Missing world filters must fail before moving anything.
            let filter = super::super::physics_filter::GameFilter {
                matrix: super::super::physics_filter::Matrix::retail(),
                ..Default::default()
            };
            assert!(world
                .step_simulation(1. / 30., [0., -9.8, 0.], &filter)
                .is_err());
            assert_eq!(world.simulation_pose(shape)?.position, [0., 4., 0.]);
            for part in world.owner_shapes(0).collect::<Vec<_>>() {
                world.set_simulation_filter(part, 3 | if painted { 0x8000 } else { 0 })?;
            }
            world.set_simulation_filter(shape, 2 | 0x18000)?;
            for _ in 0..120 {
                world.step_simulation(1. / 30., [0., -9.8, 0.], &filter)?;
            }
            let body = world.simulation_pose(shape)?;
            let touching=world.touching_shape_pairs(shape)?;
            assert_eq!(!touching.is_empty(),painted,"Persistent resting contacts must follow collision filtering");
            assert_eq!(touching,world.touching_shape_pairs(shape)?,"Reading contacts consumed them");
            assert_eq!(
                world.recovery_min_y, 0.,
                "Falling bodies moved the environment recovery boundary"
            );
            // The manual character/projectile ray and solver broadphase must
            // observe the same moved hull, not its old placement at Y=4.
            let hit = world
                .ray_hit(add(body.position, [0., 3., 0.]), [0., -1., 0.], 4.)
                .ok_or_else(|| anyhow::anyhow!("Moved solver body absent from manual ray"))?;
            assert_eq!(hit.owner, 88);
            Ok((body.position, body.linear))
        };
        let (supported, velocity) = run(true)?;
        assert!(
            supported[1] > -0.1 && supported[1] < 2.,
            "Body did not settle on painted floor: {supported:?}"
        );
        assert!(
            velocity[1].abs() < 0.1,
            "Settled body has fall velocity {velocity:?}"
        );
        let (erased, _) = run(false)?;
        assert!(
            erased[1] < -10.,
            "Native unpainted filter did not reject contacts: {erased:?}"
        );
        Ok(())
    }
    #[test]
    fn contact_mass_override_reaches_convex_and_mesh_solvers_without_mutating_body_mass(
    ) -> anyhow::Result<()> {
        unsafe extern "C" fn allow(_: i32, _: i32, _: *mut c_void) -> i32 {
            1
        }
        let run =
            |mesh: bool, scale: f32| -> anyhow::Result<(SimulationPose, Option<SimulationPose>)> {
                let mut world = if mesh {
                    floor()
                } else {
                    Collision::new(vec![])?
                };
                let vertices: Vec<V> = [-0.5, 0.5]
                    .into_iter()
                    .flat_map(|x| {
                        [-0.5, 0.5]
                            .into_iter()
                            .flat_map(move |y| [-0.5, 0.5].into_iter().map(move |z| [x, y, z]))
                    })
                    .collect();
                let mut bodies = Vec::new();
                for i in 0..if mesh { 1 } else { 2 } {
                    let shape = world.add_static_convex(&vertices, 0., 88 + i)?;
                    let position = if mesh {
                        [0., 0.5, 0.]
                    } else {
                        [i as f32, 0., 0.]
                    };
                    let linear = if i != 0 {
                        [0.; 3]
                    } else if mesh {
                        [0., -1., 0.]
                    } else {
                        [1., 0., 0.]
                    };
                    world.set_shape_transform(shape, position, [0., 0., 0., 1.])?;
                    world.set_native_body_mass(&[shape], 1., [0.; 3], [0.; 3])?;
                    let mut record = super::super::native_motion_record::MotionRecord([0; 0x120]);
                    record.0[0xbc] = 255;
                    record.0[0xbd] = 255;
                    record.0[8] = 7;
                    record.set_swept_state(&super::super::native_motion_step::MotionState {
                        transform: super::super::native_hk_transform::from_entity(
                            [1., 0., 0., 0., 1., 0., 0., 0., 1.],
                            position,
                        ),
                        previous_center: [position[0], position[1], position[2], 0.],
                        center: [position[0], position[1], position[2], 0.],
                        previous_orientation: [0., 0., 0., 1.],
                        orientation: [0., 0., 0., 1.],
                        local_center: [0.; 4],
                        delta_angle: [0.; 4],
                    });
                    record.set_linear_velocity([linear[0], linear[1], linear[2], 0.]);
                    world.set_simulation_body(
                        &[shape],
                        &record,
                        super::super::body_material::Coefficients {
                            friction: 0.,
                            restitution: 0.,
                        },
                        true,
                    )?;
                    bodies.push(shape);
                }
                let mut evidence = [0.; 3];
                {
                    let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
                    anyhow::ensure!(
                        unsafe {
                            bw_collision_test_mass_step(
                                world.handle.as_ptr(),
                                1. / 30.,
                                [0.; 3].as_ptr(),
                                allow,
                                std::ptr::null_mut(),
                                bodies[0] as i32,
                                scale,
                                evidence.as_mut_ptr(),
                            )
                        } != 0,
                        "Mass callback solver step failed"
                    );
                }
                assert!(
                    evidence[2] > 0.,
                    "Override did not reach {} contact preparation",
                    if mesh { "mesh" } else { "convex" }
                );
                assert_eq!(
                    evidence[..2],
                    [1., 1.],
                    "Contact override changed stored body mass"
                );
                Ok((
                    world.simulation_pose(bodies[0])?,
                    if mesh {
                        None
                    } else {
                        Some(world.simulation_pose(bodies[1])?)
                    },
                ))
            };
        // Mechanism bounds established before running: ordinary inelastic
        // contacts reduce closing velocity; zero effective inverse mass leaves
        // the selected locked-rotation body's input velocity exactly intact.
        // These are not original-game trajectory acceptance tolerances.
        let (normal, b) = run(false, 1.)?;
        assert!(
            normal.linear[0] < 0.9 && b.unwrap().linear[0] > 0.1,
            "Ordinary convex contact did not exchange momentum"
        );
        let (overridden, b) = run(false, 0.)?;
        assert_eq!(overridden.linear, [1., 0., 0.]);
        assert!(b.unwrap().linear[0] > 0.1);
        let (normal, _) = run(true, 1.)?;
        assert!(normal.linear[1] > -0.9);
        let (overridden, _) = run(true, 0.)?;
        assert_eq!(overridden.linear, [0., -1., 0.]);
        Ok(())
    }
    #[test]
    fn rounded_solver_geometry_keeps_core_vertices_and_rejects_expanded_box_corners() {
        let vertices: Vec<V> = [-1., 1.]
            .into_iter()
            .flat_map(|x| {
                [-1., 1.]
                    .into_iter()
                    .flat_map(move |y| [-1., 1.].into_iter().map(move |z| [x, y, z]))
            })
            .collect();
        let check = |position: V, ra: f32, rb: f32| {
            let mut output = [0.; 7];
            let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
            let n = unsafe {
                bw_collision_skin_pair(
                    vertices.as_ptr().cast(),
                    8,
                    vertices.as_ptr().cast(),
                    8,
                    position.as_ptr(),
                    ra,
                    rb,
                    output.as_mut_ptr(),
                )
            };
            assert!(n >= 0);
            (n, output)
        };
        let (n, contact) = check([2.25, 0., 0.], 0.125, 0.125);
        assert_eq!(
            n, 4,
            "Separated parallel skin faces lost their clipped support polygon"
        );
        assert_eq!(&contact[..4], &[1., 0., 0., 0.]);
        assert_eq!(contact[4], 1.125);
        let (_, contact) = check([2.25, 0., 0.], 0.125, 0.25);
        assert_eq!(contact[3], -0.125);
        assert_eq!(contact[4], 1.0625);
        assert_eq!(
            check([2.25, 2.25, 0.], 0.125, 0.125).0,
            0,
            "Spherical skin became an expanded box"
        );
        let (n, contact) = check([2.125, 2.125, 0.], 0.125, 0.125);
        assert!(n > 0);
        // Analytic geometry, f32 sqrt/GJK allowance fixed before execution;
        // this is not an original-game trajectory tolerance.
        assert!((contact[3] - (0.03125_f32.sqrt() - 0.25)).abs() < 1e-6);
        assert!(
            (contact[0] - 0.5_f32.sqrt()).abs() < 1e-6
                && (contact[1] - 0.5_f32.sqrt()).abs() < 1e-6
        );
        let (n, contact) = check([1.75, 0., 0.], 0.125, 0.125);
        assert!(n > 0);
        assert_eq!(contact[3], -0.5);
        assert_eq!(check([2.25, 0., 0.], 0., 0.).0, 0);
        assert_eq!(vertices.len(), 8); // All queries borrow unchanged core data.
    }
    #[test]
    fn rounded_solver_rest_height_includes_both_imported_skins() -> anyhow::Result<()> {
        for unlocked_rotation in [false, true] {
            let floor = Triangle {
                p: [[-100., 0., -100.], [0., 0., 100.], [100., 0., -100.]],
            };
            let mut world = Collision::with_triangle_owners(vec![floor], vec![0.125], vec![0])?;
            let vertices: Vec<V> = [-0.5, 0.5]
                .into_iter()
                .flat_map(|x| {
                    [-0.5, 0.5]
                        .into_iter()
                        .flat_map(move |y| [-0.5, 0.5].into_iter().map(move |z| [x, y, z]))
                })
                .collect();
            let shape = world.add_static_convex(&vertices, 0.125, 88)?;
            world.set_shape_transform(shape, [0., 3., 0.], [0., 0., 0., 1.])?;
            // Both locked and free rotation. For the latter, a unit cube of mass
            // 100 has core inverse inertia .06 on each axis. Neither configuration
            // is an original-character contact-response acceptance scenario.
            world.set_native_body_mass(
                &[shape],
                0.01,
                if unlocked_rotation {
                    [0.06; 3]
                } else {
                    [0.; 3]
                },
                [0.; 3],
            )?;
            let mut record = super::super::native_motion_record::MotionRecord([0; 0x120]);
            record.0[0xbc] = 255;
            record.0[0xbd] = 255;
            // An ordinary box motion, not character7: the original solver skips
            // world gravity for characters, even with stored gravity factor1.
            record.0[8] = 3;
            record.0[0x11e..].copy_from_slice(&0x3f80u16.to_be_bytes());
            record.set_swept_state(&super::super::native_motion_step::MotionState {
                transform: super::super::native_hk_transform::from_entity(
                    [1., 0., 0., 0., 1., 0., 0., 0., 1.],
                    [0., 3., 0.],
                ),
                previous_center: [0., 3., 0., 0.],
                center: [0., 3., 0., 0.],
                previous_orientation: [0., 0., 0., 1.],
                orientation: [0., 0., 0., 1.],
                local_center: [0.; 4],
                delta_angle: [0.; 4],
            });
            world.set_simulation_body(
                &[shape],
                &record,
                super::super::body_material::Coefficients {
                    friction: 0.,
                    restitution: 0.,
                },
                true,
            )?;
            for part in world.owner_shapes(0).collect::<Vec<_>>() {
                world.set_simulation_filter(part, 3 | 0x18000)?;
            }
            world.set_simulation_filter(shape, 2 | 0x18000)?;
            let filter = super::super::physics_filter::GameFilter {
                matrix: super::super::physics_filter::Matrix::retail(),
                ..Default::default()
            };
            for _ in 0..240 {
                world.step_simulation(1. / 30., [0., -9.8, 0.], &filter)?;
            }
            let pose = world.simulation_pose(shape)?;
            // Nominal0.5+0.125+0.125. Bounds are fixed before execution from
            // Box3D LINEAR_SLOP=.005 and MESH_REST_OFFSET=.005, not Wii parity.
            assert!(
                (0.745..=0.76).contains(&pose.position[1]),
                "Imported skins absent from physical resting height: {pose:?}"
            );
            assert!(
                pose.linear[1].abs() < 0.005,
                "Rounded body did not settle: {pose:?}"
            );
            if unlocked_rotation {
                assert_eq!(
                    pose.angular, [0.; 3],
                    "Free-rotation cube failed to settle into sleep: {pose:?}"
                );
            }
        }
        Ok(())
    }
    #[test]
    fn character_motion_skips_world_gravity_without_changing_source_factor() -> anyhow::Result<()> {
        for mode in [3, 7] {
            let mut world = Collision::new(vec![])?;
            let hull = test_hull();
            let shape = world.add_static_convex(&hull.vertices, hull.radius, 88)?;
            world.set_native_body_mass(&[shape], 0.01, [0.; 3], [0.; 3])?;
            let mut record = super::super::native_motion_record::MotionRecord([0; 0x120]);
            record.0[0xbc] = 255;
            record.0[0xbd] = 255;
            record.0[8] = mode;
            record.0[0x11e..].copy_from_slice(&0x3f80u16.to_be_bytes());
            record.set_linear_velocity([2., 3., 4., 0.]);
            record.set_swept_state(&super::super::native_motion_step::MotionState {
                transform: super::super::native_hk_transform::from_entity(
                    [1., 0., 0., 0., 1., 0., 0., 0., 1.],
                    [0.; 3],
                ),
                previous_center: [0.; 4],
                center: [0.; 4],
                previous_orientation: [0., 0., 0., 1.],
                orientation: [0., 0., 0., 1.],
                local_center: [0.; 4],
                delta_angle: [0.; 4],
            });
            let original = record.clone();
            world.set_simulation_body(
                &[shape],
                &record,
                super::super::body_material::Coefficients {
                    friction: 0.,
                    restitution: 0.,
                },
                true,
            )?;
            world.set_simulation_filter(shape, 3 | 0x18000)?;
            let filter = super::super::physics_filter::GameFilter {
                matrix: super::super::physics_filter::Matrix::retail(),
                ..Default::default()
            };
            world.step_simulation(1. / 30., [0., -9.8, 0.], &filter)?;
            let pose = world.simulation_pose(shape)?;
            assert_eq!(
                record, original,
                "Compatibility gravity policy modified the original motion record"
            );
            assert_eq!((pose.linear[0], pose.linear[2]), (2., 4.));
            if mode == 7 {
                assert_eq!(
                    pose.linear,
                    [2., 3., 4.],
                    "Controller-commanded character velocity received world gravity again"
                );
            } else {
                assert!(
                    pose.linear[1] < 3.,
                    "Ordinary box motion lost world gravity"
                );
            }
        }
        Ok(())
    }
    #[test]
    fn source_velocity_limits_bound_each_body_before_displacement() -> anyhow::Result<()> {
        let mut world = Collision::new(vec![])?;
        let vertices: Vec<V> = [-0.5, 0.5]
            .into_iter()
            .flat_map(|x| {
                [-0.5, 0.5]
                    .into_iter()
                    .flat_map(move |y| [-0.5, 0.5].into_iter().map(move |z| [x, y, z]))
            })
            .collect();
        let filter = super::super::physics_filter::GameFilter {
            matrix: super::super::physics_filter::Matrix::retail(),
            ..Default::default()
        };
        let mut fixtures = Vec::new();
        for (i, requested) in [[0., 0.], [2., 2.], [4., 1e6]].into_iter().enumerate() {
            let shape = world.add_static_convex(&vertices, 0., 88 + i)?;
            let position = [i as f32 * 10., 0., 0.];
            world.set_shape_transform(shape, position, [0., 0., 0., 1.])?;
            world.set_native_body_mass(&[shape], 1., [1.; 3], [0.; 3])?;
            let mut record = super::super::native_motion_record::MotionRecord([0; 0x120]);
            record.0[8] = 3;
            record.0[0xbc] = super::super::native_motion_limits::encode(requested[0])?;
            record.0[0xbd] = super::super::native_motion_limits::encode(requested[1])?;
            record.set_linear_velocity([12., 0., 0., 0.]);
            record.set_angular_velocity([0., 0., 100., 0.]);
            let original = record.clone();
            world.set_simulation_body(
                &[shape],
                &record,
                super::super::body_material::Coefficients {
                    friction: 0.,
                    restitution: 0.,
                },
                true,
            )?;
            world.set_simulation_filter(shape, 2 | 0x18000)?;
            let limits = [
                super::super::native_motion_limits::decode(record.0[0xbc]),
                super::super::native_motion_limits::decode(record.0[0xbd]),
            ];
            let mut lv = record.linear_velocity();
            let mut av = record.angular_velocity();
            // Four position substeps and one terminal-velocity publication.
            for _ in 0..5 {
                (lv, av, _, _) =
                    super::super::native_motion_step::limit_velocity(lv, av, 1. / 30., limits)?;
            }
            fixtures.push((shape, position, lv, av));
            assert_eq!(
                record, original,
                "Admission changed the original speed-limit record"
            );
        }
        world.step_simulation(1. / 30., [0.; 3], &filter)?;
        for (shape, start, lv, av) in fixtures {
            let pose = world.simulation_pose(shape)?;
            // Float integration allowances, not original Havok pose parity.
            for axis in 0..3 {
                assert!(
                    (pose.linear[axis] - lv[axis]).abs() < 2e-5,
                    "Wrong owned linear limit: {pose:?} expected {lv:?}"
                );
                assert!(
                    (pose.angular[axis] - av[axis]).abs() < 2e-5,
                    "Wrong native angular-angle limit: {pose:?} expected {av:?}"
                );
            }
            assert!(
                (pose.position[0] - start[0] - lv[0] / 30.).abs() < 2e-6,
                "Speed clipped after excessive displacement: {pose:?}, limit velocity {lv:?}"
            );
        }
        Ok(())
    }
    #[test]
    fn source_limits_survive_contact_relaxation_and_restitution() -> anyhow::Result<()> {
        let mut world = Collision::new(vec![])?;
        let hull = test_hull();
        let mut shapes = Vec::new();
        let filter = super::super::physics_filter::GameFilter {
            matrix: super::super::physics_filter::Matrix::retail(),
            ..Default::default()
        };
        for i in 0..2 {
            let shape = world.add_static_convex(&hull.vertices, hull.radius, 88 + i)?;
            world.set_shape_transform(shape, [i as f32 * 0.87, 0., 0.], [0., 0., 0., 1.])?;
            world.set_native_body_mass(&[shape], 1., [0.; 3], [0.; 3])?;
            let mut record = super::super::native_motion_record::MotionRecord([0; 0x120]);
            record.0[8] = 3;
            record.0[0xbc] =
                super::super::native_motion_limits::encode(if i == 0 { 2. } else { 0.5 })?;
            record.set_linear_velocity([if i == 0 { 12. } else { 0. }, 0., 0., 0.]);
            world.set_simulation_body(
                &[shape],
                &record,
                super::super::body_material::Coefficients {
                    friction: 0.,
                    restitution: 1.,
                },
                true,
            )?;
            world.set_simulation_filter(shape, 2 | 0x18000)?;
            shapes.push((
                shape,
                super::super::native_motion_limits::decode(record.0[0xbc]),
            ));
        }
        world.step_simulation(1. / 30., [0.; 3], &filter)?;
        for (shape, limit) in shapes {
            let pose = world.simulation_pose(shape)?;
            assert!(
                length(pose.linear) <= limit + 2e-6,
                "Post-contact velocity escaped its authored limit: {pose:?}, limit {limit}"
            );
            assert_eq!(pose.angular, [0.; 3]);
        }
        Ok(())
    }
    #[test]
    fn character_standing_on_dynamic_body_does_not_pump_resting_contacts() -> anyhow::Result<()> {
        // Isolate the support stack: a controller-driven, rotation-locked
        // character above a free dynamic cube above a static triangle floor.
        // These stability bounds are Box3D regression limits, not Wii parity.
        for (support_inverse_mass, offset, invalidate_contacts) in [
            (0.1, 0., false),
            (1., 0., false),
            (10., 0., false),
            (1., 0.2, false),
            (1., 0., true),
        ] {
            let mut world = floor();
            let vertices: Vec<V> = [-0.5, 0.5]
                .into_iter()
                .flat_map(|x| {
                    [-0.5, 0.5]
                        .into_iter()
                        .flat_map(move |y| [-0.5, 0.5].into_iter().map(move |z| [x, y, z]))
                })
                .collect();
            let hull = test_hull();
            world.set_player_hull(&hull)?;
            let mut shapes = Vec::new();
            for (mode, position, inverse_mass, inertia) in [
                (
                    3,
                    [0., 0.5, 0.],
                    support_inverse_mass,
                    [6. * support_inverse_mass; 3],
                ),
                (7, [offset, 3., 0.], 0.01, [0.; 3]),
            ] {
                let shape = if mode == 7 {
                    world.add_static_convex(&hull.vertices, hull.radius, 89)?
                } else {
                    world.add_static_convex(&vertices, 0., 88)?
                };
                world.set_shape_transform(shape, position, [0., 0., 0., 1.])?;
                world.set_native_body_mass(&[shape], inverse_mass, inertia, [0.; 3])?;
                let mut record = super::super::native_motion_record::MotionRecord([0; 0x120]);
                record.0[0xbc] = 255;
                record.0[0xbd] = 255;
                record.0[8] = mode;
                record.0[0x11e..].copy_from_slice(&0x3f80u16.to_be_bytes());
                world.set_simulation_body(
                    &[shape],
                    &record,
                    super::super::body_material::Coefficients {
                        friction: 0.5,
                        restitution: 0.,
                    },
                    true,
                )?;
                world.set_simulation_filter(
                    shape,
                    if mode == 7 {
                        3 | 0x18000 | 1 << 17
                    } else {
                        2 | 0x18000
                    },
                )?;
                shapes.push(shape);
            }
            for part in world.owner_shapes(0).collect::<Vec<_>>() {
                world.set_simulation_filter(part, 3 | 0x18000)?;
            }
            world.player_body = Some(shapes[1]);
            let mut player = Player::new([offset, 3., 0.], Motion::default());
            let filter = super::super::physics_filter::GameFilter {
                matrix: super::super::physics_filter::Matrix::retail(),
                ..Default::default()
            };
            let mut min_y = f32::INFINITY;
            let mut max_y = f32::NEG_INFINITY;
            let mut speed = 0f32;
            for tick in 0..360 {
                if invalidate_contacts && tick == 120 {
                    assert!(player.grounded);
                    // Another actor's support query replaces the shared planes.
                    // Model inward residual velocity along our own actual normal,
                    // without injecting tangential motion into the support stack.
                    let normal = world.active_support_motion(false).unwrap().normal;
                    player.velocity = mul(normal, -0.25);
                    world.with_player_queries(|| world.support_at([100.; 3], 1. / 30., 0.77));
                    assert!(world.contact_planes().is_empty());
                    // Brush movement preserves incoming velocity, too.
                    player.begin_brush_motion(5., 1.)?;
                }
                player.prepare_input_step(
                    &world,
                    [0.; 2],
                    [0., 0., 1.],
                    JumpInput {
                        pressed: tick == 180,
                        held: (180..186).contains(&tick),
                    },
                    1. / 30.,
                );
                if invalidate_contacts && tick == 120 {
                    assert!(
                        player.velocity[1].abs() < 0.001,
                        "Collision cache invalidation reissued inward support velocity: {:?}",
                        player.velocity
                    );
                }
                world.set_simulation_velocity(shapes[1], player.velocity)?;
                world.set_character_response(player.velocity, 1000.)?;
                world.step_simulation(1. / 30., [0., -9.8, 0.], &filter)?;
                player.accept_physics_pose(world.simulation_pose(shapes[1])?);
                player.finish_input_step(&world, 1. / 30.);
                if (120..175).contains(&tick) || tick >= 300 {
                    let pose = world.simulation_pose(shapes[0])?;
                    min_y = min_y.min(pose.position[1]);
                    max_y = max_y.max(pose.position[1]);
                    speed = speed.max(length(pose.linear));
                }
            }
            eprintln!("Standing stack mass={} offset={offset}: height {min_y}..{max_y}, peak speed {speed}",1./support_inverse_mass);
            eprintln!("Final player: {:?}", player.position);
            assert!(max_y-min_y<0.02 && speed<0.05,"Standing character destabilized its support: height {min_y}..{max_y}, speed {speed}");
            assert!(
                min_y > 0.47,
                "Dynamic support sank through the floor: {min_y}"
            );
            assert!(
                player.grounded
                    && player.respawns == 0
                    && world.contact_owners().contains(&Some(88)),
                "Character did not retain the dynamic support after landing: {:?}",
                player.position
            );
        }
        Ok(())
    }
    #[test]
    fn character_contact_policy_changes_response_without_changing_stored_mass() -> anyhow::Result<()>
    {
        let run = |limit: f32| -> anyhow::Result<f32> {
            let mut world = Collision::new(vec![])?;
            let vertices: Vec<V> = [-0.5, 0.5]
                .into_iter()
                .flat_map(|x| {
                    [-0.5, 0.5]
                        .into_iter()
                        .flat_map(move |y| [-0.5, 0.5].into_iter().map(move |z| [x, y, z]))
                })
                .collect();
            let mut shapes = Vec::new();
            for i in 0..2 {
                let shape = world.add_static_convex(&vertices, 0., 88 + i)?;
                let position = [i as f32, 0., 0.];
                world.set_shape_transform(shape, position, [0., 0., 0., 1.])?;
                world.set_native_body_mass(
                    &[shape],
                    if i == 0 { 0.01 } else { 1. },
                    [0.; 3],
                    [0.; 3],
                )?;
                let mut record = super::super::native_motion_record::MotionRecord([0; 0x120]);
                record.0[0xbc] = 255;
                record.0[0xbd] = 255;
                record.0[8] = if i == 0 { 7 } else { 3 };
                record.set_linear_velocity([if i == 0 { 1. } else { 0. }, 0., 0., 0.]);
                record.set_swept_state(&super::super::native_motion_step::MotionState {
                    transform: super::super::native_hk_transform::from_entity(
                        [1., 0., 0., 0., 1., 0., 0., 0., 1.],
                        position,
                    ),
                    previous_center: [position[0], 0., 0., 0.],
                    center: [position[0], 0., 0., 0.],
                    previous_orientation: [0., 0., 0., 1.],
                    orientation: [0., 0., 0., 1.],
                    local_center: [0.; 4],
                    delta_angle: [0.; 4],
                });
                world.set_simulation_body(
                    &[shape],
                    &record,
                    super::super::body_material::Coefficients {
                        friction: 0.,
                        restitution: 0.,
                    },
                    true,
                )?;
                world.set_simulation_filter(
                    shape,
                    if i == 0 {
                        3 | 0x18000 | 1 << 17
                    } else {
                        2 | 0x18000
                    },
                )?;
                shapes.push(shape);
            }
            world.player_body = Some(shapes[0]);
            world.set_character_response([1., 0., 0.], limit)?;
            let filter = super::super::physics_filter::GameFilter {
                matrix: super::super::physics_filter::Matrix::retail(),
                ..Default::default()
            };
            world.step_simulation(1. / 30., [0.; 3], &filter)?;
            assert!(
                world.character_mass_modifications > 0,
                "Dynamic contact did not consume the character policy"
            );
            {
                let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
                let mut values = [0.; 4];
                assert_eq!(
                    unsafe {
                        bw_collision_native_mass_values(
                            world.handle.as_ptr(),
                            shapes[0] as i32,
                            values.as_mut_ptr(),
                        )
                    },
                    1
                );
                assert_eq!(
                    values,
                    [0.01, 0., 0., 0.],
                    "Contact-local modification changed stored character mass/inertia"
                );
            }
            Ok(world.simulation_pose(shapes[1])?.linear[0])
        };
        // Contact-mechanism bounds, declared before execution. These do not
        // accept a Wii/Havok trajectory. A smaller response limit must reduce
        // momentum transfer through the original projected-mass factor.
        let ordinary = run(1000.)?;
        let limited = run(1.)?;
        assert!(
            ordinary > 0.95 && limited > 0.1 && limited < 0.9,
            "Response limits did not change transfer: {ordinary}, {limited}"
        );
        Ok(())
    }
    #[test]
    fn native_mass_preserves_partial_axis_lock_and_exact_inverse_values() -> anyhow::Result<()> {
        let mut world = floor();
        let hull = test_hull();
        let a = world.add_static_convex(&hull.vertices, 0.05, 88)?;
        let b = world.add_static_convex(&hull.vertices, 0.05, 88)?;
        world.group_body_shapes(&[a, b])?;
        assert!(world
            .set_native_body_mass(&[a], 0.3, [0.3, 0., 0.7], [0.; 3])
            .is_err());
        world.set_native_body_mass(&[a, b], 0.3, [0.3, 0., 0.7], [0.1, 0.2, 0.3])?;
        assert!(world
            .set_native_body_mass(&[a, b], f32::NAN, [1.; 3], [0.; 3])
            .is_err());
        let _guard = BOX3D.lock().unwrap_or_else(|e| e.into_inner());
        for shape in [a, b] {
            let mut actual = [0.; 4];
            assert_eq!(
                unsafe {
                    bw_collision_native_mass_values(
                        world.handle.as_ptr(),
                        shape as i32,
                        actual.as_mut_ptr(),
                    )
                },
                1
            );
            assert_eq!(actual, [0.3, 0.3, 0., 0.7]);
        }
        let mut angular = [0.; 3];
        assert_eq!(
            unsafe {
                bw_collision_angular_impulse(
                    world.handle.as_ptr(),
                    b as i32,
                    [1.; 3].as_ptr(),
                    angular.as_mut_ptr(),
                )
            },
            1
        );
        assert_eq!(
            angular,
            [0.3, 0., 0.7],
            "Partially locked body lost its free axes"
        );
        Ok(())
    }
    fn test_hull() -> super::super::character_hull::ConvexHull {
        let mut vertices = Vec::new();
        for x in [-0.39, 0.39] {
            for y in [0.1, 1.2] {
                for z in [-0.39, 0.39] {
                    vertices.push([x, y, z]);
                }
            }
        }
        super::super::character_hull::ConvexHull {
            vertices,
            radius: 0.05,
        }
    }
    #[test]
    fn query_body_exclusion_covers_grouped_shapes_and_restores_nested_scopes() -> anyhow::Result<()>
    {
        let mut world = Collision::new(vec![])?;
        let hull = test_hull();
        let first = world.add_static_convex(&hull.vertices, hull.radius, 88)?;
        let shifted = hull
            .vertices
            .iter()
            .map(|p| [p[0] + 1., p[1], p[2]])
            .collect::<Vec<_>>();
        let sibling = world.add_static_convex(&shifted, hull.radius, 88)?;
        let other = world.add_static_convex(&hull.vertices, hull.radius, 90)?;
        world.set_shape_transform(other, [4., 0., 0.], [0., 0., 0., 1.])?;
        world.group_body_shapes(&[first, sibling])?;
        let origin = [-3., 0.5, 0.];
        let delta = [10., 0., 0.];
        let direction = [1., 0., 0.];
        assert_eq!(world.ray_hit(origin, direction, 10.).unwrap().owner, 88);
        world.with_query_body_excluded(first, |c| {
            assert_eq!(c.ray_hit(origin, direction, 10.).unwrap().owner, 90);
            assert!(c.ray(origin, direction, 10.).unwrap() > 6.);
            assert!(c
                .ray_hits(origin, delta)
                .unwrap()
                .iter()
                .all(|hit| hit.owner == 90));
            assert!(c
                .sphere_hits(origin, delta, 0.1)
                .unwrap()
                .iter()
                .all(|hit| hit.owner == 90));
            let mut candidates = Vec::new();
            let hits = c
                .sphere_hits_filtered(origin, delta, 0.1, |candidate| {
                    candidates.push(candidate.owner);
                    Ok(true)
                })
                .unwrap();
            assert!(!hits.is_empty());
            assert_eq!(candidates, vec![90]);
            assert_eq!(
                c.sweep_sphere(origin, delta, 0.1).unwrap().unwrap().shape,
                other
            );
            let overlaps = c.overlap_candidates([-5.; 3], [8.; 3]).unwrap();
            assert_eq!(overlaps.len(), 1);
            assert_eq!(overlaps[0].shape, other);
            assert_eq!(
                c.with_query_body_excluded(other, |c| c
                    .ray_hit(origin, direction, 10.)
                    .unwrap()
                    .owner)
                    .unwrap(),
                88
            );
            assert_eq!(c.ray_hit(origin, direction, 10.).unwrap().owner, 90);
        })?;
        assert_eq!(world.ray_hit(origin, direction, 10.).unwrap().owner, 88);
        assert_eq!(world.overlap_candidates([-5.; 3], [8.; 3])?.len(), 3);
        assert!(world.with_query_body_excluded(usize::MAX, |_| ()).is_err());
        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            world
                .with_query_body_excluded(first, |c| {
                    assert_eq!(c.ray_hit(origin, direction, 10.).unwrap().owner, 90);
                    panic!("Expected query caller failure");
                })
                .unwrap();
        }));
        assert!(failed.is_err());
        assert_eq!(world.ray_hit(origin, direction, 10.).unwrap().owner, 88);
        Ok(())
    }
    #[test]
    fn query_body_exclusion_does_not_make_the_mover_stand_on_itself() -> anyhow::Result<()> {
        let mut world = floor();
        let hull = test_hull();
        world.set_player_hull(&hull)?;
        let own = world.add_static_convex(&hull.vertices, hull.radius, 88)?;
        assert_eq!(
            world
                .ray_hit([0., 3., 0.], [0., -1., 0.], 5.)
                .unwrap()
                .owner,
            88
        );
        world.with_query_body_excluded(own, |c| {
            assert_eq!(c.ray_hit([0., 3., 0.], [0., -1., 0.], 5.).unwrap().owner, 0);
            assert_eq!(c.shape_penetration(own, [0.; 3]).unwrap(), 0.);
            assert!(c.shape_penetrations(own, [0.; 3]).unwrap().is_empty());
            let mut p = [0.; 3];
            let mut v = [0., -1., 0.];
            c.move_player(&mut p, &mut v, 1. / 30., 0.77);
            assert!(
                c.contact_owners().iter().all(|owner| *owner == Some(0)),
                "Own hull contributed mover contacts"
            );
        })?;
        assert_eq!(
            world
                .ray_hit([0., 3., 0.], [0., -1., 0.], 5.)
                .unwrap()
                .owner,
            88
        );
        Ok(())
    }
    #[test]
    fn shapes_with_same_owner_can_move_independently() -> anyhow::Result<()> {
        let mut world = floor();
        let first = world.add_static_convex(&test_hull().vertices, 0.05, 88)?;
        let second = world.add_static_convex(&test_hull().vertices, 0.05, 88)?;
        let rotation = [0., 0., 0., 1.];
        world.set_shape_transform(first, [-4., 3., 0.], rotation)?;
        world.set_shape_transform(second, [4., 5., 0.], rotation)?;
        let cast = |w: &Collision, x| {
            w.ray_hits([x, 10., 0.], [0., -8., 0.])
                .unwrap()
                .into_iter()
                .next()
                .unwrap()
        };
        assert_eq!(cast(&world, -4.).shape, first);
        assert_eq!(cast(&world, 4.).shape, second);
        world.set_shape_transform(first, [-6., 3., 0.], rotation)?;
        assert_eq!(cast(&world, -6.).shape, first);
        assert_eq!(cast(&world, 4.).shape, second);
        assert!(world
            .set_shape_transform(first, [f32::NAN, 0., 0.], rotation)
            .is_err());
        assert_eq!(cast(&world, -6.).shape, first);
        assert!(world
            .set_shape_transform(usize::MAX, [0.; 3], rotation)
            .is_err());
        Ok(())
    }
    #[test]
    fn native_keyframed_step_publishes_compound_pose_and_support_motion() -> anyhow::Result<()> {
        use super::super::{dynamic_body::PlacedBody, native_motion_step::MotionState};
        let mut world = Collision::new(vec![])?;
        let vertices = test_hull().vertices;
        let left = vertices
            .iter()
            .map(|p| [p[0] - 1., p[1], p[2]])
            .collect::<Vec<_>>();
        let right = vertices
            .iter()
            .map(|p| [p[0] + 1., p[1], p[2]])
            .collect::<Vec<_>>();
        let first = world.add_static_convex(&left, 0.05, 88)?;
        let second = world.add_static_convex(&right, 0.05, 88)?;
        let separate = world.add_static_convex(&vertices, 0.05, 88)?;
        world.set_shape_transform(separate, [10., 5., 0.], [0., 0., 0., 1.])?;
        let transform = super::super::native_hk_transform::from_entity(
            [1., 0., 0., 0., 1., 0., 0., 0., 1.],
            [0., 2., 0.],
        );
        let mut body = PlacedBody::test_body(
            MotionState {
                transform,
                previous_center: [0., 2., 0., 0.],
                center: [0., 2., 0., 0.],
                previous_orientation: [0., 0., 0., 1.],
                orientation: [0., 0., 0., 1.],
                local_center: [0.; 4],
                delta_angle: [0.; 4],
            },
            [0.; 4],
            [0.; 4],
            [127; 2],
            vec![first, second],
        );
        body.motion.current.0[8] = 3;
        body.motion.replace(4, 0x3f800000)?;
        assert!(body.motion.has_stored_dynamic());
        let mut activations = 0;
        body.update_target([3., 2., 0., 0.], [0., 0., 0., 1.], 2., |before| {
            assert_eq!(before.linear_velocity(), [0.; 4]);
            activations += 1;
            Ok(())
        })?;
        assert_eq!(activations, 1);
        assert_eq!(body.state().center, [0., 2., 0., 0.]);
        body.advance_keyframed(&mut world, 0., 0.5, 2.)?;
        let cast = |w: &Collision, x| {
            w.ray_hits([x, 10., 0.], [0., -10., 0.])
                .unwrap()
                .into_iter()
                .next()
                .unwrap()
                .shape
        };
        assert_eq!(cast(&world, 2.), first);
        assert_eq!(cast(&world, 4.), second);
        assert_eq!(cast(&world, 10.), separate);
        assert!(world.min_y > 2. && world.min_y < 2.2);
        assert_eq!(body.state().center, [3., 2., 0., 2.]);
        for shape in [first, second] {
            let motion = world.shape_motion[&shape];
            assert_eq!(motion.center_of_mass, [3., 2., 0.]);
            assert_eq!(motion.linear_velocity, [6., 0., 0.]);
            assert_eq!(motion.motion_type, 4);
        }
        assert!(!world.shape_motion.contains_key(&separate));
        world.set_player_hull(&test_hull())?;
        let mut position = [2., 3.3, 0.];
        for _ in 0..10 {
            world.move_player(&mut position, &mut [0., -10., 0.], 1. / 30., 0.77);
        }
        assert_eq!(
            world.active_support_motion(false).unwrap().velocity,
            [6., 0., 0.]
        );
        // A bad later part must not move the first part, publish new motion,
        // invalidate existing contacts, or advance the owner's swept state.
        body.shapes.push(usize::MAX);
        assert!(body.advance_keyframed(&mut world, 0.5, 0.5, 2.).is_err());
        assert_eq!(body.state().center, [3., 2., 0., 2.]);
        assert_eq!(cast(&world, 2.), first);
        assert_eq!(cast(&world, 4.), second);
        assert_eq!(
            world.active_support_motion(false).unwrap().velocity,
            [6., 0., 0.]
        );
        body.shapes.pop();
        let target = super::super::native_hk_transform::from_entity(
            [1., 0., 0., 0., 1., 0., 0., 0., 1.],
            [6., 4., 0.],
        );
        body.warp(&mut world, target)?;
        assert_eq!(cast(&world, 5.), first);
        assert_eq!(cast(&world, 7.), second);
        assert_eq!(cast(&world, 10.), separate);
        assert_eq!(body.state().center, [6., 4., 0., 0.]);
        assert_eq!(body.state().delta_angle, [0.; 4]);
        assert_eq!(body.linear_velocity(), [6., 0., 0., 0.]);
        assert!(body.motion.has_stored_dynamic());
        body.motion.replace(1, 0x3f800000)?;
        assert_eq!(body.motion.current.motion_type(), 3);
        assert_eq!(body.state().center, [6., 4., 0., 0.]);
        assert!(body.advance_keyframed(&mut world, 0.5, 0.5, 2.).is_err());
        Ok(())
    }
    #[test]
    fn ground_query_retains_fixed_source_lanes_and_live_motion_precedence() -> anyhow::Result<()> {
        let triangles = floor().triangles.clone();
        let mut world = Collision::with_triangle_owners(triangles, vec![0., 0.01], vec![17, 17])?;
        let input = super::super::motion_ground::Input {
            position: [0., 1., 0., 0.],
            radius: 0.39,
            maximum: 5.,
            slope_cosine: 0.75,
            enabled: true,
            support: 0,
            supporting_body: super::super::motion_ground::SupportBody::Unknown,
        };
        let mut state = super::super::motion_ground::State::default();
        world.update_motion_ground(&mut state, input)?;
        assert!(state.hit_this_update);
        assert_eq!(state.surface_velocity, None);
        let source = [0., 0., 0., 7.];
        world.retain_fixed_source_velocity(17, source)?;
        for shape in world.owner_shapes(17) {
            assert_eq!(world.ground_body_linear_velocity(shape), Some(source));
        }
        world.update_motion_ground(&mut state, input)?;
        assert_eq!(state.surface_velocity, Some(source));
        for shape in world.owner_shapes(17).collect::<Vec<_>>() {
            world.set_shape_motion(
                shape,
                BodyMotion {
                    center_of_mass: [0.; 3],
                    linear_velocity: [1., 2., 3.],
                    angular_velocity: [0.; 3],
                    motion_type: 4,
                },
            )?;
        }
        world.update_motion_ground(&mut state, input)?;
        assert_eq!(state.surface_velocity, Some([1., 2., 3., 0.]));
        assert!(world.retain_fixed_source_velocity(100, source).is_err());
        assert!(world
            .retain_fixed_source_velocity(17, [f32::NAN; 4])
            .is_err());
        assert_eq!(world.ground_body_linear_velocity(usize::MAX), None);
        Ok(())
    }
    #[test]
    fn support_preserves_signed_surface_separation_and_fourth_normal_lane() -> anyhow::Result<()> {
        let mut world = floor();
        world.set_player_hull(&test_hull())?;
        // Hull core bottom is +0.1, radius is0.05: originY=-0.05
        // touches the floor. Probe both a gap and an overlap without moving
        // the hull or baking the support solver's temporary slop into data.
        for (y, expected) in [(-0.025, 0.025), (-0.075, -0.025)] {
            assert_ne!(
                world.support_at([3., y, 0.], 1. / 30., 0.77),
                Some(super::super::support::State::Unsupported)
            );
            let surface = world
                .active_support_motion(false)
                .expect("Missing active floor support");
            assert!(
                (surface.distance - expected).abs() < 0.00001,
                "{y}: {surface:?}"
            );
            assert!(
                (surface.normal_w - expected).abs() < 0.00001,
                "Distance vanished from retained four-lane surface normal: {surface:?}"
            );
            assert_eq!(surface.velocity, [0.; 3]);
            assert!(!surface.dynamic);
            let first = surface.distance.to_bits();
            assert_eq!(
                world
                    .active_support_motion(false)
                    .unwrap()
                    .distance
                    .to_bits(),
                first,
                "Reading the surface must not apply the solver margin a second time"
            );
        }
        Ok(())
    }
    #[test]
    fn support_uses_shape_motion_and_world_contact_lever_arm() -> anyhow::Result<()> {
        let mut world = floor();
        world.set_player_hull(&test_hull())?;
        world.move_player(&mut [3., 0., 0.], &mut [0., -10., 0.], 1. / 30., 0.77);
        assert_eq!(
            world.active_support_motion(false).unwrap().velocity,
            [0.; 3]
        );
        let mut motion = BodyMotion {
            center_of_mass: [0.; 3],
            linear_velocity: [2., 0., 0.],
            angular_velocity: [0., 3., 0.],
            motion_type: 4,
        };
        for i in 0..world.mesh_owners.len() {
            world.set_shape_motion(i, motion)?;
        }
        let surface = world.active_support_motion(false).unwrap();
        assert!(
            surface.velocity[2] < -7. && surface.velocity[2] > -11.,
            "{surface:?}"
        );
        motion.center_of_mass = [3., 0., 0.];
        for i in 0..world.mesh_owners.len() {
            world.set_shape_motion(i, motion)?;
        }
        assert!(world.active_support_motion(false).unwrap().velocity[2].abs() < 2.);
        motion.angular_velocity = [0.; 3];
        motion.motion_type = 1;
        for i in 0..world.mesh_owners.len() {
            world.set_shape_motion(i, motion)?;
        }
        assert_eq!(
            world.active_support_motion(false).unwrap().velocity,
            [0.; 3]
        );
        assert_eq!(
            world.active_support_motion(true).unwrap().velocity,
            [2., 0., 0.]
        );
        motion.motion_type = 5;
        for i in 0..world.mesh_owners.len() {
            world.set_shape_motion(i, motion)?;
        }
        assert_eq!(world.active_support_motion(true).unwrap().velocity, [0.; 3]);
        assert!(world.set_shape_motion(usize::MAX, motion).is_err());
        motion.center_of_mass[0] = f32::NAN;
        assert!(world.set_shape_motion(0, motion).is_err());
        assert_eq!(world.active_support_motion(true).unwrap().velocity, [0.; 3]);
        Ok(())
    }
    #[test]
    fn player_carries_support_motion_without_reporting_running() -> anyhow::Result<()> {
        let mut world = floor();
        world.set_player_hull(&test_hull())?;
        let mut player = Player::new([0., 1., 0.], Motion::default());
        for _ in 0..60 {
            player.step_input(&world, [0.; 2], [0., 0., 1.], false, 1. / 30.);
        }
        assert!(player.grounded);
        let surface_velocity = [2., 0., 0.];
        for shape in 0..world.mesh_owners.len() {
            world.set_shape_motion(
                shape,
                BodyMotion {
                    center_of_mass: [0.; 3],
                    linear_velocity: surface_velocity,
                    angular_velocity: [0.; 3],
                    motion_type: 4,
                },
            )?;
        }
        // The character is already travelling with the supporting body.
        player.velocity = surface_velocity;
        let before = player.position;
        player.step_input(&world, [0.; 2], [0., 0., 1.], false, 1. / 30.);
        assert_eq!(player.velocity, surface_velocity);
        assert_eq!(player.position[0], before[0] + 2. / 30.);
        assert!(player.grounded);
        let history = player
            .ground_vectors
            .expect("Supported tick must export ground motion");
        assert_eq!(history.movement, [0.; 3]);
        assert_eq!(history.desired, [0.; 3]);
        player.begin_brush_motion(5., 1.)?;
        player.set_aim_facing_target(1.);
        player.step_input(&world, [0.; 2], [0., 0., 1.], false, 1. / 30.);
        assert_eq!(
            player.yaw,
            1. / 30.,
            "Platform carry must retain stationary brush turn rate"
        );
        assert_eq!(player.velocity, surface_velocity);
        Ok(())
    }
    #[test]
    fn erased_shape_penetration_preserves_controller_contacts() -> anyhow::Result<()> {
        let mut world = floor();
        let hull = test_hull();
        let shape = world.add_static_convex(&hull.vertices, 0.05, 88)?;
        assert!(world.shape_penetration(shape, [0.; 3]).is_err());
        world.set_player_hull(&hull)?;
        let mut position = [3., 0., 0.];
        world.move_player(&mut position, &mut [0., -10., 0.], 1. / 30., 0.77);
        let contacts = world.contact_planes();
        let owners = world.contact_owners();
        let points = world.contact_points();
        assert_eq!(points.len(), contacts.len());
        assert!(!points.is_empty());
        assert!(
            points
                .iter()
                .all(|p| (p[0] - 3.).abs() < 0.5 && p[1].abs() < 0.001 && p[2].abs() < 0.5),
            "Floor contacts must retain world-space points: {points:?}"
        );
        let depth = world.shape_penetration(shape, [0.; 3])?;
        assert!(depth > 0.05, "Overlapping hull not detected: {depth}");
        let detailed = world.shape_penetrations(shape, [0.; 3])?;
        assert!(!detailed.is_empty());
        assert!(detailed.iter().all(|hit| hit.source_triangle.is_none()));
        assert_eq!(
            detailed.iter().map(|hit| hit.depth).fold(0., f32::max),
            depth
        );
        let floor_hit = world
            .ray_hits([3., 2., 0.], [0., -4., 0.])?
            .into_iter()
            .next()
            .unwrap();
        let floor_contacts = world.shape_penetrations(floor_hit.shape, [3., -0.15, 0.])?;
        assert!(!floor_contacts.is_empty());
        assert!(floor_contacts.iter().all(|hit| hit
            .source_triangle
            .is_some_and(|i| i < world.triangles.len())));
        assert!(floor_contacts.iter().any(|hit| hit.depth > 0.05));
        assert_eq!(world.contact_planes(), contacts);
        assert_eq!(world.contact_points(), points);
        assert_eq!(world.contact_owners(), owners);
        world.set_owner_solid(88, false)?;
        assert_eq!(world.shape_penetration(shape, [0.; 3])?, depth);
        assert_eq!(
            world.shape_penetrations(shape, [0.; 3])?.len(),
            detailed.len()
        );
        assert_eq!(world.shape_penetration(shape, [10., 0., 0.])?, 0.);
        // Changing participation invalidates the controller contact cache.
        assert!(world.contact_planes().is_empty());
        assert!(world.contact_points().is_empty());
        assert!(world.contact_owners().is_empty());
        assert!(world.shape_penetration(usize::MAX, [0.; 3]).is_err());
        assert!(world.shape_penetration(shape, [f32::NAN, 0., 0.]).is_err());
        Ok(())
    }
    #[test]
    fn convex_mover_lands_recovers_overlap_and_preserves_tangential_motion() {
        let mut world = floor();
        world.set_player_hull(&test_hull()).unwrap();
        for initial_y in [3.0, -0.2] {
            let mut position = [0., initial_y, 0.];
            let mut grounded = false;
            for _ in 0..90 {
                let mut velocity = [2., -10., 0.];
                grounded = world.move_player(&mut position, &mut velocity, 1. / 30., 0.77);
            }
            assert!(grounded, "not grounded at {position:?}");
            assert!(
                (position[1] + 0.05).abs() < 0.005,
                "wrong authored bottom {position:?}"
            );
            assert!(
                (position[0] - 6.).abs() < 0.03,
                "floor blocked tangential motion {position:?}"
            );
        }
    }
    #[test]
    fn convex_contacts_preserve_distinct_sources_with_equal_radii() {
        let triangles = floor().triangles.clone();
        let mut world =
            Collision::with_triangle_owners(triangles, vec![0.; 2], vec![17, 29]).unwrap();
        world.set_player_hull(&test_hull()).unwrap();
        assert_ne!(
            world.support_at([0.; 3], 1. / 30., 0.77),
            Some(super::super::support::State::Unsupported)
        );
        let owners = world.contact_owners();
        assert_eq!(owners.len(), world.contact_planes().len());
        assert!(
            owners.contains(&Some(17)),
            "missing first source: {owners:?}"
        );
        assert!(
            owners.contains(&Some(29)),
            "missing second source: {owners:?}"
        );
        assert!(owners.iter().all(Option::is_some));
        let sources = world.contact_sources();
        assert_eq!(sources.len(), owners.len());
        assert!(
            sources.contains(&(Some(0), Some(0))),
            "lost first original triangle: {sources:?}"
        );
        assert!(
            sources.contains(&(Some(1), Some(1))),
            "lost second original triangle: {sources:?}"
        );
        // Lift the hull off the triangle cores, within the support query range,
        // so GJK supplies witnesses rather than the overlap SAT fallback.
        world.support_at([0., 0.01, 0.], 1. / 30., 0.77);
        let simplexes = world.contact_simplexes();
        assert!(!simplexes.is_empty());
        assert!(simplexes.iter().all(|s| s
            .as_ref()
            .is_some_and(|pairs| !pairs.is_empty() && pairs.iter().all(|pair| pair[0] < 3))));
        world.support_at([0., 5., 0.], 1. / 30., 0.77);
        assert!(
            world.contact_owners().is_empty(),
            "old ownership survived a fresh empty query"
        );
        assert!(
            world.contact_sources().is_empty(),
            "old triangle identity survived a fresh query"
        );
        assert!(
            world.contact_simplexes().is_empty(),
            "old simplex survived a fresh query"
        );
    }
    #[test]
    fn support_probe_spans_small_gaps_but_not_open_air() {
        let mut world = floor();
        world.set_player_hull(&test_hull()).unwrap();
        use super::super::support::State;
        // The authored probe crosses the0.05 gap and finishes at rest. Its
        // average displacement is nonzero; feeding that to the classifier
        // incorrectly reports Sliding and activates slope-gravity forces.
        for dt in [1. / 30., 1. / 60.] {
            assert_eq!(
                world.support_at([0., 0., 0.], dt, 0.77),
                Some(State::Supported)
            );
        }
        assert_eq!(
            world.support_at([0., 0.2, 0.], 1. / 30., 0.77),
            Some(State::Unsupported)
        );
        assert_eq!(world.support_at([0., 0., 0.], 0., 0.77), None);
    }
    #[test]
    fn turn_motion_overrides_stopping_then_restores_and_respawn_clears_it() {
        let world = floor();
        let mut p = Player::new([0., 1., 0.], Motion::default());
        p.motion.ground_stopping_acceleration = 25.;
        let sample = |p: &mut Player| {
            p.grounded = true;
            p.velocity = [0., 0., 5.];
            p.yaw = 0.;
            p.step(&world, [0.; 3], false, 0.1);
            p.ground_vectors.unwrap().movement[2]
        };
        assert!((sample(&mut p) - 2.5).abs() < 1e-6);
        p.begin_turn_motion(10.);
        assert!((sample(&mut p) - 4.).abs() < 1e-6);
        assert_eq!(p.motion.ground_stopping_acceleration, 25.);
        p.end_turn_motion();
        assert!((sample(&mut p) - 2.5).abs() < 1e-6);
        p.begin_turn_motion(10.);
        p.respawn();
        assert!((sample(&mut p) - 2.5).abs() < 1e-6);
    }
    #[test]
    fn skid_blocks_input_facing_without_enabling_reverse_or_changing_stopping_rate() {
        use super::super::locomotion_actions::Effect;
        let world = floor();
        let mut p = Player::new([0., 1., 0.], Motion::default());
        p.grounded = true;
        p.velocity = [0., 0., 5.];
        assert_eq!(p.apply_action_motion(Effect::BeginSkid), None);
        p.step(&world, [1., 0., 0.], false, 1. / 30.);
        assert_eq!(p.yaw, 0.);
        assert!(p.turn_stopping_acceleration.is_none());
        for effect in [
            Effect::Animation("EVENT_Start_Run_Skid"),
            Effect::GameEvent(0x2b, 0.),
            Effect::ResetTurnTimer,
        ] {
            assert_eq!(p.apply_action_motion(effect), Some(effect));
        }
        assert_eq!(p.apply_action_motion(Effect::ResetMotion), None);
        p.grounded = true;
        p.step(&world, [1., 0., 0.], false, 1. / 30.);
        assert!(p.yaw > 0.);
        let before = p.yaw;
        p.begin_skid_motion();
        p.grounded = true;
        p.step(&world, [0., 0., -1.], false, 1. / 30.);
        assert!(
            p.yaw > before && p.yaw <= std::f32::consts::FRAC_PI_2,
            "skid retains the previous target rather than freezing orientation"
        );
        p.begin_skid_motion();
        p.respawn();
        assert!(!p.action_input_lock);
    }
    #[test]
    fn turn_retains_ground_direction_while_model_rotates_and_reset_releases_it() {
        use super::super::locomotion_actions::Effect;
        let world = floor();
        let mut p = Player::new([0., 1., 0.], Motion::default());
        assert_eq!(
            p.apply_action_motion(Effect::BeginTurn {
                direction: [0., 0., 1.],
                stopping_acceleration: 12.
            }),
            None
        );
        p.apply_action_motion(Effect::Facing([-1., 0., 0.]));
        p.grounded = true;
        p.velocity = [0., 0., 5.];
        p.step(&world, [0., 0., -1.], false, 1. / 30.);
        let movement = p.ground_vectors.unwrap().movement;
        assert_eq!(movement[0], 0.);
        assert!(
            movement[2] < 0.,
            "ground movement must follow the retained direction, not display yaw"
        );
        assert_eq!(p.yaw, -std::f32::consts::FRAC_PI_2);
        p.apply_action_motion(Effect::ResetMotion);
        assert!(p.movement_direction_override.is_none());
        p.grounded = true;
        p.velocity = [0., 0., 5.];
        p.step(&world, [0.; 3], false, 1. / 30.);
        assert!(p.ground_vectors.unwrap().movement[0] < 0.);
        p.apply_action_motion(Effect::BeginTurn {
            direction: [0., 0., 1.],
            stopping_acceleration: 12.,
        });
        p.respawn();
        assert!(p.movement_direction_override.is_none());
    }
    #[test]
    fn action_input_gate_filters_toward_zero_but_retains_raw_stick() {
        let mut p = Player::new([0.; 3], Motion::default());
        p.motion.input_filter_type = 1;
        p.motion.input_linear_velocity = 2.;
        assert_eq!(p.filter_stick([1., 0.], 0.5), [1., 0.]);
        p.begin_skid_motion();
        assert_eq!(
            p.filter_stick([-1., 0.], 0.1),
            [f32::from_bits(0x3f4ccccc), 0.]
        );
        assert_eq!(p.raw_stick(), [-1., 0.]);
        p.motion.input_filter_type = 0;
        assert_eq!(p.filter_stick([0., 1.], 0.1), [0.; 2]);
        assert_eq!(p.raw_stick(), [0., 1.]);
        p.end_turn_motion();
        assert_eq!(p.filter_stick([0., 1.], 0.1), [0., 1.]);
        p.begin_turn_motion(12.);
        assert_eq!(p.filter_stick([1., 0.], 0.1), [0.; 2]);
        assert_eq!(p.raw_stick(), [1., 0.]);
        p.respawn();
        assert_eq!(p.raw_stick(), [0.; 2]);
    }
    #[test]
    fn airborne_turn_uses_retained_direction_for_unequal_steering_rates() {
        use super::super::locomotion_actions::Effect;
        let world = floor();
        let mut p = Player::new([0., 10., 0.], Motion::default());
        p.motion.air_control_acceleration = 2.;
        p.motion.air_side_acceleration = 8.;
        p.apply_action_motion(Effect::BeginTurn {
            direction: [0., 0., 1.],
            stopping_acceleration: 12.,
        });
        p.apply_action_motion(Effect::Facing([1., 0., 0.]));
        p.step(&world, [1., 0., 0.], false, 0.1);
        assert!(
            (p.velocity[0] - 0.8).abs() < 1e-6,
            "input lies sideways to retained +Z direction"
        );
        p.apply_action_motion(Effect::ResetMotion);
        p.velocity = [0.; 3];
        p.step(&world, [1., 0., 0.], false, 0.1);
        assert!(
            (p.velocity[0] - 0.2).abs() < 1e-6,
            "reset uses visible +X facing as forward"
        );
    }
    #[test]
    fn turn_reverse_direction_respects_maintain_velocity_bypass() {
        let solve = |maintain| {
            ground_facing_velocity_with_reverse([0., 4.], [0., 1.], 5., maintain, [0., -1.], true)
        };
        // One retail reciprocal-sqrt refinement retains a one-ULP residual.
        assert_eq!(solve(false), [-0., -f32::from_bits(0x407fffff)]);
        assert_eq!(solve(true), [0., 4.]);
        let world = floor();
        let mut p = Player::new([0., 1., 0.], Motion::default());
        p.grounded = true;
        p.velocity = [0., 0., 5.];
        p.begin_turn_motion(12.);
        p.step(&world, [0., 0., -1.], false, 1. / 30.);
        assert!(p.ground_vectors.unwrap().movement[2] < 0.);
        assert_eq!(
            p.yaw, 0.,
            "stick input must not replace the action facing target"
        );
        p.apply_turn_facing([-1., 0., 0.]);
        assert_eq!(p.yaw, -std::f32::consts::FRAC_PI_2);
        p.grounded = true;
        p.step(&world, [0., 0., 1.], false, 1. / 30.);
        assert_eq!(p.yaw, -std::f32::consts::FRAC_PI_2);
        p.end_turn_motion();
        p.grounded = true;
        p.step(&world, [0., 0., 1.], false, 1. / 30.);
        assert!(p.yaw > -std::f32::consts::FRAC_PI_2);
    }
    #[test]
    fn convex_character_traverses_shallow_stairs() {
        let mut triangles = Vec::new();
        for i in 0..24 {
            let x = i as f32 - 2.;
            let y = (i as f32 * 0.1).min(0.8);
            let next_y = ((i + 1) as f32 * 0.1).min(0.8);
            triangles.extend([
                Triangle {
                    p: [[x, y, -2.], [x + 1., y, 2.], [x + 1., y, -2.]],
                },
                Triangle {
                    p: [[x, y, -2.], [x, y, 2.], [x + 1., y, 2.]],
                },
                Triangle {
                    p: [
                        [x + 1., y, -2.],
                        [x + 1., next_y, 2.],
                        [x + 1., next_y, -2.],
                    ],
                },
                Triangle {
                    p: [[x + 1., y, -2.], [x + 1., y, 2.], [x + 1., next_y, 2.]],
                },
            ]);
        }
        let mut world = Collision::new(triangles).unwrap();
        let authored: serde_json::Value = serde_json::from_str(include_str!(
            "../../tools/blotworx/fixtures/mickey_collision_hull.json"
        ))
        .unwrap();
        let hull = super::super::character_hull::ConvexHull {
            radius: authored["radius"].as_f64().unwrap() as f32,
            vertices: authored["vertices"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| std::array::from_fn(|i| v[i].as_f64().unwrap() as f32))
                .collect(),
        };
        world.set_player_hull(&hull).unwrap();
        let mut p = Player::new([-1.5, 1., 0.], Motion::default());
        p.yaw = std::f32::consts::FRAC_PI_2;
        for _ in 0..60 {
            p.step(&world, [0.; 3], false, 1. / 30.);
        }
        for _ in 0..120 {
            p.step(&world, [1., 0., 0.], false, 1. / 30.);
        }
        assert!(
            p.position[0] > 5.,
            "stuck on shallow stair: {:?}",
            p.position
        );
    }
    #[test]
    fn steep_contact_does_not_replenish_spent_jumps() {
        let mut world = Collision::new(vec![
            Triangle {
                p: [
                    [-100., -200., -100.],
                    [100., 200., 100.],
                    [100., 200., -100.],
                ],
            },
            Triangle {
                p: [
                    [-100., -200., -100.],
                    [-100., -200., 100.],
                    [100., 200., 100.],
                ],
            },
        ])
        .unwrap();
        world.set_player_hull(&test_hull()).unwrap();
        let mut player = Player::new([0., 2., 0.], Motion::default());
        player.double_jump_available = false;
        player.ground_jump_available = false;
        let mut touched_steep_surface = false;
        for _ in 0..30 {
            player.step(&world, [0.; 3], false, 1. / 30.);
            touched_steep_surface |= world.active_support_surface().is_some_and(|(normal, _)| {
                normal[1] < player.motion.max_slope_degrees.to_radians().cos()
            });
            assert!(
                !player.double_jump_available,
                "steep contact restored double jump"
            );
            assert!(
                !player.ground_jump_available,
                "steep contact restored ground jump"
            );
        }
        assert!(
            touched_steep_surface,
            "regression must actually touch the steep surface"
        );
    }
    #[test]
    fn convex_running_up_slope_keeps_support_but_jump_leaves_it() {
        let mut world = Collision::new(vec![
            Triangle {
                p: [[-100., -75., -100.], [100., 75., 100.], [100., 75., -100.]],
            },
            Triangle {
                p: [[-100., -75., -100.], [-100., -75., 100.], [100., 75., 100.]],
            },
        ])
        .unwrap();
        world.set_player_hull(&test_hull()).unwrap();
        let mut player = Player::new([0., 2., 0.], Motion::default());
        player.motion.maintain_ground_velocity = true;
        for _ in 0..60 {
            player.step(&world, [0.; 3], false, 1. / 30.);
        }
        assert!(player.grounded);
        // Rising along a support plane is not separation from that plane.
        let mut tangent_position = player.position;
        let mut tangent_velocity = [2., 1.5, 0.];
        assert!(world.move_player(&mut tangent_position,&mut tangent_velocity,1./30.,0.77),
            "uphill tangential motion was classified as airborne: start {:?}, end {:?}, planes {:?}",player.position,tangent_position,world.contact_planes());
        for tick in 0..240 {
            let input_normal = world.active_support_normal();
            player.step_input(
                &world,
                if tick < 120 { [1., 0.] } else { [-1., 0.] },
                [0., 0., 1.],
                false,
                1. / 30.,
            );
            assert!(
                length(player.velocity) <= player.motion.speed + 1e-4,
                "slope increased speed: {:?}",
                player.velocity
            );
            let vectors = player.ground_vectors.unwrap();
            // Check the actual contact frame, including bevel/triangle-edge
            // contacts; the aggregate need not equal the face's geometric normal.
            if let Some(normal) = input_normal {
                let dot = normal
                    .iter()
                    .zip(vectors.desired)
                    .map(|(n, v)| n * v)
                    .sum::<f32>();
                assert!(
                    dot.abs() < 1e-5,
                    "maintain-velocity target left support frame at {tick}: {:?}, normal {:?}",
                    vectors.desired,
                    normal
                );
            }
            assert!(
                player.grounded,
                "lost slope support at tick {tick}: position {:?}, velocity {:?}",
                player.position, player.velocity
            );
        }
        player.step_input(&world, [1., 0.], [0., 0., 1.], true, 1. / 30.);
        assert!(!player.grounded, "jump must leave the support plane");
    }
    #[test]
    fn convex_mover_sweeps_thin_walls_and_slides() {
        let mut world = Collision::new(vec![
            Triangle {
                p: [[0., -10., -10.], [0., 10., -10.], [0., 10., 10.]],
            },
            Triangle {
                p: [[0., -10., -10.], [0., 10., 10.], [0., -10., 10.]],
            },
        ])
        .unwrap();
        world.set_player_hull(&test_hull()).unwrap();
        let mut position = [2., 0., 0.];
        let mut velocity = [-200., 0., 0.];
        world.move_player(&mut position, &mut velocity, 1. / 30., 0.77);
        assert!((position[0] - 0.44).abs() < 0.01, "tunneled: {position:?}");
        for _ in 0..30 {
            velocity = [-2., 0., 2.];
            world.move_player(&mut position, &mut velocity, 1. / 30., 0.77);
        }
        assert!(
            (position[0] - 0.44).abs() < 0.01 && (position[2] - 2.).abs() < 0.03,
            "failed wall slide: {position:?}"
        );
    }
    #[test]
    fn convex_mover_preserves_each_static_mesh_radius() {
        let mut triangles = Vec::new();
        let mut radii = Vec::new();
        for (x, radius) in [(-5., 0.02), (5., 0.15)] {
            triangles.extend([
                Triangle {
                    p: [[x - 2., 0., -2.], [x + 2., 0., 2.], [x + 2., 0., -2.]],
                },
                Triangle {
                    p: [[x - 2., 0., -2.], [x - 2., 0., 2.], [x + 2., 0., 2.]],
                },
            ]);
            radii.extend([radius; 2]);
        }
        let mut world = Collision::with_triangle_radii(triangles, radii).unwrap();
        world.set_player_hull(&test_hull()).unwrap();
        for (x, radius) in [(-5., 0.02), (5., 0.15)] {
            let mut position = [x, 3., 0.];
            let mut velocity = [0., -200., 0.];
            world.move_player(&mut position, &mut velocity, 1. / 30., 0.77);
            assert!(
                (position[1] - (radius - 0.05)).abs() < 0.01,
                "wrong sweep radius {position:?}"
            );
            for _ in 0..10 {
                velocity = [0., -1., 0.];
                assert!(world.move_player(&mut position, &mut velocity, 1. / 30., 0.77));
            }
            // Character adapter cancels Box3D's penetration allowance;
            // contact follows the authored hull bottom and both radii.
            assert!(
                (position[1] - (radius - 0.05)).abs() < 1e-5,
                "wrong contact radius {position:?}"
            );
        }
        assert!(Collision::with_triangle_radii(vec![], vec![f32::NAN]).is_err());
    }
    #[test]
    fn ray_hits_floor_from_above() {
        let w = floor();
        let hit = w.ray([0., 2., 0.], [0., -1., 0.], 5.).unwrap();
        assert!((hit - 2.).abs() < 0.001);
        assert!(w.ray([0., 2., 0.], [0., 1., 0.], 5.).is_none());
    }
    #[test]
    fn retired_owner_leaves_all_queries_without_removing_its_neighbors() -> anyhow::Result<()> {
        let triangle = |y: f32| Triangle {
            p: [[-10., y, -10.], [0., y, 10.], [10., y, -10.]],
        };
        let mut world = Collision::with_triangle_owners(
            vec![triangle(0.), triangle(1.)],
            vec![0.; 2],
            vec![7, 91],
        )?;
        world.retire_owner(91)?;
        for shape in world.owner_shapes(91) {
            assert_ne!(
                unsafe { bw_collision_shape_payload_released(world.handle.as_ptr(), shape as i32) },
                0
            );
        }
        assert_eq!(
            world
                .ray_hit([0., 3., 0.], [0., -1., 0.], 5.)
                .unwrap()
                .owner,
            7
        );
        assert!(world
            .ray_hits([0., 3., 0.], [0., -5., 0.])?
            .iter()
            .all(|h| h.owner == 7));
        assert!(world.overlap_sphere([0., 1., 0.], 0.1)?.is_empty());
        world.set_owner_solid(91, true)?;
        assert!(
            world.overlap_sphere([0., 1., 0.], 0.1)?.is_empty(),
            "Paint restored a destroyed collider"
        );
        Ok(())
    }
    #[test]
    fn rejects_invalid_geometry() {
        // An empty world is valid while jigsaw and convex shapes are being installed.
        assert!(Collision::new(vec![])
            .unwrap()
            .ray([0.; 3], [0., -1., 0.], 1.)
            .is_none());
        assert!(Collision::new(vec![Triangle {
            p: [[f32::NAN; 3]; 3]
        }])
        .is_err());
    }
    #[test]
    fn aim_query_returns_nearest_authored_owner_and_surface() {
        let triangle = |y: f32| Triangle {
            p: [[-10., y, -10.], [0., y, 10.], [10., y, -10.]],
        };
        // Owners intentionally sort differently from distance and input order.
        let world = Collision::with_triangle_owners(
            vec![triangle(0.), triangle(1.)],
            vec![0.; 2],
            vec![7, 91],
        )
        .unwrap();
        let hit = world.ray_hit([0., 3., 0.], [0., -1., 0.], 5.).unwrap();
        assert_eq!(hit.owner, 91);
        assert!((hit.distance - 2.).abs() < 1e-6);
        assert_eq!(hit.position, [0., 1., 0.]);
        assert_eq!(hit.normal, [0., 1., 0.]);
        assert_eq!(
            Some(hit.distance),
            world.ray([0., 3., 0.], [0., -1., 0.], 5.)
        );
        assert_eq!(
            world
                .ray_hit([0., 0.5, 0.], [0., -1., 0.], 5.)
                .unwrap()
                .owner,
            7
        );
        assert!(world.ray_hit([0., 3., 0.], [0., 1., 0.], 5.).is_none());
        assert!(world
            .ray_hit([f32::NAN, 0., 0.], [0., -1., 0.], 5.)
            .is_none());
    }
    #[test]
    fn all_hit_ray_retains_multiple_surfaces_inside_one_mesh_and_other_owners() {
        let triangle = |y: f32| Triangle {
            p: [[-10., y, -10.], [0., y, 10.], [10., y, -10.]],
        };
        let world = Collision::with_triangle_owners(
            vec![triangle(0.), triangle(1.), triangle(2.)],
            vec![0.; 3],
            vec![7, 7, 91],
        )
        .unwrap();
        assert_eq!(world.triangle_count(), 3);
        #[cfg(test)]
        assert_eq!(world.triangles.len(), world.triangle_count());
        let hits = world.ray_hits([0., 3., 0.], [0., -4., 0.]).unwrap();
        assert_eq!(hits.len(), 3, "{hits:?}");
        let mut sorted = hits.iter().collect::<Vec<_>>();
        sorted.sort_by(|a, b| a.fraction.total_cmp(&b.fraction));
        assert_eq!(
            sorted.iter().map(|h| h.owner).collect::<Vec<_>>(),
            vec![91, 7, 7]
        );
        assert_eq!(
            sorted.iter().map(|h| h.source_triangle).collect::<Vec<_>>(),
            vec![Some(2), Some(1), Some(0)]
        );
        assert_eq!(
            sorted.iter().map(|h| h.fraction).collect::<Vec<_>>(),
            vec![0.25, 0.5, 0.75]
        );
        assert_eq!(sorted[1].shape, sorted[2].shape);
        assert_ne!(sorted[1].triangle, sorted[2].triangle);
        assert!(hits
            .iter()
            .all(|h| h.normal == [0., 1., 0.] && h.triangle.is_some()));
        assert!(world
            .ray_hits([0., 3., 0.], [0., 1., 0.])
            .unwrap()
            .is_empty());
        assert!(world.ray_hits([0.; 3], [0.; 3]).unwrap().is_empty());
        assert!(world.ray_hits([f32::NAN; 3], [0., -1., 0.]).is_err());
        // Interleaved source owners must take the grouping fallback while
        // retaining the same global source-triangle identities.
        let interleaved = Collision::with_triangle_owners(
            vec![triangle(0.), triangle(1.), triangle(2.)],
            vec![0.; 3],
            vec![7, 91, 7],
        )
        .unwrap();
        assert_eq!(interleaved.triangle_count(), 3);
        #[cfg(test)]
        assert_eq!(interleaved.triangles.len(), interleaved.triangle_count());
        let mut interleaved_hits = interleaved.ray_hits([0., 3., 0.], [0., -4., 0.]).unwrap();
        interleaved_hits.sort_by(|a, b| a.fraction.total_cmp(&b.fraction));
        assert_eq!(
            interleaved_hits
                .iter()
                .map(|hit| (hit.owner, hit.source_triangle))
                .collect::<Vec<_>>(),
            vec![(7, Some(2)), (91, Some(1)), (7, Some(0))]
        );
        // Exercise internal BVH nodes, not just a single triangle leaf.
        let layered = Collision::with_triangle_owners(
            (0..64).map(|y| triangle(y as f32)).collect(),
            vec![0.; 64],
            vec![8; 64],
        )
        .unwrap();
        let layered_hits = layered.ray_hits([0., 65., 0.], [0., -66., 0.]).unwrap();
        assert_eq!(layered_hits.len(), 64);
        for hit in &layered_hits {
            assert_eq!(hit.position[1], hit.source_triangle.unwrap() as f32);
        }
        let mut indices = layered_hits
            .iter()
            .map(|h| h.triangle.unwrap())
            .collect::<Vec<_>>();
        indices.sort_unstable();
        indices.dedup();
        assert_eq!(indices.len(), 64);
    }
    #[test]
    fn triangle_provenance_survives_welding_culling_and_coincident_surfaces() {
        let t = Triangle {
            p: [[-2., 0., -2.], [0., 0., 2.], [2., 0., -2.]],
        };
        let degenerate = Triangle { p: [[0.; 3]; 3] };
        let world = Collision::with_triangle_owners(
            vec![degenerate, t.clone(), t],
            vec![0.; 3],
            vec![9; 3],
        )
        .unwrap();
        let mut ids = world
            .ray_hits([0., 1., 0.], [0., -2., 0.])
            .unwrap()
            .into_iter()
            .map(|h| h.source_triangle.unwrap())
            .collect::<Vec<_>>();
        ids.sort_unstable();
        assert_eq!(ids, vec![1, 2]);
    }
    #[test]
    fn projectile_sphere_sweep_preserves_volume_and_owner() {
        let triangle = |y: f32| Triangle {
            p: [[-10., y, -10.], [0., y, 10.], [10., y, -10.]],
        };
        let world = Collision::with_triangle_owners(
            vec![triangle(0.), triangle(1.)],
            vec![0.; 2],
            vec![7, 91],
        )
        .unwrap();
        let hit = world
            .sweep_sphere([0., 3., 0.], [0., -4., 0.], 0.25)
            .unwrap()
            .unwrap();
        assert_eq!(hit.owner, 91);
        assert_eq!(hit.source_triangle, Some(1));
        let ray = world
            .ray_hits([0., 3., 0.], [0., -4., 0.])
            .unwrap()
            .into_iter()
            .find(|ray| ray.source_triangle == hit.source_triangle)
            .unwrap();
        assert_eq!(hit.shape, ray.shape);
        // Box3D's shape-cast tolerance is tested as geometry, not declared
        // equivalent to Havok's contact timing.
        assert!((hit.fraction - 0.4375).abs() < 0.002, "{hit:?}");
        assert!(hit.normal[1] > 0.99);
        assert!(world
            .sweep_sphere([0., 3., 0.], [0., 4., 0.], 0.25)
            .unwrap()
            .is_none());
        assert!(world
            .sweep_sphere([0., 3., 0.], [0.; 3], 0.25)
            .unwrap()
            .is_none());
        assert!(world
            .sweep_sphere([0., 3., 0.], [0., -4., 0.], f32::NAN)
            .is_err());
    }
    #[test]
    fn lands_and_jumps() {
        let w = floor();
        let mut p = Player::new([0., 3., 0.], Motion::default());
        for _ in 0..240 {
            p.step(&w, [0.; 3], false, 1. / 30.);
        }
        assert!(p.grounded);
        assert!(
            p.position[1].abs() < 0.01,
            "floor contact: {:?}",
            p.position
        );
        let origin = p.position[1];
        let mut apex = 0f32;
        for i in 0..180 {
            p.step(&w, [0.; 3], (i == 0, true), 1. / 30.);
            apex = apex.max(p.position[1]);
        }
        // Four completed retail arcs in the recorded VillageTest capture.
        assert!(
            (apex - origin - 2.389667).abs() < 0.00005,
            "held relative apex {}",
            apex - origin
        );
        assert!(p.grounded);
    }
    #[test]
    fn tap_jump_uses_first_stage_and_holding_does_not_auto_repeat() {
        let w = floor();
        let mut p = Player::new([0.; 3], Motion::default());
        for _ in 0..3 {
            p.step(&w, [0.; 3], false, 1. / 30.);
        }
        let origin = p.position[1];
        let mut apex = 0f32;
        for i in 0..180 {
            p.step(&w, [0.; 3], i == 0, 1. / 30.);
            apex = apex.max(p.position[1]);
        }
        // Synthetic tap: 6 m/s launch, one coast update, then 30 m/s² gravity.
        // A matching original-game short-press capture is still required.
        assert!(
            (apex - origin - 0.9).abs() < 0.00005,
            "tap relative apex {}",
            apex - origin
        );
        assert!(p.grounded);
        for _ in 0..60 {
            p.step(&w, [0.; 3], (false, true), 1. / 120.);
        }
        assert!(p.grounded && p.position[1].abs() < 0.01);
    }
    #[test]
    fn grounded_diagonal_has_no_speed_bonus() {
        let w = floor();
        let mut a = Player::new([0.; 3], Motion::default());
        let mut b = Player::new([0.; 3], Motion::default());
        // Compare equal initial facing alignment; turn transients legitimately
        // differ when both characters start facing the same world direction.
        a.yaw = std::f32::consts::FRAC_PI_2;
        b.yaw = std::f32::consts::FRAC_PI_4;
        // Establish ground contact first. Independent air-axis acceleration
        // intentionally gives a different transient diagonal trajectory.
        for _ in 0..2 {
            a.step(&w, [0.; 3], false, 1. / 120.);
            b.step(&w, [0.; 3], false, 1. / 120.);
        }
        assert!(a.grounded && b.grounded);
        for _ in 0..120 {
            a.step(&w, [1., 0., 0.], false, 1. / 120.);
            b.step(&w, [1., 0., 1.], false, 1. / 120.);
        }
        assert!((length(a.position) - length(b.position)).abs() < 0.001);
    }
    #[test]
    fn priority_speed_override_drives_ground_motion_without_editing_authored_speed() {
        let w = floor();
        let mut p = Player::new([0.; 3], Motion::default());
        let authored = p.motion.speed;
        for _ in 0..3 {
            p.step(&w, [0.; 3], false, 1. / 30.);
        }
        p.speed_overrides.set(1, 0.7).unwrap();
        p.speed_overrides.set(3, 0.2).unwrap();
        p.speed_overrides.remove(1);
        for _ in 0..60 {
            p.step(&w, [0., 0., 1.], false, 1. / 30.);
        }
        assert!(p.grounded);
        assert!((p.velocity[2] - 0.2).abs() < 0.001, "{:?}", p.velocity);
        assert_eq!(p.motion.speed, authored);
        p.speed_overrides.remove(3);
        for _ in 0..60 {
            p.step(&w, [0., 0., 1.], false, 1. / 30.);
        }
        assert!((p.velocity[2] - authored).abs() < 0.001, "{:?}", p.velocity);
    }
    #[test]
    fn brush_facing_target_turns_while_stationary_and_restores_input_on_stop() {
        let w = floor();
        let mut p = Player::new([0.; 3], Motion::default());
        for _ in 0..3 {
            p.step(&w, [0.; 3], false, 1. / 30.);
        }
        p.begin_brush_motion(2.5, 5.23598).unwrap();
        p.set_aim_facing_target(0.5);
        p.step(&w, [0.; 3], false, 1. / 30.);
        assert!(p.yaw > 0. && p.yaw < 0.5);
        for _ in 0..30 {
            p.step(&w, [0., 0., 1.], false, 1. / 30.);
        }
        assert_eq!(p.facing_target, 0.5);
        assert!((p.yaw - 0.5).abs() < 0.001);
        p.end_brush_motion();
        p.step(&w, [0., 0., 1.], false, 1. / 30.);
        assert_eq!(p.facing_target, 0.);
        assert_eq!(p.max_move_speed(), p.motion.speed);
    }
    #[test]
    fn brush_strafes_sideways_without_rotating_movement_into_aim_direction() {
        let w = floor();
        let mut p = Player::new([0.; 3], Motion::default());
        for _ in 0..3 {
            p.step(&w, [0.; 3], false, 1. / 30.);
        }
        p.begin_brush_motion(2.5, 5.23598).unwrap();
        p.set_aim_facing_target(0.);
        for _ in 0..30 {
            p.step(&w, [1., 0., 0.], false, 1. / 30.);
        }
        assert!(p.grounded);
        assert!((p.velocity[0] - 2.5).abs() < 0.001, "{:?}", p.velocity);
        assert!(p.velocity[2].abs() < 0.001);
        assert_eq!(p.yaw, 0.);
        assert!(!p.motion.maintain_ground_velocity);
        p.end_brush_motion();
        assert!(!p.maintains_ground_velocity());
        // Restoration reads the current setting, not a saved start-time copy.
        p.begin_brush_motion(2.5, 5.23598).unwrap();
        p.motion.maintain_ground_velocity = true;
        p.end_brush_motion();
        assert!(p.maintains_ground_velocity());
    }
    #[test]
    fn ground_facing_matches_retail_turn_startup_samples() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tools/blotworx/fixtures/ground_turn_startup.json"
        ))
        .unwrap();
        let tolerance = fixture["tolerance"].as_f64().unwrap() as f32;
        for sample in fixture["samples"].as_array().unwrap() {
            let pair =
                |key: &str| -> [f32; 2] { serde_json::from_value(sample[key].clone()).unwrap() };
            let limited = ground_velocity_step(
                pair("incoming"),
                pair("desired"),
                12.,
                25.,
                sample["dt"].as_f64().unwrap() as f32,
            );
            let actual = ground_facing_velocity(limited, pair("facing"), 5., false);
            let expected = pair("expected");
            assert!(
                (actual[0] - expected[0]).hypot(actual[1] - expected[1]) < tolerance,
                "sample {}: {actual:?} != {expected:?}",
                sample["seconds"]
            );
        }
        assert_eq!(
            ground_facing_velocity([6., 8.], [0., 1.], 5., true),
            [3., 4.]
        );
        assert_eq!(
            ground_facing_velocity([6., 8.], [0., 1.], 5., false),
            [0., 5.]
        );
    }
    #[test]
    fn ground_movement_uses_facing_before_this_ticks_turn() {
        let mut world = floor();
        world.set_player_hull(&test_hull()).unwrap();
        let mut player = Player::new([0.; 3], Motion::default());
        for _ in 0..2 {
            player.step(&world, [0.; 3], false, 1. / 30.);
        }
        assert!(player.grounded);
        player.step(&world, [-0.8660254, 0., -0.5], false, 1. / 30.);
        let vectors = player.ground_vectors.unwrap();
        assert_eq!(vectors.movement[0], 0.);
        assert!((vectors.movement[2] - 0.4).abs() < 1e-6);
        assert!((vectors.desired[0] + 4.330127).abs() < 1e-5);
        assert!((vectors.desired[2] + 2.5).abs() < 1e-6);
        let mut history = super::super::ground_history::History::default();
        history
            .push(vectors.movement, vectors.desired, 1. / 30.)
            .unwrap();
        assert!((history.latest().unwrap().speed - 0.4).abs() < 1e-6);
        assert!((history.latest().unwrap().secondary_speed - 5.).abs() < 1e-6);
        assert!(
            player.position[0].abs() < 1e-6 && player.position[2] > 0.013,
            "incoming-facing movement: {:?}, velocity {:?}",
            player.position,
            player.velocity
        );
        assert!((player.yaw + std::f32::consts::TAU / 10.).abs() < 1e-6);
        let mut motion = Motion::default();
        motion.maintain_ground_velocity = true;
        let mut player = Player::new([0.; 3], motion);
        for _ in 0..2 {
            player.step(&world, [0.; 3], false, 1. / 30.);
        }
        assert!(player.grounded);
        player.step(&world, [-0.8660254, 0., -0.5], false, 1. / 30.);
        assert!(player.position[0] < 0. && player.position[2] < 0.);
    }
    #[test]
    fn wall_blocks_motion() {
        let mut w = floor().triangles.clone();
        w.push(Triangle {
            p: [[2., 0., -10.], [2., 5., 10.], [2., 5., -10.]],
        });
        w.push(Triangle {
            p: [[2., 0., -10.], [2., 0., 10.], [2., 5., 10.]],
        });
        let w = Collision::new(w).unwrap();
        let mut p = Player::new([0.; 3], Motion::default());
        for _ in 0..240 {
            p.step(&w, [1., 0., 0.], false, 1. / 120.);
        }
        assert!(p.position[0] <= 1.71, "wall contact: {:?}", p.position);
    }
    #[test]
    fn respawn_clears_velocity() {
        let mut p = Player::new([1., 2., 3.], Motion::default());
        p.velocity = [20.; 3];
        p.position = [0., -100., 0.];
        p.step(&floor(), [0.; 3], false, 1. / 120.);
        assert_eq!(p.position, p.spawn);
        assert_eq!(p.velocity, [0.; 3]);
    }
}
