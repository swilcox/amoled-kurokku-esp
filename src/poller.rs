//! HTTP polling of kurokku-esp-server (`GET /api/v1/devices/{id}/instruction`).

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
    poll_interval: Duration,
}

impl HttpPoller {
    pub fn new(base_url: &str, device_id: &str, poll_interval: Duration) -> Self {
        let url = format!(
            "{}/api/v1/devices/{}/instruction?display_type={}&firmware_version={}",
            base_url.trim_end_matches('/'),
            device_id,
            DISPLAY_TYPE,
            FIRMWARE_VERSION
        );
        Self { url, poll_interval }
    }

    pub fn poll(&mut self) -> Result<ServerResponse> {
        // The TLS cert bundle makes https:// work; harmless for http://.
        let mut conn = EspHttpConnection::new(&HttpConfig {
            timeout: Some(Duration::from_secs(10)),
            crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
            ..Default::default()
        })
        .map_err(|e| anyhow::anyhow!("HTTP connection failed: {e}"))?;

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
