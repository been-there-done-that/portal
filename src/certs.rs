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
    ///
    /// If mkcert is installed and its CA is already trusted by the OS, portal
    /// borrows mkcert's CA so all dynamically generated host certs are signed by
    /// a CA that browsers already trust — no admin password required.
    pub fn ensure_ca(&self) -> Result<()> {
        let ca_pem_path = self.dir.join("ca.pem");
        let ca_key_path = self.dir.join("ca-key.pem");

        if ca_pem_path.exists() && ca_key_path.exists() {
            return Ok(());
        }

        // Prefer mkcert's CA when available: it is already trusted by the OS
        // (mkcert -install was run previously) so no extra trust step is needed.
        if let Some(mkcert_dir) = mkcert_caroot() {
            let src_cert = mkcert_dir.join("rootCA.pem");
            let src_key = mkcert_dir.join("rootCA-key.pem");
            if src_cert.exists() && src_key.exists() {
                std::fs::copy(&src_cert, &ca_pem_path)?;
                std::fs::copy(&src_key, &ca_key_path)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&ca_key_path, std::fs::Permissions::from_mode(0o600))?;
                }
                tracing::info!("Using mkcert CA (already trusted by OS)");
                return Ok(());
            }
        }

        // Fallback: generate portal's own self-signed CA.
        let key_pair = KeyPair::generate()
            .map_err(|e| Error::Cert(format!("CA key generation failed: {e}")))?;

        let mut params = CertificateParams::default();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        params.not_after = rcgen::date_time_ymd(2034, 1, 1);
        params
            .distinguished_name
            .push(DnType::CommonName, "Portal Local CA");

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
        let ca_pem_bytes = self.ca_pem()?;

        // 2. Disk cache hit
        if cert_path.exists() && key_path.exists() {
            let ck = load_certified_key_with_ca(&cert_path, &key_path, &ca_pem_bytes)?;
            let ck = Arc::new(ck);
            self.cache.insert(hostname.to_string(), Arc::clone(&ck));
            return Ok(ck);
        }

        // 3. Generate new host cert signed by CA
        let ca_pem = String::from_utf8(ca_pem_bytes.clone())
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
        if let Some(ip) = lan_ip_san() {
            host_params.subject_alt_names.push(rcgen::SanType::IpAddress(ip));
        }
        host_params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        host_params.not_after = rcgen::date_time_ymd(2034, 1, 1);
        host_params
            .distinguished_name
            .push(DnType::CommonName, hostname);
        // AKI is required by RFC 5280 and enforced by Chrome/Safari/modern validators.
        // EKU serverAuth tells TLS clients this cert is valid for HTTPS.
        host_params.use_authority_key_identifier_extension = true;
        host_params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];

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

        let ck = Arc::new(load_certified_key_with_ca(&cert_path, &key_path, &ca_pem_bytes)?);
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
    let ca_path = crate::config::dirs_for_state().join("certs").join("ca.pem");
    if !ca_path.exists() {
        return false;
    }
    // verify-cert checks all keychains (system + user login) for trust.
    Command::new("security")
        .args([
            "verify-cert",
            "-c",
            ca_path.to_str().unwrap_or(""),
            "-L",
            "-p",
            "ssl",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
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

/// Return the mkcert CA root directory if mkcert is installed.
fn mkcert_caroot() -> Option<std::path::PathBuf> {
    let out = std::process::Command::new("mkcert")
        .arg("-CAROOT")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8(out.stdout).ok()?;
    Some(std::path::PathBuf::from(path.trim()))
}

fn load_certified_key(
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> Result<CertifiedKey> {
    load_certified_key_with_ca(cert_path, key_path, &[])
}

/// Like `load_certified_key` but appends the CA cert to the chain so clients
/// receive the full leaf → CA chain during the TLS handshake.
fn load_certified_key_with_ca(
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
    ca_pem: &[u8],
) -> Result<CertifiedKey> {
    // Load leaf cert
    let cert_pem = std::fs::read(cert_path)?;
    let mut certs: Vec<CertificateDer<'static>> = {
        let mut reader = BufReader::new(cert_pem.as_slice());
        rustls_pemfile::certs(&mut reader).collect::<std::io::Result<Vec<_>>>()?
    };

    // Append CA cert so the full chain is sent during TLS handshake
    if !ca_pem.is_empty() {
        let mut reader = BufReader::new(ca_pem);
        let ca_certs: Vec<CertificateDer<'static>> =
            rustls_pemfile::certs(&mut reader).collect::<std::io::Result<Vec<_>>>()?;
        certs.extend(ca_certs);
    }

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
fn login_keychain_path() -> String {
    use std::process::Command;
    // `security default-keychain` prints e.g. `    "/Users/foo/Library/Keychains/login.keychain-db"`
    if let Ok(out) = Command::new("security")
        .args(["default-keychain"])
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        let clean = text.trim().trim_matches('"');
        if !clean.is_empty() {
            return clean.to_string();
        }
    }
    // Fallback: conventional path
    let home = std::env::var("HOME").unwrap_or_default();
    format!("{home}/Library/Keychains/login.keychain-db")
}

#[cfg(target_os = "macos")]
fn install_system_trust_impl(ca_path: &std::path::Path) -> Result<()> {
    use std::process::Command;

    let ca_str = ca_path
        .to_str()
        .ok_or_else(|| Error::Cert("invalid CA path".into()))?;

    let is_root = unsafe { nix::libc::geteuid() } == 0;
    if is_root {
        // Root: write directly to System.keychain.
        let status = Command::new("security")
            .args([
                "add-trusted-cert",
                "-d",
                "-r",
                "trustRoot",
                "-k",
                "/Library/Keychains/System.keychain",
                ca_str,
            ])
            .status()?;
        if !status.success() {
            return Err(Error::Cert(format!(
                "security add-trusted-cert failed with exit code {:?}",
                status.code()
            )));
        }
    } else {
        // Non-root: add to the user's login keychain. No sudo required.
        // macOS marks it as a trusted root for all policies in the user trust store.
        let keychain = login_keychain_path();
        let status = Command::new("security")
            .args([
                "add-trusted-cert",
                "-r",
                "trustRoot",
                "-k",
                keychain.as_str(),
                ca_str,
            ])
            .status()?;
        if !status.success() {
            return Err(Error::Cert(format!(
                "security add-trusted-cert failed with exit code {:?}",
                status.code()
            )));
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn install_system_trust_impl(ca_path: &std::path::Path) -> Result<()> {
    use std::process::Command;

    let is_root = unsafe { nix::libc::geteuid() } == 0;
    if !is_root {
        let has_tty = unsafe { nix::libc::isatty(0) } == 1;
        if !has_tty {
            return Err(Error::Cert(
                "installing the CA requires admin privileges; run: sudo portal cert install".into(),
            ));
        }
        let status = Command::new("sudo")
            .args([
                "portal",
                "cert",
                "install",
            ])
            .status()?;
        if !status.success() {
            return Err(Error::Cert(format!(
                "sudo portal cert install failed with exit code {:?}",
                status.code()
            )));
        }
        return Ok(());
    }

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
// System trust store removal
// ---------------------------------------------------------------------------

/// Return the LAN IP to embed as a SAN in host certs, if LAN mode is active.
pub fn lan_ip_san() -> Option<std::net::IpAddr> {
    std::env::var("PORTLESS_LAN_IP")
        .ok()
        .and_then(|s| s.parse().ok())
}

/// Remove the portal CA from the system trust store.
/// Best-effort: returns Ok even if the cert was never trusted.
pub fn untrust_system_ca() -> Result<()> {
    let ca_path = crate::config::dirs_for_state().join("certs").join("ca.pem");
    untrust_system_ca_impl(&ca_path)
}

#[cfg(target_os = "macos")]
fn untrust_system_ca_impl(ca_path: &std::path::Path) -> Result<()> {
    if !ca_path.exists() {
        return Ok(());
    }
    let status = std::process::Command::new("security")
        .args([
            "remove-trusted-cert",
            "-d",
            ca_path.to_str().ok_or_else(|| Error::Cert("invalid CA path".into()))?,
        ])
        .status()?;
    if !status.success() {
        tracing::warn!("security remove-trusted-cert exited {:?}", status.code());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn untrust_system_ca_impl(_ca_path: &std::path::Path) -> Result<()> {
    let dest = std::path::Path::new("/usr/local/share/ca-certificates/portal-ca.crt");
    if dest.exists() {
        std::fs::remove_file(dest)?;
        let _ = std::process::Command::new("update-ca-certificates").status();
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn untrust_system_ca_impl(_ca_path: &std::path::Path) -> Result<()> {
    Ok(()) // no-op on unsupported platforms
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

    #[test]
    fn untrust_system_ca_does_not_panic_when_cert_missing() {
        // Should be a no-op / best-effort — never panics even if cert doesn't exist
        let result = untrust_system_ca();
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // AKI / EKU extension tests (require x509-parser)
    // -----------------------------------------------------------------------

    /// Parse the leaf cert DER from a CertifiedKey (index 0 in the chain).
    fn leaf_der(ck: &rustls::sign::CertifiedKey) -> Vec<u8> {
        ck.cert[0].as_ref().to_vec()
    }

    /// Parse the first CA cert DER from a PEM byte slice.
    fn ca_der_from_pem(ca_pem: &[u8]) -> Vec<u8> {
        use std::io::BufReader as StdBufReader;
        let mut reader = StdBufReader::new(ca_pem);
        let der: Vec<rustls::pki_types::CertificateDer<'static>> =
            rustls_pemfile::certs(&mut reader)
                .collect::<std::io::Result<Vec<_>>>()
                .expect("reading CA DER");
        der.into_iter()
            .next()
            .expect("CA PEM should contain at least one cert")
            .as_ref()
            .to_vec()
    }

    #[test]
    fn host_cert_has_authority_key_identifier() {
        use x509_parser::prelude::*;

        let (_tmp, store) = make_store();
        store.ensure_ca().expect("ensure_ca");
        let ck = store.cert_for_host("aki.localhost").expect("cert_for_host");

        let der = leaf_der(&ck);
        let (_, cert) = X509Certificate::from_der(&der).expect("parse leaf DER");

        let has_aki = cert.extensions().iter().any(|ext| {
            matches!(
                ext.parsed_extension(),
                ParsedExtension::AuthorityKeyIdentifier(_)
            )
        });
        assert!(has_aki, "leaf cert must contain an AKI extension");
    }

    #[test]
    fn host_cert_aki_matches_ca_subject_key_identifier() {
        use x509_parser::prelude::*;

        let (_tmp, store) = make_store();
        store.ensure_ca().expect("ensure_ca");
        let ck = store
            .cert_for_host("aki-ski.localhost")
            .expect("cert_for_host");
        let ca_pem = store.ca_pem().expect("ca_pem");

        let leaf_der_bytes = leaf_der(&ck);
        let (_, leaf) = X509Certificate::from_der(&leaf_der_bytes).expect("parse leaf DER");

        let ca_der_bytes = ca_der_from_pem(&ca_pem);
        let (_, ca_cert) = X509Certificate::from_der(&ca_der_bytes).expect("parse CA DER");

        // Extract AKI key identifier from leaf cert
        let aki_id: Vec<u8> = leaf
            .extensions()
            .iter()
            .find_map(|ext| {
                if let ParsedExtension::AuthorityKeyIdentifier(aki) = ext.parsed_extension() {
                    aki.key_identifier.as_ref().map(|ki| ki.0.to_vec())
                } else {
                    None
                }
            })
            .expect("leaf cert must have AKI with key identifier");

        // Extract SKI from CA cert
        let ski_id: Vec<u8> = ca_cert
            .extensions()
            .iter()
            .find_map(|ext| {
                if let ParsedExtension::SubjectKeyIdentifier(ski) = ext.parsed_extension() {
                    Some(ski.0.to_vec())
                } else {
                    None
                }
            })
            .expect("CA cert must have SKI extension");

        assert_eq!(
            aki_id, ski_id,
            "leaf cert AKI key identifier must match CA SKI"
        );
    }

    #[test]
    fn host_cert_has_server_auth_eku() {
        use x509_parser::prelude::*;

        let (_tmp, store) = make_store();
        store.ensure_ca().expect("ensure_ca");
        let ck = store.cert_for_host("eku.localhost").expect("cert_for_host");

        let der = leaf_der(&ck);
        let (_, cert) = X509Certificate::from_der(&der).expect("parse leaf DER");

        let has_server_auth = cert.extensions().iter().any(|ext| {
            if let ParsedExtension::ExtendedKeyUsage(eku) = ext.parsed_extension() {
                eku.server_auth
            } else {
                false
            }
        });
        assert!(
            has_server_auth,
            "leaf cert must contain EKU serverAuth (id-kp-serverAuth)"
        );
    }
}
