//! sony-connect-cli: command-line control for Sony headphones (MDR v2).

use std::process::ExitCode;

use sony_core::{AncMode, Device, DeviceState, UltMode};

const USAGE: &str = "\
usage: sony-connect-cli [--device AA:BB:CC:DD:EE:FF] <command>

commands:
  list                                   list paired Sony devices
  status [--json]                        show battery, volume, ANC, ULT, ...
  anc <off|nc|ambient> [--level N] [--voice]
                                         set noise control (ambient level 0-20)
  ult <off|1|2>                          set ULT mode
  volume <0-100>                         set volume (percent)

env: SONY_CONNECT_DEBUG=1 dumps MDR frames; SONY_CONNECT_RFCOMM_CHANNEL=N skips SDP";

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn usage_err(msg: &str) -> anyhow::Error {
    anyhow::anyhow!("{msg}\n\n{USAGE}")
}

async fn open(device: &Option<String>) -> anyhow::Result<Device> {
    Ok(match device {
        Some(a) => Device::connect(a).await?,
        None => Device::connect_first().await?,
    })
}

fn opt<T: std::fmt::Display>(v: &Option<T>) -> String {
    v.as_ref()
        .map(|x| x.to_string())
        .unwrap_or_else(|| "-".into())
}

fn print_status(name: &str, address: &str, s: &DeviceState) {
    println!("device:      {name} ({address})");
    println!("model:       {}", opt(&s.model));
    println!("firmware:    {}", opt(&s.firmware));
    match &s.battery {
        Some(b) => println!(
            "battery:     {}%{}",
            b.level,
            if b.charging { " (charging)" } else { "" }
        ),
        None => println!("battery:     -"),
    }
    println!(
        "volume:      {}",
        s.volume
            .map(|v| format!("{v}%"))
            .unwrap_or_else(|| "-".into())
    );
    let anc = match s.anc {
        Some(AncMode::Off) => "off",
        Some(AncMode::NoiseCancelling) => "noise cancelling",
        Some(AncMode::Ambient) => "ambient",
        None => "-",
    };
    println!("anc:         {anc}");
    println!(
        "ambient:     level {} / 20, focus on voice {}",
        opt(&s.ambient_level),
        opt(&s.focus_on_voice)
    );
    let ult = match s.ult {
        Some(UltMode::Off) => "off",
        Some(UltMode::Ult1) => "ULT 1",
        Some(UltMode::Ult2) => "ULT 2",
        None => "-",
    };
    println!("ult:         {ult}");
}

async fn run() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut device = None;
    if let Some(i) = args.iter().position(|a| a == "--device" || a == "-d") {
        args.remove(i);
        if i >= args.len() {
            return Err(usage_err("--device needs an address"));
        }
        device = Some(args.remove(i));
    }
    if args.iter().any(|a| a == "-h" || a == "--help") || args.is_empty() {
        println!("{USAGE}");
        return Ok(());
    }
    let cmd = args.remove(0);
    match cmd.as_str() {
        "list" => {
            let devs = sony_core::discover().await?;
            if devs.is_empty() {
                println!("no paired Sony devices found");
            }
            for d in devs {
                println!(
                    "{}  {}  {}",
                    d.address,
                    if d.connected {
                        "connected"
                    } else {
                        "paired   "
                    },
                    d.name
                );
            }
        }
        "status" => {
            let json = args.iter().any(|a| a == "--json");
            let mut dev = open(&device).await?;
            let st = dev.refresh().await?;
            if json {
                let out = serde_json::json!({ "device": dev.info(), "state": st });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                let info = dev.info().clone();
                print_status(&info.name, &info.address, &st);
            }
            dev.disconnect().await?;
        }
        "anc" => {
            let mode = match args.first().map(String::as_str) {
                Some("off") => AncMode::Off,
                Some("nc") => AncMode::NoiseCancelling,
                Some("ambient") => AncMode::Ambient,
                _ => return Err(usage_err("anc needs off|nc|ambient")),
            };
            let voice = args.iter().any(|a| a == "--voice");
            let level_arg = match args.iter().position(|a| a == "--level") {
                Some(i) => Some(
                    args.get(i + 1)
                        .and_then(|v| v.parse::<u8>().ok())
                        .filter(|v| *v <= 20)
                        .ok_or_else(|| usage_err("--level needs a number 0-20"))?,
                ),
                None => None,
            };
            let mut dev = open(&device).await?;
            // Keep the current ambient level unless one was given.
            let cur = dev.refresh().await?;
            let level = level_arg.or(cur.ambient_level).unwrap_or(20);
            dev.set_anc(mode, level, voice).await?;
            let st = dev.refresh().await?;
            println!(
                "anc: {:?}, ambient level {}, focus on voice {}",
                st.anc.unwrap_or(mode),
                opt(&st.ambient_level),
                opt(&st.focus_on_voice)
            );
            dev.disconnect().await?;
        }
        "ult" => {
            let mode = match args.first().map(String::as_str) {
                Some("off") | Some("0") => UltMode::Off,
                Some("1") => UltMode::Ult1,
                Some("2") => UltMode::Ult2,
                _ => return Err(usage_err("ult needs off|1|2")),
            };
            let mut dev = open(&device).await?;
            dev.set_ult(mode).await?;
            println!("ult: {:?}", mode);
            dev.disconnect().await?;
        }
        "volume" => {
            let v: u8 = args
                .first()
                .and_then(|v| v.parse().ok())
                .filter(|v| *v <= 100)
                .ok_or_else(|| usage_err("volume needs a number 0-100"))?;
            let mut dev = open(&device).await?;
            dev.set_volume(v).await?;
            let st = dev.refresh().await?;
            println!(
                "volume: {}",
                st.volume
                    .map(|v| format!("{v}%"))
                    .unwrap_or_else(|| "-".into())
            );
            dev.disconnect().await?;
        }
        other => return Err(usage_err(&format!("unknown command {other:?}"))),
    }
    Ok(())
}
