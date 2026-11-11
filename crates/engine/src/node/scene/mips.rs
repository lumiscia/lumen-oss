//! What `render_3d` needs to filter the rasters a scene samples trilinearly and anisotropically
//! when they are shown smaller than their size; the chains are [`crate::gpu::mips`].

use lumen_3d::Bounds;
use lumen_3d::glam::{Mat4, Vec2, Vec3, Vec4};
use lumen_gpu::{Size, wgpu};

use crate::gpu::mips::full_levels;

pub(super) const RASTER_SAMPLER: u64 = 0x3d00_0005;
/// The most texels a pixel's footprint is sampled along its long side, as hardware allows.
const ANISOTROPY: u16 = 16;

/// A raster shown with no more texels than this per pixel keeps its detail unfiltered.
const MINIFIED_ABOVE: f32 = 1.01;
/// A model's raster with no side longer than this has no chain, since what it loses to aliasing
/// is too little to see.
const MODEL_RASTER_MIN_SIDE: u32 = 64;

/// The sampler `render_3d` reads a raster with when it has a mip chain.
pub(super) fn raster_sampler() -> wgpu::SamplerDescriptor<'static> {
    wgpu::SamplerDescriptor {
        label: Some("lumen 3d mipmapped raster sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        anisotropy_clamp: ANISOTROPY,
        ..wgpu::SamplerDescriptor::default()
    }
}

/// How many mip levels a raster of `size` shown at up to `texels_per_pixel` needs.
pub(super) fn levels_for(size: Size, texels_per_pixel: f32) -> u32 {
    let needed = if texels_per_pixel.is_finite() {
        texels_per_pixel.max(1.0).log2().floor() as u32 + 2
    } else {
        u32::MAX
    };
    needed.clamp(1, full_levels(size))
}

/// Whether a raster of `size` that a model shows on a surface of unknown shape has a chain.
pub(super) fn model_raster_is_mipped(size: Size) -> bool {
    size.width.max(size.height) > MODEL_RASTER_MIN_SIDE
}

/// Whether showing `texels_per_pixel` of a raster in a pixel loses detail without mipmaps.
pub(super) fn is_minified(texels_per_pixel: f32) -> bool {
    texels_per_pixel > MINIFIED_ABOVE
}

/// The most texels of a plane's raster, `size` of them across the unit square in the XY plane that
/// `world` places, that a pixel of a `target`-sized render covers under `view_proj`, at the
/// square's corners and centre; infinite where it comes up to the camera, and `None` where it is
/// off screen.
pub(super) fn plane_texels_per_pixel(
    world: &Mat4,
    view_proj: &Mat4,
    size: Size,
    target: Size,
) -> Option<f32> {
    let to_clip = *view_proj * *world;
    let points = [
        (-0.5, -0.5),
        (0.5, -0.5),
        (-0.5, 0.5),
        (0.5, 0.5),
        (0.0, 0.0),
    ];
    let clip = points.map(|(x, y)| to_clip * Vec4::new(x, y, 0.0, 1.0));
    let outside = |inside: fn(Vec4) -> bool| clip.iter().all(|&point| !inside(point));
    if outside(|c| c.x >= -c.w)
        || outside(|c| c.x <= c.w)
        || outside(|c| c.y >= -c.w)
        || outside(|c| c.y <= c.w)
        || outside(|c| c.z >= 0.0)
        || outside(|c| c.z <= c.w)
    {
        return None;
    }
    if clip.iter().any(|c| c.w <= 1e-6) {
        return Some(f32::INFINITY);
    }
    let half = Vec2::new(target.width as f32, target.height as f32) * 0.5;
    // Where a step of a texel along `axis` moves the point in the target's pixels.
    let step = |c: Vec4, axis: Vec4| {
        let d = to_clip * axis;
        (Vec2::new(d.x, d.y) * c.w - Vec2::new(c.x, c.y) * d.w) / (c.w * c.w) * half
    };
    let (du, dv) = (
        Vec4::new(1.0 / size.width.max(1) as f32, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 1.0 / size.height.max(1) as f32, 0.0, 0.0),
    );
    let mut worst = 0.0f32;
    for c in clip {
        let (u, v) = (step(c, du), step(c, dv));
        // The smaller singular value of the step's matrix is how far a texel moves along the
        // direction it moves least.
        let sum = u.length_squared() + v.length_squared();
        let det = u.x * v.y - u.y * v.x;
        let spread = (sum * sum - 4.0 * det * det).max(0.0).sqrt();
        let least = ((sum - spread) * 0.5).max(0.0).sqrt();
        worst = worst.max(if least > 0.0 {
            1.0 / least
        } else {
            f32::INFINITY
        });
    }
    Some(worst)
}

/// Whether any of `bounds` is in view under `view_proj`. An empty box, whose place is unknown, is.
pub(super) fn bounds_in_view(bounds: &Bounds, view_proj: &Mat4) -> bool {
    if bounds.is_empty() {
        return true;
    }
    let clip: Vec<Vec4> = (0..8)
        .map(|corner| {
            let pick =
                |bit: u32, low: f32, high: f32| if corner >> bit & 1 == 0 { low } else { high };
            let point = Vec3::new(
                pick(0, bounds.min.x, bounds.max.x),
                pick(1, bounds.min.y, bounds.max.y),
                pick(2, bounds.min.z, bounds.max.z),
            );
            *view_proj * point.extend(1.0)
        })
        .collect();
    let outside = |inside: fn(Vec4) -> bool| clip.iter().all(|&point| !inside(point));
    !(outside(|c| c.x >= -c.w)
        || outside(|c| c.x <= c.w)
        || outside(|c| c.y >= -c.w)
        || outside(|c| c.y <= c.w)
        || outside(|c| c.z >= 0.0)
        || outside(|c| c.z <= c.w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_and_planes_reaching_past_both_the_near_and_far_planes_are_in_view() {
        let projection = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, 0.1, 100.0);
        let through = Bounds {
            min: Vec3::new(-10.0, -10.0, -200.0),
            max: Vec3::new(10.0, 10.0, 1.0),
        };
        assert!(bounds_in_view(&through, &projection));
        let behind = Bounds {
            min: Vec3::new(-10.0, -10.0, 1.0),
            max: Vec3::new(10.0, 10.0, 5.0),
        };
        assert!(!bounds_in_view(&behind, &projection));
        // A plane lying along the view axis, from behind the camera to past the far plane.
        let along = Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2)
            * Mat4::from_scale(Vec3::new(400.0, 1.0, 1.0));
        let view = Mat4::from_translation(Vec3::new(0.5, 0.0, 0.0));
        let texels = plane_texels_per_pixel(
            &along,
            &(projection * view),
            Size::new(64, 64),
            Size::new(100, 100),
        );
        assert!(texels.is_some());
    }

    /// A camera `distance` along +Z from a plane at the origin, whose unit square is `width`
    /// across, over a 100 x 100 render with a 90 degree field of view.
    fn texels_per_pixel(width: f32, tilt: f32, distance: f32, texels: u32) -> Option<f32> {
        let world = Mat4::from_rotation_x(tilt) * Mat4::from_scale(Vec3::splat(width));
        let view = Mat4::from_translation(Vec3::new(0.0, 0.0, -distance));
        let projection = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, 0.1, 100.0);
        plane_texels_per_pixel(
            &world,
            &(projection * view),
            Size::new(texels, texels),
            Size::new(100, 100),
        )
    }

    #[test]
    fn a_plane_covering_its_texels_is_not_minified_and_a_smaller_one_is() {
        // At distance 10 a 90 degree view is 20 units across 100 pixels: 5 pixels a unit.
        let facing = |width: f32, texels: u32| texels_per_pixel(width, 0.0, 10.0, texels).unwrap();
        assert!(!is_minified(facing(20.0, 100)));
        assert!(!is_minified(facing(20.0, 50)));
        assert!((facing(20.0, 200) - 2.0).abs() < 0.01);
        assert!(is_minified(facing(4.0, 100)));
        // Tilted back 60 degrees, its far half is squeezed to a fifth of the way it was.
        let tilted = texels_per_pixel(20.0, 1.047, 10.0, 100).unwrap();
        assert!(is_minified(tilted), "{tilted}");
    }

    #[test]
    fn a_plane_behind_the_camera_needs_no_mips() {
        assert_eq!(texels_per_pixel(1.0, 0.0, -10.0, 100), None);
    }

    #[test]
    fn levels_cover_the_footprint_and_one_more() {
        let size = Size::new(1024, 512);
        assert_eq!(levels_for(size, 1.0), 2);
        assert_eq!(levels_for(size, 3.9), 3);
        assert_eq!(levels_for(size, 4.0), 4);
        assert_eq!(levels_for(size, 1e9), 11);
        assert_eq!(levels_for(Size::new(1, 1), 8.0), 1);
        assert_eq!(levels_for(Size::new(256, 256), f32::INFINITY), 9);
    }
}
