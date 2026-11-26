//! The hinge, as the pages need it: shut or open, and nothing else.
//!
//! The recovery's cover screen follows the phone being shut; the hinge is what
//! says so when the inner touch panel's driver is not there to say it. This is
//! the only caller the sensors crate was built for, and it keeps the answer
//! cheap: the work happens on the hinge's own thread, and a page asks for the
//! latest answer without waiting on anything.

#[cfg(unix)]
use crate::hinge::Hinge;

/// The phone being shut, from the hinge.
///
/// Only the outer display is driven from this: the inner one folds with the
/// same sensor and its rules are not these.
#[derive(Default)]
pub struct FlipController {
    #[cfg(unix)]
    hinge: Option<Hinge>,
}

impl FlipController {
    /// Start following the hinge. A start that cannot find the service still
    /// returns a controller: it answers nothing until the service comes back,
    /// which is what a phone without the sensors up should get, not a failure
    /// that keeps the pages from drawing.
    pub fn start() -> Self {
        #[cfg(unix)]
        {
            FlipController {
                hinge: Hinge::open().ok(),
            }
        }
        #[cfg(not(unix))]
        {
            FlipController {}
        }
    }

    /// Whether the phone is shut: `Some(true)` shut, `Some(false)` open, and
    /// `None` while the hinge is not answering.
    pub fn folded(&self) -> Option<bool> {
        #[cfg(unix)]
        {
            self.hinge.as_ref()?.folded()
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    /// The hinge's angle, for a page that wants to show it.
    pub fn angle(&self) -> Option<f32> {
        #[cfg(unix)]
        {
            self.hinge.as_ref()?.angle()
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    /// Whether the hinge is answering at all.
    pub fn online(&self) -> bool {
        #[cfg(unix)]
        {
            self.hinge.as_ref().is_some_and(|hinge| hinge.online())
        }
        #[cfg(not(unix))]
        {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_controller_without_the_service_answers_nothing() {
        // No socket is opened here: the answer is absent, not an error.
        assert_eq!(FlipController::default().folded(), None);
        assert_eq!(FlipController::default().angle(), None);
        assert!(!FlipController::default().online());
    }
}
