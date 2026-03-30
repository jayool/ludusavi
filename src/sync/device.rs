use serde::{Deserialize, Serialize};

/// Unique identity for this machine.
/// Equivalent to SyncSourceEntity in EmuSync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceIdentity {
    pub id: String,
    pub name: String,
}

impl DeviceIdentity {
    /// Loads the device identity from disk, or creates a new one if it doesn't exist.
    pub fn load_or_create(app_dir: &crate::prelude::StrictPath) -> Self {
        let path = app_dir.joined("ludusavi-device.json");

        if path.is_file() {
            if let Some(content) = path.read() {
                if let Ok(identity) = serde_json::from_str::<Self>(&content) {
                    return identity;
                }
            }
        }

        let identity = Self {
            id: uuid(),
            name: whoami::devicename(),
        };

        if let Ok(json) = serde_json::to_string_pretty(&identity) {
            let _ = path.write_with_content(&json);
        }

        identity
    }
}

fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    // Simple UUID v4-like without pulling in a new crate
    // chrono and sha1 are already available in the project
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();

    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        rand_u32(),
        rand_u32() & 0xffff,
        rand_u32() & 0x0fff,
        (rand_u32() & 0x3fff) | 0x8000,
        nanos as u64 * rand_u32() as u64 & 0xffffffffffff,
    )
}

fn rand_u32() -> u32 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::SystemTime;

    let mut hasher = DefaultHasher::new();
    SystemTime::now().hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    hasher.finish() as u32
}
