use crate::prelude::StrictPath;
use serde::{Deserialize, Serialize};

/// Unique identity for this machine.
/// Equivalent to SyncSourceEntity in EmuSync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceIdentity {
    pub id: String,
    pub name: String,
}

impl DeviceIdentity {
    pub fn load_or_create(app_dir: &StrictPath) -> Self {
        let path = app_dir.joined("ludusavi-device.json");

        if path.is_file() {
            if let Some(content) = path.read() {
                if let Ok(identity) = serde_json::from_str::<Self>(&content) {
                    return identity;
                }
            }
        }

        let identity = Self {
            id: Self::generate_id(),
            name: whoami::devicename(),
        };

        if let Ok(json) = serde_json::to_string_pretty(&identity) {
            let _ = path.write_with_content(&json);
        }

        identity
    }

    fn generate_id() -> String {
        // Uses sha1 (already a dependency) + current time + thread id
        // to generate a unique ID without adding new dependencies.
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        use std::time::SystemTime;

        let mut hasher = DefaultHasher::new();
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .hash(&mut hasher);
        std::thread::current().id().hash(&mut hasher);
        let h1 = hasher.finish();

        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
            .hash(&mut hasher);
        let h2 = hasher.finish();

        format!(
            "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
            (h1 >> 32) as u32,
            (h1 & 0xffff) as u16,
            (h2 & 0x0fff) as u16,
            ((h2 >> 16) & 0x3fff) as u16 | 0x8000,
            h1 & 0xffffffffffff_u64,
        )
    }
}
