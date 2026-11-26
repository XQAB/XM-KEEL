//! Where the hinge is, as a value: the angle, and the fold state it implies.
//!
//! Kept apart from the socket so that the hysteresis, which is the part with
//! rules in it, can be tested on a host.

// Only the socket's own thread pushes readings; on a host there is no such
// thread, so the setters look unused.
#![cfg_attr(not(unix), allow(dead_code))]

/// Below this the phone counts as folded.
pub const FOLDED_BELOW: f32 = 20.0;
/// Above this it counts as open.
pub const OPEN_ABOVE: f32 = 150.0;

/// The fold state, as a reader sees it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Reading {
    pub(crate) angle: Option<f32>,
    pub(crate) folded: Option<bool>,
    pub(crate) online: bool,
    pub(crate) generation: u64,
}

impl Reading {
    /// The angle, if the sensor is answering.
    pub fn angle(&self) -> Option<f32> {
        self.angle
    }

    /// Folded, open, or unknown, with the hysteresis applied.
    pub fn folded(&self) -> Option<bool> {
        self.folded
    }

    /// Whether the sensor was last seen answering.
    pub fn online(&self) -> bool {
        self.online
    }

    pub(crate) fn push(&mut self, angle: f32) {
        self.angle = Some(angle);
        self.folded = fold(angle, self.folded);
        self.online = true;
        self.generation += 1;
    }

    pub(crate) fn offline(&mut self) {
        self.angle = None;
        self.folded = None;
        self.online = false;
        self.generation += 1;
    }
}

/// Fold this angle, keeping the last answer while it is between the two
/// thresholds. `true` is folded.
///
/// One real fold walks 179 → 116 → 89 → 43 → 20 → 1, and back through the same
/// middle; a bare threshold at each end would chatter there, which is why the
/// middle keeps what it had.
pub fn fold(angle: f32, previous: Option<bool>) -> Option<bool> {
    if angle <= FOLDED_BELOW {
        Some(true)
    } else if angle >= OPEN_ABOVE {
        Some(false)
    } else {
        previous
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fold_walks_from_open_to_folded_and_back() {
        let mut folded = fold(179.0, None);
        assert_eq!(folded, Some(false));
        for angle in [116.0, 89.0, 43.0, 20.0, 10.0, 1.0] {
            folded = fold(angle, folded);
        }
        assert_eq!(folded, Some(true));
        for angle in [22.0, 88.0, 116.0, 148.0, 179.0] {
            folded = fold(angle, folded);
        }
        assert_eq!(folded, Some(false));
    }

    #[test]
    fn an_unknown_start_stays_unknown_in_the_middle() {
        assert_eq!(fold(89.0, None), None);
        assert_eq!(fold(0.0, None), Some(true));
        assert_eq!(fold(180.0, None), Some(false));
    }

    #[test]
    fn a_reading_keeps_its_answer_and_goes_quiet_when_offline() {
        let mut reading = Reading::default();
        assert_eq!(reading.folded(), None);
        reading.push(179.0);
        reading.push(120.0);
        assert_eq!(reading.folded(), Some(false));
        reading.push(40.0);
        assert_eq!(reading.angle(), Some(40.0));
        assert_eq!(reading.folded(), Some(false));
        reading.push(1.0);
        assert_eq!(reading.folded(), Some(true));
        reading.offline();
        assert_eq!(reading.folded(), None);
        assert_eq!(reading.angle(), None);
        assert!(!reading.online());
    }
}
