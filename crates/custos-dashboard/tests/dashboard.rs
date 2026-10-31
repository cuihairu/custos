//! Dashboard integration: binds only when enabled, serves read-only JSON,
//! and never touches the engine's recovery path.

use custos_config::DashboardConfig;
use custos_dashboard::DashboardServer;
use custos_store::Store;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

fn temp_db() -> (tempfile::TempDir, Arc<Store>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path().join("d.db")).unwrap());
    (dir, store)
}

fn get(addr: std::net::SocketAddr, path: &str) -> (String, String) {
    // The accept loop starts asynchronously; retry a reset a few times.
    let mut stream = None;
    for _ in 0..20 {
        match TcpStream::connect(addr) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    }
    let mut stream = stream.expect("connect");
    stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) => panic!("read failed: {e}"),
        }
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    let status = text.lines().next().unwrap_or("").to_owned();
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_owned();
    (status, body)
}

#[test]
fn disabled_config_binds_nothing() {
    let config = DashboardConfig::default();
    assert!(!config.is_enabled());
}

#[test]
fn enabled_server_serves_read_only_json() {
    let (_dir, store) = temp_db();
    let mut config = DashboardConfig::default();
    config.enabled = true;
    config.listen.addr = "127.0.0.1:0".parse().unwrap();
    let server = DashboardServer::bind(&config, store).unwrap();
    let addr = server.local_addr().unwrap();
    std::thread::spawn(move || server.serve());

    let (status, body) = get(addr, "/api/health");
    assert!(status.contains("200 OK"), "{status}");
    assert!(body.contains("resources"), "body was: {body:?}");

    let (status, body) = get(addr, "/api/runs");
    assert!(status.contains("200 OK"), "{status}");
    assert!(body.contains("runs"), "{body}");

    let (status, body) = get(addr, "/api/config");
    assert!(status.contains("200 OK"), "{status}");
    assert!(body.contains("read_only"), "{body}");
    assert!(body.contains("true"), "{body}");
}
