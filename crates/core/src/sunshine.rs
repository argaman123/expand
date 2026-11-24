//! The stream host on the Mac: Sunshine, run hidden with a private config so it never
//! touches the user's own Sunshine settings.
//!
//! Checked on the Mac on 2026-11-11 with the Homebrew beta build:
//! - `output_name` takes the numeric CoreGraphics display ID.
//! - `sunshine <conf> --creds <user> <pass>` sets the web credentials for that config.
//! - `POST /api/pin` wants the token from `GET /api/csrf-token` in an `X-CSRF-Token` header, and the
//!   `pairing_id` of the waiting request, which `GET /api/pin` lists.
//! - Started from a process without Screen Recording permission, Sunshine still serves its
//!   web UI but cannot capture, so readiness is judged from its log, not from its ports.

use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use tokio::process::{Child, Command};

const STREAM_PORT: u16 = 47989;
const API_BASE: &str = "https://127.0.0.1:47990";

#[derive(Debug)]
pub enum EngineError {
    NotInstalled(&'static str),
    AlreadyRunning(&'static str),
    StartTimeout(&'static str),
    StartFailed(&'static str, String),
    Api(String),
    Io(std::io::Error),
}

impl EngineError {
    /// The part of the system that failed, for showing to the user.
    pub fn part(&self) -> &str {
        match self {
            EngineError::NotInstalled(p)
            | EngineError::AlreadyRunning(p)
            | EngineError::StartTimeout(p)
            | EngineError::StartFailed(p, _) => p,
            EngineError::Api(_) => "Sunshine",
            EngineError::Io(_) => "Mac app",
        }
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::NotInstalled(_) => write!(f, "not found on this Mac"),
            EngineError::AlreadyRunning(_) => write!(f, "already running; quit it and try again"),
            EngineError::StartTimeout(_) => write!(f, "did not start in time"),
            EngineError::StartFailed(_, reason) => write!(f, "{reason}"),
            EngineError::Api(reason) => write!(f, "{reason}"),
            EngineError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for EngineError {}

pub fn conf_path(data_dir: &Path) -> PathBuf {
    data_dir.join("sunshine.conf")
}

pub fn log_path(data_dir: &Path) -> PathBuf {
    data_dir.join("sunshine.log")
}

/// The private config. Paths use `/` because this only ever runs on the Mac.
pub fn conf_text(output_name: &str, data_dir: &Path) -> String {
    let dir = data_dir.display();
    format!(
        "output_name = {output_name}\n\
         file_state = {dir}/sunshine_state.json\n\
         credentials_file = {dir}/sunshine_state.json\n\
         log_path = {dir}/sunshine.log\n\
         origin_web_ui_allowed = pc\n"
    )
}

/// Reads Sunshine's startup log. `None` while it is still starting.
pub fn log_verdict(log: &str) -> Option<Result<(), String>> {
    if log.contains("No screen capture permission") {
        return Some(Err(
            "Screen Recording permission is missing (System Settings → Privacy & Security)".to_string()
        ));
    }
    if log.contains("Unable to find display or encoder") {
        return Some(Err("could not capture the virtual display".to_string()));
    }
    if log.contains("Configuration UI available") {
        return Some(Ok(()));
    }
    None
}

/// The ID of the pairing request Sunshine is holding, from `GET /api/pin`.
pub fn pending_pairing_id(reply: &serde_json::Value) -> Option<String> {
    reply.get("pairings")?.as_array()?.first()?.get("id")?.as_str().map(str::to_string)
}

/// Whether any viewer is already paired, from `GET /api/clients/list`. This private Sunshine
/// only ever pairs with our own PC, so any client is that PC.
pub fn has_paired_client(reply: &serde_json::Value) -> bool {
    reply.get("named_certs").and_then(|c| c.as_array()).is_some_and(|c| !c.is_empty())
}

/// Attempts (half a second apart) during which an existing pairing is not yet believed, so a
/// viewer that lost its own pairing has time to ask for a new one.
pub const PAIRED_GRACE_ATTEMPTS: u32 = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinDecision {
    /// A pairing request is waiting: give it the PIN.
    Submit(String),
    /// Nothing is waiting and a viewer is already paired.
    AlreadyPaired,
    /// Nothing to do yet.
    Wait,
}

pub fn pin_decision(pending: Option<String>, has_paired: bool, attempt: u32) -> PinDecision {
    match pending {
        Some(id) => PinDecision::Submit(id),
        None if has_paired && attempt >= PAIRED_GRACE_ATTEMPTS => PinDecision::AlreadyPaired,
        None => PinDecision::Wait,
    }
}

pub fn find_exe() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.push(PathBuf::from("/opt/homebrew/bin"));
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs.iter()
        .flat_map(|d| [d.join("sunshine"), d.join("sunshine-beta")])
        .find(|p| p.is_file())
}

pub async fn is_port_in_use() -> bool {
    tokio::net::TcpStream::connect(("127.0.0.1", STREAM_PORT)).await.is_ok()
}

fn quiet(exe: &Path) -> Command {
    let mut cmd = Command::new(exe);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
    cmd
}

/// Sets the web credentials stored beside `conf`. Must run before [`Host::spawn`].
pub async fn set_credentials(exe: &Path, conf: &Path, user: &str, pass: &str) -> Result<(), EngineError> {
    let status = quiet(exe).arg(conf).args(["--creds", user, pass]).status().await.map_err(EngineError::Io)?;
    if status.success() {
        Ok(())
    } else {
        Err(EngineError::StartFailed("Sunshine", "could not set its credentials".to_string()))
    }
}

/// A running Sunshine.
pub struct Host {
    child: Child,
    log: PathBuf,
}

impl Host {
    /// `log` is the `log_path` named in `conf`; it is cleared first so old runs are not misread.
    pub async fn spawn(exe: &Path, conf: &Path, log: &Path) -> Result<Host, EngineError> {
        if is_port_in_use().await {
            return Err(EngineError::AlreadyRunning("Sunshine"));
        }
        let _ = tokio::fs::remove_file(log).await;
        let child = quiet(exe).arg(conf).spawn().map_err(EngineError::Io)?;
        Ok(Host { child, log: log.to_path_buf() })
    }

    pub async fn wait_ready(&mut self, timeout: Duration) -> Result<(), EngineError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let log = tokio::fs::read_to_string(&self.log).await.unwrap_or_default();
            match log_verdict(&log) {
                Some(Ok(())) => return Ok(()),
                Some(Err(reason)) => return Err(EngineError::StartFailed("Sunshine", reason)),
                None => {}
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                return Err(EngineError::StartFailed("Sunshine", format!("exited during startup ({status})")));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(EngineError::StartTimeout("Sunshine"));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Hands Sunshine the PIN the viewer is pairing with. `Ok(true)` means the viewer is paired
    /// (now, or already). `Ok(false)` means nothing to do yet: try again shortly, with the next
    /// `attempt` number.
    pub async fn submit_pin(
        &self,
        user: &str,
        pass: &str,
        pin: &str,
        client_name: &str,
        attempt: u32,
    ) -> Result<bool, EngineError> {
        let api = |e: reqwest::Error| EngineError::Api(e.to_string());
        // Sunshine's local API uses a self-signed certificate. This client only ever talks to
        // 127.0.0.1, and must never be redirected or proxied anywhere else.
        let http = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(api)?;
        let get = |path: &str| http.get(format!("{API_BASE}{path}")).basic_auth(user, Some(pass)).send();

        let pending: serde_json::Value = get("/api/pin").await.map_err(api)?.json().await.map_err(api)?;
        let clients: serde_json::Value = get("/api/clients/list").await.map_err(api)?.json().await.map_err(api)?;
        let pairing_id = match pin_decision(pending_pairing_id(&pending), has_paired_client(&clients), attempt) {
            PinDecision::Submit(id) => id,
            PinDecision::AlreadyPaired => return Ok(true),
            PinDecision::Wait => return Ok(false),
        };
        let token: serde_json::Value = get("/api/csrf-token").await.map_err(api)?.json().await.map_err(api)?;
        let csrf = token.get("csrf_token").and_then(|t| t.as_str()).unwrap_or_default();

        let reply = http
            .post(format!("{API_BASE}/api/pin"))
            .basic_auth(user, Some(pass))
            .header("X-CSRF-Token", csrf)
            .json(&serde_json::json!({ "pin": pin, "name": client_name, "pairing_id": pairing_id }))
            .send()
            .await
            .map_err(api)?;
        if !reply.status().is_success() {
            let detail = reply.text().await.unwrap_or_default();
            return Err(EngineError::Api(format!("Sunshine refused the PIN: {detail}")));
        }
        let body: serde_json::Value = reply.json().await.map_err(api)?;
        Ok(body.get("status").and_then(|s| s.as_bool()).unwrap_or(false))
    }

    /// Sunshine's process id while it runs: the events it posts carry it (`keys_stream`).
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    pub async fn wait(&mut self) -> std::io::Result<ExitStatus> {
        self.child.wait().await
    }

    pub async fn kill(&mut self) {
        let _ = self.child.kill().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn conf_text_has_isolated_paths() {
        let t = conf_text("6", Path::new("/d"));
        assert!(t.contains("output_name = 6\n"));
        assert!(t.contains("file_state = /d/sunshine_state.json\n"));
        assert!(t.contains("credentials_file = /d/sunshine_state.json\n"));
        assert!(t.contains("log_path = /d/sunshine.log\n"));
        assert!(t.contains("origin_web_ui_allowed = pc\n"));
    }

    // Lines below are from real Sunshine logs captured on 2026-11-11.
    #[test]
    fn log_verdict_ready() {
        let log = "Info: Found H.264 encoder: h264_videotoolbox [videotoolbox]\nInfo: Configuration UI available at [https://localhost:47990]\n";
        assert_eq!(log_verdict(log), Some(Ok(())));
    }

    #[test]
    fn log_verdict_missing_permission() {
        let log = "Error: No screen capture permission!\nInfo: Configuration UI available at [https://localhost:47990]\n";
        let reason = log_verdict(log).unwrap().unwrap_err();
        assert!(reason.contains("Screen Recording"), "{reason}");
    }

    #[test]
    fn log_verdict_no_encoder() {
        let log = "Fatal: Unable to find display or encoder during startup.\nInfo: Configuration UI available at [https://localhost:47990]\n";
        assert!(log_verdict(log).unwrap().is_err());
    }

    #[test]
    fn log_verdict_still_starting() {
        assert_eq!(log_verdict("Info: Creating encoder [h264_videotoolbox]\n"), None);
    }

    // Response captured from the Homebrew beta on 2026-11-11.
    #[test]
    fn pending_pairing_id_is_read_from_the_api_reply() {
        let reply: serde_json::Value = serde_json::from_str(
            r#"{"pairings":[{"address":"192.168.1.10","id":"0f1e2d3c4b5a69788796a5b4c3d2e1f0","name":"Moonlight"}]}"#,
        )
        .unwrap();
        assert_eq!(pending_pairing_id(&reply).as_deref(), Some("0f1e2d3c4b5a69788796a5b4c3d2e1f0"));
    }

    #[test]
    fn no_pending_pairing_gives_none() {
        let reply: serde_json::Value = serde_json::from_str(r#"{"pairings":[]}"#).unwrap();
        assert_eq!(pending_pairing_id(&reply), None);
        assert_eq!(pending_pairing_id(&serde_json::json!({})), None);
    }

    #[test]
    fn paired_clients_are_detected() {
        let some = serde_json::json!({"named_certs": [{"name": "Sidescreen PC", "uuid": "x"}], "status": true});
        let none = serde_json::json!({"named_certs": [], "status": true});
        assert!(has_paired_client(&some));
        assert!(!has_paired_client(&none));
        assert!(!has_paired_client(&serde_json::json!({})));
    }

    // Review finding: a waiting request always wins; "already paired" is only believed after
    // the viewer has had time to ask and did not.
    #[test]
    fn pin_decision_prefers_a_pending_request() {
        assert_eq!(pin_decision(Some("abc".into()), true, 0), PinDecision::Submit("abc".into()));
        assert_eq!(pin_decision(Some("abc".into()), false, 9), PinDecision::Submit("abc".into()));
    }

    #[test]
    fn pin_decision_waits_before_trusting_an_old_pairing() {
        assert_eq!(pin_decision(None, true, 0), PinDecision::Wait);
        assert_eq!(pin_decision(None, true, PAIRED_GRACE_ATTEMPTS - 1), PinDecision::Wait);
        assert_eq!(pin_decision(None, true, PAIRED_GRACE_ATTEMPTS), PinDecision::AlreadyPaired);
        assert_eq!(pin_decision(None, false, 50), PinDecision::Wait);
    }

    #[test]
    fn engine_error_names_its_part() {
        let e = EngineError::AlreadyRunning("Sunshine");
        assert_eq!(e.part(), "Sunshine");
        assert!(e.to_string().contains("already running"));
        assert_eq!(EngineError::NotInstalled("Virtual display").part(), "Virtual display");
        assert!(EngineError::NotInstalled("Virtual display").to_string().contains("not found"));
    }
}
