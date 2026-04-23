//! Integration tests for OTA download durability.
//!
//! Spins up a bare `std::net::TcpListener` that serves a fixed payload (or
//! deliberately drops the connection mid-stream) and verifies
//! `rhythm_server::self_update::download_release` behaves correctly:
//!
//! * On success, the destination file exists with complete contents and no
//!   sibling `.tmp` lingers.
//! * On mid-stream drop, no destination file is created and no `.tmp` is
//!   left behind.
//! * After `cleanup_stale_downloads`, stale `.download` and `.download.tmp`
//!   files are removed.
//!
//! These tests avoid introducing a dev-dependency on an HTTP mock crate.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use rhythm_server::self_update::{cleanup_stale_downloads, download_release};

enum Response {
    /// Full happy-path response.
    Full(Vec<u8>),
    /// Send headers advertising `content_length` but only write `actual` bytes
    /// before closing the socket.
    Short {
        content_length: usize,
        actual: Vec<u8>,
    },
    /// Send a non-success status.
    Status(u16),
}

fn spawn_fixture(response: Response) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            // Drain the request so the client doesn't see a RST on close.
            let mut buf = [0u8; 1024];
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
            let _ = stream.read(&mut buf);

            match response {
                Response::Full(body) => {
                    let headers = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(headers.as_bytes());
                    let _ = stream.write_all(&body);
                }
                Response::Short {
                    content_length,
                    actual,
                } => {
                    let headers = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        content_length
                    );
                    let _ = stream.write_all(headers.as_bytes());
                    let _ = stream.write_all(&actual);
                    // Drop stream without sending remaining bytes.
                }
                Response::Status(code) => {
                    let reason = match code {
                        404 => "Not Found",
                        500 => "Internal Server Error",
                        _ => "Error",
                    };
                    let headers = format!(
                        "HTTP/1.1 {} {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        code, reason
                    );
                    let _ = stream.write_all(headers.as_bytes());
                }
            }
        }
    });
    // Small sleep to let the listener enter accept() before the client connects;
    // connect() on loopback is effectively instant so this is just belt-and-braces.
    thread::sleep(Duration::from_millis(10));
    port
}

fn test_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .expect("client")
}

fn unique_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("rhythm-ota-{}-{}", name, nanos));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn tmp_sibling(path: &std::path::Path) -> PathBuf {
    let file_name = path.file_name().unwrap().to_str().unwrap();
    path.parent().unwrap().join(format!("{}.tmp", file_name))
}

#[test]
fn download_release_writes_complete_file_and_cleans_tmp() {
    let payload = b"PAYLOAD-with-some-bytes-to-check-integrity".to_vec();
    let port = spawn_fixture(Response::Full(payload.clone()));

    let dir = unique_dir("download-ok");
    let dest = dir.join("artifact.download");

    download_release(
        &test_client(),
        &format!("http://127.0.0.1:{}/artifact", port),
        &dest,
    )
    .expect("download should succeed");

    assert_eq!(std::fs::read(&dest).unwrap(), payload);
    assert!(!tmp_sibling(&dest).exists(), "no .tmp should linger");
}

#[test]
fn download_release_leaves_no_partial_file_on_mid_stream_drop() {
    let payload = b"PARTIAL".to_vec();
    let port = spawn_fixture(Response::Short {
        content_length: 1024,
        actual: payload,
    });

    let dir = unique_dir("download-dropped");
    let dest = dir.join("artifact.download");

    let result = download_release(
        &test_client(),
        &format!("http://127.0.0.1:{}/artifact", port),
        &dest,
    );
    assert!(
        result.is_err(),
        "mid-stream drop must surface as an error, got {:?}",
        result
    );
    assert!(
        !dest.exists(),
        "destination must not exist after failed download"
    );
    assert!(!tmp_sibling(&dest).exists(), "no .tmp must linger");
}

#[test]
fn download_release_surfaces_non_success_http_status() {
    let port = spawn_fixture(Response::Status(404));

    let dir = unique_dir("download-404");
    let dest = dir.join("artifact.download");

    let result = download_release(
        &test_client(),
        &format!("http://127.0.0.1:{}/artifact", port),
        &dest,
    );
    assert!(result.is_err(), "404 must error");
    assert!(!dest.exists());
    assert!(!tmp_sibling(&dest).exists());
}

#[test]
fn retry_after_failed_download_succeeds_against_healthy_server() {
    // First attempt against a broken server fails; a stale .tmp from that
    // failure must not block a subsequent successful retry.
    let bad_port = spawn_fixture(Response::Short {
        content_length: 1024,
        actual: b"PARTIAL".to_vec(),
    });

    let dir = unique_dir("download-retry");
    let dest = dir.join("artifact.download");

    let _ = download_release(
        &test_client(),
        &format!("http://127.0.0.1:{}/artifact", bad_port),
        &dest,
    );

    let payload = b"COMPLETE PAYLOAD".to_vec();
    let good_port = spawn_fixture(Response::Full(payload.clone()));
    download_release(
        &test_client(),
        &format!("http://127.0.0.1:{}/artifact", good_port),
        &dest,
    )
    .expect("retry should succeed");

    assert_eq!(std::fs::read(&dest).unwrap(), payload);
    assert!(!tmp_sibling(&dest).exists());
}

#[test]
fn cleanup_stale_downloads_clears_leftover_partial_files() {
    let dir = unique_dir("cleanup");
    std::fs::write(dir.join("bundle.tar.gz.download"), b"stale").unwrap();
    std::fs::write(dir.join("bundle.tar.gz.download.tmp"), b"stale-tmp").unwrap();
    std::fs::write(dir.join("rootfs.ext2.gz.download.tmp"), b"stale2").unwrap();
    std::fs::write(dir.join("keep.txt"), b"keep").unwrap();

    cleanup_stale_downloads(&dir);

    assert!(!dir.join("bundle.tar.gz.download").exists());
    assert!(!dir.join("bundle.tar.gz.download.tmp").exists());
    assert!(!dir.join("rootfs.ext2.gz.download.tmp").exists());
    assert!(
        dir.join("keep.txt").exists(),
        "files outside the download-staging naming convention must be preserved"
    );
}

#[test]
fn concurrent_downloads_do_not_clobber_each_other() {
    // Sanity check that two simultaneous downloads to different destinations
    // don't step on each other's .tmp siblings.
    let payload_a = b"AAAAAAAAAAAAAAAAAAAA".to_vec();
    let payload_b = b"BBBBBBBBBBBBBBBBBBBB".to_vec();
    let port_a = spawn_fixture(Response::Full(payload_a.clone()));
    let port_b = spawn_fixture(Response::Full(payload_b.clone()));

    let dir = unique_dir("concurrent");
    let dest_a = Arc::new(dir.join("a.download"));
    let dest_b = Arc::new(dir.join("b.download"));

    let results: Arc<Mutex<Vec<Result<(), String>>>> = Arc::new(Mutex::new(Vec::new()));

    let mut handles = Vec::new();
    for (port, dest) in [(port_a, dest_a.clone()), (port_b, dest_b.clone())] {
        let results = results.clone();
        handles.push(thread::spawn(move || {
            let client = test_client();
            let result = download_release(
                &client,
                &format!("http://127.0.0.1:{}/artifact", port),
                &dest,
            );
            results.lock().unwrap().push(result);
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    assert!(
        results.lock().unwrap().iter().all(|r| r.is_ok()),
        "both downloads must succeed, got {:?}",
        results.lock().unwrap()
    );
    assert_eq!(std::fs::read(&*dest_a).unwrap(), payload_a);
    assert_eq!(std::fs::read(&*dest_b).unwrap(), payload_b);
    assert!(!tmp_sibling(&dest_a).exists());
    assert!(!tmp_sibling(&dest_b).exists());
}
