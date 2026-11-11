use serde::{Deserialize, Serialize};

use super::{Model, ModelError, ModelScene};

/// What a model offers a `model_3d` node to choose from: the clips `animation` names, the
/// materials `texture_material` names and the cameras `camera` names.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub animations: Vec<AnimationInfo>,
    pub materials: Vec<MaterialInfo>,
    #[serde(default)]
    pub cameras: Vec<CameraInfo>,
}

/// One animation clip. `animation` selects the first clip of that `name`, or else the clip at that
/// `index`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationInfo {
    pub index: usize,
    /// Absent when the file doesn't name the clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Seconds from the start to the last key of any channel, morph target weights' included.
    pub duration: f32,
}

/// One material. `texture_material` selects the first material of that `name`; an unnamed material
/// can't be selected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterialInfo {
    pub index: usize,
    /// Absent when the file doesn't name the material.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whether a texture multiplies its base colour, which a `texture` input replaces.
    pub has_base_color_texture: bool,
}

/// One camera. `camera` selects the first camera of that `name`, or else the camera at that
/// `index`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraInfo {
    pub index: usize,
    /// Absent when the file doesn't name the camera.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ModelInfo {
    /// Reads a binary glTF as [`Model::from_glb`] does and reports what it holds.
    pub fn from_glb(bytes: &[u8]) -> Result<Self, ModelError> {
        Model::from_glb(bytes).map(|model| model.scene.info())
    }
}

impl ModelScene {
    /// The scene's animation clips, materials and cameras, in file order.
    pub fn info(&self) -> ModelInfo {
        let named = |name: &str| (!name.is_empty()).then(|| name.to_string());
        ModelInfo {
            animations: self
                .animations
                .iter()
                .enumerate()
                .map(|(index, animation)| AnimationInfo {
                    index,
                    name: named(&animation.name),
                    duration: animation.duration,
                })
                .collect(),
            materials: self
                .materials
                .iter()
                .enumerate()
                .map(|(index, material)| MaterialInfo {
                    index,
                    name: named(&material.name),
                    has_base_color_texture: material.maps.base_color.is_some(),
                })
                .collect(),
            cameras: self
                .cameras
                .iter()
                .enumerate()
                .map(|(index, camera)| CameraInfo {
                    index,
                    name: named(&camera.name),
                })
                .collect(),
        }
    }
}
