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
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
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

#[cfg(test)]
mod tests {
    use super::*;

    /// El formato es UUID-v4-shape: 8-4-4-4-12 hex chars con un '4' fijo
    /// al inicio del tercer grupo (versión 4) y un nibble del cuarto grupo
    /// en el rango 8-b (variant RFC 4122).
    #[test]
    fn generate_id_has_uuid_v4_shape() {
        let id = DeviceIdentity::generate_id();
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.len(), 5, "expected 5 dash-separated groups in {id}");
        assert_eq!(parts[0].len(), 8, "first group should be 8 chars");
        assert_eq!(parts[1].len(), 4, "second group should be 4 chars");
        assert_eq!(parts[2].len(), 4, "third group should be 4 chars");
        assert_eq!(parts[3].len(), 4, "fourth group should be 4 chars");
        assert_eq!(parts[4].len(), 12, "fifth group should be 12 chars");

        // Versión 4 (UUID v4): el tercer grupo empieza con '4'.
        assert!(parts[2].starts_with('4'), "expected v4 marker in {id}");
        // Variant RFC 4122: el primer nibble del cuarto grupo está en 8..=b.
        let variant = parts[3].chars().next().unwrap();
        assert!(
            matches!(variant, '8' | '9' | 'a' | 'b'),
            "expected RFC 4122 variant 8/9/a/b, got {variant} in {id}"
        );

        // Todos los caracteres son hex lowercase.
        for c in id.chars() {
            assert!(
                c == '-' || c.is_ascii_hexdigit() && !c.is_uppercase(),
                "unexpected char {c:?} in {id}"
            );
        }
    }

    #[test]
    fn generate_id_produces_different_ids_on_consecutive_calls() {
        let a = DeviceIdentity::generate_id();
        // Pequeña pausa para garantizar nanos distintos en sistemas muy rápidos.
        std::thread::sleep(std::time::Duration::from_micros(10));
        let b = DeviceIdentity::generate_id();
        assert_ne!(a, b, "two consecutive ids should differ");
    }

    #[test]
    fn device_identity_round_trips_through_json() {
        let original = DeviceIdentity {
            id: "12345678-aaaa-4bbb-8ccc-ddddeeeeffff".into(),
            name: "Jayo-PC".into(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: DeviceIdentity = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, original.id);
        assert_eq!(parsed.name, original.name);
    }
}
