//! Keyframe animation model and sampling primitives.

use crate::{
    error::{LumenError, PropertyError},
    node::{NodeId, PropertyValue, TrackId},
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PropertyPath(pub String);

impl PropertyPath {
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum InterpolationMode {
    Step = 0,
    Linear = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extrapolation {
    Hold,
    DefaultValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimatableType {
    Float,
    Int,
    Boolean,
    Color,
    Vector2,
    String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Keyframe {
    pub time_frame: u32,
    pub value: PropertyValue,
    pub interpolation: InterpolationMode,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeyframeTrack {
    pub id: TrackId,
    pub node_id: NodeId,
    pub property_path: PropertyPath,
    pub value_type: AnimatableType,
    pub keys: Vec<Keyframe>,
    pub before_extrapolation: Extrapolation,
    pub after_extrapolation: Extrapolation,
}

impl KeyframeTrack {
    pub fn new(
        id: TrackId,
        node_id: NodeId,
        property_path: PropertyPath,
        value_type: AnimatableType,
    ) -> Self {
        Self {
            id,
            node_id,
            property_path,
            value_type,
            keys: Vec::new(),
            before_extrapolation: Extrapolation::Hold,
            after_extrapolation: Extrapolation::Hold,
        }
    }

    pub fn set_key(&mut self, frame: u32, value: PropertyValue, interpolation: InterpolationMode) {
        if let Some(existing) = self.keys.iter_mut().find(|key| key.time_frame == frame) {
            existing.value = value;
            existing.interpolation = interpolation;
            return;
        }

        self.keys.push(Keyframe {
            time_frame: frame,
            value,
            interpolation,
        });
        self.keys.sort_by_key(|key| key.time_frame);
    }

    pub fn remove_key(&mut self, frame: u32) -> Option<Keyframe> {
        let index = self.keys.iter().position(|key| key.time_frame == frame)?;
        Some(self.keys.remove(index))
    }

    pub fn sample(&self, frame: u32) -> Result<PropertyValue, LumenError> {
        if self.keys.is_empty() {
            return Err(PropertyError::MissingProperty {
                node_id: self.node_id,
                property_path: self.property_path.0.clone(),
            }
            .into());
        }

        if self.keys.len() == 1 {
            return Ok(self.keys[0].value.clone());
        }

        if frame <= self.keys[0].time_frame {
            return Ok(match self.before_extrapolation {
                Extrapolation::Hold => self.keys[0].value.clone(),
                Extrapolation::DefaultValue => self.default_value(),
            });
        }

        if frame >= self.keys[self.keys.len() - 1].time_frame {
            return Ok(match self.after_extrapolation {
                Extrapolation::Hold => self.keys[self.keys.len() - 1].value.clone(),
                Extrapolation::DefaultValue => self.default_value(),
            });
        }

        let mut right_index = 1usize;
        while right_index < self.keys.len() && self.keys[right_index].time_frame < frame {
            right_index += 1;
        }
        let left = &self.keys[right_index - 1];
        let right = &self.keys[right_index];

        if matches!(right.interpolation, InterpolationMode::Step) {
            return Ok(left.value.clone());
        }

        let range = (right.time_frame - left.time_frame) as f64;
        let t = if range == 0.0 {
            0.0
        } else {
            (frame - left.time_frame) as f64 / range
        };
        Ok(interpolate_property_value(&left.value, &right.value, t))
    }

    fn default_value(&self) -> PropertyValue {
        match self.value_type {
            AnimatableType::Float => PropertyValue::Float(0.0),
            AnimatableType::Int => PropertyValue::Int(0),
            AnimatableType::Boolean => PropertyValue::Bool(false),
            AnimatableType::Color => PropertyValue::Color([0, 0, 0, 0]),
            AnimatableType::Vector2 => PropertyValue::Vector2(0.0, 0.0),
            AnimatableType::String => PropertyValue::String(String::new()),
        }
    }
}

fn interpolate_property_value(
    left: &PropertyValue,
    right: &PropertyValue,
    t: f64,
) -> PropertyValue {
    match (left, right) {
        (PropertyValue::Float(a), PropertyValue::Float(b)) => PropertyValue::Float(a + (b - a) * t),
        (PropertyValue::Int(a), PropertyValue::Int(b)) => {
            let value = *a as f64 + ((*b as f64 - *a as f64) * t);
            PropertyValue::Int(value.round() as i64)
        }
        (PropertyValue::Color(a), PropertyValue::Color(b)) => {
            let lerp = |x: u8, y: u8| -> u8 {
                (x as f64 + (y as f64 - x as f64) * t)
                    .round()
                    .clamp(0.0, 255.0) as u8
            };
            PropertyValue::Color([
                lerp(a[0], b[0]),
                lerp(a[1], b[1]),
                lerp(a[2], b[2]),
                lerp(a[3], b[3]),
            ])
        }
        (PropertyValue::Vector2(ax, ay), PropertyValue::Vector2(bx, by)) => {
            PropertyValue::Vector2(ax + (bx - ax) * t, ay + (by - ay) * t)
        }
        _ => left.clone(),
    }
}
