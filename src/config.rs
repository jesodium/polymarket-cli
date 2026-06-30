use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const ENV_VAR: &str = "POLYMARKET_PRIVATE_KEY";
const SIG_TYPE_ENV_VAR: &str = "POLYMARKET_SIGNATURE_TYPE";
const PROXY_ENV_VAR: &str = "POLYMARKET_PROXY_ADDRESS";
pub(crate) const DEFAULT_SIGNATURE_TYPE: &str = "proxy";

/// OS keychain coordinates for the private key (one wallet per machine).
const KEYRING_SERVICE: &str = "fiberglass";
const KEYRING_USER: &str = "private-key";

pub(crate) const NO_WALLET_MSG: &str =
    "No wallet configured. Run `polymarket wallet create` or `polymarket wallet import <key>`";

#[derive(Serialize, Deserialize)]
pub(crate) struct Config {
    /// The private key, when stored in the plaintext config file. Absent when
    /// the key lives in the OS keychain (the default on machines that have one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_key: Option<String>,
    pub chain_id: u64,
    #[serde(default = "default_signature_type")]
    pub signature_type: String,
    /// Optional override for the funder/proxy wallet. Accounts created via the
    /// Polymarket web UI (Magic/email) get a server-assigned proxy that does
    /// not match the locally-derived CREATE2 address; set this to the real one
    /// (look it up with `polymarket profiles get <address>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_address: Option<String>,
}

fn default_signature_type() -> String {
    DEFAULT_SIGNATURE_TYPE.to_string()
}

pub(crate) enum KeySource {
    Flag,
    EnvVar,
    Keychain,
    ConfigFile,
    None,
}

impl KeySource {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Flag => "--private-key flag",
            Self::EnvVar => "POLYMARKET_PRIVATE_KEY env var",
            Self::Keychain => "OS keychain",
            Self::ConfigFile => "config file (plaintext)",
            Self::None => "not configured",
        }
    }
}

/// Where `save_wallet` actually put the key, so callers can tell the user.
pub(crate) enum KeyStorage {
    Keychain,
    PlaintextFile,
}

// --- OS keychain -----------------------------------------------------------
// Best-effort: every operation degrades to "not available" rather than failing,
// so a headless box with no secret service still works off the plaintext file.

/// Keychain service name; overridable for test isolation.
fn keyring_service() -> String {
    std::env::var("POLYMARKET_KEYRING_SERVICE").unwrap_or_else(|_| KEYRING_SERVICE.to_string())
}

fn keychain_set(key: &str) -> bool {
    keyring::Entry::new(&keyring_service(), KEYRING_USER)
        .and_then(|e| e.set_password(key))
        .is_ok()
}

fn keychain_get() -> Option<String> {
    keyring::Entry::new(&keyring_service(), KEYRING_USER)
        .ok()?
        .get_password()
        .ok()
}

fn keychain_delete() {
    if let Ok(e) = keyring::Entry::new(&keyring_service(), KEYRING_USER) {
        let _ = e.delete_credential();
    }
}

pub(crate) fn config_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not determine home directory")?;
    Ok(home.join(".config").join("polymarket"))
}

pub fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.json"))
}

pub fn config_exists() -> bool {
    config_path().is_ok_and(|p| p.exists())
}

/// Delete the wallet config file only. Other files in the config directory
/// (e.g. the paper trading account) are left untouched.
pub fn delete_config() -> Result<()> {
    keychain_delete();
    let path = config_path()?;
    if path.exists() {
        fs::remove_file(&path).context("Failed to remove config file")?;
    }
    Ok(())
}

/// Load config from disk. Returns `Ok(None)` if no config file exists,
/// or `Err` if the file exists but can't be read or parsed.
pub fn load_config() -> Result<Option<Config>> {
    let path = config_path()?;
    let data = match fs::read_to_string(&path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(anyhow::anyhow!(e).context(format!("Failed to read {}", path.display())));
        }
    };
    let config = serde_json::from_str(&data)
        .context(format!("Invalid JSON in config file {}", path.display()))?;
    Ok(Some(config))
}

/// Priority: CLI flag > env var > config file > default ("proxy").
pub fn resolve_signature_type(cli_flag: Option<&str>) -> Result<String> {
    if let Some(st) = cli_flag {
        return Ok(st.to_string());
    }
    if let Ok(st) = std::env::var(SIG_TYPE_ENV_VAR)
        && !st.is_empty()
    {
        return Ok(st);
    }
    if let Some(config) = load_config()? {
        return Ok(config.signature_type);
    }
    Ok(DEFAULT_SIGNATURE_TYPE.to_string())
}

pub fn save_wallet(key: &str, chain_id: u64, signature_type: &str) -> Result<KeyStorage> {
    // A freshly created/imported wallet starts with no proxy override; the
    // derived address applies until the user sets one with `wallet set-proxy`.
    // Prefer the OS keychain; only fall back to a plaintext file if there is no
    // keychain backend (e.g. a headless server with no secret service).
    let in_keychain = keychain_set(key);
    write_config(&Config {
        private_key: (!in_keychain).then(|| key.to_string()),
        chain_id,
        signature_type: signature_type.to_string(),
        proxy_address: None,
    })?;
    Ok(if in_keychain {
        KeyStorage::Keychain
    } else {
        KeyStorage::PlaintextFile
    })
}

/// Migrate a plaintext key from the config file into the OS keychain and scrub
/// it from disk. Returns the resulting storage. Errors if no wallet exists.
pub fn secure_existing_key() -> Result<KeyStorage> {
    let (key, _) = resolve_key(None)?;
    let key = key.ok_or_else(|| anyhow::anyhow!("{}", NO_WALLET_MSG))?;
    if !keychain_set(&key) {
        anyhow::bail!("No OS keychain available on this machine; key left in the config file.");
    }
    // Drop the plaintext key from the file, keep the rest of the config.
    if let Some(mut config) = load_config()? {
        config.private_key = None;
        write_config(&config)?;
    }
    Ok(KeyStorage::Keychain)
}

/// Resolve the funder/proxy override. Priority: env var > config file.
/// `None` means "use the derived proxy address".
pub fn resolve_proxy_address() -> Result<Option<String>> {
    if let Ok(v) = std::env::var(PROXY_ENV_VAR)
        && !v.is_empty()
    {
        return Ok(Some(v));
    }
    Ok(load_config()?.and_then(|c| c.proxy_address))
}

/// Set (or clear, with `None`) the proxy override in the config file,
/// preserving the rest of the wallet config. Errors if no wallet exists.
pub fn set_proxy_address(proxy: Option<&str>) -> Result<()> {
    let mut config = load_config()?.ok_or_else(|| anyhow::anyhow!("{}", NO_WALLET_MSG))?;
    config.proxy_address = proxy.map(str::to_string);
    write_config(&config)
}

/// Set the signature type in the config file, preserving the rest of the
/// wallet config. Errors if no wallet exists.
pub fn set_signature_type(signature_type: &str) -> Result<()> {
    let mut config = load_config()?.ok_or_else(|| anyhow::anyhow!("{}", NO_WALLET_MSG))?;
    config.signature_type = signature_type.to_string();
    write_config(&config)
}

/// Write the wallet config to disk with owner-only permissions.
fn write_config(config: &Config) -> Result<()> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir).context("Failed to create config directory")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    }

    let json = serde_json::to_string_pretty(config)?;
    let path = config_path()?;

    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .context("Failed to create config file")?;
        file.write_all(json.as_bytes())
            .context("Failed to write config file")?;
    }

    #[cfg(not(unix))]
    {
        fs::write(&path, &json).context("Failed to write config file")?;
    }

    Ok(())
}

/// Priority: CLI flag > env var > OS keychain > plaintext config file.
pub fn resolve_key(cli_flag: Option<&str>) -> Result<(Option<String>, KeySource)> {
    if let Some(key) = cli_flag {
        return Ok((Some(key.to_string()), KeySource::Flag));
    }
    if let Ok(key) = std::env::var(ENV_VAR)
        && !key.is_empty()
    {
        return Ok((Some(key), KeySource::EnvVar));
    }
    if let Some(key) = keychain_get() {
        return Ok((Some(key), KeySource::Keychain));
    }
    if let Some(key) = load_config()?.and_then(|c| c.private_key) {
        return Ok((Some(key), KeySource::ConfigFile));
    }
    Ok((None, KeySource::None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Mutex to serialize env var tests (set_var is not thread-safe)
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    unsafe fn set(var: &str, val: &str) {
        unsafe { std::env::set_var(var, val) };
    }

    unsafe fn unset(var: &str) {
        unsafe { std::env::remove_var(var) };
    }

    #[test]
    fn resolve_key_flag_overrides_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        unsafe { set(ENV_VAR, "env_key") };
        let (key, source) = resolve_key(Some("flag_key")).unwrap();
        assert_eq!(key.unwrap(), "flag_key");
        assert!(matches!(source, KeySource::Flag));
        unsafe { unset(ENV_VAR) };
    }

    #[test]
    fn resolve_key_env_var_returns_env_value() {
        let _lock = ENV_LOCK.lock().unwrap();
        unsafe { set(ENV_VAR, "env_key_value") };
        let (key, source) = resolve_key(None).unwrap();
        assert_eq!(key.unwrap(), "env_key_value");
        assert!(matches!(source, KeySource::EnvVar));
        unsafe { unset(ENV_VAR) };
    }

    #[test]
    fn resolve_key_skips_empty_env_var() {
        let _lock = ENV_LOCK.lock().unwrap();
        unsafe { set(ENV_VAR, "") };
        let (_, source) = resolve_key(None).unwrap();
        assert!(!matches!(source, KeySource::EnvVar));
        unsafe { unset(ENV_VAR) };
    }

    #[test]
    fn resolve_sig_type_flag_overrides_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        unsafe { set(SIG_TYPE_ENV_VAR, "eoa") };
        assert_eq!(
            resolve_signature_type(Some("gnosis-safe")).unwrap(),
            "gnosis-safe"
        );
        unsafe { unset(SIG_TYPE_ENV_VAR) };
    }

    #[test]
    fn resolve_sig_type_env_var_returns_env_value() {
        let _lock = ENV_LOCK.lock().unwrap();
        unsafe { set(SIG_TYPE_ENV_VAR, "eoa") };
        assert_eq!(resolve_signature_type(None).unwrap(), "eoa");
        unsafe { unset(SIG_TYPE_ENV_VAR) };
    }

    #[test]
    fn resolve_proxy_env_var_takes_precedence() {
        let _lock = ENV_LOCK.lock().unwrap();
        unsafe { set(PROXY_ENV_VAR, "0x1234567890123456789012345678901234567890") };
        assert_eq!(
            resolve_proxy_address().unwrap(),
            Some("0x1234567890123456789012345678901234567890".to_string())
        );
        unsafe { unset(PROXY_ENV_VAR) };
    }

    #[test]
    fn resolve_sig_type_without_env_returns_nonempty() {
        let _lock = ENV_LOCK.lock().unwrap();
        unsafe { unset(SIG_TYPE_ENV_VAR) };
        let result = resolve_signature_type(None).unwrap();
        assert!(!result.is_empty());
    }

    #[test]
    fn config_omits_private_key_when_in_keychain() {
        // When the key lives in the keychain, the file must not carry it.
        let c = Config {
            private_key: None,
            chain_id: 137,
            signature_type: "proxy".into(),
            proxy_address: None,
        };
        let json = serde_json::to_string(&c).unwrap();
        assert!(
            !json.contains("private_key"),
            "key leaked into file: {json}"
        );
    }

    // Hits the real OS keychain, so it's opt-in: `cargo test -- --ignored`.
    // Uses an isolated service name and cleans up after itself.
    #[test]
    #[ignore]
    fn keychain_roundtrip() {
        let _lock = ENV_LOCK.lock().unwrap();
        unsafe { set("POLYMARKET_KEYRING_SERVICE", "fiberglass-sbtest") };
        assert!(keychain_set("0xdeadbeef"), "no keychain backend available");
        assert_eq!(keychain_get().as_deref(), Some("0xdeadbeef"));
        keychain_delete();
        assert!(keychain_get().is_none());
        unsafe { unset("POLYMARKET_KEYRING_SERVICE") };
    }
}
