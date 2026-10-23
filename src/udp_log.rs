use std::net::UdpSocket;
use std::sync::RwLock;

use esp_idf_svc::log::{EspIdfLogFilter, EspLogger};

struct UdpTarget {
    socket: UdpSocket,
    target: String,
    hostname: String,
}

static UDP_TARGET: RwLock<Option<UdpTarget>> = RwLock::new(None);

static LOGGER: CompositeLogger = CompositeLogger {
    esp: EspLogger::new(EspIdfLogFilter::new()),
};

struct CompositeLogger {
    esp: EspLogger,
}

impl log::Log for CompositeLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.esp.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        self.esp.log(record);

        if let Ok(guard) = UDP_TARGET.read() {
            if let Some(udp) = guard.as_ref() {
                let pri = syslog_priority(record.level());
                let module = record.module_path().unwrap_or("-");
                // RFC5424: <PRI>VERSION TIMESTAMP HOSTNAME APP-NAME PROCID MSGID SD MSG
                // The MSG carries a leading UTF-8 BOM (U+FEFF -> EF BB BF). Per
                // RFC5424 §6.4 a UTF-8 MSG MUST start with the BOM; without it,
                // strict parsers (e.g. Vector's syslog_loose) reject any frame
                // whose message contains non-ASCII bytes — server-sent text like
                // "78°F" or kana would otherwise fail to parse downstream.
                let msg = format!(
                    "<{}>1 - {} {} - - - \u{feff}{}",
                    pri,
                    udp.hostname,
                    module,
                    record.args(),
                );
                let _ = udp.socket.send_to(msg.as_bytes(), &udp.target);
            }
        }
    }

    fn flush(&self) {
        self.esp.flush();
    }
}

fn syslog_priority(level: log::Level) -> u8 {
    // facility=1 (user-level) * 8 + severity
    let severity = match level {
        log::Level::Error => 3,
        log::Level::Warn => 4,
        log::Level::Info => 6,
        log::Level::Debug => 7,
        log::Level::Trace => 7,
    };
    8 + severity
}

pub fn init() {
    log::set_logger(&LOGGER)
        .map(|()| LOGGER.esp.filter().initialize())
        .expect("logger already set");
}

pub fn enable_udp(host: &str, device_id: &str) {
    let socket = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(e) => {
            log::warn!("UDP log socket bind failed: {}", e);
            return;
        }
    };
    if let Err(e) = socket.set_nonblocking(true) {
        log::warn!("UDP socket set_nonblocking failed: {}", e);
    }

    let new_target = UdpTarget {
        socket,
        target: host.to_string(),
        hostname: device_id.to_string(),
    };
    // Replace any existing target so the syslog destination can be changed at
    // runtime via a remote `config` update. Drop the write guard before
    // logging — `log()` takes a read lock and would otherwise deadlock.
    match UDP_TARGET.write() {
        Ok(mut guard) => *guard = Some(new_target),
        Err(_) => {
            log::warn!("UDP target lock poisoned; syslog not updated");
            return;
        }
    }
    log::info!("UDP syslog enabled → {}", host);
}

/// Disable UDP syslog, dropping the current target. Subsequent logs go to
/// serial only until `enable_udp` is called again.
pub fn disable_udp() {
    if let Ok(mut guard) = UDP_TARGET.write() {
        *guard = None;
    }
    log::info!("UDP syslog disabled");
}
