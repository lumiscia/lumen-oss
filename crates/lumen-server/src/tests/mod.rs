mod render;

use lumen::{
    clip::{Layer, Timeline},
    sequence::{
        ColorRGBA,
        element::{ElementProperties, Font, TextElement, Transform},
    },
};

fn create_test_timeline() -> Timeline {
    let mut layers = (0..12)
        .into_iter()
        .map(|_| Layer::new())
        .collect::<Vec<_>>();

    for i in 0..120 {
        for (idx, layer) in layers.iter_mut().enumerate() {
            layer.insert(
                Box::new(TextElement {
                    font: Font::Arial,
                    color: ColorRGBA(
                        (100 + i * 50) % 255,
                        (150 + i * 30) % 255,
                        (120 + i * 20) % 255,
                        255,
                    ),
                    text: format!("Hello on layer {} ({})!", idx, i),
                    properties: ElementProperties {
                        start: 0,
                        duration: 100,
                        transform: Transform {
                            x: Some(((300 + i as u16 * 100) % 1920) as i32),
                            y: Some(((i as u32 * 300 as u32) % 1080) as i32),
                            width: None,
                            height: None,
                        },
                        transition_in: None,
                        transition_out: None,
                        effects: Vec::new(),
                    },
                }),
                (i * 30) as usize,
                30,
            );
        }
    }

    Timeline { layers }
}
