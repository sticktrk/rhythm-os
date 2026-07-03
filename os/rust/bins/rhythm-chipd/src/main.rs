mod backend;
mod service;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use anyhow::{Context, Result};

use rhythm_matter::chip_rpc::{ChipRpcRequestEnvelope, ChipRpcResponseEnvelope};

use crate::backend::build_backend_from_env;
use crate::service::ChipControllerService;

const BUILD_VERSION: &str = match option_env!("RHYTHM_BUILD_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

fn main() -> Result<()> {
    let socket_path = match parse_args()? {
        ParsedArgs::Version => {
            println!("rhythm-chipd {}", BUILD_VERSION);
            return Ok(());
        }
        ParsedArgs::Serve { socket_path } => socket_path,
    };
    serve(socket_path)
}

enum ParsedArgs {
    Version,
    Serve { socket_path: PathBuf },
}

fn parse_args() -> Result<ParsedArgs> {
    parse_args_from(std::env::args().skip(1))
}

fn parse_args_from<I>(args: I) -> Result<ParsedArgs>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--version" || arg == "-V" {
            return Ok(ParsedArgs::Version);
        }
        if arg == "--socket" {
            let value = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("Missing value for --socket"))?;
            return Ok(ParsedArgs::Serve {
                socket_path: PathBuf::from(value),
            });
        }
    }
    anyhow::bail!("Usage: rhythm-chipd --socket /path/to/socket")
}

fn serve(socket_path: PathBuf) -> Result<()> {
    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    if socket_path.exists() {
        let _ = fs::remove_file(&socket_path);
    }

    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("binding {}", socket_path.display()))?;
    let mut service = ChipControllerService::new(build_backend_from_env());

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                // The accept loop is serial: without socket timeouts, one
                // client that connects and never sends a newline (or never
                // drains our response) parks read_line/write forever and
                // wedges ALL Matter light control until an external restart.
                // Clients send their request immediately after connecting,
                // so 30s is generous.
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(30)));
                let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(30)));
                if let Err(error) = handle_stream(&mut service, &mut stream) {
                    eprintln!("rhythm-chipd request error: {error:#}");
                }
            }
            Err(error) => {
                return Err(anyhow::anyhow!(error))
                    .with_context(|| format!("accepting {}", socket_path.display()));
            }
        }
    }

    Ok(())
}

fn handle_stream(service: &mut ChipControllerService, stream: &mut UnixStream) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone().context("cloning client stream")?);
    let mut line = String::new();
    let bytes = reader
        .read_line(&mut line)
        .context("reading chipd request line")?;
    if bytes == 0 {
        return Ok(());
    }

    let envelope: ChipRpcRequestEnvelope =
        serde_json::from_str(line.trim_end()).context("decoding chipd request")?;
    let response = match service.handle(envelope.request) {
        Ok(result) => ChipRpcResponseEnvelope::ok(envelope.id, result),
        Err(error) => ChipRpcResponseEnvelope::error(envelope.id, format!("{error:#}")),
    };

    serde_json::to_writer(&mut *stream, &response).context("encoding chipd response")?;
    stream
        .write_all(b"\n")
        .context("writing chipd response newline")?;
    stream.flush().context("flushing chipd response")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    use rhythm_matter::chip_rpc::{
        ChipInitControllerRequest, ChipRpcListDevicesResponse, ChipRpcRequest, ChipRpcResponse,
    };

    use crate::backend::FakeChipBackend;

    fn unique_test_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rhythm-chipd-main-{}-{}", name, nanos));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn string_error<T>(result: Result<T>) -> String {
        match result {
            Ok(_) => panic!("expected error"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn parse_args_from_accepts_version_and_socket_forms() {
        match parse_args_from(vec!["--version".to_string()]).unwrap() {
            ParsedArgs::Version => {}
            ParsedArgs::Serve { .. } => panic!("expected version"),
        }
        match parse_args_from(vec!["-V".to_string()]).unwrap() {
            ParsedArgs::Version => {}
            ParsedArgs::Serve { .. } => panic!("expected version"),
        }
        match parse_args_from(vec![
            "--socket".to_string(),
            "/tmp/rhythm-chipd.sock".to_string(),
        ])
        .unwrap()
        {
            ParsedArgs::Serve { socket_path } => {
                assert_eq!(socket_path, PathBuf::from("/tmp/rhythm-chipd.sock"));
            }
            ParsedArgs::Version => panic!("expected serve"),
        }
    }

    #[test]
    fn parse_args_from_reports_missing_socket_or_usage() {
        assert_eq!(
            string_error(parse_args_from(vec!["--socket".to_string()])),
            "Missing value for --socket"
        );
        assert_eq!(
            string_error(parse_args_from(Vec::<String>::new())),
            "Usage: rhythm-chipd --socket /path/to/socket"
        );
    }

    #[test]
    fn handle_stream_returns_ok_for_empty_client() {
        let (mut server, client) = UnixStream::pair().unwrap();
        drop(client);
        let mut service = ChipControllerService::new(Box::new(FakeChipBackend::default()));

        handle_stream(&mut service, &mut server).unwrap();
    }

    #[test]
    fn handle_stream_writes_success_and_error_responses() {
        let mut service = ChipControllerService::new(Box::new(FakeChipBackend::default()));

        let (mut server, mut client) = UnixStream::pair().unwrap();
        let request = ChipRpcRequestEnvelope {
            id: 7,
            request: ChipRpcRequest::ListDevices,
        };
        serde_json::to_writer(&mut client, &request).unwrap();
        client.write_all(b"\n").unwrap();

        handle_stream(&mut service, &mut server).unwrap();

        let mut response_line = String::new();
        BufReader::new(client)
            .read_line(&mut response_line)
            .unwrap();
        let response: ChipRpcResponseEnvelope = serde_json::from_str(&response_line).unwrap();
        assert_eq!(response.id, 7);
        let list: ChipRpcListDevicesResponse = response.into_result().unwrap();
        assert!(list.devices.is_empty());

        let dir = unique_test_dir("stream-error");
        let (mut server, mut client) = UnixStream::pair().unwrap();
        let request = ChipRpcRequestEnvelope {
            id: 8,
            request: ChipRpcRequest::InitController(ChipInitControllerRequest {
                fabric_id: "fabric-test".to_string(),
                operational_fabric_id: 0,
                ipk_hex: "00112233445566778899aabbccddeeff".to_string(),
                storage_path: dir.join("chip.json").display().to_string(),
                ble_controller: None,
            }),
        };
        serde_json::to_writer(&mut client, &request).unwrap();
        client.write_all(b"\n").unwrap();

        handle_stream(&mut service, &mut server).unwrap();

        let mut response_line = String::new();
        BufReader::new(client)
            .read_line(&mut response_line)
            .unwrap();
        let response: ChipRpcResponseEnvelope = serde_json::from_str(&response_line).unwrap();
        assert_eq!(response.id, 8);
        match response.response {
            ChipRpcResponse::Error { error } => {
                assert_eq!(
                    error.message,
                    "Fake CHIP operational fabric id must be non-zero"
                );
            }
            ChipRpcResponse::Ok { .. } => panic!("expected error response"),
        }

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn handle_stream_surfaces_invalid_json_request() {
        let (mut server, mut client) = UnixStream::pair().unwrap();
        client.write_all(b"{ not valid json }\n").unwrap();
        let mut service = ChipControllerService::new(Box::new(FakeChipBackend::default()));

        let error = handle_stream(&mut service, &mut server).unwrap_err();

        assert!(format!("{error:#}").contains("decoding chipd request"));
    }
}
