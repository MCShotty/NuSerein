//! Owner-operated Windows capture isolation; no Discord traffic or audio devices.
use client_core::screen::{RequestSettings, Resolution, SourceId};
use model::voice_settings::{VideoCodec, VideoResolution, VideoSettings};

const USAGE: &str = "Owner-controlled Windows capture probe (never started by Serein or CI).
  windows_capture --list
  windows_capture --owner-controlled --source display:0xHANDLE --stage capture-only|readback|encode
    --resolution source|480|720|1080|1440|2160|4320 --fps 15|30|60
    --warmup 10 --seconds 30 [--backend ffmpeg] [--codec h264|h265|av1]
    [--adapter VENDOR:DEVICE:luid:WINDOWS_LUID] [--revision COMMIT]
Encode requires the explicit codec and exact renderer adapter helper key. All runs disable
system audio and local preview. Capture/readback stages never construct an encoder.
Readback adds the production conversion into a reusable I420 sink. Encode drains into a
local sink without Discord transport; use the normal app for the full-sharing comparison.
Source handles are transient; enumerate immediately before each owner-controlled run.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
	CaptureOnly,
	Readback,
	Encode,
}
struct Arguments {
	owner: bool,
	settings: RequestSettings,
	video: VideoSettings,
	adapter: Option<model::VideoAdapter>,
	stage: Stage,
	warmup: u32,
	seconds: u32,
	revision: Option<String>,
}
enum Command {
	Help,
	List,
	Run(Arguments),
}

fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, &'static str> {
	let args: Vec<_> = args.into_iter().take(33).collect();
	if args.len() > 32 || args.iter().any(|arg| arg.len() > 256 || !arg.is_ascii()) {
		return Err("Too many, oversized or non-ASCII probe arguments");
	}
	if args == ["--help"] || args == ["-h"] || args.is_empty() {
		return Ok(Command::Help);
	}
	if args == ["--list"] {
		return Ok(Command::List);
	}
	let mut owner = false;
	let mut source = None;
	let mut resolution = Resolution::Preset(VideoResolution::P1440);
	let mut fps = 60;
	let mut video = VideoSettings {
		..Default::default()
	};
	let mut codec_set = false;
	let mut adapter = None;
	let mut stage = None;
	let mut warmup = 10;
	let mut seconds = 30;
	let mut revision = None;
	let mut seen = std::collections::HashSet::new();
	let mut args = args.into_iter();
	while let Some(key) = args.next() {
		if !seen.insert(key.clone()) {
			return Err("Duplicate probe option");
		}
		if key == "--owner-controlled" {
			owner = true;
			continue;
		}
		let value = args.next().ok_or("Probe option needs a value")?;
		match key.as_str() {
			"--source" => {
				let (kind, id) = value
					.split_once(':')
					.ok_or("Source must be display:HANDLE or window:HANDLE")?;
				let id = if let Some(hex) = id.strip_prefix("0x") {
					u64::from_str_radix(hex, 16).ok()
				} else {
					id.parse().ok()
				}
				.filter(|id| *id != 0)
				.ok_or("Invalid source handle")?;
				source = Some(match kind {
					"display" => SourceId::Display(id),
					"window" => SourceId::Window(id),
					_ => return Err("Unknown source kind"),
				});
			}
			"--resolution" => {
				resolution = match value.as_str() {
					"source" => Resolution::Source,
					"480" => Resolution::Preset(VideoResolution::P480),
					"720" => Resolution::Preset(VideoResolution::P720),
					"1080" => Resolution::Preset(VideoResolution::P1080),
					"1440" => Resolution::Preset(VideoResolution::P1440),
					"2160" => Resolution::Preset(VideoResolution::P2160),
					"4320" => Resolution::Preset(VideoResolution::P4320),
					_ => return Err("Unsupported resolution preset"),
				}
			}
			"--fps" => {
				fps = value
					.parse()
					.ok()
					.filter(|fps| matches!(fps, 15 | 30 | 60))
					.ok_or("FPS must be 15, 30 or 60")?
			}
			"--stage" => {
				stage = Some(match value.as_str() {
					"capture-only" => Stage::CaptureOnly,
					"readback" => Stage::Readback,
					"encode" => Stage::Encode,
					_ => return Err("Unknown capture stage"),
				})
			}
			"--warmup" => {
				warmup = value
					.parse()
					.ok()
					.filter(|seconds| *seconds <= 60)
					.ok_or("Warmup must be 0..60 seconds")?
			}
			"--seconds" => {
				seconds = value
					.parse()
					.ok()
					.filter(|seconds| (5..=600).contains(seconds))
					.ok_or("Sample must be 5..600 seconds")?
			}
			"--backend" => {
				if value != "ffmpeg" {
					return Err("Backend must be ffmpeg");
				}
			}
			"--codec" => {
				video.codec = match value.as_str() {
					"h264" => VideoCodec::H264,
					"h265" => VideoCodec::H265,
					"av1" => VideoCodec::Av1,
					_ => return Err("Unsupported codec"),
				};
				codec_set = true;
			}
			"--adapter" => {
				adapter = Some(
					model::VideoAdapter::from_helper_key(&value)
						.filter(|adapter| {
							matches!(
								adapter.identity,
								model::VideoAdapterIdentity::WindowsLuid(_)
							)
						})
						.ok_or("Adapter must be an identified Windows helper key")?,
				)
			}
			"--revision" => {
				if !(7..=40).contains(&value.len())
					|| !value.bytes().all(|byte| byte.is_ascii_hexdigit())
				{
					return Err("Revision must be a 7..40 digit commit ID");
				}
				revision = Some(value);
			}
			_ => return Err("Unknown probe option"),
		}
	}
	if !owner {
		return Err("Pass --owner-controlled only for a source you explicitly elect to capture");
	}
	let stage = stage.ok_or("Select an explicit --stage")?;
	if stage == Stage::Encode && (!codec_set || adapter.is_none()) {
		return Err("Encode requires explicit --codec and --adapter matching the app");
	}
	Ok(Command::Run(Arguments {
		owner,
		settings: RequestSettings {
			source: source.ok_or("Select an explicit --source from --list")?,
			resolution,
			fps,
			cursor: true,
			audio: false,
		},
		video,
		adapter,
		stage,
		warmup,
		seconds,
		revision,
	}))
}

fn main() {
	match parse(std::env::args().skip(1)) {
		Ok(Command::Help) => println!("{USAGE}"),
		Ok(command) => {
			if let Err(error) = run(command) {
				eprintln!("Capture probe: {error}");
				std::process::exit(1);
			}
		}
		Err(error) => {
			eprintln!("Capture probe: {error}\n\n{USAGE}");
			std::process::exit(2);
		}
	}
}

#[cfg(target_os = "windows")]
fn run(command: Command) -> Result<(), &'static str> {
	use discord_voice::screen::probe;
	match command {
		Command::Help => Ok(()),
		Command::List => {
			for source in discord_voice::screen::sources()? {
				let id = match source.id {
					SourceId::Display(id) => format!("display:0x{id:x}"),
					SourceId::Window(id) => format!("window:0x{id:x}"),
					_ => continue,
				};
				println!(
					"{}",
					serde_json::json!({"source":id,"name":source.name,"dimensions":source.dimensions})
				);
			}
			Ok(())
		}
		Command::Run(args) => {
			let stage = match args.stage {
				Stage::CaptureOnly => probe::Stage::CaptureOnly,
				Stage::Readback => probe::Stage::Readback,
				Stage::Encode => probe::Stage::Encode,
			};
			probe::run(
				probe::Request {
					owner_controlled: args.owner,
					settings: args.settings,
					video: args.video,
					adapter: args.adapter,
					stage,
					warmup_seconds: args.warmup,
					sample_seconds: args.seconds,
				},
				|mut report| {
					report["revision"] = serde_json::json!(args.revision);
					println!("{report}");
				},
			)
		}
	}
}
#[cfg(not(target_os = "windows"))]
fn run(command: Command) -> Result<(), &'static str> {
	if let Command::Run(args) = command {
		// Validate the portable CLI without accidentally opening another platform's capture.
		let _ = (
			args.owner,
			args.settings,
			args.video,
			args.adapter,
			args.stage,
			args.warmup,
			args.seconds,
			args.revision,
		);
	}
	Err("This owner-controlled probe requires Windows; no capture was started")
}

#[cfg(test)]
mod tests {
	use super::*;
	fn args(values: &[&str]) -> Vec<String> {
		values.iter().map(|value| (*value).to_owned()).collect()
	}
	#[test]
	fn capture_requires_explicit_gate_source_and_stage_and_never_enables_audio() {
		assert!(matches!(parse(args(&["--list"])), Ok(Command::List)));
		assert!(parse(args(&["--source", "display:1", "--stage", "capture-only"])).is_err());
		assert!(parse(args(&["--owner-controlled", "--stage", "readback"])).is_err());
		assert!(
			parse(args(&[
				"--owner-controlled",
				"--source",
				"window:0",
				"--stage",
				"readback"
			]))
			.is_err()
		);
		let Command::Run(config) = parse(args(&[
			"--owner-controlled",
			"--source",
			"display:0x123",
			"--stage",
			"readback",
		]))
		.unwrap() else {
			panic!("run")
		};
		assert_eq!(config.settings.source, SourceId::Display(0x123));
		assert!(!config.settings.audio);
		assert_eq!(config.settings.fps, 60);
		assert_eq!(config.stage, Stage::Readback);
	}
	#[test]
	fn encode_identity_and_repeated_or_unbounded_options_are_rejected() {
		assert!(
			parse(args(&[
				"--owner-controlled",
				"--source",
				"display:1",
				"--stage",
				"encode",
				"--codec",
				"h264"
			]))
			.is_err()
		);
		assert!(
			parse(args(&[
				"--owner-controlled",
				"--source",
				"display:1",
				"--stage",
				"readback",
				"--fps",
				"60",
				"--fps",
				"30"
			]))
			.is_err()
		);
		assert!(
			parse(args(&[
				"--owner-controlled",
				"--source",
				"display:1",
				"--stage",
				"readback",
				"--seconds",
				"601"
			]))
			.is_err()
		);
		assert!(
			parse(args(&[
				"--owner-controlled",
				"--source",
				"display:1",
				"--stage",
				"readback",
				"--adapter",
				"00001002:0000744c:unknown"
			]))
			.is_err()
		);
		let Command::Run(config) = parse(args(&[
			"--owner-controlled",
			"--source",
			"display:1",
			"--stage",
			"encode",
			"--codec",
			"av1",
			"--adapter",
			"00001002:0000744c:luid:0000000000001234",
		]))
		.unwrap() else {
			panic!("run")
		};
		assert_eq!(config.video.codec, VideoCodec::Av1);
	}
}
