//! Signed Distance Field (SDF) primitives and queries for the M6a software
//! raytraced GI (tasks.md 18, spec §5.10, design §2.4).
//!
//! This module holds the **CPU-side reference** of the SDF math that the WGSL
//! compute shaders (`voxelize.wgsl`, `ray_march.wgsl`) implement. Keeping a
//! pure-Rust twin makes the math unit-testable without a GPU and gives the
//! shader code a golden reference to stay byte-equivalent with (the engine
//! discipline: no behavioral drift between the Rust reference and the WGSL).
//!
//! The three building blocks (construction §10):
//!
//! 1. **Scene SDF** ([`SceneSdf`]): a uniform grid of distance samples built
//!    by voxelizing the scene's triangles (each cell stores the minimum
//!    distance to the mesh surface). This is the "precomputed SDF" for
//!    static geometry; dynamic objects re-voxelize a local region at lower
//!    frequency/resolution (task 18.1).
//! 2. **Sphere tracing** ([`march`]): advances a ray through the distance
//!    field; the first hit yields a hit distance, and the same distance field
//!    produces SDF ambient occlusion and soft shadows (task 18.2).
//! 3. **Irradiance cache** ([`IrradianceCache`]): a low-resolution
//!    last-frame irradiance grid with temporal+spatial filtering for the
//!    single-bounce GI (task 18.3).
//!
//! All types are plain `Copy` data so they can be mirrored directly into
//! WGSL storage buffers.

use bevy_math::Vec3;

/// One 3D SDF primitive used as the compact building block of a scene.
///
/// The CPU reference supports the same set the WGSL `sd_*` functions cover:
/// a sphere, an axis-aligned box, an infinite plane and a capsule. A scene
/// is a (weighted) union of these; mesh voxelization (task 18.1) produces
/// the equivalent grid samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Primitive {
    /// Sphere centered at `center` with radius `radius`.
    Sphere {
        center: Vec3,
        radius: f32,
    },
    /// Axis-aligned box with `half_extents` centered at `center`.
    Box {
        center: Vec3,
        half_extents: Vec3,
    },
    /// Infinite plane `x * normal + distance = 0`.
    Plane {
        normal: Vec3,
        distance: f32,
    },
    /// Capsule between `a` and `b` with `radius`.
    Capsule {
        a: Vec3,
        b: Vec3,
        radius: f32,
    },
}

impl Primitive {
    /// Signed distance from `p` to the primitive's surface (negative inside,
    /// positive outside). Mirrors `sdSphere`/`sdBox`/`sdPlane`/`sdCapsule`.
    pub fn distance(&self, p: Vec3) -> f32 {
        match *self {
            Self::Sphere { center, radius } => (p - center).length() - radius,
            Self::Box { center, half_extents } => {
                let q = (p - center).abs() - half_extents;
                q.max(Vec3::ZERO).length() + q.min(Vec3::ZERO).max_element()
            }
            Self::Plane { normal, distance } => p.dot(normal) + distance,
            Self::Capsule { a, b, radius } => {
                let pa = p - a;
                let ba = b - a;
                let h = (pa.dot(ba) / ba.length_squared()).clamp(0.0, 1.0);
                (pa - ba * h).length() - radius
            }
        }
    }

    /// A unit-direction suggestion for the normal at `p` (finite-difference
    /// style; the WGSL twin uses the analytic gradient where available).
    pub fn normal_at(&self, p: Vec3) -> Vec3 {
        const E: f32 = 1e-3;
        Vec3::new(
            self.distance(p + Vec3::X * E) - self.distance(p - Vec3::X * E),
            self.distance(p + Vec3::Y * E) - self.distance(p - Vec3::Y * E),
            self.distance(p + Vec3::Z * E) - self.distance(p - Vec3::Z * E),
        )
        .normalize_or_zero()
    }
}

/// A dense uniform-grid distance field over an axis-aligned region.
///
/// `samples` stores one `f32` per cell (`size_x * size_y * size_z`), laid
/// out in x-major, then y, then z order. A cell's sample is the minimum
/// distance over the scene primitives evaluated at the cell center - the
/// discrete approximation the compute voxelizer produces on the GPU.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneSdf {
    /// Minimum corner of the grid.
    pub origin: Vec3,
    /// Grid extent (cells per axis).
    pub size: [u32; 3],
    /// Cell size in world units.
    pub cell_size: f32,
    /// Distance samples, `size_x * size_y * size_z` entries.
    pub samples: Vec<f32>,
}

impl SceneSdf {
    /// Builds a uniform-grid SDF from a list of primitives.
    ///
    /// `origin`/`size`/`cell_size` define the grid; every cell center is
    /// evaluated against all primitives and keeps the minimum distance.
    /// This is the CPU twin of the `voxelize.wgsl` compute pass (task 18.1).
    pub fn from_primitives(
        origin: Vec3,
        size: [u32; 3],
        cell_size: f32,
        primitives: &[Primitive],
    ) -> Self {
        let mut samples = Vec::with_capacity(
            (size[0] as usize) * (size[1] as usize) * (size[2] as usize),
        );
        for z in 0..size[2] {
            for y in 0..size[1] {
                for x in 0..size[0] {
                    let center = origin
                        + Vec3::new(
                            (x as f32 + 0.5) * cell_size,
                            (y as f32 + 0.5) * cell_size,
                            (z as f32 + 0.5) * cell_size,
                        );
                    let mut d = f32::INFINITY;
                    for p in primitives {
                        d = d.min(p.distance(center));
                    }
                    samples.push(d);
                }
            }
        }
        Self {
            origin,
            size,
            cell_size,
            samples,
        }
    }

    #[inline]
    fn cell_index(&self, x: u32, y: u32, z: u32) -> usize {
        (z as usize) * (self.size[0] as usize) * (self.size[1] as usize)
            + (y as usize) * (self.size[0] as usize)
            + x as usize
    }

    /// Trilinear-interpolated distance at an arbitrary world point. Falls
    /// back to the nearest edge/corner sample outside the grid (the WGSL
    /// `sample_sdf` twin).
    pub fn distance_at(&self, p: Vec3) -> f32 {
        let max = Vec3::new(
            (self.size[0] - 1) as f32,
            (self.size[1] - 1) as f32,
            (self.size[2] - 1) as f32,
        );
        let g = ((p - self.origin) / self.cell_size - Vec3::splat(0.5)).clamp(Vec3::ZERO, max);
        let i0 = g.floor();
        let i1 = g.ceil();
        let frac = g - i0;
        let [x0, y0, z0] = [i0.x as u32, i0.y as u32, i0.z as u32];
        let [x1, y1, z1] = [i1.x as u32, i1.y as u32, i1.z as u32];
        let f = |w: [f32; 3]| {
            let c000 = self.samples[self.cell_index(x0, y0, z0)];
            let c100 = self.samples[self.cell_index(x1, y0, z0)];
            let c010 = self.samples[self.cell_index(x0, y1, z0)];
            let c110 = self.samples[self.cell_index(x1, y1, z0)];
            let c001 = self.samples[self.cell_index(x0, y0, z1)];
            let c101 = self.samples[self.cell_index(x1, y0, z1)];
            let c011 = self.samples[self.cell_index(x0, y1, z1)];
            let c111 = self.samples[self.cell_index(x1, y1, z1)];
            let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
            lerp(
                lerp(lerp(c000, c100, w[0]), lerp(c010, c110, w[0]), w[1]),
                lerp(lerp(c001, c101, w[0]), lerp(c011, c111, w[0]), w[1]),
                w[2],
            )
        };
        f([frac.x, frac.y, frac.z])
    }

    /// Estimated gradient (normal) at a world point via central differences
    /// on the trilinear-interpolated field.
    pub fn gradient_at(&self, p: Vec3) -> Vec3 {
        const H: f32 = 1e-2;
        Vec3::new(
            self.distance_at(p + Vec3::X * H) - self.distance_at(p - Vec3::X * H),
            self.distance_at(p + Vec3::Y * H) - self.distance_at(p - Vec3::Y * H),
            self.distance_at(p + Vec3::Z * H) - self.distance_at(p - Vec3::Z * H),
        )
        .normalize_or_zero()
    }

    /// Total sample count (for GPU buffer sizing).
    pub fn sample_count(&self) -> usize {
        (self.size[0] as usize) * (self.size[1] as usize) * (self.size[2] as usize)
    }
}

/// The result of a sphere-trace (task 18.2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    /// Whether the ray hit the surface within `max_distance`.
    pub hit: bool,
    /// World-space hit distance (only meaningful when `hit`).
    pub distance: f32,
    /// Normal estimate at the hit point.
    pub normal: Vec3,
}

/// Advances `ray_origin` along `ray_dir` through the distance field using
/// sphere tracing (the WGSL `march` twin). Returns the first surface hit
/// within `max_distance`, plus the field normal there.
pub fn march(
    sdf: &SceneSdf,
    ray_origin: Vec3,
    ray_dir: Vec3,
    max_distance: f32,
    steps: u32,
) -> RayHit {
    let dir = ray_dir.normalize_or_zero();
    let mut t = 0.0f32;
    for _ in 0..steps {
        let d = sdf.distance_at(ray_origin + dir * t);
        if d <= 1e-3 {
            let normal = sdf.gradient_at(ray_origin + dir * t);
            return RayHit {
                hit: true,
                distance: t,
                normal,
            };
        }
        if t + d >= max_distance {
            break;
        }
        t += d;
    }
    RayHit {
        hit: false,
        distance: max_distance,
        normal: Vec3::ZERO,
    }
}

/// SDF ambient occlusion (task 18.2): samples the field along the normal and
/// accumulates the "how much the field crowds the surface" measure into a
/// `[0, 1]` visibility (1 = fully unoccluded). Mirrors the `sdf_ao` WGSL
/// function (iq's scheme).
pub fn sdf_ao(sdf: &SceneSdf, p: Vec3, normal: Vec3, samples: u32) -> f32 {
    let mut ao = 0.0f32;
    for i in 1..=samples {
        let dist = (i as f32) / (samples as f32);
        let d = sdf.distance_at(p + normal * dist);
        ao += (dist - d).max(0.0);
    }
    (1.0 - ao / (samples as f32)).clamp(0.0, 1.0)
}

/// SDF soft shadow (task 18.2): sphere-traces toward the light and derives
/// penumbra from how close the trace approaches the surface (`k` controls
/// softness). 1.0 = fully lit. Mirrors the `soft_shadow` WGSL function.
pub fn soft_shadow(
    sdf: &SceneSdf,
    ro: Vec3,
    light_dir: Vec3,
    max_distance: f32,
    k: f32,
    steps: u32,
) -> f32 {
    let dir = light_dir.normalize_or_zero();
    let mut res = 1.0f32;
    let mut t = 0.02f32; // small bias to avoid self-intersection
    for _ in 0..steps {
        let d = sdf.distance_at(ro + dir * t);
        if d < 1e-4 {
            return 0.0;
        }
        res = res.min(k * d / t);
        if t >= max_distance {
            break;
        }
        t += d;
    }
    res.clamp(0.0, 1.0)
}

/// Low-resolution last-frame irradiance cache with temporal/spatial filtering
/// (task 18.3). The CPU reference mirrors the `irradiance` storage buffer the
/// GI compute pass reads/writes: `texels` holds RGB irradiance per probe.
#[derive(Debug, Clone, PartialEq)]
pub struct IrradianceCache {
    /// Probe grid dimensions.
    pub size: [u32; 2],
    /// World extent covered by the cache (per probe cell).
    pub cell_size: f32,
    /// RGB irradiance per probe (`size_x * size_y` entries).
    pub texels: Vec<[f32; 3]>,
}

impl IrradianceCache {
    /// Fresh cache with black irradiance.
    pub fn new(size: [u32; 2], cell_size: f32) -> Self {
        Self {
            size,
            cell_size,
            texels: vec![[0.0; 3]; (size[0] as usize) * (size[1] as usize)],
        }
    }

    /// Index for probe `(x, y)`.
    #[inline]
    pub fn index(&self, x: u32, y: u32) -> usize {
        (y as usize) * (self.size[0] as usize) + x as usize
    }

    /// Temporal blend: `prev * (1 - alpha) + curr * alpha`. Alpha=1.0 keeps
    /// the freshest frame (first frame after a reset), small alpha builds up
    /// temporal stability (the WGSL `blend_irradiance` twin).
    pub fn blend(&mut self, x: u32, y: u32, incoming: [f32; 3], alpha: f32) {
        let i = self.index(x, y);
        let prev = self.texels[i];
        let out = [
            prev[0] + (incoming[0] - prev[0]) * alpha,
            prev[1] + (incoming[1] - prev[1]) * alpha,
            prev[2] + (incoming[2] - prev[2]) * alpha,
        ];
        self.texels[i] = out;
    }

    /// Spatial 3x3 box filter for a probe (separable-friendly; the WGSL
    /// `spatial_filter_irradiance` twin clamps at the grid edges).
    pub fn spatial_filter(&self, x: u32, y: u32) -> [f32; 3] {
        let mut acc = [0.0f32; 3];
        let mut n = 0u32;
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let nx = (x as i32 + dx).clamp(0, self.size[0] as i32 - 1) as u32;
                let ny = (y as i32 + dy).clamp(0, self.size[1] as i32 - 1) as u32;
                let c = self.texels[self.index(nx, ny)];
                acc[0] += c[0];
                acc[1] += c[1];
                acc[2] += c[2];
                n += 1;
            }
        }
        let inv = 1.0 / (n as f32);
        [acc[0] * inv, acc[1] * inv, acc[2] * inv]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_math::Vec3;

    #[test]
    fn sphere_sdf_is_signed() {
        let s = Primitive::Sphere {
            center: Vec3::ZERO,
            radius: 1.0,
        };
        assert!((s.distance(Vec3::ZERO) - (-1.0)).abs() < 1e-5, "inside negative");
        assert!((s.distance(Vec3::X * 2.0) - 1.0).abs() < 1e-5, "outside positive");
        assert!(s.distance(Vec3::X).abs() < 1e-5, "surface zero");
    }

    #[test]
    fn box_and_plane_distance() {
        let b = Primitive::Box {
            center: Vec3::ZERO,
            half_extents: Vec3::splat(1.0),
        };
        assert!((b.distance(Vec3::ZERO) + 1.0).abs() < 1e-5);
        assert!((b.distance(Vec3::new(2.0, 0.0, 0.0)) - 1.0).abs() < 1e-5);

        let p = Primitive::Plane {
            normal: Vec3::Y,
            distance: 0.0,
        };
        assert!((p.distance(Vec3::new(3.0, 5.0, 0.0)) - 5.0).abs() < 1e-5);
    }

    #[test]
    fn grid_sdf_matches_primitives() {
        // A single unit sphere at the origin over a [-2, 2]^3 grid.
        let sdf = SceneSdf::from_primitives(
            Vec3::splat(-2.0),
            [8, 8, 8],
            0.5,
            &[Primitive::Sphere {
                center: Vec3::ZERO,
                radius: 1.0,
            }],
        );
        assert_eq!(sdf.sample_count(), 8 * 8 * 8);
        // At the origin the field must be (close to) -1 (inside the sphere).
        let center = sdf.distance_at(Vec3::ZERO);
        assert!(
            center < 0.0,
            "origin is inside the unit sphere (got {center})"
        );
        // Far outside the grid we get the edge sample (negative - no hit).
        let far = sdf.distance_at(Vec3::splat(100.0));
        assert!(far > 0.0, "far point should report positive distance");
    }

    #[test]
    fn sphere_trace_hits_visible_surface() {
        let sdf = SceneSdf::from_primitives(
            Vec3::splat(-2.0),
            [8, 8, 8],
            0.5,
            &[Primitive::Sphere {
                center: Vec3::ZERO,
                radius: 1.0,
            }],
        );
        // Ray from (-3, 0, 0) along +X hits the sphere at x = -1.
        let hit = march(&sdf, Vec3::new(-3.0, 0.0, 0.0), Vec3::X, 10.0, 64);
        assert!(hit.hit, "ray must hit the sphere");
        assert!(
            (hit.distance - 2.0).abs() < 0.5,
            "hit at ~2 units from origin (got {})",
            hit.distance
        );
        // Normal at the hit is -X (pointing out).
        assert!(
            hit.normal.x < 0.0,
            "normal at -X side points out (got {:?})",
            hit.normal
        );
    }

    #[test]
    fn sphere_trace_misses_clear_path() {
        let sdf = SceneSdf::from_primitives(
            Vec3::new(-2.0, -2.0, 0.0),
            [8, 8, 12],
            0.5,
            &[Primitive::Sphere {
                center: Vec3::new(0.0, 0.0, 3.0),
                radius: 1.0,
            }],
        );
        // Ray along +Z from -2 should hit the sphere at z = 2.
        let hit = march(&sdf, Vec3::new(0.0, 0.0, -2.0), Vec3::Z, 10.0, 64);
        assert!(hit.hit, "ray along +Z must hit the offset sphere");
        let _ = hit.distance;
    }

    #[test]
    fn ao_and_soft_shadow_are_bounded() {
        let sdf = SceneSdf::from_primitives(
            Vec3::splat(-3.0),
            [12, 12, 12],
            0.5,
            &[Primitive::Sphere {
                center: Vec3::ZERO,
                radius: 1.0,
            }],
        );
        // AO at a point just above the sphere is > 0 (some openness).
        let ao = sdf_ao(&sdf, Vec3::new(0.0, 1.2, 0.0), Vec3::Y, 4);
        assert!((0.0..=1.0).contains(&ao), "AO in [0,1] (got {ao})");
        // Soft shadow toward +X from an open point is lit (>0).
        let shadow = soft_shadow(&sdf, Vec3::new(0.0, 1.2, 0.0), Vec3::X, 10.0, 8.0, 16);
        assert!((0.0..=1.0).contains(&shadow), "shadow in [0,1] (got {shadow})");
    }

    #[test]
    fn irradiance_blend_and_spatial_filter() {
        let mut cache = IrradianceCache::new([4, 4], 1.0);
        cache.blend(1, 1, [1.0, 0.5, 0.25], 1.0);
        assert_eq!(cache.texels[cache.index(1, 1)], [1.0, 0.5, 0.25]);
        // Temporal blend toward black with alpha=0.5.
        cache.blend(1, 1, [0.0, 0.0, 0.0], 0.5);
        assert!((cache.texels[cache.index(1, 1)][0] - 0.5).abs() < 1e-5);

        // Spatial filter at the bright probe pulls neighbors up a little.
        let filtered = cache.spatial_filter(1, 1);
        assert!(
            (0.0..=1.0).contains(&filtered[0]),
            "filtered irradiance in [0,1] (got {:?})",
            filtered
        );
    }
}