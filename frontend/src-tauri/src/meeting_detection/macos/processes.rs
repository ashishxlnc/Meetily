use plist::Value;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_void};
use std::path::{Path, PathBuf};

const PROC_PIDPATH_BUFFER_SIZE: usize = 4096;
const TEAMS_BUNDLE_ID: &str = "com.microsoft.teams2";

extern "C" {
    fn proc_pidpath(pid: c_int, buffer: *mut c_void, buffer_size: u32) -> c_int;
}

#[derive(Debug, Clone)]
pub struct ProcessIdentity {
    pub executable_path: PathBuf,
    pub bundle_id: Option<String>,
    pub application_name: Option<String>,
}

fn executable_path(process_id: i32) -> Result<PathBuf, String> {
    let mut buffer = vec![0_u8; PROC_PIDPATH_BUFFER_SIZE];
    let length = unsafe {
        proc_pidpath(
            process_id,
            buffer.as_mut_ptr() as *mut c_void,
            buffer.len() as u32,
        )
    };
    if length <= 0 {
        return Err(format!("proc_pidpath failed for PID {}", process_id));
    }

    let path = unsafe { CStr::from_ptr(buffer.as_ptr() as *const c_char) }
        .to_string_lossy()
        .into_owned();
    Ok(PathBuf::from(path))
}

fn application_bundles(path: &Path) -> Vec<PathBuf> {
    let mut bundles = Vec::new();
    let mut current = path.parent();
    while let Some(directory) = current {
        if directory.extension().and_then(|value| value.to_str()) == Some("app") {
            bundles.push(directory.to_path_buf());
        }
        current = directory.parent();
    }
    bundles
}

fn bundle_identifier(bundle: &Path) -> Option<String> {
    let info_plist = bundle.join("Contents/Info.plist");
    Value::from_file(info_plist)
        .ok()?
        .as_dictionary()?
        .get("CFBundleIdentifier")?
        .as_string()
        .map(str::to_owned)
}

pub fn process_identity(process_id: i32) -> Result<ProcessIdentity, String> {
    let executable_path = executable_path(process_id)?;
    let bundles = application_bundles(&executable_path);

    for bundle in &bundles {
        if let Some(bundle_id) = bundle_identifier(bundle) {
            if bundle_id == TEAMS_BUNDLE_ID {
                return Ok(ProcessIdentity {
                    executable_path,
                    bundle_id: Some(bundle_id),
                    application_name: bundle
                        .file_stem()
                        .and_then(|value| value.to_str())
                        .map(str::to_owned),
                });
            }
        }
    }

    let fallback_bundle = bundles
        .iter()
        .find(|bundle| {
            bundle
                .file_stem()
                .and_then(|value| value.to_str())
                .map(|name| name.to_lowercase().contains("teams"))
                .unwrap_or(false)
        })
        .cloned();

    Ok(ProcessIdentity {
        executable_path,
        bundle_id: fallback_bundle.as_deref().and_then(bundle_identifier),
        application_name: fallback_bundle
            .as_deref()
            .and_then(Path::file_stem)
            .and_then(|value| value.to_str())
            .map(str::to_owned),
    })
}

pub fn is_microsoft_teams(identity: &ProcessIdentity) -> bool {
    identity.bundle_id.as_deref() == Some(TEAMS_BUNDLE_ID)
        || (identity.bundle_id.is_none()
            && identity
                .application_name
                .as_deref()
                .map(|name| name.to_lowercase().contains("teams"))
                .unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_bundle_identifier_matches_teams() {
        let identity = ProcessIdentity {
            executable_path: PathBuf::from(
                "/Applications/Microsoft Teams.app/Contents/MacOS/MSTeams",
            ),
            bundle_id: Some(TEAMS_BUNDLE_ID.into()),
            application_name: Some("Microsoft Teams".into()),
        };
        assert!(is_microsoft_teams(&identity));
    }

    #[test]
    fn localized_name_is_only_used_when_bundle_identifier_is_missing() {
        let missing_bundle = ProcessIdentity {
            executable_path: PathBuf::new(),
            bundle_id: None,
            application_name: Some("Microsoft Teams".into()),
        };
        assert!(is_microsoft_teams(&missing_bundle));

        let different_bundle = ProcessIdentity {
            executable_path: PathBuf::new(),
            bundle_id: Some("com.example.teams-notes".into()),
            application_name: Some("Teams Notes".into()),
        };
        assert!(!is_microsoft_teams(&different_bundle));
    }
}
