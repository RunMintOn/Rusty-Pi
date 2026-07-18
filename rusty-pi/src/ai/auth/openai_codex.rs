//! OpenAI Codex OAuth — device code & browser login, credential storage, token refresh.
//!
//! Mirrors `@earendil-works/pi-ai/src/auth/oauth/openai-codex.ts`.

use base64::Engine;
use sha2::Digest;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const DEVICE_USER_CODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
const DEVICE_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
const DEVICE_CODE_TIMEOUT_SECS: u64 = 15 * 60;
const DEFAULT_POLL_INTERVAL_SECS: u64 = 5;
const MIN_POLL_INTERVAL_MS: u64 = 1000;

/// Credential file path derived from home directory.
fn credentials_path() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).ok()?;
    Some(std::path::PathBuf::from(home).join(".config/pi-codex-credentials.json"))
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// OAuth credential with access token, refresh token, expiry, and account id.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodexCredential {
    pub access: String,
    pub refresh: String,
    pub expires_at: i64, // epoch ms
    pub account_id: String,
}

impl CodexCredential {
    /// Returns true if the access token is expired or expires within 60 seconds.
    pub fn is_expired(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        self.expires_at <= now + 60_000
    }

    /// Save this credential to the default file path.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = credentials_path()
            .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory for credential storage"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    /// Load a saved credential from the default file path.
    pub fn load() -> anyhow::Result<Option<Self>> {
        let path = match credentials_path() {
            Some(p) => p,
            None => return Ok(None),
        };
        if !path.exists() {
            return Ok(None);
        }
        let json = std::fs::read_to_string(&path)?;
        let cred: Self = serde_json::from_str(&json)?;
        Ok(Some(cred))
    }

    /// Delete the credential file.
    pub fn delete() -> anyhow::Result<()> {
        if let Some(path) = credentials_path()
            && path.exists() {
                std::fs::remove_file(&path)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// PKCE
// ---------------------------------------------------------------------------

/// Generate PKCE code verifier and SHA-256 challenge (base64url-encoded, no padding).
pub fn generate_pkce() -> (String, String) {
    let verifier_bytes: Vec<u8> = (0..32).map(|_| rand::random::<u8>()).collect();
    let verifier = base64_url_encode(&verifier_bytes);

    let hash = sha2::Sha256::digest(&verifier_bytes);
    let challenge = base64_url_encode(&hash);

    (verifier, challenge)
}

fn base64_url_encode(data: &[u8]) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    URL_SAFE_NO_PAD.encode(data)
}

// ---------------------------------------------------------------------------
// Device code polling
// ---------------------------------------------------------------------------

/// Poll the device auth endpoint until the user authenticates.
async fn poll_device_auth(
    device_auth_id: &str,
    user_code: &str,
    interval_secs: u64,
    signal: Option<tokio::sync::watch::Receiver<bool>>,
) -> anyhow::Result<(String, String)> {
    let client = reqwest::Client::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(DEVICE_CODE_TIMEOUT_SECS);
    let mut interval_ms = (interval_secs * 1000).max(MIN_POLL_INTERVAL_MS);
    let mut slow_count = 0;

    while std::time::Instant::now() < deadline {
        // Check abort signal
        if let Some(ref rx) = signal && *rx.borrow() {
            anyhow::bail!("Login cancelled");
        }

        // Wait before next poll (except first)
        if slow_count > 0 || interval_ms > 0 {
            let sleep = tokio::time::sleep(std::time::Duration::from_millis(interval_ms));
            tokio::select! {
                _ = sleep => {}
                _ = wait_for_abort(signal.clone()) => {
                    anyhow::bail!("Login cancelled");
                }
            }
        }

        let resp = client
            .post(DEVICE_TOKEN_URL)
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "device_auth_id": device_auth_id,
                "user_code": user_code,
            }))
            .send()
            .await?;

        if resp.status().is_success() {
            let json: serde_json::Value = resp.json().await?;
            let auth_code = json["authorization_code"].as_str().ok_or_else(|| {
                anyhow::anyhow!("Missing authorization_code in device auth response")
            })?.to_string();
            let code_verifier = json["code_verifier"].as_str().ok_or_else(|| {
                anyhow::anyhow!("Missing code_verifier in device auth response")
            })?.to_string();
            return Ok((auth_code, code_verifier));
        }

        if resp.status() == 403 || resp.status() == 404 {
            // Still waiting for user
            continue;
        }

        // Check error code
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if body.contains("deviceauth_authorization_pending") {
            continue;
        }
        if body.contains("slow_down") {
            slow_count += 1;
            interval_ms = (interval_ms as f64 * 1.5) as u64;
            continue;
        }

        anyhow::bail!("Device auth failed ({}): {}", status, body);
    }

    anyhow::bail!(
        "Device flow timed out{}",
        if slow_count > 0 { " after one or more slow_down responses. This is often caused by clock drift in WSL or VM environments." } else { "" }
    );
}

// ---------------------------------------------------------------------------
// Token operations
// ---------------------------------------------------------------------------

/// Exchange an authorization code for tokens.
async fn exchange_code(
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> anyhow::Result<CodexCredential> {
    let client = reqwest::Client::new();
    let resp = client
        .post(TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=authorization_code&client_id={}&code={}&code_verifier={}&redirect_uri={}",
            CLIENT_ID, code, verifier, redirect_uri
        ))
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Token exchange failed ({}): {}", status, body);
    }

    let json: serde_json::Value = resp.json().await?;
    let access = json["access_token"].as_str().ok_or_else(|| anyhow::anyhow!("Missing access_token"))?;
    let refresh = json["refresh_token"].as_str().ok_or_else(|| anyhow::anyhow!("Missing refresh_token"))?;
    let expires_in = json["expires_in"].as_i64().ok_or_else(|| anyhow::anyhow!("Missing expires_in"))?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let account_id = extract_account_id(access)?;

    Ok(CodexCredential {
        access: access.to_string(),
        refresh: refresh.to_string(),
        expires_at: now + expires_in * 1000,
        account_id,
    })
}

/// Refresh an access token using the refresh token.
pub async fn refresh_token(refresh: &str) -> anyhow::Result<CodexCredential> {
    let client = reqwest::Client::new();
    let resp = client
        .post(TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=refresh_token&refresh_token={}&client_id={}",
            refresh, CLIENT_ID
        ))
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Token refresh failed ({}): {}", status, body);
    }

    let json: serde_json::Value = resp.json().await?;
    let access = json["access_token"].as_str().ok_or_else(|| anyhow::anyhow!("Missing access_token"))?;
    let refresh = json["refresh_token"].as_str().ok_or_else(|| anyhow::anyhow!("Missing refresh_token"))?;
    let expires_in = json["expires_in"].as_i64().ok_or_else(|| anyhow::anyhow!("Missing expires_in"))?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let account_id = extract_account_id(access)?;

    Ok(CodexCredential {
        access: access.to_string(),
        refresh: refresh.to_string(),
        expires_at: now + expires_in * 1000,
        account_id,
    })
}

/// Extract the OpenAI account ID from a JWT access token.
fn extract_account_id(token: &str) -> anyhow::Result<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        anyhow::bail!("Invalid JWT token format");
    }
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let decoded = URL_SAFE_NO_PAD.decode(parts[1])
        .map_err(|_| anyhow::anyhow!("Invalid JWT payload encoding"))?;
    let payload: serde_json::Value = serde_json::from_slice(&decoded)?;
    let account_id = payload["https://api.openai.com/auth"]["chatgpt_account_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing chatgpt_account_id in JWT"))?;
    Ok(account_id.to_string())
}

// ---------------------------------------------------------------------------
// Device code login
// ---------------------------------------------------------------------------

/// Perform the full device code OAuth login flow.
pub async fn device_code_login(
    signal: Option<tokio::sync::watch::Receiver<bool>>,
) -> anyhow::Result<CodexCredential> {
    let client = reqwest::Client::new();

    // Start device auth
    let resp = client
        .post(DEVICE_USER_CODE_URL)
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({ "client_id": CLIENT_ID }))
        .send()
        .await?;

    if !resp.status().is_success() {
        if resp.status() == 404 {
            anyhow::bail!("OpenAI Codex device code login is not enabled for this server. Use browser login or verify the server URL.");
        }
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Device code request failed ({}): {}", status, body);
    }

    let json: serde_json::Value = resp.json().await?;
    let device_auth_id = json["device_auth_id"].as_str().ok_or_else(|| {
        anyhow::anyhow!("Missing device_auth_id")
    })?.to_string();
    let user_code = json["user_code"].as_str().ok_or_else(|| {
        anyhow::anyhow!("Missing user_code")
    })?.to_string();
    let interval = json["interval"].as_u64().unwrap_or(DEFAULT_POLL_INTERVAL_SECS);

    println!(
        "\nOpenAI Codex Device Login\n\
        ─────────────────────────\n\
        Visit:  https://auth.openai.com/codex/device\n\
        Code:   {}\n\
        \n\
        Waiting for authorization... (Ctrl+C to cancel)\n",
        user_code
    );

    let (auth_code, code_verifier) = poll_device_auth(&device_auth_id, &user_code, interval, signal).await?;
    let cred = exchange_code(&auth_code, &code_verifier,
        "https://auth.openai.com/deviceauth/callback").await?;
    cred.save()?;
    println!("✓ Authorization complete. Credentials saved.\n");
    Ok(cred)
}

// ---------------------------------------------------------------------------
// Browser login (local HTTP server)
// ---------------------------------------------------------------------------

/// Perform the browser-based OAuth login flow (starts local HTTP server on port 1455).
pub async fn browser_login(
    signal: Option<tokio::sync::watch::Receiver<bool>>,
) -> anyhow::Result<CodexCredential> {
    let (verifier, challenge) = generate_pkce();
    let state = generate_state();

    let authorize_url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope=openid+profile+email+offline_access&\
         code_challenge={}&code_challenge_method=S256&state={}&\
         id_token_add_organizations=true&codex_cli_simplified_flow=true&originator=pi",
        "https://auth.openai.com/oauth/authorize",
        CLIENT_ID,
        "http://localhost:1455/auth/callback",
        challenge,
        state,
    );

    println!(
        "\nOpenAI Codex Browser Login\n\
        ──────────────────────────\n\
        Opening: {}\n\
        \n\
        A browser window should open. Complete login to finish.\n\
        (Ctrl+C to cancel)\n",
        authorize_url
    );

    // Try to open browser
    open_browser(&authorize_url);

    // Start local server and wait for callback
    let listener = tokio::net::TcpListener::bind("127.0.0.1:1455").await
        .map_err(|e| anyhow::anyhow!("Failed to start OAuth callback server on port 1455: {}. \
            Check if another process is using the port.", e))?;

    let code = wait_for_callback(&listener, &state, signal).await?;

    let cred = exchange_code(&code, &verifier, "http://localhost:1455/auth/callback").await?;
    cred.save()?;
    println!("✓ Authorization complete. Credentials saved.\n");
    Ok(cred)
}

/// Generate a random state string for CSRF protection.
fn generate_state() -> String {
    let bytes: Vec<u8> = (0..16).map(|_| rand::random::<u8>()).collect();
    hex::encode(&bytes)
}

/// Attempt to open a URL in the default browser. Errors are silently ignored.
#[cfg(target_os = "linux")]
fn open_browser(url: &str) {
    let _ = std::process::Command::new("xdg-open")
        .arg(url).spawn();
}

#[cfg(target_os = "macos")]
fn open_browser(url: &str) {
    let _ = std::process::Command::new("open")
        .arg(url).spawn();
}

#[cfg(target_os = "windows")]
fn open_browser(url: &str) {
    let _ = std::process::Command::new("cmd")
        .args(["/c", "start", url]).spawn();
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn open_browser(_url: &str) {
}

/// Wait for the OAuth callback on the local HTTP server.
async fn wait_for_callback(
    listener: &tokio::net::TcpListener,
    expected_state: &str,
    signal: Option<tokio::sync::watch::Receiver<bool>>,
) -> anyhow::Result<String> {
    let timeout = std::time::Duration::from_secs(300); // 5 min timeout

    tokio::select! {
        result = async {
            loop {
                let (mut stream, _) = listener.accept().await?;
                use tokio::io::AsyncBufReadExt;
                let mut reader = tokio::io::BufReader::new(&mut stream);
                let mut request_line = String::new();
                reader.read_line(&mut request_line).await?;

                if let Some(path) = request_line.split_whitespace().nth(1)
                    && let Ok(uri) = url::Url::parse(&format!("http://localhost{}", path)) {
                        // Check state
                        let state = uri.query_pairs().find(|(k, _)| k == "state").map(|(_, v)| v.to_string());
                        if let Some(ref s) = state && s != expected_state {
                            respond(&mut stream, 400, "State mismatch").await?;
                            continue;
                        }

                        if let Some(code) = uri.query_pairs().find(|(k, _)| k == "code").map(|(_, v)| v.to_string()) {
                            respond(&mut stream, 200, "✓ Authentication complete. You can close this window.").await?;
                            return anyhow::Result::Ok(code);
                        }
                }
                respond(&mut stream, 400, "Missing authorization code").await?;
                anyhow::bail!("Missing authorization code in callback");
            }
        } => result,
        _ = tokio::time::sleep(timeout) => {
            anyhow::bail!("OAuth callback timed out after 5 minutes");
        }
        _ = wait_for_abort(signal) => {
            anyhow::bail!("Login cancelled");
        }
    }
}

/// Wait for an abort signal (if provided), or block forever.
async fn wait_for_abort(signal: Option<tokio::sync::watch::Receiver<bool>>) {
    if let Some(mut rx) = signal {
        let _ = rx.changed().await;
    } else {
        std::future::pending::<()>().await;
    }
}

/// Send an HTTP response to the TCP stream.
async fn respond(
    stream: &mut tokio::net::TcpStream,
    status: u16,
    body: &str,
) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;
    let status_text = if status == 200 { "OK" } else { "Bad Request" };
    let response = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n<html><body><p>{}</p></body></html>",
        status, status_text, body.len() + 45, body
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Top-level login & credential resolution
// ---------------------------------------------------------------------------

/// Load credentials from the most available source:
/// 1. `OPENAI_CODEX_TOKEN` env var
/// 2. Stored credential file (if not expired)
/// 3. Run OAuth flow
pub async fn resolve_codex_token(
    signal: Option<tokio::sync::watch::Receiver<bool>>,
) -> anyhow::Result<String> {
    // 1. Env var
    if let Ok(token) = std::env::var("OPENAI_CODEX_TOKEN")
        && !token.is_empty() {
        return Ok(token);
    }

    // 2. Stored credentials
    if let Ok(Some(cred)) = CodexCredential::load() {
        if !cred.is_expired() {
            return Ok(cred.access);
        }
        // Try refresh
        if let Ok(new_cred) = refresh_token(&cred.refresh).await {
            let _ = new_cred.save();
            return Ok(new_cred.access);
        }
    }

    // 3. OAuth flow — device code by default
    println!("OpenAI Codex: No token found. Starting device code login...");
    let cred = device_code_login(signal).await?;
    Ok(cred.access)
}
