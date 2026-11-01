//! Explicit synthetic command-line checks, kept outside native application startup.
#[cfg(debug_assertions)]
use super::{
	Command, audio, avatars, cache, components_demo, font_import, notification_runtime, spotify,
	uploads, voice,
};
use super::{Envelope, Event, app_settings, egui, extensions, updater};
#[cfg(feature = "demo")]
use super::{access_marks_demo, dm_demo, polls_demo, post_menu_demo, slash_demo};

pub(super) fn before_startup() -> bool {
	#[cfg(all(debug_assertions, feature = "demo"))]
	if std::env::args().any(|arg| arg == "--demo")
		&& std::env::args().any(|arg| arg == "--demo-check-pr565")
	{
		ui::debug_pr565(
			test_support::demo_state(),
			test_support::message(1, model::Id(20)).author,
		);
		avatars::debug_profile_resolution_check();
		ui::MessagingUi::debug_call_membership_check(test_support::call_demo_state());
		let runtime = tokio::runtime::Builder::new_current_thread()
			.enable_all()
			.build()
			.unwrap();
		uploads::debug_reservation_check(runtime.handle());
		discord_gateway::debug_voice_state_retry_check();
		ui::MessagingUi::debug_double_click_reaction_check(
			test_support::demo_state(),
			test_support::message(60_000 << 22, model::Id(20)),
		);
		local_store::LocalStore::debug_double_click_reaction_check();
		return true;
	}
	#[cfg(all(debug_assertions, feature = "demo"))]
	if std::env::args().any(|arg| arg == "--demo")
		&& std::env::args().any(|arg| arg == "--demo-check-window-geometry")
	{
		app_settings::debug_window_geometry_check();
		return true;
	}
	#[cfg(all(debug_assertions, feature = "demo"))]
	if std::env::args().any(|arg| arg == "--demo")
		&& std::env::args().any(|arg| arg == "--demo-check-voice-preferences")
	{
		cache::debug_voice_preferences_check();
		return true;
	}
	#[cfg(all(debug_assertions, feature = "demo"))]
	if std::env::args().any(|arg| arg == "--demo")
		&& std::env::args().any(|arg| arg == "--demo-check-heic")
	{
		avatars::debug_heic_check();
		uploads::debug_heic_check();
		println!(
			"Offline HEIC check passed: brand recognition, malformed rejection, upload preview admission, decoded aspect ratio and 4096px original viewer routing. Valid synthetic 6000x4000 HEIC decoding and scaling checked on Windows; owner photos unverified."
		);
		return true;
	}
	#[cfg(all(debug_assertions, feature = "demo"))]
	if std::env::args().any(|arg| arg == "--demo")
		&& std::env::args().any(|arg| arg == "--demo-check-resume-send")
	{
		discord_gateway::debug_recovery_check();
		ui::debug_resume_send_check(test_support::demo_state());
		return true;
	}
	#[cfg(all(debug_assertions, feature = "demo"))]
	if std::env::args().any(|arg| arg == "--demo")
		&& std::env::args().any(|arg| arg == "--demo-check-audio")
	{
		audio::debug_voice_message_check();
		println!(
			"Offline audio check passed: 24 MiB admission and complete range decoding, bounded buffering."
		);
		return true;
	}
	#[cfg(all(debug_assertions, feature = "demo"))]
	if std::env::args().any(|arg| arg == "--demo")
		&& std::env::args().any(|arg| arg == "--demo-check-call-cues")
	{
		voice::debug_call_cues_check();
		return true;
	}
	#[cfg(all(debug_assertions, feature = "demo"))]
	if std::env::args().any(|arg| arg == "--demo")
		&& std::env::args().any(|arg| arg == "--demo-check-fonts")
	{
		font_import::debug_check();
		return true;
	}
	#[cfg(all(debug_assertions, feature = "demo"))]
	if std::env::args().any(|arg| arg == "--demo")
		&& std::env::args().any(|arg| arg == "--demo-check-customization")
	{
		font_import::debug_check();
		let mut state = test_support::chat_demo_state();
		let mut permissions = test_support::permission_snapshot(&state);
		for guild in &mut permissions.guilds {
			guild.owner = state.user.as_ref().map(|user| user.id);
		}
		state.permissions.replace(permissions).unwrap();
		ui::debug_channel_creation(state);
		return true;
	}
	#[cfg(all(debug_assertions, feature = "demo"))]
	if std::env::args().any(|arg| arg == "--demo")
		&& std::env::args().any(|arg| arg == "--demo-check-spotify")
	{
		spotify::debug_check();
		discord_api::spotify::debug_check();
		discord_gateway::debug_spotify_check();
		return true;
	}
	#[cfg(all(debug_assertions, feature = "demo"))]
	if std::env::args().any(|arg| arg == "--demo")
		&& std::env::args().any(|arg| arg == "--demo-check-forward")
	{
		let mut state = test_support::demo_state();
		ui::debug_forward_check(&mut state);
		println!(
			"Forward debug check passed: picker, bounded destinations, optional note, draft preservation and queue rejection."
		);
		return true;
	}
	#[cfg(all(debug_assertions, feature = "demo"))]
	if std::env::args().any(|arg| arg == "--demo")
		&& std::env::args().any(|arg| arg == "--demo-check-notification-badge")
	{
		notification_runtime::debug_badge_check();
		return true;
	}
	#[cfg(all(debug_assertions, feature = "demo"))]
	if std::env::args().any(|arg| arg == "--demo")
		&& std::env::args().any(|arg| arg == "--demo-check-notification-click")
	{
		let mut state = test_support::demo_state();
		for channel in [model::Id(22), model::Id(20)] {
			let target = platform::notifications::debug_activation_check(channel);
			state.select(target);
			assert_eq!(state.selected, Some(channel));
		}
		println!(
			"Notification click debug check passed: DM/guild navigation and stale click rejection."
		);
		return true;
	}
	#[cfg(all(debug_assertions, feature = "demo"))]
	if std::env::args().any(|arg| arg == "--demo")
		&& std::env::args().any(|arg| arg == "--demo-check-settings-sliders")
	{
		ui::design::debug_slider_check();
		return true;
	}

	#[cfg(all(debug_assertions, feature = "demo"))]
	if std::env::args().any(|arg| arg == "--demo")
		&& std::env::args().any(|arg| arg == "--demo-check-mic-preview")
	{
		voice::debug_mic_preview_check();
		return true;
	}

	#[cfg(all(debug_assertions, feature = "demo"))]
	if std::env::args().any(|arg| arg == "--demo")
		&& std::env::args().any(|arg| arg == "--demo-check-components")
	{
		components_demo::check();
		return true;
	}
	#[cfg(all(debug_assertions, feature = "demo"))]
	if std::env::args().any(|arg| arg == "--demo")
		&& std::env::args().any(|arg| arg == "--demo-check-suggestion-clicks")
	{
		let mut state = test_support::demo_state();
		let channel = state
			.channels
			.iter()
			.find(|c| c.guild.is_some() && c.kind == 0)
			.unwrap()
			.id;
		state.select(channel);
		state.gateway_connected = true;
		ui::debug_suggestion_pointer_check(&mut state, channel);
		println!(
			"Suggestion pointer debug check passed: members, emoji and channels insert on click without sending."
		);
		return true;
	}

	#[cfg(all(debug_assertions, feature = "demo"))]
	if std::env::args().any(|arg| arg == "--demo")
		&& std::env::args().any(|arg| arg == "--demo-check-member-search")
	{
		let mut state = test_support::demo_state();
		let channel = state
			.channels
			.iter()
			.find(|c| c.guild.is_some() && c.kind == 0)
			.unwrap()
			.id;
		state.select(channel);
		state.gateway_connected = true;
		let Some(Command::MemberSearch(request)) = state.search_members(channel, "Outside", 0)
		else {
			panic!("search should be permitted");
		};
		let event = discord_gateway::debug_member_search_check(request.clone());
		state.apply(Envelope {
			generation: state.generation,
			event,
		});
		ui::debug_member_search_check(&state, channel);
		let Some(Command::MemberSearch(_)) = state.search_members(channel, "Replacement", 0) else {
			panic!("replacement search");
		};
		state.apply(Envelope {
			generation: state.generation,
			event: Event::MemberSearch {
				request,
				result: Ok(vec![]),
			},
		});
		assert!(
			!state.member_search[0].finished,
			"stale result must be ignored"
		);
		state.demo = false;
		assert!(
			state
				.request_author_members(&[model::Id(987654321)])
				.is_none()
		);
		let user = model::Id(987654324);
		let Some(Command::MemberSearch(author_request)) = state.request_author_members(&[user])
		else {
			panic!("visible author lookup should be permitted");
		};
		assert!(state.request_author_members(&[user]).is_none());
		let guild = author_request.guild;
		let mut event = discord_gateway::debug_member_search_check(author_request);
		let role = model::Id(987654322);
		state.apply(Envelope {
			generation: state.generation,
			event: Event::Permissions(client_core::permissions::Event::Role {
				guild,
				role: model::permissions::Role {
					icon: None,
					unicode_emoji: None,
					id: role,
					name: "Verified".into(),
					color: 0x00ff00,
					secondary_color: None,
					tertiary_color: None,
					position: 1,
					hoist: false,
					bits: 0,
				},
			}),
		});
		if let Event::MemberSearch {
			result: Ok(rows), ..
		} = &mut event
		{
			rows[0].roles = vec![role];
		}
		let mut message = test_support::message(987654323, channel);
		message.author.id = user;
		message.author_roles.clear();
		state.members = None;
		assert_eq!(state.message_author_color(&message), None);
		state.apply(Envelope {
			generation: state.generation,
			event,
		});
		assert_eq!(state.message_author_color(&message), Some(0x00ff00));
		assert!(state.request_author_members(&[user]).is_none());
		assert_eq!(
			state.member_search[0].request.as_ref().unwrap().query,
			"Replacement"
		);

		println!(
			"Member search debug check passed: remote mentions and visible-author role colors use bounded, independent Gateway lookups."
		);
		return true;
	}

	false
}

pub(super) fn before_window(demo: bool) -> bool {
	#[cfg(feature = "demo")]
	if demo && std::env::args().any(|arg| arg == "--demo-check-polls") {
		polls_demo::check();
		return true;
	}
	#[cfg(feature = "demo")]
	if demo && std::env::args().any(|arg| arg == "--demo-check-access-marks") {
		access_marks_demo::check();
		return true;
	}
	#[cfg(feature = "demo")]
	if demo && std::env::args().any(|arg| arg == "--demo-check-switcher") {
		dm_demo::check();
		return true;
	}
	#[cfg(feature = "demo")]
	if demo && std::env::args().any(|arg| arg == "--demo-check-slash-commands") {
		slash_demo::check();
		return true;
	}
	#[cfg(feature = "demo")]
	if demo && std::env::args().any(|arg| arg == "--demo-check-forum-sidebar") {
		post_menu_demo::check_sidebar();
		return true;
	}
	#[cfg(feature = "demo")]
	if demo && std::env::args().any(|arg| arg == "--demo-check-post-menu") {
		post_menu_demo::check();
		return true;
	}
	#[cfg(all(debug_assertions, feature = "demo"))]
	if demo && std::env::args().any(|arg| arg == "--demo-check-image-sharing") {
		let mut state = test_support::demo_state();
		let selection = ui::debug_image_sharing(&mut state);
		let channel = state.selected.unwrap();
		let ctx = egui::Context::default();
		let runtime = tokio::runtime::Runtime::new().expect("offline artwork runtime");
		let mut uploads = uploads::Uploads::default();
		for newer_draft in [None, Some("Next message")] {
			state.drafts.insert(channel, selection.1.clone());
			uploads
				.start_image_share(
					state.generation,
					channel,
					selection.clone(),
					runtime.handle(),
					&ctx,
					true,
				)
				.unwrap();
			assert!(uploads.image_send(&mut state, true).is_none());
			if let Some(text) = newer_draft {
				state.drafts.insert(channel, text.into());
			}
			runtime.block_on(async {
				tokio::time::timeout(std::time::Duration::from_secs(5), async {
					while uploads.busy() {
						uploads.poll(state.generation, Some(channel), true, &ctx);
						tokio::task::yield_now().await;
					}
				})
				.await
				.unwrap();
			});
			assert!(uploads.take_notice().is_none());
			assert!(
				matches!(uploads.image_send(&mut state, true), Some(Command::Send { content, .. }) if content.is_empty())
			);
			assert_eq!(state.drafts.get(&channel).map(String::as_str), newer_draft);
			assert_eq!(
				uploads
					.take_source(state.generation, channel)
					.unwrap()
					.len(),
				2
			);
			assert!(uploads.image_send(&mut state, true).is_none());
		}
		println!(
			"Offline artwork composition passed: explicit Send, inline previews, Markdown with text, attachment batches, newer drafts and no restore button while sending."
		);
		return true;
	}
	#[cfg(feature = "demo")]
	if demo && std::env::args().any(|arg| arg == "--demo-check-extensions") {
		demo_check_extensions();
		return true;
	}
	#[cfg(feature = "demo")]
	if demo && std::env::args().any(|arg| arg == "--demo-check-updates") {
		demo_check_updates();
		return true;
	}
	false
}

/// Offline updater flow and settings rendering; never opens an account or installs a package.
#[cfg(feature = "demo")]
fn demo_check_updates() {
	updater::debug_check().expect("offline updater validation");
	let ctx = egui::Context::default();
	ui::fonts::install(&ctx);
	ui::design::apply(&ctx);
	let runtime = tokio::runtime::Builder::new_current_thread()
		.enable_all()
		.build()
		.unwrap();
	let mut updater = updater::Updater::new(true);
	let mut messaging = ui::MessagingUi::default();
	messaging.build.version = env!("CARGO_PKG_VERSION");
	messaging.updates.auto_update = false;
	messaging.updates.check_requested = true;
	assert!(!updater.sync(&ctx, &runtime, &mut messaging.updates, false));
	assert!(messaging.updates.available && !messaging.updates.ready);
	assert!(!messaging.updates.log.is_empty());
	messaging.updates.download_requested = true;
	assert!(!updater.sync(&ctx, &runtime, &mut messaging.updates, false));
	assert!(messaging.updates.ready);
	messaging.updates.restart_requested = true;
	assert!(!updater.sync(&ctx, &runtime, &mut messaging.updates, false));
	let restored: local_store::AppPreferences = serde_json::from_str("{}").unwrap();
	assert!(!restored.auto_update && restored.update_nightly);
	let mut settings = app_settings::Settings::default();
	messaging.updates.nightly = true;
	settings.observe(&messaging);
	let encoded = serde_json::to_string(&settings.current).unwrap();
	settings.current = serde_json::from_str(&encoded).unwrap();
	settings.apply(&mut messaging);
	assert!(!messaging.updates.auto_update && messaging.updates.nightly);
	messaging.open_update_settings();
	let mut state = test_support::demo_state();
	for size in [[1120.0, 760.0], [760.0, 520.0]] {
		for theme in [egui::ThemePreference::Dark, egui::ThemePreference::Light] {
			ctx.set_theme(theme);
			let output = ctx.run_ui(
				egui::RawInput {
					screen_rect: Some(egui::Rect::from_min_size(
						egui::Pos2::ZERO,
						egui::vec2(size[0], size[1]),
					)),
					..Default::default()
				},
				|ui| {
					let _ = messaging.show(ui, &mut state);
				},
			);
			assert!(!output.shapes.is_empty());
			output.drop_without_applying_deltas();
		}
	}
	println!("Offline update flow, preference compatibility, and settings rendering passed.");
}

/// One offline debug path through the shipped Wasm, reducer, and egui rows.
#[cfg(feature = "demo")]
fn demo_check_extensions() {
	let _ = extensions::demo_check_examples().expect("starter packages activate with consent");
	let mut state = test_support::demo_state();
	state.set_preserve_deleted_messages(true);
	let channel = state.selected.expect("demo conversation");
	state.timeline.clear();
	let mut message = test_support::message(600, channel);
	message.content = "A useful message stays readable".into();
	message.attachments.clear();
	message.embeds.clear();
	state.timeline.insert(message.clone(), true, false).unwrap();
	state.apply(Envelope {
		generation: state.generation,
		event: Event::Delete {
			channel,
			id: message.id,
		},
	});
	assert!(state.timeline.is_deleted(message.id));
	assert!(
		state.timeline.get(message.id).is_none(),
		"deleted messages cannot receive service actions"
	);
	assert_eq!(
		state.timeline.get_display(message.id).unwrap().content,
		message.content
	);
	state
		.timeline
		.insert(message.clone(), false, false)
		.unwrap();
	assert!(
		state.timeline.get(message.id).is_none(),
		"stale history cannot resurrect a deletion"
	);
	let ctx = egui::Context::default();
	ui::fonts::install(&ctx);
	ui::design::apply(&ctx);
	let mut messaging = ui::MessagingUi::default();
	let mut saw_deleted = false;
	let mut saw_author = false;
	let mut saw_avatar = false;
	for _ in 0..3 {
		let mut deleted_color = egui::Color32::TRANSPARENT;
		let frame = ctx.run_ui(
			egui::RawInput {
				screen_rect: Some(egui::Rect::from_min_size(
					egui::Pos2::ZERO,
					egui::vec2(1120.0, 760.0),
				)),
				..Default::default()
			},
			|ui| {
				deleted_color = ui::design::palette(ui).danger;
				let _ = messaging.show(ui, &mut state);
			},
		);
		let body_left = frame.shapes.iter().find_map(|shape| match &shape.shape {
			egui::Shape::Text(text) if text.galley.text() == message.content => Some(text.pos.x),
			_ => None,
		});
		for shape in &frame.shapes {
			if let egui::Shape::Text(text) = &shape.shape {
				assert!(
					!text
						.galley
						.text()
						.contains("Deleted - kept by Message delete protector")
				);
				if text.galley.text() == message.content {
					assert!(
						text.galley
							.job
							.sections
							.iter()
							.all(|section| section.format.color == deleted_color)
					);
					saw_deleted = true;
				}
				saw_author |= text.galley.text() == message.author.name
					&& body_left.is_some_and(|left| (text.pos.x - left).abs() < 0.1);
			}
			if let egui::Shape::Rect(image) = &shape.shape
				&& image.brush.is_some()
			{
				let rect = image.rect;
				saw_avatar |= (rect.width() - 40.0).abs() < 0.1
					&& (rect.height() - 40.0).abs() < 0.1
					&& body_left.is_some_and(|left| (rect.right() + 16.0 - left).abs() < 0.1);
			}
		}
		frame.drop_without_applying_deltas();
	}
	assert!(
		saw_deleted && saw_author && saw_avatar,
		"retained row renders red text ({saw_deleted}), author ({saw_author}), and a normal 40-pixel avatar ({saw_avatar})"
	);
	state.discard_preserved_deleted(message.id);
	assert!(
		state.timeline.get_display(message.id).is_none(),
		"local remove drops the retained payload"
	);
	state.set_preserve_deleted_messages(false);
	let next = test_support::message(601, channel);
	state.timeline.insert(next.clone(), true, false).unwrap();
	state.apply(Envelope {
		generation: state.generation,
		event: Event::Delete {
			channel,
			id: next.id,
		},
	});
	assert!(
		state.timeline.get_display(next.id).is_none(),
		"loaded deletes disappear with the extension disabled"
	);
	assert!(state.timeline.get(next.id).is_none());
	println!(
		"Extension debug check passed: starter packages, consent, retained deleted row, stale-history rejection and local remove."
	);
}
