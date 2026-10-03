//! cargo run -p rhythm-os --example ha-migration-preview -- OLD.json HA-catalog.json OUTPUT
use anyhow::{ensure, Context, Result};
use std::io::{Read, Write};
use std::path::Path;

fn read(path: &str, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "Input exceeds migration size limit"
    );
    Ok(bytes)
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(args.len() == 4, "Usage: ha-migration-preview OLD-backup.json HA-canonical-catalog.json NEW-output-directory");
    let result = rhythm_os::ha_migration::preview(
        &read(&args[1], 32 * 1024 * 1024)?,
        &read(&args[2], 8 * 1024 * 1024)?,
    )?;
    let output = Path::new(&args[3]);
    // Exclusive directory creation prevents an interrupted/repeated conversion
    // from replacing a reviewed package. Re-run into a new directory and compare
    // input_sha256; identical inputs produce identical review output.
    std::fs::create_dir(output)
        .context("Use a new output directory; retain previous review and rollback material")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(output, std::fs::Permissions::from_mode(0o700))?;
    }
    for (name, value) in [
        ("review.json", serde_json::to_value(&result)?),
        (
            "profiles.json",
            serde_json::json!({"format":"rhythm-ha-profiles","version":1,"profiles":result.profiles}),
        ),
    ] {
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(output.join(name))?;
        file.write_all(&serde_json::to_vec_pretty(&value)?)?;
        file.sync_all()?;
    }
    std::fs::File::open(output)?.sync_all()?;
    println!("Migration preview saved. No device ownership, credentials or writers changed. Review every mapping before cutover.");
    Ok(())
}
