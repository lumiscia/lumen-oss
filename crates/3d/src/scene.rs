use glam::{Mat4, Vec3};

use crate::{Bounds, Camera, Light};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Blending {
    /// Hides what is behind it.
    Opaque,
    /// Hides what is behind it where its alpha reaches `cutoff`, and draws nothing elsewhere.
    Mask { cutoff: f32 },
    /// Shows what is behind it through its alpha, drawn after opaque objects.
    Blend,
}

/// How a texture is read: what lies past its edges and how its texels are filtered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sampler {
    pub wrap_u: Wrap,
    pub wrap_v: Wrap,
    /// Filtering where a texel covers more than a pixel.
    pub mag: Filter,
    /// Filtering where a texel covers less than a pixel.
    pub min: Filter,
    /// Filtering between mip levels, or `None` to read only the full-size level.
    pub mipmap: Option<Filter>,
}

impl Default for Sampler {
    /// Repeating, with trilinear filtering.
    fn default() -> Self {
        Self {
            wrap_u: Wrap::Repeat,
            wrap_v: Wrap::Repeat,
            mag: Filter::Linear,
            min: Filter::Linear,
            mipmap: Some(Filter::Linear),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Wrap {
    Repeat,
    MirroredRepeat,
    ClampToEdge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Filter {
    Nearest,
    Linear,
}

/// A texture a surface reads through its mesh's texture coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Map<T> {
    pub texture: T,
    pub sampler: Sampler,
}

/// A metallic-roughness material's textures, each multiplying its factor. Base colour and
/// emissive textures hold sRGB-encoded colour with straight alpha, the rest linear values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Maps<T> {
    pub base_color: Option<Map<T>>,
    /// Roughness in green and metalness in blue.
    pub metallic_roughness: Option<Map<T>>,
    /// Tangent-space normals, +X along U and +Y against V.
    pub normal: Option<Map<T>>,
    pub emissive: Option<Map<T>>,
    /// Ambient occlusion in red.
    pub occlusion: Option<Map<T>>,
}

impl<T> Default for Maps<T> {
    fn default() -> Self {
        Self {
            base_color: None,
            metallic_roughness: None,
            normal: None,
            emissive: None,
            occlusion: None,
        }
    }
}

impl<T> Maps<T> {
    /// The same maps reading the textures `texture` gives for each of these.
    pub fn map<U>(&self, mut texture: impl FnMut(&T) -> U) -> Maps<U> {
        let mut one = |map: &Option<Map<T>>| {
            map.as_ref().map(|map| Map {
                texture: texture(&map.texture),
                sampler: map.sampler,
            })
        };
        Maps {
            base_color: one(&self.base_color),
            metallic_roughness: one(&self.metallic_roughness),
            normal: one(&self.normal),
            emissive: one(&self.emissive),
            occlusion: one(&self.occlusion),
        }
    }
}

/// Which of a mesh's texture coordinates a map reads, and how they are moved first, as
/// `KHR_texture_transform` moves them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapUv {
    /// 0 reads the first set, 1 the second.
    pub set: u8,
    /// The rows of an affine transform of `(u, v, 1)`: the map reads at
    /// `(rows[0] · (u, v, 1), rows[1] · (u, v, 1))`.
    pub transform: [[f32; 3]; 2],
}

impl Default for MapUv {
    /// The first set, unmoved.
    fn default() -> Self {
        Self {
            set: 0,
            transform: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        }
    }
}

impl MapUv {
    /// `KHR_texture_transform`'s scale, then rotation in radians, then offset, of texture
    /// coordinates in `set`.
    pub fn new(set: u8, offset: [f32; 2], rotation: f32, scale: [f32; 2]) -> Self {
        let (sin, cos) = rotation.sin_cos();
        Self {
            set,
            transform: [
                [cos * scale[0], sin * scale[1], offset[0]],
                [-sin * scale[0], cos * scale[1], offset[1]],
            ],
        }
    }
}

/// Where a morphed mesh's vertices find their morph targets' displacements: vertex
/// `first_vertex + i` reads `targets` [`MorphDelta`](crate::MorphDelta)s from
/// `first_delta + i * targets`, one per target in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Morph {
    pub first_delta: u32,
    pub first_vertex: u32,
    pub targets: u32,
}

/// How an object's surface is shaded, in linear light.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surface<T> {
    /// Linear colour with straight alpha, which the texture and base colour map are multiplied by.
    pub base_color: [f32; 4],
    /// A Lumen raster, premultiplied and sRGB-encoded, whose texels colour the surface through
    /// the mesh's texture coordinates.
    pub texture: Option<T>,
    pub maps: Maps<T>,
    /// The texture coordinates each map reads: base colour, metallic-roughness, normal, emissive
    /// and occlusion.
    pub map_uvs: [MapUv; 5],
    /// Multiplies the base colour by the mesh's vertex colours.
    pub vertex_colors: bool,
    /// 0 is a dielectric, 1 a bare metal.
    pub metallic: f32,
    /// 0 is a mirror finish, 1 fully diffuse.
    pub roughness: f32,
    /// Linear light the surface gives off on its own.
    pub emissive: [f32; 3],
    /// How strongly the normal map tilts normals.
    pub normal_scale: f32,
    /// How much the occlusion map darkens light from the surroundings, 0 to 1.
    pub occlusion_strength: f32,
    pub blending: Blending,
    /// Shows the base colour as is, ignoring lights.
    pub unlit: bool,
    /// Draws the back of each face too.
    pub double_sided: bool,
    /// Lights each face by its own flat normal, for meshes without vertex normals.
    pub flat_normals: bool,
    /// Blocks light from shadow-casting lights: where its alpha reaches its mask's cutoff, or half
    /// for a blended surface.
    pub cast_shadows: bool,
    /// Darkens where shadows fall on it. An unlit surface shows none.
    pub receive_shadows: bool,
    /// Shows nothing but the shadows falling on it, as black whose alpha is the fraction of light
    /// they take away, times the base colour's alpha. It hides what is behind it, casts no shadows
    /// and is drawn before other objects.
    pub shadow_catcher: bool,
}

impl<T> Default for Surface<T> {
    /// An opaque, white, half-rough dielectric.
    fn default() -> Self {
        Self {
            base_color: [1.0; 4],
            texture: None,
            maps: Maps::default(),
            map_uvs: [MapUv::default(); 5],
            vertex_colors: false,
            metallic: 0.0,
            roughness: 0.5,
            emissive: [0.0; 3],
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            blending: Blending::Opaque,
            unlit: false,
            double_sided: false,
            flat_normals: false,
            cast_shadows: true,
            receive_shadows: true,
            shadow_catcher: false,
        }
    }
}

/// The most nodes one frame's scene may expand to, counting a node each time it is reached, which
/// also bounds the objects it draws.
pub const MAX_SCENE_NODES: usize = 100_000;

/// One mesh drawn somewhere in a scene. `M` names the mesh and `T` the texture to whoever draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct Object<M, T> {
    /// Mesh to world. A skinned object's vertices move by its joints instead, and this only says
    /// which way its faces wind.
    pub world: Mat4,
    pub mesh: M,
    /// The first of the mesh's indices the object draws.
    pub first_index: u32,
    /// Indices the mesh draws, three to a triangle.
    pub index_count: u32,
    pub surface: Surface<T>,
    /// The point blended objects sort by, in world space.
    pub center: Vec3,
    /// Where the object's joint matrices start in the scene's `joints`, for a skinned object.
    pub first_joint: Option<u32>,
    /// For a morphed object, where its displacements are and where its weights start in the
    /// scene's `weights`.
    pub morph: Option<(Morph, u32)>,
    /// What the object covers in world space, at least, which shadows are fitted to. A scene
    /// without lights casting shadows may leave it empty.
    pub bounds: Bounds,
    /// For an object drawing [`Mesh::textured_quad`](crate::Mesh::textured_quad), the part of it
    /// drawn: its left, top, right and bottom edges as fractions of the quad's width and height
    /// from its top left, where they are and with the texture coordinates they have there. Its
    /// raster is read only from within the part. `None` draws the whole mesh.
    pub part: Option<[f32; 4]>,
}

/// What a scene holds at one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene<M, T> {
    pub objects: Vec<Object<M, T>>,
    pub lights: Vec<Light>,
    /// The first camera found, if any.
    pub camera: Option<Camera>,
    /// Skinned objects' joint matrices, which take their vertices to world space.
    pub joints: Vec<Mat4>,
    /// Morphed objects' morph target weights.
    pub weights: Vec<f32>,
}

impl<M, T> Default for Scene<M, T> {
    fn default() -> Self {
        Self {
            objects: Vec::new(),
            lights: Vec::new(),
            camera: None,
            joints: Vec::new(),
            weights: Vec::new(),
        }
    }
}
