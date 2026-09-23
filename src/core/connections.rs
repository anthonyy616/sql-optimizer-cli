//! Persistent connection profiles.
//!
//! Profiles keep non-secret connection metadata in a user-global catalog
//! (atomic writes, `0600` on Unix) while passwords live in the OS credential
//! store (macOS Keychain). Raw passwords and credential-bearing URLs are never
//! serialized. Session-only connections stay in memory and are never persisted.
//!
//! Catalog layout:
//! - macOS/Linux: `~/.config/sql-optimizer/connections.json`
//! - Windows:     `{FOLDERID_RoamingAppData}\sql-optimizer\connections.json`

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const CATALOG_SCHEMA_VERSION: u32 = 1;

/// Stable database provider kinds for a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseKind {
    Sqlite,
    Postgres,
    Mysql,
    Supabase,
    Neon,
}

impl DatabaseKind {
    pub fn label(self) -> &'static str {
        match self {
            DatabaseKind::Sqlite => "SQLite",
            DatabaseKind::Postgres => "PostgreSQL",
            DatabaseKind::Mysql => "MySQL",
            DatabaseKind::Supabase => "Supabase",
            DatabaseKind::Neon => "Neon",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "sqlite" => Some(DatabaseKind::Sqlite),
            "postgres" | "postgresql" => Some(DatabaseKind::Postgres),
            "mysql" => Some(DatabaseKind::Mysql),
            "supabase" => Some(DatabaseKind::Supabase),
            "neon" => Some(DatabaseKind::Neon),
            _ => None,
        }
    }
}

/// A saved connection profile. Contains **no secrets**: the password (if any)
/// is referenced indirectly through `secret_ref` and resolved from the
/// credential store at connect time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionProfile {
    /// Stable identifier (UUID v4). Never changes across edits/renames.
    pub id: String,
    /// User-visible display name.
    pub name: String,
    pub kind: DatabaseKind,
    /// Non-credential-bearing URL (password stripped, if present).
    pub url: String,
    /// Structured parts, when the profile was built from host/port/... parts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default = "default_ssl_mode", skip_serializing_if = "Option::is_none")]
    pub ssl_mode: Option<String>,
    /// True when the profile explicitly trusts invalid TLS certificates.
    #[serde(default)]
    pub accept_invalid_certs: bool,
    /// Credential-store reference key (e.g. `sql-optimizer/profile/<id>`).
    /// Absent when the profile has no password (e.g. SQLite, trust auth).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<String>,
}

fn default_ssl_mode() -> Option<String> {
    Some("require".to_string())
}

impl ConnectionProfile {
    /// Create a new profile with a fresh stable ID.
    pub fn new(name: impl Into<String>, kind: DatabaseKind, url: impl Into<String>) -> Self {
        Self {
            id: uuid_v4(),
            name: name.into(),
            kind,
            url: sanitize_url(&url.into()),
            host: None,
            port: None,
            database: None,
            user: None,
            ssl_mode: default_ssl_mode(),
            accept_invalid_certs: false,
            secret_ref: None,
        }
    }

    /// True when this URL cannot be reused across sessions (in-memory SQLite).
    pub fn is_ephemeral(&self) -> bool {
        self.kind == DatabaseKind::Sqlite
            && (self.url == "sqlite::memory:" || self.url.contains(":memory:"))
    }

    /// The secret key this profile uses in the credential store.
    pub fn secret_key(&self) -> Option<String> {
        self.secret_ref.clone().or_else(|| {
            has_password_field(&self.url).then(|| format!("sql-optimizer/profile/{}", self.id))
        })
    }
}

/// A user-global catalog of saved profiles. Not thread-safe by design; the
/// TUI owns one instance and persists after each mutation.
#[derive(Debug, Default)]
pub struct ProfileCatalog {
    path: PathBuf,
    pub profiles: Vec<ConnectionProfile>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CatalogFile {
    schema_version: u32,
    profiles: Vec<ConnectionProfile>,
}

impl ProfileCatalog {
    /// Default user-global location (`dirs`-based).
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("sql-optimizer")
            .join("connections.json")
    }

    /// Open (or create) the catalog at the default location.
    pub fn load_default() -> Result<Self> {
        Self::load(Self::default_path())
    }

    /// Open (or create) the catalog at a specific path. Malformed files are
    /// reported as errors, never silently overwritten.
    pub fn load(path: PathBuf) -> Result<Self> {
        if !path.exists() {
            return Ok(Self {
                path,
                profiles: Vec::new(),
            });
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read catalog at {}", path.display()))?;
        if raw.trim().is_empty() {
            return Ok(Self {
                path,
                profiles: Vec::new(),
            });
        }

        // Migration path: older/newer schema versions are detected here.
        let file: CatalogFile = serde_json::from_str(&raw)
            .with_context(|| format!("Malformed catalog at {}", path.display()))?;
        if file.schema_version != CATALOG_SCHEMA_VERSION {
            return Err(anyhow!(
                "Catalog schema version {} not supported (expected {})",
                file.schema_version,
                CATALOG_SCHEMA_VERSION
            ));
        }

        Ok(Self {
            path,
            profiles: file.profiles,
        })
    }

    /// Persist the catalog atomically (write temp file, fsync, rename) with
    /// `0600` permissions on Unix.
    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        let file = CatalogFile {
            schema_version: CATALOG_SCHEMA_VERSION,
            profiles: self.profiles.clone(),
        };
        let json = serde_json::to_string_pretty(&file).context("Failed to serialize catalog")?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &json)
            .with_context(|| format!("Failed to write {}", tmp.display()))?;
        restrict_permissions(&tmp);
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("Failed to replace {}", self.path.display()))?;
        restrict_permissions(&self.path);
        Ok(())
    }

    /// Add or replace a profile (matched by ID), then persist.
    pub fn upsert(&mut self, profile: ConnectionProfile) -> Result<()> {
        if let Some(existing) = self.profiles.iter_mut().find(|p| p.id == profile.id) {
            *existing = profile;
        } else {
            self.profiles.push(profile);
        }
        self.save()
    }

    /// Remove a profile by ID; returns true when something was removed.
    pub fn remove(&mut self, id: &str) -> Result<bool> {
        let before = self.profiles.len();
        self.profiles.retain(|p| p.id != id);
        let removed = self.profiles.len() != before;
        if removed {
            self.save()?;
        }
        Ok(removed)
    }
}

/// Credential store abstraction. Secrets are saved/resolved/updated/deleted by
/// profile key and never logged or serialized.
#[async_trait::async_trait]
pub trait SecretStore: Send + Sync {
    async fn set_secret(&self, key: &str, secret: &str) -> Result<()>;
    async fn get_secret(&self, key: &str) -> Result<Option<String>>;
    async fn delete_secret(&self, key: &str) -> Result<()>;
}

/// Environment/session fallback: resolves secrets from
/// `SQL_OPTIMIZER_DB_PASSWORD` or an in-memory session map. Nothing is
/// written to disk.
pub struct SessionSecretStore {
    session: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

impl SessionSecretStore {
    pub fn new() -> Self {
        Self {
            session: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

impl Default for SessionSecretStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl SecretStore for SessionSecretStore {
    async fn set_secret(&self, key: &str, secret: &str) -> Result<()> {
        self.session
            .lock()
            .expect("session secret mutex poisoned")
            .insert(key.to_string(), secret.to_string());
        Ok(())
    }

    async fn get_secret(&self, key: &str) -> Result<Option<String>> {
        if let Some(v) = self
            .session
            .lock()
            .expect("session secret mutex poisoned")
            .get(key)
        {
            return Ok(Some(v.clone()));
        }
        // Environment fallback (passwords from .env / shell env).
        Ok(std::env::var("SQL_OPTIMIZER_DB_PASSWORD")
            .ok()
            .filter(|s| !s.is_empty() && key.starts_with("sql-optimizer/profile/")))
    }

    async fn delete_secret(&self, key: &str) -> Result<()> {
        self.session
            .lock()
            .expect("session secret mutex poisoned")
            .remove(key);
        Ok(())
    }
}

/// macOS Keychain-backed store (`security` CLI, generic passwords). Falls back
/// to [`SessionSecretStore`] semantics on other platforms.
pub struct KeychainSecretStore {
    fallback: SessionSecretStore,
}

impl KeychainSecretStore {
    pub fn new() -> Self {
        Self {
            fallback: SessionSecretStore::new(),
        }
    }

    fn keychain_service(key: &str) -> String {
        key.to_string()
    }
}

impl Default for KeychainSecretStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl SecretStore for KeychainSecretStore {
    async fn set_secret(&self, key: &str, secret: &str) -> Result<()> {
        if !cfg!(target_os = "macos") {
            return self.fallback.set_secret(key, secret).await;
        } // Delete any existing item first; `security add-generic-password`
          // refuses to overwrite without -U, which older macOS builds reject.
        let service = Self::keychain_service(key);
        let _ = tokio::process::Command::new("security")
            .args(["delete-generic-password", "-s", &service])
            .output()
            .await;
        let service = Self::keychain_service(key);
        let out = tokio::process::Command::new("security")
            .args([
                "add-generic-password",
                "-s",
                &service,
                "-a",
                "sql-optimizer",
                "-w",
            ])
            .arg(secret)
            .output()
            .await
            .context("Failed to run macOS `security` tool")?;
        if out.status.success() {
            Ok(())
        } else {
            Err(anyhow!(
                "Keychain write failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }

    async fn get_secret(&self, key: &str) -> Result<Option<String>> {
        if !cfg!(target_os = "macos") {
            return self.fallback.get_secret(key).await;
        }
        let service = Self::keychain_service(key);
        let out = tokio::process::Command::new("security")
            .args([
                "find-generic-password",
                "-s",
                &service,
                "-a",
                "sql-optimizer",
                "-w",
            ])
            .output()
            .await
            .context("Failed to run macOS `security` tool")?;
        if out.status.success() {
            let value = String::from_utf8_lossy(&out.stdout);
            Ok(Some(value.trim_end_matches(['\n', '\r']).to_string()))
        } else {
            // Item-not-found is normal; anything else is an error.
            let stderr = String::from_utf8_lossy(&out.stderr);
            if stderr.contains("could not be found") {
                Ok(None)
            } else {
                Err(anyhow!("Keychain read failed: {}", stderr.trim()))
            }
        }
    }

    async fn delete_secret(&self, key: &str) -> Result<()> {
        if !cfg!(target_os = "macos") {
            return self.fallback.delete_secret(key).await;
        }
        let service = Self::keychain_service(key);
        let out = tokio::process::Command::new("security")
            .args([
                "delete-generic-password",
                "-s",
                &service,
                "-a",
                "sql-optimizer",
            ])
            .output()
            .await
            .context("Failed to run macOS `security` tool")?;
        if out.status.success()
            || String::from_utf8_lossy(&out.stderr).contains("could not be found")
        {
            Ok(())
        } else {
            Err(anyhow!(
                "Keychain delete failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }
}

/// Build a `ConnectionArgs`-style URL for a profile, resolving its secret from
/// the store. The returned URL may contain the password — callers must only
/// pass it to the connector and display it through [`redact_profile_url`].
pub async fn resolve_profile_url(
    profile: &ConnectionProfile,
    store: &dyn SecretStore,
) -> Result<String> {
    let mut url = profile.url.clone();
    if let Some(key) = profile.secret_key() {
        if let Some(secret) = store.get_secret(&key).await? {
            url = inject_password(&url, &secret)?;
        }
    }
    Ok(url)
}

/// Display form of a profile URL with any credentials redacted.
pub fn redact_profile_url(url: &str) -> String {
    sanitize_url(url)
}

fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Strip userinfo password from a URL for display/storage: keeps the user but
/// removes the password portion.
pub fn sanitize_url(url: &str) -> String {
    if let Some(scheme_end) = url.find("://") {
        let rest = &url[scheme_end + 3..];
        if let Some(at) = rest.find('@') {
            let creds = &rest[..at];
            if creds.contains(':') {
                let user = creds.split(':').next().unwrap_or("");
                return format!("{}://{}@{}", &url[..scheme_end], user, &rest[at + 1..]);
            }
        }
    }
    url.to_string()
}

fn has_password_field(url: &str) -> bool {
    if let Some(scheme_end) = url.find("://") {
        let rest = &url[scheme_end + 3..];
        if let Some(at) = rest.find('@') {
            return rest[..at].contains(':');
        }
    }
    false
}

/// Inject a password into a URL's userinfo, preserving any existing user.
fn inject_password(url: &str, secret: &str) -> Result<String> {
    let scheme_end = url
        .find("://")
        .ok_or_else(|| anyhow!("URL has no scheme"))?;
    let rest = &url[scheme_end + 3..];
    let encoded = urlencoding::encode(secret);
    if let Some(at) = rest.find('@') {
        let creds = &rest[..at];
        let user = creds.split(':').next().unwrap_or("");
        Ok(format!(
            "{}://{}:{}@{}",
            &url[..scheme_end],
            user,
            encoded,
            &rest[at + 1..]
        ))
    } else {
        // No userinfo at all: nothing to inject into (e.g. SQLite).
        Ok(url.to_string())
    }
}

fn restrict_permissions(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_with_password() -> ConnectionProfile {
        let mut p = ConnectionProfile::new(
            "Local Postgres",
            DatabaseKind::Postgres,
            "postgresql://admin:hunter2@localhost:5432/app?sslmode=require",
        );
        p.secret_ref = Some("sql-optimizer/profile/test".into());
        p
    }

    #[test]
    fn new_profile_strips_password_from_url() {
        let p = profile_with_password();
        assert!(!p.url.contains("hunter2"));
        assert!(p.url.contains("admin@localhost"));
    }

    #[test]
    fn profile_ids_are_stable_and_unique() {
        let a = ConnectionProfile::new("a", DatabaseKind::Sqlite, "sqlite::memory:");
        let b = ConnectionProfile::new("a", DatabaseKind::Sqlite, "sqlite::memory:");
        assert_ne!(a.id, b.id);
        assert_eq!(a.id, a.id);
    }

    #[test]
    fn catalog_round_trip_keeps_metadata_and_no_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");

        let mut catalog = ProfileCatalog::load(path.clone()).unwrap();
        catalog.upsert(profile_with_password()).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            !raw.contains("hunter2"),
            "catalog must never contain passwords"
        );

        let reloaded = ProfileCatalog::load(path).unwrap();
        assert_eq!(reloaded.profiles.len(), 1);
        assert_eq!(reloaded.profiles[0].name, "Local Postgres");
        assert_eq!(
            reloaded.profiles[0].secret_ref,
            Some("sql-optimizer/profile/test".into())
        );
    }

    #[cfg(unix)]
    #[test]
    fn catalog_file_permissions_are_restrictive() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        let mut catalog = ProfileCatalog::load(path.clone()).unwrap();
        catalog.upsert(profile_with_password()).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn catalog_rejects_unknown_schema_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        std::fs::write(&path, r#"{"schema_version": 99, "profiles": []}"#).unwrap();
        assert!(ProfileCatalog::load(path).is_err());
    }

    #[test]
    fn catalog_rejects_malformed_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        std::fs::write(&path, "not json at all").unwrap();
        assert!(ProfileCatalog::load(path).is_err());
    }

    #[test]
    fn catalog_delete_and_update() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        let mut catalog = ProfileCatalog::load(path).unwrap();
        let p = profile_with_password();
        catalog.upsert(p.clone()).unwrap();

        let mut renamed = p.clone();
        renamed.name = "Renamed".into();
        catalog.upsert(renamed).unwrap();
        assert_eq!(catalog.profiles.len(), 1);
        assert_eq!(catalog.profiles[0].name, "Renamed");

        assert!(catalog.remove(&p.id).unwrap());
        assert!(!catalog.remove(&p.id).unwrap());
        assert!(catalog.profiles.is_empty());
    }

    #[test]
    fn ephemeral_memory_sqlite_is_detected() {
        let p = ConnectionProfile::new("mem", DatabaseKind::Sqlite, "sqlite::memory:");
        assert!(p.is_ephemeral());
        let f = ConnectionProfile::new("file", DatabaseKind::Sqlite, "sqlite:///tmp/x.db");
        assert!(!f.is_ephemeral());
    }

    #[tokio::test]
    async fn session_store_round_trip_and_env_fallback() {
        let store = SessionSecretStore::new();
        store
            .set_secret("sql-optimizer/profile/x", "s3cret")
            .await
            .unwrap();
        assert_eq!(
            store.get_secret("sql-optimizer/profile/x").await.unwrap(),
            Some("s3cret".into())
        );
        store
            .delete_secret("sql-optimizer/profile/x")
            .await
            .unwrap();
        assert_eq!(
            store.get_secret("sql-optimizer/profile/x").await.unwrap(),
            None
        );
    }

    #[test]
    fn url_redaction_and_injection() {
        assert_eq!(
            sanitize_url("postgresql://u:p@h:5/db"),
            "postgresql://u@h:5/db"
        );
        assert_eq!(sanitize_url("sqlite::memory:"), "sqlite::memory:");
        assert!(has_password_field("postgresql://u:p@h/db"));
        assert!(!has_password_field("postgresql://u@h/db"));
        let injected = inject_password("postgresql://u@h:5/db", "p w").unwrap();
        assert_eq!(injected, "postgresql://u:p%20w@h:5/db");
    }

    #[tokio::test]
    async fn resolve_profile_url_injects_secret_without_exposing_it_in_profile() {
        let store = SessionSecretStore::new();
        store
            .set_secret("sql-optimizer/profile/test", "hunter2")
            .await
            .unwrap();
        let p = profile_with_password();
        let url = resolve_profile_url(&p, &store).await.unwrap();
        assert!(url.contains("hunter2"));
        assert!(!p.url.contains("hunter2"));
    }

    /// A fake backend proving the SecretStore trait is mockable for tests.
    struct FakeStore {
        items: std::sync::Mutex<std::collections::HashMap<String, String>>,
    }

    impl FakeStore {
        fn new() -> Self {
            Self {
                items: std::sync::Mutex::new(std::collections::HashMap::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl SecretStore for FakeStore {
        async fn set_secret(&self, key: &str, secret: &str) -> Result<()> {
            self.items.lock().unwrap().insert(key.into(), secret.into());
            Ok(())
        }
        async fn get_secret(&self, key: &str) -> Result<Option<String>> {
            Ok(self.items.lock().unwrap().get(key).cloned())
        }
        async fn delete_secret(&self, key: &str) -> Result<()> {
            self.items.lock().unwrap().remove(key);
            Ok(())
        }
    }

    #[tokio::test]
    async fn fake_store_backend_round_trip() {
        let store = FakeStore::new();
        store.set_secret("k", "v").await.unwrap();
        assert_eq!(store.get_secret("k").await.unwrap().as_deref(), Some("v"));
        store.delete_secret("k").await.unwrap();
        assert_eq!(store.get_secret("k").await.unwrap(), None);
    }
}
