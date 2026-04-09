use std::io::BufReader;
use std::path::PathBuf;
use std::sync::Arc;

use dashmap::DashMap;
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::sign::CertifiedKey;

use crate::error::{Error, Result};

// ---------------------------------------------------------------------------
// CertStore
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct CertStore {
    dir: PathBuf,
    cache: Arc<DashMap<String, Arc<CertifiedKey>>>,
}

impl CertStore {
    /// Create a new `CertStore` rooted at `dir`.
    /// Creates `dir` and `dir/hosts/` if they do not already exist.
    pub fn new(dir: PathBuf) -> Self {
        let hosts_dir = dir.join("hosts");
        std::fs::create_dir_all(&hosts_dir).expect("failed to create cert directory");
        CertStore {
            dir,
            cache: Arc::new(DashMap::new()),
        }
    }

    /// Generate the local CA certificate and key if not already present (idempotent).
    pub fn ensure_ca(&self) -> Result<()> {
        let ca_pem_path = self.dir.join("ca.pem");
        let ca_key_path = self.dir.join("ca-key.pem");

        if ca_pem_path.exists() && ca_key_path.exists() {
            return Ok(());
        }

        // Generate key pair
        let key_pair = KeyPair::generate()
            .map_err(|e| Error::Cert(format!("CA key generation failed: {e}")))?;

        // Build certificate params
        let mut params = CertificateParams::default();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        params.not_after = rcgen::date_time_ymd(2034, 1, 1);
        params
            .distinguished_name
            .push(DnType::CommonName, "Portal Local CA");

        // Self-sign
        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| Error::Cert(format!("CA self-sign failed: {e}")))?;

        std::fs::write(&ca_pem_path, cert.pem())?;
        std::fs::write(&ca_key_path, key_pair.serialize_pem())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&ca_key_path, std::fs::Permissions::from_mode(0o600))?;
        }

        tracing::info!(
            "Generated local CA certificate at {}",
            ca_pem_path.display()
        );
        Ok(())
    }

    /// Read the raw PEM bytes for the CA certificate.
    pub fn ca_pem(&self) -> Result<Vec<u8>> {
        Ok(std::fs::read(self.dir.join("ca.pem"))?)
    }

    /// Return a `CertifiedKey` for the given hostname, using the in-memory cache.
    /// Falls back to disk, then generates a new cert signed by the local CA.
    pub fn cert_for_host(&self, hostname: &str) -> Result<Arc<CertifiedKey>> {
        // 1. In-memory cache hit
        if let Some(entry) = self.cache.get(hostname) {
            return Ok(Arc::clone(&*entry));
        }

        let safe = safe_hostname(hostname);
        let cert_path = self.dir.join("hosts").join(format!("{safe}.pem"));
        let key_path = self.dir.join("hosts").join(format!("{safe}-key.pem"));

        // 2. Disk cache hit
        if cert_path.exists() && key_path.exists() {
            let ck = load_certified_key(&cert_path, &key_path)?;
            let ck = Arc::new(ck);
            self.cache.insert(hostname.to_string(), Arc::clone(&ck));
            return Ok(ck);
        }

        // 3. Generate new host cert signed by CA
        let ca_pem = String::from_utf8(self.ca_pem()?)
            .map_err(|_| Error::Cert("CA PEM is not valid UTF-8".into()))?;
        let ca_key_pem = std::fs::read_to_string(self.dir.join("ca-key.pem"))?;

        let ca_key_pair = KeyPair::from_pem(&ca_key_pem)
            .map_err(|e| Error::Cert(format!("failed to load CA key: {e}")))?;
        let ca_params = CertificateParams::from_ca_cert_pem(&ca_pem)
            .map_err(|e| Error::Cert(format!("failed to parse CA cert: {e}")))?;
        let ca_cert = ca_params
            .self_signed(&ca_key_pair)
            .map_err(|e| Error::Cert(format!("failed to re-self-sign CA: {e}")))?;

        let host_key = KeyPair::generate()
            .map_err(|e| Error::Cert(format!("host key generation failed: {e}")))?;

        let mut host_params = CertificateParams::new(vec![hostname.to_string()])
            .map_err(|e| Error::Cert(format!("failed to build host params: {e}")))?;
        host_params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        host_params.not_after = rcgen::date_time_ymd(2034, 1, 1);
        host_params
            .distinguished_name
            .push(DnType::CommonName, hostname);

        let host_cert = host_params
            .signed_by(&host_key, &ca_cert, &ca_key_pair)
            .map_err(|e| Error::Cert(format!("failed to sign host cert: {e}")))?;

        std::fs::write(&cert_path, host_cert.pem())?;
        std::fs::write(&key_path, host_key.serialize_pem())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
        }

        let ck = Arc::new(load_certified_key(&cert_path, &key_path)?);
        self.cache.insert(hostname.to_string(), Arc::clone(&ck));
        Ok(ck)
    }

    /// Install the local CA into the OS trust store.
    pub fn install_system_trust(&self) -> Result<()> {
        let ca_path = self.dir.join("ca.pem");
        install_system_trust_impl(&ca_path)
    }
}

// ---------------------------------------------------------------------------
// System trust store check
// ---------------------------------------------------------------------------

/// Returns true if the Portal local CA certificate is already present in the
/// OS system trust store.  Returns false if not found or if the check fails.
#[cfg(target_os = "macos")]
pub fn is_ca_trusted() -> bool {
    use std::process::Command;
    Command::new("security")
        .args([
            "find-certificate",
            "-c",
            "Portal Local CA",
            "/Library/Keychains/System.keychain",
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
pub fn is_ca_trusted() -> bool {
    std::path::Path::new("/usr/local/share/ca-certificates/portal-ca.crt").exists()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn is_ca_trusted() -> bool {
    false
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn safe_hostname(hostname: &str) -> String {
    hostname.replace('.', "_").replace('*', "wildcard")
}

fn load_certified_key(
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> Result<CertifiedKey> {
    // Load cert chain
    let cert_pem = std::fs::read(cert_path)?;
    let certs: Vec<CertificateDer<'static>> = {
        let mut reader = BufReader::new(cert_pem.as_slice());
        rustls_pemfile::certs(&mut reader).collect::<std::io::Result<Vec<_>>>()?
    };

    // Load private key
    let key_pem = std::fs::read(key_path)?;
    let key = {
        let mut reader = BufReader::new(key_pem.as_slice());
        let keys: Vec<_> =
            rustls_pemfile::pkcs8_private_keys(&mut reader).collect::<std::io::Result<Vec<_>>>()?;
        let key_der = keys
            .into_iter()
            .next()
            .ok_or_else(|| Error::Cert("no private key found".into()))?;
        rustls::crypto::ring::sign::any_supported_type(&PrivateKeyDer::Pkcs8(key_der))
            .map_err(|e| Error::Cert(e.to_string()))?
    };

    Ok(CertifiedKey::new(certs, key))
}

// ---------------------------------------------------------------------------
// OS-specific trust store installation
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn install_system_trust_impl(ca_path: &std::path::Path) -> Result<()> {
    use std::process::Command;
    let status = Command::new("security")
        .args([
            "add-trusted-cert",
            "-d",
            "-r",
            "trustRoot",
            "-k",
            "/Library/Keychains/System.keychain",
            ca_path
                .to_str()
                .ok_or_else(|| Error::Cert("invalid CA path".into()))?,
        ])
        .status()?;
    if !status.success() {
        return Err(Error::Cert(format!(
            "security add-trusted-cert failed with exit code {:?}",
            status.code()
        )));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn install_system_trust_impl(ca_path: &std::path::Path) -> Result<()> {
    use std::process::Command;
    let dest = std::path::Path::new("/usr/local/share/ca-certificates/portal-ca.crt");
    std::fs::copy(ca_path, dest)?;
    let status = Command::new("update-ca-certificates").status()?;
    if !status.success() {
        return Err(Error::Cert(format!(
            "update-ca-certificates failed with exit code {:?}",
            status.code()
        )));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn install_system_trust_impl(ca_path: &std::path::Path) -> Result<()> {
    use std::process::Command;
    let status = Command::new("certutil")
        .args([
            "-addstore",
            "-f",
            "Root",
            ca_path
                .to_str()
                .ok_or_else(|| Error::Cert("invalid CA path".into()))?,
        ])
        .status()?;
    if !status.success() {
        return Err(Error::Cert(format!(
            "certutil -addstore failed with exit code {:?}",
            status.code()
        )));
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn install_system_trust_impl(_ca_path: &std::path::Path) -> Result<()> {
    Err(Error::Cert(
        "system trust store installation not supported on this OS".into(),
    ))
}

// ---------------------------------------------------------------------------
// PortlessCertResolver
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct PortlessCertResolver {
    store: CertStore,
}

impl PortlessCertResolver {
    pub fn new(store: CertStore) -> Self {
        PortlessCertResolver { store }
    }
}

impl rustls::server::ResolvesServerCert for PortlessCertResolver {
    fn resolve(&self, client_hello: rustls::server::ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let name = client_hello.server_name()?;
        self.store.cert_for_host(name).ok()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_store() -> (TempDir, CertStore) {
        let tmp = TempDir::new().expect("tempdir");
        let store = CertStore::new(tmp.path().to_path_buf());
        (tmp, store)
    }

    #[test]
    fn generates_ca_certificate() {
        let (_tmp, store) = make_store();
        store.ensure_ca().expect("ensure_ca failed");

        assert!(store.dir.join("ca.pem").exists(), "ca.pem should exist");
        assert!(
            store.dir.join("ca-key.pem").exists(),
            "ca-key.pem should exist"
        );
    }

    #[test]
    fn ca_is_stable_across_calls() {
        let (_tmp, store) = make_store();
        store.ensure_ca().expect("first ensure_ca");
        let first = store.ca_pem().expect("read ca.pem first time");
        store.ensure_ca().expect("second ensure_ca");
        let second = store.ca_pem().expect("read ca.pem second time");
        assert_eq!(first, second, "ca.pem should be identical across calls");
    }

    #[test]
    fn generates_host_cert_signed_by_ca() {
        let (_tmp, store) = make_store();
        store.ensure_ca().expect("ensure_ca");
        let ck = store
            .cert_for_host("myapp.localhost")
            .expect("cert_for_host failed");
        // Just verify the Arc is valid (non-null) and cert chain is non-empty
        assert!(!ck.cert.is_empty(), "cert chain should not be empty");
    }

    #[test]
    fn host_cert_cached_on_second_call() {
        let (_tmp, store) = make_store();
        store.ensure_ca().expect("ensure_ca");
        let first = store.cert_for_host("cached.localhost").expect("first call");
        let second = store
            .cert_for_host("cached.localhost")
            .expect("second call");
        assert!(
            Arc::ptr_eq(&first, &second),
            "second call should return the same Arc"
        );
    }

    #[test]
    fn is_ca_trusted_returns_bool_without_panicking() {
        // Just verify the function is callable and returns a bool.
        // In a normal test environment the portal CA is not installed,
        // so we expect false. On a machine where it IS installed this
        // passes regardless.
        let trusted = is_ca_trusted();
        // Either outcome is acceptable; the function must not panic.
        let _ = trusted;
    }

    #[test]
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    fn is_ca_trusted_always_false_on_unsupported_platform() {
        assert!(
            !is_ca_trusted(),
            "unsupported platforms must always return false"
        );
    }
}
