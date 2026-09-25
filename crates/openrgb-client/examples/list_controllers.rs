use std::{env, error::Error, time::Duration};

use openrgb_client::{Client, DeviceType};

fn main() -> Result<(), Box<dyn Error>> {
    let host = env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let port = env::args()
        .nth(2)
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(6742);
    let mut client = Client::connect(
        (host.as_str(), port),
        "Kroma OpenRGB Inspector",
        Duration::from_secs(2),
    )?;

    println!("OpenRGB protocol {}", client.protocol_version());
    for controller in client.controllers()? {
        let matrices: Vec<String> = controller
            .zones
            .iter()
            .filter_map(|zone| {
                zone.matrix.as_ref().map(|matrix| {
                    format!(
                        "{}={}x{} ({} mapped LEDs)",
                        zone.name,
                        matrix.width,
                        matrix.height,
                        matrix.leds.iter().flatten().count()
                    )
                })
            })
            .collect();
        println!(
            "{}: {:?} name={:?} serial={:?} matrices=[{}]",
            controller.id,
            controller.device_type,
            controller.name,
            controller.serial,
            matrices.join(", ")
        );
        if controller.device_type == DeviceType::Keyboard && matrices.is_empty() {
            println!("  keyboard has no matrix metadata and cannot be mapped per-key");
        }
    }
    Ok(())
}
