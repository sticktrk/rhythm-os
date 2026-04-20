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
    let mut args = std::env::args().skip(1);
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
