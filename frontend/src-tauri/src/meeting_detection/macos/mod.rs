mod power_assertions;
mod processes;

pub struct TeamsMeetingObservation {
    pub process_id: i32,
    pub assertion_types: Vec<String>,
}

pub fn detect_teams_meeting() -> Result<Option<TeamsMeetingObservation>, String> {
    for assertion in power_assertions::active_no_sleep_assertions()? {
        let identity = match processes::process_identity(assertion.process_id) {
            Ok(identity) => identity,
            Err(error) => {
                log::debug!(
                    "Could not resolve assertion owner PID {}: {}",
                    assertion.process_id,
                    error
                );
                continue;
            }
        };

        if processes::is_microsoft_teams(&identity) {
            log::debug!(
                "Teams assertion candidate: pid={}, bundle={:?}, app={:?}, path={}, types={:?}",
                assertion.process_id,
                identity.bundle_id,
                identity.application_name,
                identity.executable_path.display(),
                assertion.assertion_types
            );
            return Ok(Some(TeamsMeetingObservation {
                process_id: assertion.process_id,
                assertion_types: assertion.assertion_types,
            }));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires an active Microsoft Teams call"]
    fn detects_live_teams_call() {
        let detection = detect_teams_meeting().expect("power assertion query should succeed");
        assert!(
            detection.is_some(),
            "no active Teams call assertion was detected"
        );
    }
}
