#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
#[cfg(feature = "demo")]
mod access_marks_demo;
mod app_settings;
mod audio;
mod avatars;
mod cache;
mod captcha;
#[cfg(feature = "demo")]
mod channel_demo;
mod clipboard;
#[cfg(feature = "demo")]
mod community_admin_demo;
mod desktop_commands;
mod desktop_events;
mod desktop_frame;
mod desktop_history;
mod desktop_preferences;
mod desktop_presence;
mod desktop_startup;
mod desktop_window;
#[cfg(feature = "demo")]
mod offline_checks;

#[cfg(feature = "demo")]
mod community_demo;
#[cfg(feature = "demo")]
mod components_demo;
mod connection;
mod credentials;
#[cfg(feature = "demo")]
mod dm_demo;
mod downloads;
mod emoji_upload;
mod extension_app;
mod extension_bridge;
mod extension_data_events;
mod extension_events;
mod extension_forum_data;
mod extension_member_details;
mod extension_message_content;
mod extensions;
mod font_import;
mod force_sync;
mod game_activity;
mod game_mentions;
mod gpu;
mod gpu_video;
#[cfg(feature = "demo")]
mod group_demo;
mod group_icon;
#[cfg(feature = "demo")]
mod inbox_demo;
mod interaction_uploads;
mod media_output;
mod member_cache_search;
mod message_speech;
mod notification_runtime;
mod notification_sounds;
mod ogg_opus;
#[cfg(feature = "demo")]
mod onboarding_demo;
mod pointer;
#[cfg(feature = "demo")]
mod polls_demo;
#[cfg(feature = "demo")]
mod post_menu_demo;
#[cfg(feature = "demo")]
mod profile_settings_demo;
mod proxy_auth;
mod reading_settings;
mod registered_games;
#[cfg(feature = "demo")]
mod rendering_demo;
mod screen;
#[cfg(feature = "demo")]
mod server_settings_demo;
mod sign_in;
#[cfg(feature = "demo")]
mod slash_demo;
mod sound_upload;
mod soundboard;
mod spotify;
mod startup;
mod sticker_upload;
mod toggle_setting;
mod tray_window;
mod updater;
mod uploads;
mod video;
mod video_capabilities;
mod voice;
mod voice_messages;
mod watch;
use client_core::{
	Command, Envelope, Event, State,
	auth::{AuthState, Failure, SessionSecret},
};
use eframe::egui;
use model::Delivery;
use std::{sync::Arc, time::Duration};
use zeroize::Zeroizing;

/// Run explicit offline checks before native startup, or launch the configured desktop client.
fn main() -> eframe::Result {
	platform::memory::tune_allocator();
	if let Some(status) = discord_voice::audio::speech_worker_command() {
		std::process::exit(status);
	}
	if let Some(status) = video_capabilities::probe_command() {
		std::process::exit(status);
	}
	#[cfg(feature = "demo")]
	if offline_checks::before_startup() {
		return Ok(());
	}
	let demo = std::env::args().any(|arg| arg == "--demo");
	let start_minimized = startup::minimized_launch(demo, std::env::args());
	if !cfg!(feature = "demo")
		&& std::env::args().any(|arg| arg == "--demo" || arg.starts_with("--demo-"))
	{
		eprintln!("Demo support is not included; rebuild with --features demo and run with --demo");
		std::process::exit(2);
	}
	// Fixture runs may overlap; real launches show the running client instead of duplicating it.
	let mut instance = if demo {
		platform::single_instance::Instance::default()
	} else {
		match platform::single_instance::claim() {
			platform::single_instance::Launch::Primary(instance) => instance,
			platform::single_instance::Launch::Forwarded => return Ok(()),
		}
	};
	let frame_sample = std::env::args()
		.find_map(|arg| arg.strip_prefix("--demo-frame-sample").map(str::to_owned))
		.map(|value| {
			parse_frame_sample(
				demo && std::env::args().any(|arg| arg == "--demo-friends"),
				&value,
			)
			.unwrap_or_else(|reason| {
				eprintln!("{reason}");
				std::process::exit(2);
			})
		});
	#[cfg(feature = "demo")]
	if offline_checks::before_window(demo) {
		return Ok(());
	}
	// Native GPU/window capabilities are selected before the first window exists.
	let preferences = if demo {
		Ok(local_store::AppPreferences::default())
	} else {
		local_store::LocalStore::open_default().and_then(|store| store.app_preferences())
	};
	let (gpu_preference, transparency_available, hide_window_decorations, window_geometry) =
		preferences
			.as_ref()
			.map(|value| {
				(
					value.gpu_preference,
					value.transparency_blur,
					value.hide_window_decorations,
					value.window_geometry,
				)
			})
			.unwrap_or_default();
	#[cfg(feature = "demo")]
	let transparency_available =
		transparency_available || demo && std::env::args().any(|arg| arg == "--demo-transparency");
	#[cfg(target_os = "windows")]
	let icon = include_bytes!("../../../packaging/windows/serein.png").as_slice();
	#[cfg(target_os = "linux")]
	let icon = include_bytes!("../../../packaging/linux/hicolor/256x256/apps/serein.png").as_slice();
	let options = eframe::NativeOptions {
		viewport: {
			let builder = egui::ViewportBuilder::default()
				.with_transparent(transparency_available)
				.with_inner_size(
					window_geometry
						.map_or(app_settings::DEFAULT_WINDOW_SIZE, |geometry| geometry.size)
						.map(|value| value as f32),
				)
				// Maximize before the first configure so the first frame is not painted small.
				.with_maximized(
					cfg!(target_os = "linux")
						&& window_geometry.is_some_and(|geometry| geometry.maximized),
				)
				.with_min_inner_size(app_settings::MIN_WINDOW_SIZE.map(|size| size as f32))
				.with_active(!start_minimized)
				.with_app_id("cz.viceverse.serein");
			#[cfg(any(target_os = "windows", target_os = "linux"))]
			let builder = builder
				.with_icon(eframe::icon_data::from_png_bytes(icon).expect("bundled app icon"));
			if cfg!(target_os = "macos") {
				// Discord-style inline title bar: traffic lights sit over the app's own strip.
				// eframe swaps in the egui logo when no icon is set; an empty icon keeps the
				// bundle's Serein.icns in the Dock and app switcher.
				builder
					.with_icon(egui::IconData::default())
					.with_title_shown(false)
					.with_titlebar_shown(false)
					.with_fullsize_content_view(true)
			} else if cfg!(target_os = "windows") {
				// The app paints its own caption strip and buttons; see `ui::design::window_controls`.
				builder.with_decorations(false)
			} else if cfg!(target_os = "linux") {
				builder.with_decorations(!hide_window_decorations)
			} else {
				builder
			}
		},
		renderer: eframe::Renderer::Wgpu,
		wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
			wgpu_setup: eframe::egui_wgpu::WgpuSetup::CreateNew(
				eframe::egui_wgpu::WgpuSetupCreateNew {
					// Avoid Intel Vulkan driver startup crashes; keep the diagnostic override.
					#[cfg(target_os = "windows")]
					instance_descriptor: {
						let mut descriptor =
							eframe::wgpu::InstanceDescriptor::new_without_display_handle_from_env();
						descriptor.backends = eframe::wgpu::Backends::from_env()
							.unwrap_or(eframe::wgpu::Backends::DX12);
						// An HWND swapchain is always opaque; only a DirectComposition one carries
						// alpha to the desktop and its acrylic backdrop. The env override still wins.
						if transparency_available
							&& eframe::wgpu::Dx12SwapchainKind::from_env().is_none()
						{
							descriptor.backend_options.dx12.presentation_system =
								eframe::wgpu::Dx12SwapchainKind::DxgiFromVisual;
						}
						descriptor
					},
					// Only adapters that can present to this window are eligible; the saved
					// preference just orders them. A power hint alone picks GPUs the display is
					// not wired to, which fails outright on Wayland.
					native_adapter_selector: Some(std::sync::Arc::new(
						move |adapters: &[eframe::wgpu::Adapter],
						      surface: Option<&eframe::wgpu::Surface<'_>>| {
							gpu::select(gpu_preference, adapters, surface)
						},
					)),
					..gpu::setup()
				},
			),
			// Keep cursor-driven redraws synchronized even where AutoVsync selects FifoRelaxed.
			surface: eframe::egui_wgpu::SurfaceConfig {
				present_mode: eframe::wgpu::PresentMode::Fifo,
				..eframe::egui_wgpu::SurfaceConfig::LOW_LATENCY
			},
			..Default::default()
		},
		persist_window: false,
		persistence_path: None,
		..Default::default()
	};
	eframe::run_native(
		"Serein",
		options,
		Box::new(move |cc| {
			let mut desktop =
				Desktop::new(cc, demo, frame_sample, transparency_available, preferences)?;
			let wake = cc.egui_ctx.clone();
			instance.listen(
				desktop.window.clone(),
				desktop.tray_window.restorer(),
				move || wake.request_repaint(),
			);
			desktop.instance = instance;
			if let Some(geometry) = window_geometry {
				app_settings::restore_window_geometry(&desktop.window, geometry);
				desktop.maximize_after_show =
					geometry.maximized && app_settings::maximize_after_show(&desktop.window);
			}
			if start_minimized {
				cc.egui_ctx
					.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
			}
			Ok(Box::new(desktop))
		}),
	)
}

fn parse_frame_sample(demo: bool, value: &str) -> Result<(Duration, Duration), &'static str> {
	let invalid = "Use --demo --demo-friends --demo-frame-sample=WARMUP,SAMPLE (whole seconds, warmup 1..600, sample 1..600)";
	let (warmup, sample) = value
		.strip_prefix('=')
		.and_then(|value| value.split_once(','))
		.ok_or(invalid)?;
	let warmup = warmup.parse::<u64>().map_err(|_| invalid)?;
	let sample = sample.parse::<u64>().map_err(|_| invalid)?;
	if !demo || !(1..=600).contains(&warmup) || !(1..=600).contains(&sample) {
		return Err(invalid);
	}
	Ok((Duration::from_secs(warmup), Duration::from_secs(sample)))
}

struct FrameSample {
	ready: std::time::Instant,
	duration: Duration,
	started: Option<std::time::Instant>,
	complete: bool,
}

/// Opt-in aggregate callback wall time; excludes tessellation/presentation, not a CPU timer.
/// Sample mode emits two bounded records and never schedules repaints.
struct FrameMetrics {
	enabled: bool,
	started: Option<std::time::Instant>,
	sample: Option<FrameSample>,
	frames: u64,
	inputless: u64,
	viewport_focused: u64,
	search_focused: u64,
	viewport_size: Option<[f32; 2]>,
	pixels_per_point: f32,
	max_micros: u64,
	buckets: [u64; 8],
	reflows: (u64, u64),
}
impl Default for FrameMetrics {
	fn default() -> Self {
		Self::new(None)
	}
}
impl FrameMetrics {
	fn new(sample: Option<(Duration, Duration)>) -> Self {
		Self {
			enabled: sample.is_some()
				|| std::env::var_os("SEREIN_FRAME_DIAGNOSTICS").is_some_and(|v| v == "1"),
			started: None,
			sample: sample.map(|(warmup, duration)| FrameSample {
				ready: std::time::Instant::now() + warmup,
				duration,
				started: None,
				complete: false,
			}),
			frames: 0,
			inputless: 0,
			viewport_focused: 0,
			search_focused: 0,
			viewport_size: None,
			pixels_per_point: 0.0,
			max_micros: 0,
			buckets: [0; 8],
			reflows: (0, 0),
		}
	}
	fn sample_active_at(&mut self, now: std::time::Instant) -> bool {
		let viewport_size = self.viewport_size;
		let pixels_per_point = self.pixels_per_point;
		let Some(sample) = &mut self.sample else {
			return true;
		};
		if sample.complete || now < sample.ready {
			return false;
		}
		let started = *sample.started.get_or_insert_with(|| {
			Self::sample_record(serde_json::json!({
				"serein_frame_sample": "start",
				"viewport_size": viewport_size,
				"pixels_per_point": pixels_per_point,
			}));
			now
		});
		let elapsed = now.duration_since(started);
		if elapsed < sample.duration {
			return true;
		}
		sample.complete = true;
		Self::sample_record(serde_json::json!({
			"serein_frame_sample": "complete",
			"elapsed_ms": elapsed.as_millis(),
			"callbacks": self.frames,
			"without_input": self.inputless,
			"viewport_focused": self.viewport_focused,
			"search_focused": self.search_focused,
			"viewport_size": viewport_size,
			"pixels_per_point": pixels_per_point,
			"callback_wall_us_limits": [1000, 2000, 4000, 8000, 16000, 32000, 64000],
			"callback_wall_us_buckets": self.buckets,
			"max_callback_wall_us": self.max_micros,
		}));
		false
	}
	fn sample_record(record: serde_json::Value) {
		use std::io::Write;
		let mut output = std::io::stdout().lock();
		let _ = writeln!(output, "{record}");
		let _ = output.flush();
	}
	fn begin(&mut self, ctx: &egui::Context, search_focused: bool) {
		self.started = None;
		if !self.enabled {
			return;
		}
		if self.sample.is_some() {
			self.viewport_size = ctx.input(|i| {
				i.viewport()
					.inner_rect
					.map(|rect| [rect.width(), rect.height()])
			});
			self.pixels_per_point = ctx.pixels_per_point();
		}
		let now = std::time::Instant::now();
		if self.sample_active_at(now) {
			self.started = Some(now);
			self.inputless += u64::from(ctx.input(|i| i.events.is_empty()));
			self.viewport_focused += u64::from(ctx.input(|i| i.focused));
			self.search_focused += u64::from(search_focused);
		}
	}
	fn finish(&mut self) {
		if let Some(started) = self.started.take() {
			let micros = started.elapsed().as_micros();
			let bucket = [1000, 2000, 4000, 8000, 16000, 32000, 64000]
				.partition_point(|limit| *limit < micros);
			self.buckets[bucket] += 1;
			self.max_micros = self
				.max_micros
				.max(u64::try_from(micros).unwrap_or(u64::MAX));
			self.frames += 1;
		}
	}
}
impl Drop for FrameMetrics {
	fn drop(&mut self) {
		if self.enabled && self.sample.is_none() {
			use std::io::Write;
			let _ = writeln!(
				std::io::stderr(),
				// Preserve the legacy diagnostic label; elapsed callback time is wall time.
				"[Serein frames] callbacks={} without_input={} cpu_us_buckets(1000,2000,4000,8000,16000,32000,64000,above)={:?} reflows(total,consecutive)={:?}",
				self.frames,
				self.inputless,
				self.buckets,
				self.reflows
			);
		}
	}
}
/// Every session teardown ends the same way; only saved data and what follows differ.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum SessionEnd {
	#[default]
	Logout,
	Switch(model::Id),
	Add,
}
impl SessionEnd {
	/// Switching and adding keep this account's cached data and saved login.
	fn forgets(self) -> bool {
		self == Self::Logout
	}
}
struct Desktop {
	#[cfg(feature = "demo")]
	inbox_demo: inbox_demo::Store,
	proxy_auth: proxy_auth::Authentication,
	api_proxy: tokio::sync::watch::Sender<Option<discord_api::proxy::ApiProxy>>,
	extensions: extension_bridge::Bridge,
	extension_close_pending: bool,
	login: Option<platform::LoginView>,
	captcha: captcha::Captcha,
	connection: Option<connection::Connection>,
	state: State,
	messaging: ui::MessagingUi,
	/// Last invite counter a local Rich Presence client published, so it opens exactly once.
	rpc_invite_seen: u64,
	pointer: pointer::Pointer,
	downloads: downloads::Downloads,
	audio: audio::Audio,
	video: video::Video,
	/// Synthetic stream/camera textures the demo view borrows by id.
	_demo_textures: Vec<egui::TextureHandle>,
	/// Offline fixture flags start (and optionally pause) the demo attachment without input.
	demo_video_autoplay: Option<bool>,
	notifications: platform::notifications::Notifications,
	notification_runtime: notification_runtime::Runtime,
	uploads: uploads::Uploads,
	voice_messages: voice_messages::VoiceMessages,
	interaction_files: interaction_uploads::Files,
	group_icon: group_icon::GroupIcon,
	create_server_icon: group_icon::GroupIcon,
	profile_avatar: group_icon::GroupIcon,
	server_profile_image: group_icon::GroupIcon,
	server_icon: group_icon::GroupIcon,
	community_artwork: group_icon::GroupIcon,
	community_artwork_scope: Option<(u64, model::Id, Option<model::Id>, bool, u64)>,
	webhook_avatar: group_icon::GroupIcon,
	webhook_avatar_scope: Option<(u64, model::Id, model::Id, u64)>,
	role_icon: group_icon::GroupIcon,
	role_icon_scope: Option<(u64, model::Id, model::Id, u64)>,
	emoji_upload: emoji_upload::EmojiUpload,
	sticker_upload: sticker_upload::StickerUpload,
	sound_upload: sound_upload::SoundUpload,
	clipboard: Option<clipboard::Paste>,
	download_close_pending: bool,
	window: Arc<winit::window::Window>,
	/// The recent-page request whose cached window was already asked for.
	cached_history_request: cache::HistoryLoad,
	force_sync: force_sync::Sync,
	member_cache_load: cache::HistoryLoad,
	member_cache_search: member_cache_search::Search,
	expression_cache: cache::ExpressionCacheSchedule,
	sticker_pack_cache_load: cache::HistoryLoad,
	sticker_pack_cache_save: cache::HistoryLoad,
	gif_cache: cache::GifCache,
	member_cache_saved: Option<(u64, model::Id, u64, u64)>,
	member_cache_cleared: cache::MemberCacheClear,
	server_cache_load: cache::HistoryLoad,
	server_cache_failed: bool,
	/// The saved maximized state still has to be re-applied once the window is shown.
	maximize_after_show: bool,
	monitor_geometry: Option<(Option<egui::Rect>, Option<f32>, Option<egui::Vec2>)>,
	monitor_period: Option<Duration>,
	frame_metrics: FrameMetrics,
	message_speaker: Option<platform::message_speech::Speaker>,
	message_speech_target: Option<(u64, model::Id, model::Id)>,
	timezone_focused: bool,
	timezone_checked: std::time::Instant,
	#[cfg(feature = "demo")]
	rendering_demo: Option<rendering_demo::RenderingDemo>,
	avatars: Option<avatars::AvatarWorker>,
	avatar_start_failed: bool,
	avatar_clear_account: Option<model::Id>,
	avatar_cleanup: Option<std::sync::mpsc::Receiver<Result<(), &'static str>>>,
	voice: voice::Voice,
	runtime: tokio::runtime::Runtime,
	store: Option<credentials::Store>,
	cache: Option<cache::Cache>,
	cache_pending: usize,
	cache_clears: cache::HistoryClears,
	cache_forgets: cache::AccountForgets,
	cache_error: bool,
	cache_status: &'static str,
	appearance: egui::ThemePreference,
	appearance_changed: bool,
	transparency_available: bool,
	window_blur: Option<platform::window_effects::Blur>,
	window_transparent: bool,
	reading: reading_settings::ReadingSettings,
	app_settings: app_settings::Settings,
	font_picker: Option<std::sync::mpsc::Receiver<font_import::Selected>>,
	font_families: Option<std::sync::mpsc::Receiver<Vec<String>>>,
	updater: updater::Updater,
	game_activity: toggle_setting::Settings,
	game_mentions: game_mentions::Loader,
	registered_games: registered_games::Registered,
	tray_setting: toggle_setting::Settings,
	startup: startup::Startup,
	tray: Option<platform::tray::Tray>,
	hotkeys: platform::hotkeys::Hotkeys,
	/// Linux desktop light/dark preference; winit reports it everywhere else.
	system_theme: platform::system_theme::SystemTheme,
	tray_error: Option<&'static str>,
	tray_window: tray_window::State,
	/// Native menu bar, retitled whenever the interface language changes.
	#[cfg(target_os = "macos")]
	app_menu: Option<(platform::app_menu::AppMenu, ui::i18n::Language)>,
	/// Show requests from later launches and macOS Dock reopens.
	instance: platform::single_instance::Instance,
	/// `--demo-reply`: keeps two synthetic typists active on the selected fixture channel.
	#[cfg(feature = "demo")]
	demo_typing: bool,
	variant_changed: bool,
	pending_save: Option<Arc<SessionSecret>>,
	/// Written under the account's own entry on every READY, including a launch restore, so
	/// the switcher can always get back to an account it lists.
	pending_account_save: Option<Arc<SessionSecret>>,
	/// Current login, sharing the connection allocation, so enabling plaintext storage
	/// can save it without another authentication, including after disabling/re-enabling.
	current_login: Option<Arc<SessionSecret>>,
	/// The plaintext fallback choice last handed to the credential worker.
	plaintext_login: bool,
	/// One keyed-then-launch fallback save waiting for bounded credential-queue capacity.
	plaintext_save_pending: bool,
	account_presences: std::collections::BTreeMap<model::Id, model::OwnPresence>,
	presence_load_pending: bool,
	deferred_connect: Option<(SessionSecret, bool)>,
	presence_authoritative: bool,
	presence_saved: Option<(model::Id, model::OwnPresence)>,
	/// Saved account awaiting the owner's confirmation before it is forgotten.
	confirming_forget: Option<model::Id>,
	credential_status: &'static str,
	/// Until when the sign-in failure copy button reads "Copied".
	sign_in_copied: Option<f64>,
	forgetting: bool,
	confirming_close: bool,
	confirming_logout: bool,
	/// What the pending session teardown is for: logging out, switching or adding an account.
	end_intent: SessionEnd,
	/// Saved account whose token is being read for a switch.
	switching: Option<model::Id>,
	/// A switcher-roster write is in flight; failures stop retrying for this session.
	roster_pending: bool,
	roster_failed: bool,
	/// Session generation whose account was last recorded in the switcher roster.
	roster_generation: Option<u64>,
	close_approved: bool,
	fixture_only: bool,
	authorized: bool,
	/// Sign-in screen disclosures, folded away until the owner opens them.
	about_open: bool,
	token_open: bool,
	/// Height the sign-in card took last frame, so a tall card stays fully reachable.
	sign_in_height: f32,
	#[cfg(feature = "demo")]
	synthetic_id: u64,
	#[cfg(feature = "demo")]
	demo_groups: group_demo::Groups,
	token_input: Zeroizing<String>,
}
/// Check only navigation whose effective access can change with this event.
fn access_candidates(state: &State, event: &Event) -> Vec<model::Id> {
	use client_core::permissions::Event as Permission;
	let (guilds, channel): (Option<Vec<model::Id>>, Option<model::Id>) = match event {
		Event::Startup(_) | Event::Ready { .. } | Event::Permissions(Permission::Snapshot(_)) => {
			(None, None)
		}
		Event::Permissions(permission) => match permission {
			Permission::Guild(guild) => (Some(vec![guild.id]), None),
			Permission::Role { guild, .. }
			| Permission::RoleRemoved { guild, .. }
			| Permission::Member { guild, .. }
			| Permission::Owner { guild, .. }
			| Permission::UnavailableGuild(guild) => (Some(vec![*guild]), None),
			Permission::Members(members) => (Some(members.iter().map(|m| m.0).collect()), None),
			Permission::Channel { channel, .. } => (None, Some(*channel)),
			Permission::Snapshot(_) => unreachable!(),
		},
		Event::ChannelCreated(channel) | Event::ChannelRestored(channel) => {
			// A new channel cannot revoke existing access. Duplicate IDs can replace metadata.
			if state.channel(channel.id).is_none() {
				return Vec::new();
			}
			(None, Some(channel.id))
		}
		Event::ChannelChanged(patch) | Event::ThreadChanged { patch, .. } => (None, Some(patch.id)),
		Event::Unavailable(id) | Event::ThreadRemoved { id, .. } => (None, Some(*id)),
		Event::ChannelAction(client_core::channel_actions::Event::Finished {
			channel,
			result:
				Ok(
					client_core::channel_actions::Outcome::Deleted
					| client_core::channel_actions::Outcome::Channel { .. },
				),
			..
		}) => (None, Some(*channel)),
		Event::UserAction(client_core::user_actions::Event::Written {
			action: client_core::user_actions::Action::CloseDm(channel),
			result: Ok(()),
			..
		}) => (None, Some(*channel)),
		Event::GroupAction(client_core::group_actions::Event::Written {
			channel,
			result: Ok(None),
			..
		}) => (None, Some(*channel)),
		Event::ServerAction(client_core::server_actions::Event::Written {
			action: client_core::server_actions::Action::Leave(guild),
			result: Ok(None),
			..
		}) => (Some(vec![*guild]), None),
		Event::ThreadsSync { guild, .. } => (Some(vec![*guild]), None),
		_ => return Vec::new(),
	};
	state
		.channels
		.iter()
		.filter(|c| {
			c.supports_text()
				&& channel.is_none_or(|id| c.id == id || c.parent_id == Some(id))
				&& guilds
					.as_ref()
					.is_none_or(|ids| c.guild.is_some_and(|id| ids.contains(&id)))
		})
		.map(|c| c.id)
		.collect()
}
fn user_action_notice(event: &Event) -> Option<(ui::design::Level, &'static str)> {
	let Event::UserAction(client_core::user_actions::Event::Written { action, result, .. }) = event
	else {
		return None;
	};
	Some(match result {
		Ok(()) if matches!(action, client_core::user_actions::Action::OpenDm(_)) => {
			(ui::design::Level::Error, action.completion_label())
		}
		Ok(()) => (ui::design::Level::Success, action.completion_label()),
		Err(failure) => (ui::design::Level::Error, failure.label()),
	})
}
fn queue_channel_preferences(
	cache: Option<&cache::Cache>,
	messaging: &mut ui::MessagingUi,
	generation: u64,
	account: model::Id,
) -> bool {
	if !messaging.channel_preferences_reload || messaging.channel_preferences_load_pending {
		return false;
	}
	let Some(cache) = cache else {
		messaging.channel_preferences_reload = false;
		messaging.channel_preferences_status =
			"Local storage is unavailable; channel preferences could not be restored.";
		return false;
	};
	let accepted = cache.queue(
		generation,
		account,
		cache::Operation::LoadChannelPreferences,
	);
	// Keep one request until the bounded worker has room. Its completions wake the UI;
	// queue pressure is not a failed read and needs neither a timer nor another click.
	messaging.channel_preferences_reload = !accepted;
	messaging.channel_preferences_load_pending = accepted;
	if accepted {
		messaging.channel_preferences_status = "";
	}
	accepted
}

fn wants_cached_history(state: &State, channel: model::Id, request: u64) -> bool {
	state.selected == Some(channel)
		&& state.request == request
		&& state.history_pending
		&& state.freshness == model::Freshness::Loading
		&& state.timeline.row_count() == 0
		&& state.can_read_history(channel)
		&& state
			.channels
			.iter()
			.any(|c| c.id == channel && c.supports_text())
}
fn hydrate_cached_history(
	state: &mut State,
	channel: model::Id,
	request: u64,
	messages: Vec<model::Message>,
	start: bool,
) {
	if wants_cached_history(state, channel, request)
		&& messages.iter().all(|message| message.channel == channel)
		&& state.timeline.seed_cache(messages).is_ok()
	{
		// The channel's introduction can lead the cached window right away; the network page
		// replaces this once it arrives.
		state.older_exhausted = start;
		state.revision += 1;
	}
	state.enforce_resident_budget();
}
fn hydrate_cache_result(state: &mut State, safety: &cache::HistorySafety, outcome: cache::Outcome) {
	if let cache::Outcome::Channel {
		channel,
		request,
		messages,
		start,
		epoch,
	} = outcome
		&& safety.allows(epoch)
	{
		hydrate_cached_history(state, channel, request, messages, start);
	}
}

/// Soft radial accent glow behind the pre-session screens instead of a flat canvas.
fn accent_glow(ui: &egui::Ui) {
	let accent = ui::design::palette(ui).accent;
	let rect = ui.max_rect();
	let glow = rect.center() - egui::vec2(0.0, rect.height() * 0.1);
	let radius = rect.width().max(rect.height()) * 0.55;
	let mut mesh = egui::Mesh::default();
	let alpha = if ui.visuals().dark_mode { 0.22 } else { 0.12 };
	mesh.colored_vertex(glow, accent.gamma_multiply(alpha));
	const SEGMENTS: u32 = 48;
	for i in 0..=SEGMENTS {
		let angle = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
		mesh.colored_vertex(
			glow + egui::vec2(angle.cos(), angle.sin()) * radius,
			egui::Color32::TRANSPARENT,
		);
	}
	for i in 1..=SEGMENTS {
		mesh.add_triangle(0, i, i + 1);
	}
	ui.painter().add(egui::Shape::mesh(mesh));
}
fn recovery_draft(state: &State, channel: model::Id) -> String {
	state
		.drafts
		.get(&channel)
		.filter(|text| !text.is_empty())
		.cloned()
		.or_else(|| {
			state
				.pending
				.iter()
				.rev()
				.find(|pending| {
					pending.channel == channel && pending.delivery != Delivery::Confirmed
				})
				.map(|pending| pending.content.clone())
		})
		.unwrap_or_default()
}
/// Staged files may upload whenever the channel accepts attachments. The history
/// window's freshness is not a channel state: Send from older history jumps to the
/// latest page, so that window is reloading while the upload is dispatched.
fn attachment_send_allowed(state: &State, channel: model::Id) -> bool {
	!state.demo
		&& state.can_attach(channel)
		&& state.auth == AuthState::Authenticated
		&& state.gateway_connected
		&& state.selected == Some(channel)
}
fn confirmed_recovery_channel(state: &State, event: &Event) -> Option<model::Id> {
	let (message, nonce) = match event {
		Event::SendResult {
			result: Ok(message),
			nonce,
		} => (message, nonce.as_str()),
		Event::Message(message) => (message, message.nonce.as_deref()?),
		_ => return None,
	};
	if state.user.as_ref().map(|user| user.id) != Some(message.author.id) {
		return None;
	}
	state
		.pending
		.iter()
		.any(|pending| pending.channel == message.channel && pending.nonce == nonce)
		.then_some(message.channel)
}
fn changes_active_history(state: &State, event: &Event) -> bool {
	let channel = match event {
		Event::History {
			channel, request, ..
		} if state.history_request(*request) == state.request && state.history_pending => channel,
		Event::Message(message)
		| Event::SendResult {
			result: Ok(message),
			..
		} => &message.channel,
		Event::ServerAction(client_core::server_actions::Event::InviteSent {
			result: Ok(sent),
			..
		}) => &sent.1.channel,
		Event::Patch(patch) => &patch.channel,
		Event::Edited { channel, .. }
		| Event::Delete { channel, .. }
		| Event::DeleteBulk { channel, .. } => channel,
		_ => return false,
	};
	state.selected == Some(*channel)
}
/// Synthetic People rows with presence; never a Discord member directory.
#[cfg(feature = "demo")]
fn demo_members(
	state: &State,
	guild: Option<model::Id>,
	channel: model::Id,
	request: u64,
) -> model::MemberList {
	let mut members = vec![
		model::Member {
			user: test_support::message(2, channel).author,
			nick: None,
			roles: if guild.is_some() {
				vec![model::Id(9001)]
			} else {
				vec![]
			},
			status: Some("idle".into()),
			custom_status: None,
			clients: model::ClientPlatforms::default(),
			activities: vec![],
		},
		model::Member {
			user: test_support::message(1, channel).author,
			nick: None,
			roles: if guild.is_some() {
				vec![model::Id(9002)]
			} else {
				vec![]
			},
			status: Some("online".into()),
			custom_status: Some("🌙 semifluent in synthetic data".into()),
			clients: model::ClientPlatforms::default(),
			activities: vec![model::RichActivity {
				kind: 0,
				name: "Stardew Valley".into(),
				details: Some("Tending the synthetic farm".into()),
				state: Some("Spring - Day 12".into()),
				image: Some(model::ActivityImage::Asset {
					application: model::Id(9001),
					asset: model::Id(9002),
				}),
				small_image: None,
				ends_at: None,
				started_at: None,
			}],
		},
	];
	if guild.is_none() {
		let mut users = state
			.channel(channel)
			.map(|channel| channel.recipients.clone())
			.unwrap_or_default();
		if let Some(user) = &state.user
			&& !users.iter().any(|recipient| recipient.id == user.id)
		{
			users.push(user.clone());
		}
		members = users
			.into_iter()
			.map(|user| {
				let mut member = members
					.iter()
					.find(|member| member.user.id == user.id)
					.cloned()
					.unwrap_or(model::Member {
						user: user.clone(),
						nick: None,
						roles: vec![],
						status: None,
						custom_status: None,
						clients: model::ClientPlatforms::default(),
						activities: vec![],
					});
				member.user = user;
				member
			})
			.collect();
	} else {
		for (id, name, status) in [
			(9003, "Alex (synthetic)", "online"),
			(9004, "Sam (synthetic)", "offline"),
		] {
			let mut member = members[0].clone();
			member.user.id = model::Id(id);
			member.user.name = name.into();
			member.user.kind = if id == 9003 {
				model::AccountKind::VerifiedBot
			} else {
				model::AccountKind::Bot
			};
			member.roles.clear();
			member.status = Some(status.into());
			members.push(member);
		}
	}
	model::MemberList {
		guild,
		channel,
		request,
		total: members.len() as u64,
		start: 0,
		slots: members
			.into_iter()
			.map(|m| Some(model::MemberSlot::Person(m)))
			.collect(),
		lazy: false,
		groups: vec![],
		ranges: vec![],
		freshness: model::Freshness::Fresh,
	}
}

#[cfg(target_os = "windows")]
fn align_undecorated_surface(window: &winit::window::Window) {
	use winit::platform::windows::WindowExtWindows as _;
	// egui-winit turns on winit's 1px restored-client shift for custom chrome.
	// Maximized skips the shift.
	window.set_undecorated_shadow(false);
}

impl eframe::App for Desktop {
	fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
		if self.window_transparent {
			egui::Color32::TRANSPARENT.to_normalized_gamma_f32()
		} else {
			// Opaque windows need opaque pixels too, including uncovered panel corners.
			visuals.panel_fill.to_opaque().to_normalized_gamma_f32()
		}
	}
	fn persist_egui_memory(&self) -> bool {
		false
	}
	fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
		// Before the pass begins, so the whole frame resolves System to the same theme.
		self.sync_system_theme(ctx);
		if self.maximize_after_show && self.window.is_visible() == Some(true) {
			self.maximize_after_show = false;
			self.window.set_maximized(true);
		}
		// Read native size independently of viewport rectangles: Wayland has no global position,
		// so egui-winit cannot supply either rectangle there. Native scale excludes egui zoom.
		if !self.fixture_only
			&& !self.state.demo
			&& self.app_settings.loaded
			&& !self.maximize_after_show
			&& let Some(viewport) = raw_input.viewports.get(&raw_input.viewport_id)
			&& viewport.minimized != Some(true)
			&& viewport.fullscreen != Some(true)
			&& !viewport.close_requested()
			&& self.window.is_visible() != Some(false)
		{
			// While maximized, keep the normal geometry so un-maximizing after a restart returns to it.
			let geometry = if viewport.maximized == Some(true) {
				local_store::WindowGeometry {
					maximized: true,
					..self.app_settings.current.window_geometry.unwrap_or(
						local_store::WindowGeometry {
							size: app_settings::DEFAULT_WINDOW_SIZE,
							position: None,
							maximized: true,
						},
					)
				}
			} else {
				let size = self
					.window
					.inner_size()
					.to_logical::<u32>(self.window.scale_factor());
				local_store::WindowGeometry {
					size: [size.width, size.height],
					position: self
						.window
						.outer_position()
						.ok()
						.map(|position| [position.x, position.y]),
					maximized: false,
				}
			};
			if geometry.is_valid() && self.app_settings.current.window_geometry != Some(geometry) {
				self.app_settings.current.window_geometry = Some(geometry);
				self.app_settings.state.dirty = true;
			}
		}
		// Monitor size also detects Wayland moves without a global window position.
		if let Some(viewport) = raw_input.viewports.get(&raw_input.viewport_id) {
			let geometry = (
				viewport.outer_rect,
				viewport.native_pixels_per_point,
				viewport.monitor_size,
			);
			if self.monitor_geometry != Some(geometry) {
				self.monitor_geometry = Some(geometry);
				self.monitor_period = self.refresh_monitor();
			}
		}
		if let Some(period) = self.frame_period() {
			raw_input.predicted_dt = period.as_secs_f32();
		}
		let track = self.messaging.tracking_pointer();
		let intercepted = self.pointer.intercept(
			raw_input,
			&self.window,
			ctx.pixels_per_point(),
			track,
			self.messaging.wants_mouse_buttons(),
		);
		if intercepted.middle.pressed.is_some() || intercepted.side.back || intercepted.side.forward
		{
			ui::scroll::reset_wheel(ctx);
		}
		self.messaging.middle_button(intercepted.middle);
		self.messaging.side_buttons(intercepted.side);
		if track
			&& let Some(egui::Event::PointerMoved(pos)) = raw_input.events.last()
			&& !ctx.content_rect().contains(*pos)
		{
			ctx.request_repaint();
		}
	}
	fn on_exit(&mut self) {
		if self.updater.finish_restart().is_err() {
			eprintln!(
				"Could not hand off the prepared update. The installed app was not replaced."
			);
		}
	}
	fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
		self.frame_logic(ctx, frame);
	}
	fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
		self.show_frame(ui, frame);
	}
}

/// Holds owner-enabled plaintext logins, inside the private local data directory.
fn plaintext_login_dir() -> Option<std::path::PathBuf> {
	local_store::data_dir()
		.ok()
		.map(|root| root.join("plaintext-login"))
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn unavailable_parent_captures_child_threads_for_scoped_cache_cleanup() {
		let mut state = test_support::demo_state();
		state.demo = false;
		let event = Event::Unavailable(model::Id(20));
		let mut candidates = access_candidates(&state, &event);
		assert!(candidates.contains(&model::Id(20)));
		assert!(
			candidates.contains(&model::Id(28)),
			"child thread is part of the removed scope"
		);
		assert!(
			!candidates.contains(&model::Id(21)),
			"sibling conversations are not invalidated"
		);
		state.apply(Envelope {
			generation: state.generation,
			event,
		});
		candidates.retain(|channel| !state.can_read_history(*channel));
		assert!(candidates.contains(&model::Id(20)) && candidates.contains(&model::Id(28)));
		assert!(state.channel(model::Id(20)).is_none() && state.channel(model::Id(28)).is_none());
		assert!(state.can_read_history(model::Id(21)));
	}
	#[test]
	fn permission_loss_in_another_guild_does_not_purge_selected_guild() {
		use model::{Id, Patch, permissions as p};
		let mut state = test_support::demo_state();
		state.demo = false;
		let mut guild = state.guilds[0].clone();
		guild.id = Id(11);
		state.guilds.push(guild);
		for id in [100, 101] {
			let mut channel = state.channel(Id(20)).unwrap().clone();
			channel.id = Id(id);
			channel.guild = Some(Id(11));
			channel.parent_id = None;
			state.channels.push(channel);
		}
		state.invalidate_navigation();
		let snapshot = test_support::permission_snapshot(&state);
		state.permissions.replace(snapshot).unwrap();
		assert_eq!(state.selected, Some(Id(20)));
		let event = Event::Permissions(client_core::permissions::Event::Channel {
			channel: Id(100),
			guild: Some(Id(11)),
			overwrites: Patch::Value(vec![p::Overwrite {
				id: Id(11),
				kind: 0,
				allow: 0,
				deny: p::READ_MESSAGE_HISTORY,
			}]),
		});
		let mut candidates = access_candidates(&state, &event);
		let guilds: std::collections::BTreeMap<_, _> = candidates
			.iter()
			.filter_map(|id| {
				state
					.channel(*id)
					.and_then(|channel| channel.guild.map(|guild| (*id, guild)))
			})
			.collect();
		state.apply(Envelope {
			generation: state.generation,
			event,
		});
		candidates.retain(|channel| !state.can_read_history(*channel));
		assert_eq!(candidates, vec![Id(100)]);
		let affected: std::collections::BTreeSet<_> = candidates
			.iter()
			.filter_map(|id| guilds.get(id).copied())
			.collect();
		assert_eq!(affected, [Id(11)].into_iter().collect());
		assert!(state.can_read_history(Id(20)) && state.can_read_history(Id(101)));
		assert_eq!(state.selected, Some(Id(20)));
	}
	#[cfg(feature = "demo")]
	#[test]
	fn demo_group_members_preserve_recipients_and_deduplicate_current_account() {
		let mut state = test_support::demo_state();
		let channel = model::Id(29);
		state.selected = Some(channel);
		let recipient = state.channel(channel).unwrap().recipients[1].clone();
		for include_current in [false, true] {
			if include_current {
				let current = state.user.clone().unwrap();
				state
					.channels
					.iter_mut()
					.find(|c| c.id == channel)
					.unwrap()
					.recipients = vec![current, recipient.clone()];
			}
			let Some(Command::Members { guild, request, .. }) = state.request_members() else {
				panic!("synthetic group member request");
			};
			let expected = state.members.as_ref().unwrap().clone();
			let response = demo_members(&state, guild, channel, request);
			state.apply(Envelope {
				generation: state.generation,
				event: Event::Members(response),
			});
			let list = state.members.as_ref().unwrap();
			assert_eq!(list.total, if include_current { 2 } else { 3 });
			assert_eq!(list.total, expected.total);
			let users = |list: &model::MemberList| {
				list.slots
					.iter()
					.filter_map(|slot| match slot {
						Some(model::MemberSlot::Person(member)) => Some(member.user.clone()),
						_ => None,
					})
					.collect::<Vec<_>>()
			};
			assert!(users(list) == users(&expected));
			assert!(users(list).contains(&recipient));
		}
	}

	#[test]
	fn user_action_results_use_toasts() {
		let success = Event::UserAction(client_core::user_actions::Event::Written {
			action: client_core::user_actions::Action::Nickname {
				user: model::Id(2),
				text: "Synthetic".into(),
			},
			request: 1,
			result: Ok(()),
		});
		let (level, text) = user_action_notice(&success).unwrap();
		assert!(matches!(level, ui::design::Level::Success));
		assert_eq!(text, "Nickname saved");
	}
	#[test]
	fn frame_sample_is_bounded_demo_only_and_excludes_warmup() {
		for (demo, value) in [
			(false, "=8,15"),
			(true, "=0,15"),
			(true, "=8,0"),
			(true, "=601,15"),
			(true, "=8,601"),
			(true, "=8,15,1"),
			(true, "=8.5,15"),
			(true, ""),
		] {
			assert!(parse_frame_sample(demo, value).is_err());
		}
		let durations = parse_frame_sample(true, "=8,15").unwrap();
		let mut metrics = FrameMetrics::new(Some(durations));
		let ready = metrics.sample.as_ref().unwrap().ready;
		assert!(!metrics.sample_active_at(ready - Duration::from_secs(1)));
		assert_eq!(metrics.frames, 0);
		assert!(metrics.sample_active_at(ready));
		assert!(metrics.sample_active_at(ready + Duration::from_secs(14)));
		assert!(!metrics.sample_active_at(ready + Duration::from_secs(15)));
		assert!(!metrics.sample_active_at(ready + Duration::from_secs(16)));
		assert!(metrics.sample.as_ref().unwrap().complete);
	}
	#[test]
	fn disk_cache_does_not_replace_resident_previews_or_deleted_positions() {
		for deleted in [false, true] {
			let mut state = test_support::demo_state();
			let alpha = state.selected.unwrap();
			let beta = model::Id(900);
			let mut other = state
				.channels
				.iter()
				.find(|c| c.id == alpha)
				.unwrap()
				.clone();
			other.id = beta;
			other.guild = None;
			other.kind = 1;
			state.channels.push(other);
			state.timeline.clear();
			state
				.timeline
				.insert(test_support::message(1001, alpha), false, false)
				.unwrap();
			if deleted {
				state.timeline.delete(model::Id(1001)).unwrap();
			}
			state.select(beta).unwrap();
			state.apply(Envelope {
				generation: state.generation,
				event: Event::History {
					channel: beta,
					request: state.request,
					older: false,
					messages: vec![test_support::message(1002, beta)],
				},
			});
			state.select(alpha).unwrap();
			let request = state.request;
			assert_eq!(state.timeline.row_count(), 1);
			assert!(!wants_cached_history(&state, alpha, request));
			hydrate_cached_history(
				&mut state,
				alpha,
				request,
				vec![test_support::message(1003, alpha)],
				false,
			);
			assert_eq!(
				state.timeline.row_ids().collect::<Vec<_>>(),
				[model::Id(1001)]
			);
			assert_eq!(state.timeline.is_empty(), deleted);
			assert_eq!(state.freshness, model::Freshness::Loading);
			state.clear_cached_history();
			assert_eq!(
				state.timeline.row_count(),
				1,
				"Clear keeps the displayed conversation"
			);
			state.select(beta).unwrap();
			assert_eq!(
				state.timeline.row_count(),
				0,
				"Cleared dormant windows cannot return"
			);
			assert!(wants_cached_history(&state, beta, state.request));
		}
	}
	#[test]
	fn delayed_cache_results_cannot_hydrate_after_a_known_deletion() {
		let mut state = test_support::demo_state();
		let channel = state.selected.unwrap();
		state.timeline.clear();
		state.history(None);
		let request = state.request;
		let safety = cache::HistorySafety::default();
		let outcome = |epoch| cache::Outcome::Channel {
			channel,
			request,
			start: false,
			epoch,
			messages: vec![test_support::message(1000, channel)],
		};
		let old_epoch = safety.epoch();
		safety.invalidate();
		hydrate_cache_result(&mut state, &safety, outcome(old_epoch));
		assert!(state.timeline.is_empty());
		safety.block();
		hydrate_cache_result(&mut state, &safety, outcome(safety.epoch()));
		assert!(state.timeline.is_empty());
		safety.cleared();
		hydrate_cache_result(&mut state, &safety, outcome(safety.epoch()));
		assert_eq!(state.timeline.len(), 1);
	}

	#[test]
	fn an_adopted_preload_page_is_saved_like_the_clicks_own() {
		let mut state = test_support::demo_state();
		state.demo = false;
		state.auth = AuthState::Authenticated;
		state.gateway_connected = true;
		let channel = model::Id(21);
		state.observe_last_message(channel, model::Id(2_000));
		let Some(Command::Prefetch(preload)) =
			state.preload_conversation(None, Some(channel), std::time::Instant::now())
		else {
			panic!("pressed conversation preloads");
		};
		let Command::History { request, .. } = *preload else {
			panic!()
		};
		assert!(
			state.select(channel).is_none(),
			"the click adopts the preload"
		);
		let page = |request| Event::History {
			channel,
			request,
			older: false,
			messages: vec![],
		};
		assert!(changes_active_history(&state, &page(request)));
		assert!(!changes_active_history(&state, &page(request ^ 1)));
	}
	#[test]
	fn cached_history_requires_current_readable_navigation_and_pending_request() {
		let mut state = test_support::demo_state();
		let channel = state.selected.unwrap();
		state.timeline.clear();
		state.history(None);
		let request = state.request;
		hydrate_cached_history(
			&mut state,
			channel,
			request,
			vec![test_support::message(1000, channel)],
			false,
		);
		assert_eq!(state.timeline.len(), 1);
		assert_eq!(state.freshness, model::Freshness::Loading);
		state.timeline.clear();
		let permissions = state.permissions.clone();
		state.permissions.channels.remove(&channel);
		state.permissions.clear_cache();
		assert!(!state.can_read_history(channel));
		hydrate_cached_history(
			&mut state,
			channel,
			request,
			vec![test_support::message(1000, channel)],
			false,
		);
		assert!(
			state.timeline.is_empty(),
			"Loaded navigation alone does not authorize cached history"
		);
		state.permissions = permissions;
		for invalid in [
			vec![test_support::message(1001, model::Id(999))],
			vec![test_support::message(1002, channel)],
		] {
			hydrate_cached_history(&mut state, channel, request.wrapping_sub(1), invalid, false);
			assert!(state.timeline.is_empty());
		}
		hydrate_cached_history(
			&mut state,
			channel,
			request,
			vec![test_support::message(1001, model::Id(999))],
			false,
		);
		assert!(state.timeline.is_empty());
		state.history_pending = false;
		hydrate_cached_history(
			&mut state,
			channel,
			request,
			vec![test_support::message(1000, channel)],
			false,
		);
		assert!(state.timeline.is_empty());
		state.history_pending = true;
		let mut channels = state.channels.clone();
		channels.retain(|c| c.id != channel);
		state.apply(Envelope {
			generation: state.generation,
			event: Event::Ready {
				user: state.user.clone().unwrap(),
				guilds: state.guilds.clone(),
				permissions: test_support::permission_snapshot(&state),
				channels,
			},
		});
		hydrate_cached_history(
			&mut state,
			channel,
			request,
			vec![test_support::message(1000, channel)],
			false,
		);
		assert!(state.timeline.is_empty());
		// Even an inconsistent queued-cache admission state cannot bypass current navigation.
		state.selected = Some(channel);
		state.request = request;
		state.freshness = model::Freshness::Loading;
		state.history_pending = true;
		hydrate_cached_history(
			&mut state,
			channel,
			request,
			vec![test_support::message(1000, channel)],
			false,
		);
		assert!(state.timeline.is_empty());
		let mut restored = test_support::demo_state()
			.channels
			.into_iter()
			.find(|c| c.id == channel)
			.unwrap();
		restored.kind = 4; // Categories do not support text; voice channels do.
		state.channels.push(restored);
		hydrate_cached_history(
			&mut state,
			channel,
			request,
			vec![test_support::message(1000, channel)],
			false,
		);
		assert!(state.timeline.is_empty());
	}
	#[test]
	fn attachment_sends_stay_allowed_while_the_latest_page_reloads() {
		let mut state = test_support::demo_state();
		state.demo = false;
		let channel = state.selected.unwrap();
		assert!(attachment_send_allowed(&state, channel));
		// Send from older history jumps to the latest page before the upload dispatches.
		state.history(None);
		assert_eq!(state.freshness, model::Freshness::Loading);
		assert!(attachment_send_allowed(&state, channel));
		state.gateway_connected = false;
		assert!(!attachment_send_allowed(&state, channel));
		state.gateway_connected = true;
		state.freshness = model::Freshness::Unavailable;
		assert!(!attachment_send_allowed(&state, channel));
		state.freshness = model::Freshness::Fresh;
		state.selected = None;
		assert!(!attachment_send_allowed(&state, channel));
	}
	#[test]
	fn correlated_confirmation_preserves_other_pending_text_and_ignores_unrelated_events() {
		let mut state = test_support::demo_state();
		let channel = state.selected.unwrap();
		state.drafts.insert(channel, "first pending draft".into());
		let Command::Send { nonce, .. } = state.prepare_send().unwrap() else {
			panic!()
		};
		state.drafts.insert(channel, "second pending draft".into());
		state.prepare_send().unwrap();
		let mut message = test_support::message(1000, channel);
		message.author = state.user.clone().unwrap();
		message.nonce = Some(nonce.clone());
		let mut foreign = message.clone();
		foreign.author.id = model::Id(999);
		assert!(confirmed_recovery_channel(&state, &Event::Message(foreign)).is_none());
		let confirmation = Event::SendResult {
			nonce,
			result: Ok(message),
		};
		assert_eq!(
			confirmed_recovery_channel(&state, &confirmation),
			Some(channel)
		);
		state.apply(Envelope {
			generation: state.generation,
			event: confirmation,
		});
		assert_eq!(recovery_draft(&state, channel), "second pending draft");
		state.drafts.insert(channel, String::new());
		assert_eq!(recovery_draft(&state, channel), "second pending draft");
		state.drafts.insert(channel, "new unsent edit".into());
		assert_eq!(recovery_draft(&state, channel), "new unsent edit");
		assert!(!changes_active_history(
			&state,
			&Event::Message(test_support::message(1001, model::Id(999)))
		));
		assert!(!changes_active_history(
			&state,
			&Event::History {
				channel,
				request: state.request.wrapping_sub(1),
				older: false,
				messages: vec![]
			}
		));
		assert!(changes_active_history(
			&state,
			&Event::DeleteBulk {
				channel,
				ids: vec![model::Id(1000)]
			}
		));
	}
}
