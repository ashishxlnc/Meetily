use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Runtime, State};

#[cfg(target_os = "macos")]
mod macos;

const POLL_INTERVAL: Duration = Duration::from_secs(2);
const START_CONFIRMATION: Duration = Duration::from_secs(6);
const END_CONFIRMATION: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingDetectionEvent {
    pub provider: &'static str,
    pub active: bool,
    pub process_id: Option<i32>,
    pub assertion_types: Vec<String>,
}

#[derive(Debug, Clone)]
struct Observation {
    process_id: i32,
    assertion_types: Vec<String>,
}

#[derive(Debug)]
enum DetectionState {
    Inactive,
    CandidateActive {
        since: Instant,
        observation: Observation,
    },
    Active {
        observation: Observation,
    },
    CandidateEnded {
        since: Instant,
        previous: Observation,
    },
}

impl Default for DetectionState {
    fn default() -> Self {
        Self::Inactive
    }
}

#[derive(Debug, PartialEq)]
enum Transition {
    Started,
    Ended,
}

fn advance_state(
    state: &mut DetectionState,
    observation: Option<Observation>,
    now: Instant,
) -> Option<Transition> {
    match (&mut *state, observation) {
        (DetectionState::Inactive, Some(observation)) => {
            *state = DetectionState::CandidateActive {
                since: now,
                observation,
            };
            None
        }
        (
            DetectionState::CandidateActive {
                since,
                observation: current,
            },
            Some(observation),
        ) => {
            *current = observation;
            if now.duration_since(*since) >= START_CONFIRMATION {
                *state = DetectionState::Active {
                    observation: current.clone(),
                };
                Some(Transition::Started)
            } else {
                None
            }
        }
        (DetectionState::CandidateActive { .. }, None) => {
            *state = DetectionState::Inactive;
            None
        }
        (
            DetectionState::Active {
                observation: current,
            },
            Some(observation),
        ) => {
            *current = observation;
            None
        }
        (DetectionState::Active { observation }, None) => {
            *state = DetectionState::CandidateEnded {
                since: now,
                previous: observation.clone(),
            };
            None
        }
        (DetectionState::CandidateEnded { .. }, Some(observation)) => {
            *state = DetectionState::Active { observation };
            None
        }
        (DetectionState::CandidateEnded { since, .. }, None) => {
            if now.duration_since(*since) >= END_CONFIRMATION {
                *state = DetectionState::Inactive;
                Some(Transition::Ended)
            } else {
                None
            }
        }
        (DetectionState::Inactive, None) => None,
    }
}

pub struct MeetingDetectionManager {
    enabled: AtomicBool,
    state: Mutex<DetectionState>,
}

impl MeetingDetectionManager {
    pub fn new(enabled: bool) -> Arc<Self> {
        Arc::new(Self {
            enabled: AtomicBool::new(enabled),
            state: Mutex::new(DetectionState::Inactive),
        })
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
        if !enabled {
            *self.state.lock().unwrap() = DetectionState::Inactive;
        }
        log::info!("Teams meeting auto-detection enabled: {}", enabled);
    }

    pub fn snapshot(&self) -> MeetingDetectionEvent {
        if !self.enabled.load(Ordering::SeqCst) {
            return MeetingDetectionEvent::inactive();
        }

        match &*self.state.lock().unwrap() {
            DetectionState::Active { observation }
            | DetectionState::CandidateEnded {
                previous: observation,
                ..
            } => MeetingDetectionEvent::active(observation),
            DetectionState::Inactive | DetectionState::CandidateActive { .. } => {
                MeetingDetectionEvent::inactive()
            }
        }
    }

    pub async fn run<R: Runtime>(self: Arc<Self>, app: AppHandle<R>) {
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;

            if !self.enabled.load(Ordering::SeqCst) {
                continue;
            }

            #[cfg(target_os = "macos")]
            let observation = match macos::detect_teams_meeting() {
                Ok(value) => value.map(|value| Observation {
                    process_id: value.process_id,
                    assertion_types: value.assertion_types,
                }),
                Err(error) => {
                    log::warn!("Failed to inspect macOS power assertions: {}", error);
                    continue;
                }
            };

            #[cfg(not(target_os = "macos"))]
            let observation: Option<Observation> = None;

            // The preference may have changed while the platform query was in
            // progress. Do not publish a stale transition after being disabled.
            if !self.enabled.load(Ordering::SeqCst) {
                continue;
            }

            let mut state = self.state.lock().unwrap();
            let transition = advance_state(&mut state, observation, Instant::now());
            let event = match transition {
                Some(Transition::Started) => match &*state {
                    DetectionState::Active { observation } => {
                        Some(MeetingDetectionEvent::active(observation))
                    }
                    _ => None,
                },
                Some(Transition::Ended) => Some(MeetingDetectionEvent::inactive()),
                None => None,
            };
            drop(state);

            if let Some(event) = event {
                log::info!(
                    "Teams meeting confirmed {}: pid={:?}, assertions={:?}",
                    if event.active { "active" } else { "ended" },
                    event.process_id,
                    event.assertion_types
                );
                if let Err(error) = app.emit("meeting-detection-changed", event) {
                    log::warn!("Failed to emit meeting detection event: {}", error);
                }
            }
        }
    }
}

impl MeetingDetectionEvent {
    fn active(observation: &Observation) -> Self {
        Self {
            provider: "microsoft-teams",
            active: true,
            process_id: Some(observation.process_id),
            assertion_types: observation.assertion_types.clone(),
        }
    }

    fn inactive() -> Self {
        Self {
            provider: "microsoft-teams",
            active: false,
            process_id: None,
            assertion_types: Vec::new(),
        }
    }
}

#[tauri::command]
pub fn get_meeting_detection_state(
    manager: State<'_, Arc<MeetingDetectionManager>>,
) -> MeetingDetectionEvent {
    manager.snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(pid: i32) -> Observation {
        Observation {
            process_id: pid,
            assertion_types: vec!["NoIdleSleepAssertion".into()],
        }
    }

    #[test]
    fn requires_stable_signal_before_starting() {
        let start = Instant::now();
        let mut state = DetectionState::Inactive;
        assert_eq!(advance_state(&mut state, Some(observation(1)), start), None);
        assert_eq!(
            advance_state(&mut state, Some(observation(1)), start + START_CONFIRMATION),
            Some(Transition::Started)
        );
    }

    #[test]
    fn transient_end_does_not_stop_meeting() {
        let start = Instant::now();
        let mut state = DetectionState::Active {
            observation: observation(1),
        };
        assert_eq!(advance_state(&mut state, None, start), None);
        assert_eq!(
            advance_state(
                &mut state,
                Some(observation(2)),
                start + Duration::from_secs(5)
            ),
            None
        );
        assert!(matches!(state, DetectionState::Active { .. }));
    }

    #[test]
    fn requires_stable_absence_before_ending() {
        let start = Instant::now();
        let mut state = DetectionState::Active {
            observation: observation(1),
        };
        assert_eq!(advance_state(&mut state, None, start), None);
        assert_eq!(
            advance_state(&mut state, None, start + END_CONFIRMATION),
            Some(Transition::Ended)
        );
    }
}
