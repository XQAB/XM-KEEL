//! What the recovery service keeps for itself stays in its own folder,
//! where install scripts that empty /tmp leave it.

use std::path::Path;

#[test]
fn the_logs_the_recovery_saves_are_in_its_own_folder() {
    for log in [keel_sys::LOG, keel_install::ENGINE_LOG, keel_install::SCRIPT_LOG] {
        assert!(Path::new(log).starts_with(keel_sys::RUN), "{log}");
    }
    assert!(!Path::new(keel_sys::RUN).starts_with("/tmp"));
}
