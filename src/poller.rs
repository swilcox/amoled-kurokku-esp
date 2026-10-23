//! HTTP polling of kurokku-esp-server (`GET /api/v1/devices/{id}/instruction`),
//! and device events (`POST /api/v1/devices/{id}/events`).

use std::time::Duration;

use anyhow::Result;
use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};

use crate::protocol::{ServerResponse, DISPLAY_TYPE};

/// Firmware version reported to the server on each poll: Cargo version plus
/// the git short hash from build.rs (suffixed `-dirty` for uncommitted
/// builds). The `+` is percent-encoded so the query string survives
/// form-decoding; the server sees e.g. `0.2.0+abc1234`.
pub const FIRMWARE_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "%2B", env!("KUROKKU_GIT_HASH"));

pub struct HttpPoller {
    url: String,
    events_url: String,
    poll_interval: Duration,
}

fn connect() -> Result<EspHttpConnection> {
    // The TLS cert bundle makes https:// work; harmless for http://.
    EspHttpConnection::new(&HttpConfig {
        timeout: Some(Duration::from_secs(10)),
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        ..Default::default()
    })
    .map_err(|e| anyhow::anyhow!("HTTP connection failed: {e}"))
}

impl HttpPoller {
    pub fn new(base_url: &str, device_id: &str, poll_interval: Duration) -> Self {
        let base = format!("{}/api/v1/devices/{}", base_url.trim_end_matches('/'), device_id);
        let url = format!("{base}/instruction?display_type={DISPLAY_TYPE}&firmware_version={FIRMWARE_VERSION}");
        Self { url, events_url: format!("{base}/events"), poll_interval }
    }

    /// Report an input event (e.g. "next"); the effect arrives on the next
    /// poll.
    pub fn send_event(&self, event: &str) -> Result<()> {
        let body = serde_json::to_vec(&serde_json::json!({ "event": event }))?;
        let len = body.len().to_string();
        let mut conn = connect()?;
        let headers = [("Content-Type", "application/json"), ("Content-Length", len.as_str())];
        conn.initiate_request(esp_idf_svc::http::Method::Post, &self.events_url, &headers)
            .map_err(|e| anyhow::anyhow!("HTTP request failed: {e}"))?;
        conn.write_all(&body).map_err(|e| anyhow::anyhow!("HTTP write failed: {e}"))?;
        conn.initiate_response()
            .map_err(|e| anyhow::anyhow!("HTTP response failed: {e}"))?;
        match conn.status() {
            200..=299 => Ok(()),
            s => anyhow::bail!("server returned status {s}"),
        }
    }

    pub fn poll(&mut self) -> Result<ServerResponse> {
        let mut conn = connect()?;

        conn.initiate_request(esp_idf_svc::http::Method::Get, &self.url, &[])
            .map_err(|e| anyhow::anyhow!("HTTP request failed: {e}"))?;
        conn.initiate_response()
            .map_err(|e| anyhow::anyhow!("HTTP response failed: {e}"))?;
        let status = conn.status();
        if status != 200 {
            anyhow::bail!("server returned status {status}");
        }

        let mut buf = [0u8; 1024];
        let mut body = Vec::new();
        loop {
            let n = conn.read(&mut buf).map_err(|e| anyhow::anyhow!("HTTP read failed: {e}"))?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&buf[..n]);
        }

        let resp: ServerResponse = serde_json::from_slice(&body)?;
        if let Some(ms) = resp.poll_interval_ms {
            self.poll_interval = Duration::from_millis(ms.max(500));
        }
        Ok(resp)
    }

    pub fn interval(&self) -> Duration {
        self.poll_interval
    }
}
