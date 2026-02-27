use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use crate::{
    error::LumenError,
    node::{InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue, VectorData},
    render::RenderContext,
};

static INPUT_PORT_DEFS_CACHE: OnceLock<Mutex<HashMap<u16, &'static [InputPortDef]>>> =
    OnceLock::new();

const OUTPUT_PORT_DEFS: [OutputPortDef; 1] = [OutputPortDef {
    name: "output",
    kind: PortKind::Vector,
}];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VectorMultiMerge {
    pub input_count: u16,
}

impl Default for VectorMultiMerge {
    fn default() -> Self {
        Self { input_count: 2 }
    }
}

impl VectorMultiMerge {
    fn normalized_input_count(&self) -> u16 {
        self.input_count.max(1)
    }

    pub fn input_port_name(index: u16) -> String {
        format!("input_{index}")
    }

    fn cached_input_port_defs(&self) -> &'static [InputPortDef] {
        let input_count = self.normalized_input_count();
        let cache = INPUT_PORT_DEFS_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let mut cache_guard = match cache.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        if let Some(existing_defs) = cache_guard.get(&input_count) {
            return existing_defs;
        }

        let defs: Vec<InputPortDef> = (0..input_count)
            .map(|index| {
                let name: &'static str = Box::leak(Self::input_port_name(index).into_boxed_str());
                InputPortDef {
                    name,
                    kind: PortKind::Vector,
                    optional: true,
                }
            })
            .collect();
        let defs: &'static [InputPortDef] = Box::leak(defs.into_boxed_slice());

        cache_guard.insert(input_count, defs);
        defs
    }
}

impl NodeEval for VectorMultiMerge {
    fn input_port_defs(&self) -> &'static [InputPortDef] {
        self.cached_input_port_defs()
    }

    fn output_port_defs(&self) -> &'static [OutputPortDef] {
        &OUTPUT_PORT_DEFS
    }

    fn evaluate(
        &self,
        inputs: &NodeInputs,
        _ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError> {
        let mut merged = Vec::new();
        for index in 0..self.normalized_input_count() {
            let port_name = Self::input_port_name(index);
            if let Some(vector) = inputs.get_vector_optional(&port_name)? {
                merged.push(vector.clone());
            }
        }

        let output = match merged.len() {
            0 => VectorData::Group {
                children: Vec::new(),
                position: Default::default(),
            },
            1 => merged.pop().expect("length checked"),
            _ => VectorData::Group {
                children: merged,
                position: Default::default(),
            },
        };

        Ok(PortValue::Vector(output))
    }
}
