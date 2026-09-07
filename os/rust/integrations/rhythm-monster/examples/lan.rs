//! Reads credentials from a private file, never CLI arguments or logs.
use rhythm_monster::{
    lan::{LightLanClient, LightTransport},
    LightCredentials, LightProperty,
};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: lan PRIVATE_CREDENTIALS_FILE [power|off|on|static-test]")?;
    let action = args.next().unwrap_or_else(|| "power".into());
    let bytes = zeroize::Zeroizing::new(std::fs::read(path)?);
    let credentials: LightCredentials = serde_json::from_slice(&bytes)?;
    let client = LightLanClient::new(credentials)?;
    match action.as_str() {
        "power" => println!(
            "Verified power: {}",
            client.read(LightProperty::Power).await?
        ),
        "off" | "on" => {
            client
                .write(vec![(
                    LightProperty::Power,
                    serde_json::json!(u8::from(action == "on")),
                )])
                .await?;
            println!("Power write verified");
        }
        "static-test" => {
            use rhythm_core::lighting::LightingCommand;
            use rhythm_monster::controller::static_values;
            let props = [
                LightProperty::Mode,
                LightProperty::Color,
                LightProperty::Saturation,
                LightProperty::Brightness,
                LightProperty::ColorBrightness,
                LightProperty::Power,
            ];
            let mut saved = Vec::new();
            for p in props {
                let v = client.read(p).await?;
                p.validate(&v)?;
                saved.push((p, v));
            }
            let result = client
                .write(static_values(&LightingCommand::new(35, 2700)))
                .await;
            let restore = client.write(saved).await;
            restore?;
            result?;
            println!("Static color and brightness verified; original state restored");
        }
        _ => return Err("unsupported operation".into()),
    }
    Ok(())
}
