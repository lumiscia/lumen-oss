use std::ops::Deref;

use crate::composition::Composition;

use super::cache::composition_fingerprint;

/// A composition that can no longer be edited, fingerprinted once when it is frozen. Rendering
/// a `Composition` fingerprints all of it on every call, since it may have been edited in place
/// since the last one; rendering a `FrozenComposition` reuses its fingerprint.
#[derive(Debug)]
pub struct FrozenComposition {
    composition: Composition,
    fingerprint: u64,
}

impl FrozenComposition {
    pub fn new(composition: Composition) -> Self {
        let fingerprint = composition_fingerprint(&composition);
        Self {
            composition,
            fingerprint,
        }
    }

    /// The composition, to be edited and frozen again.
    pub fn into_inner(self) -> Composition {
        self.composition
    }
}

impl From<Composition> for FrozenComposition {
    fn from(composition: Composition) -> Self {
        Self::new(composition)
    }
}

impl Deref for FrozenComposition {
    type Target = Composition;

    fn deref(&self) -> &Composition {
        &self.composition
    }
}

/// A composition `GpuCompositionRenderer` renders, including references and shared owners.
pub trait RenderComposition: sealed::Fingerprinted {}

impl RenderComposition for Composition {}

impl RenderComposition for FrozenComposition {}

macro_rules! forward_composition {
    ($($wrapper:ty),+ $(,)?) => {
        $(
            impl<T: RenderComposition + ?Sized> RenderComposition for $wrapper {}

            impl<T: RenderComposition + ?Sized> sealed::Fingerprinted for $wrapper {
                fn composition(&self) -> &Composition {
                    sealed::Fingerprinted::composition(&**self)
                }

                fn fingerprint(&self) -> u64 {
                    sealed::Fingerprinted::fingerprint(&**self)
                }
            }
        )+
    };
}

forward_composition!(&T, &mut T, Box<T>, std::sync::Arc<T>, std::rc::Rc<T>);

pub(super) mod sealed {
    use crate::composition::Composition;

    pub trait Fingerprinted {
        fn composition(&self) -> &Composition;

        /// `composition_fingerprint` of the composition.
        fn fingerprint(&self) -> u64;
    }

    impl Fingerprinted for Composition {
        fn composition(&self) -> &Composition {
            self
        }

        fn fingerprint(&self) -> u64 {
            super::composition_fingerprint(self)
        }
    }

    impl Fingerprinted for super::FrozenComposition {
        fn composition(&self) -> &Composition {
            &self.composition
        }

        fn fingerprint(&self) -> u64 {
            self.fingerprint
        }
    }
}
