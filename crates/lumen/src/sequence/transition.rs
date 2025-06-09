use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
pub enum Transition {
    Fade { duration: u64 },
    SlideLeft { duration: u64 },
    SlideRight { duration: u64 },
    SlideUp { duration: u64 },
    SlideDown { duration: u64 },
    Dissolve { duration: u64 },
}

impl Transition {
    pub fn duration(&self) -> u64 {
        match self {
            Transition::Fade { duration }
            | Transition::SlideLeft { duration }
            | Transition::SlideRight { duration }
            | Transition::SlideUp { duration }
            | Transition::SlideDown { duration }
            | Transition::Dissolve { duration } => *duration,
        }
    }
}
