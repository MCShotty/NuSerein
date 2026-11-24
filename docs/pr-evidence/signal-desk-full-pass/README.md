# Signal Desk: screen coverage

Base: `c4c0e2b6` (includes separate Watch and webcam indicators). This is a source inventory plus a native capture plan, not a claim that a person clicked every state. Existing dark conversation, narrow light conversation, appearance and Stage frames were inspected before editing. None of those is an after image.

| Surface | Source finding and disposition |
| --- | --- |
| Sign-in, saved accounts, restore | Long full-width actions can wrap; restore artwork stops moving in reduced-motion or unfocused windows. Credential and verification flows are unchanged. |
| Workspace, Friends, DMs, server/channel navigation | Toggle selection now has a persistent shape, not just a brighter glyph. Quick switcher selection uses the navigation outline. Friends counts above 99 no longer display exactly 99. |
| Conversation, composer, forwarding, reaction menus | Forward targets reserve separate single lines for channel and server with full accessible names. Quick reactions have an explicit focus ring. Immediate hints wrap within the viewport. |
| Profiles, search, media, emoji, GIFs, attachments, voice/video | The same size- and color-aware progress widget replaces 65 spinner call sites in 47 source files, including overlays that paint into an existing media rectangle. It does not start media, change device state, or alter playback. |
| Server menus, moderation, community and Stage | Mod View and event/editor windows fit narrower viewports. Server menu rows have explicit accessible names and focus outlines. Editing an existing Stage topic is secondary to participating. |
| Account, channel, server and extension settings | Compact form rows give controls a separate line. Action labels grow vertically where needed. Dialog footer actions wrap. No controls were removed and the original permission gates remain. |
| Feedback and empty states | Shorter empty-state stacks on compact panels; a single glyph without a decorative backing disc. Toasts respect reduced motion, available width, item count and UTF-8 byte limits. |
| Tray, OS menus, notifications, temp webviews and extension definitions | Included in the source inventory. Platform-owned presentation and authentication handoffs were not redesigned or exercised against a live session. |

## Native verification

The `profile_preview` example exposes its actual accepted routes via `--demo --list-pages`. Linux CI on design PRs requests all 58 routes at their default dimensions, plus eight narrow/light versions (560 × 580 logical points). The `offline-native-design-gallery` artifact contains PNGs and a per-frame manifest. A manifest records capture, not a visual approval. Sign-in, OS-owned dialogs, externally hosted verification and hardware-specific states are not covered by those 58 routes. No live account or device is needed.

Routes: `profile`, `events`, `event-detail`, `event-edit`, `stage`, `stickers`, `slash-commands`, `slash-command-search`, `slash-command-options`, `profile-card`, `collectibles`, `member-tags`, `conversation`, `channels`, `markdown`, `embeds`, `channel-links`, `channel-link-replies`, `dm-tags`, `dm-intro`, `channel-intro`, `post-intro`, `account`, `appearance`, `voice`, `storage`, `general`, `chat`, `language`, `keybinds`, `extensions`, `server`, `server-engagement`, `server-stickers`, `server-safety`, `server-emoji`, `server-members`, `server-roles`, `server-invites`, `server-integrations`, `server-audit-log`, `server-soundboard`, `server-automod`, `server-moderation`, `server-applications`, `server-access`, `server-bans`, `server-mfa`, `mod-view`, `force-sync`, `forum`, `forum-post`, `forum-gallery`, `forum-settings`, `friends`, `screen-share`, `emoji`, `gif-favorites`.

## Source inventory

A structural scan covered 434 first-party presentation, adapter, example, locale and extension source/config files. The list deliberately includes supporting code that owns no screen. The signal column gives concrete widget, dialog or rendering entry points found in a file; it is not a line-by-line manual review. Vendored toolkit internals and binary assets are excluded.

<details><summary>Files by area</summary>


### apps/desktop

| File | Source signals (sample) |
| --- | --- |
| [`apps/desktop/Cargo.toml`](../../../apps/desktop/Cargo.toml) | Support / data |
| [`apps/desktop/build.rs`](../../../apps/desktop/build.rs) | Support / data |
| [`apps/desktop/examples/audio_devices.rs`](../../../apps/desktop/examples/audio_devices.rs) | `Context`, `RawInput`, `Rect`, `Pos2` |
| [`apps/desktop/examples/double_click_settings.rs`](../../../apps/desktop/examples/double_click_settings.rs) | `Context`, `apply`, `RawInput`, `Rect` |
| [`apps/desktop/examples/extension_settings.rs`](../../../apps/desktop/examples/extension_settings.rs) | `Shape`, `Context`, `apply`, `RawInput` |
| [`apps/desktop/examples/fixtures/conversation_qol.rs`](../../../apps/desktop/examples/fixtures/conversation_qol.rs) | Support / data |
| [`apps/desktop/examples/fixtures/native_metrics.rs`](../../../apps/desktop/examples/fixtures/native_metrics.rs) | `Context` |
| [`apps/desktop/examples/focused_keybinds.rs`](../../../apps/desktop/examples/focused_keybinds.rs) | `Context`, `Modifiers`, `RawInput`, `Key` |
| [`apps/desktop/examples/gif_favorites.rs`](../../../apps/desktop/examples/gif_favorites.rs) | Support / data |
| [`apps/desktop/examples/group_call.rs`](../../../apps/desktop/examples/group_call.rs) | `Context`, `apply`, `RawInput`, `Rect` |
| [`apps/desktop/examples/profile_preview.rs`](../../../apps/desktop/examples/profile_preview.rs) | `ui`, `channel_settings`, `apply_admin_preview`, `non_theme_catalog_loads_community_preview` |
| [`apps/desktop/examples/protector_lifecycle.rs`](../../../apps/desktop/examples/protector_lifecycle.rs) | `Context` |
| [`apps/desktop/examples/scrolling_preferences.rs`](../../../apps/desktop/examples/scrolling_preferences.rs) | `MouseWheelUnit`, `Context`, `apply`, `RawInput` |
| [`apps/desktop/examples/spotify_activity.rs`](../../../apps/desktop/examples/spotify_activity.rs) | `Context`, `apply`, `RawInput`, `Rect` |
| [`apps/desktop/examples/video_controls.rs`](../../../apps/desktop/examples/video_controls.rs) | `Context`, `Event`, `FullOutput`, `RawInput` |
| [`apps/desktop/examples/video_formats.rs`](../../../apps/desktop/examples/video_formats.rs) | Support / data |
| [`apps/desktop/examples/video_orientation.rs`](../../../apps/desktop/examples/video_orientation.rs) | Support / data |
| [`apps/desktop/src/access_marks_demo.rs`](../../../apps/desktop/src/access_marks_demo.rs) | `Shape`, `Context`, `apply`, `RawInput` |
| [`apps/desktop/src/app_settings.rs`](../../../apps/desktop/src/app_settings.rs) | `maximize_after_show`, `video_settings_are_observed_and_restored`, `legacy_preferences_without_voice_settings_keep_suppression_disabled` |
| [`apps/desktop/src/audio.rs`](../../../apps/desktop/src/audio.rs) | `check_ogg_headers`, `ffmpeg_audio_selection_requires_a_decoder_and_valid_headers`, `decode_bounded_audio_and_reject_malformed_headers` |
| [`apps/desktop/src/audio/source.rs`](../../../apps/desktop/src/audio/source.rs) | Support / data |
| [`apps/desktop/src/audio/streaming.rs`](../../../apps/desktop/src/audio/streaming.rs) | Support / data |
| [`apps/desktop/src/audio/stretch.rs`](../../../apps/desktop/src/audio/stretch.rs) | Support / data |
| [`apps/desktop/src/avatars.rs`](../../../apps/desktop/src/avatars.rs) | `open_preview_decoder`, `guide_artwork_urls_are_confined_and_headers_use_the_bounded_embed_edge`, `video_previews_use_ffmpeg_with_bounded_frames_and_deadlines` |
| [`apps/desktop/src/cache.rs`](../../../apps/desktop/src/cache.rs) | `item_pressure_evicts_inactive_history_and_keeps_current_pages_and_edits`, `cached_older_page_remains_pending_and_service_reconciles_its_covered_rows`, `scoped_sync_pages_follow_parent_cleanup_and_preserve_other_scopes`, `background_sync_pages_keep_correlation_and_reject_live_edit_races` |
| [`apps/desktop/src/captcha.rs`](../../../apps/desktop/src/captcha.rs) | `Context`, `Theme` |
| [`apps/desktop/src/channel_demo.rs`](../../../apps/desktop/src/channel_demo.rs) | `forum_settings` |
| [`apps/desktop/src/clipboard.rs`](../../../apps/desktop/src/clipboard.rs) | `Context`, `ColorImage`, `Id`, `Color32` |
| [`apps/desktop/src/community_admin_demo.rs`](../../../apps/desktop/src/community_admin_demo.rs) | Support / data |
| [`apps/desktop/src/community_demo.rs`](../../../apps/desktop/src/community_demo.rs) | Support / data |
| [`apps/desktop/src/components_demo.rs`](../../../apps/desktop/src/components_demo.rs) | `Context`, `apply`, `Shape`, `Rect` |
| [`apps/desktop/src/connection.rs`](../../../apps/desktop/src/connection.rs) | `stream_preview_session_failure`, `stream_preview_capacity_is_local_but_auth_failures_end_the_session` |
| [`apps/desktop/src/connection/events.rs`](../../../apps/desktop/src/connection/events.rs) | `Context` |
| [`apps/desktop/src/connection/lanes.rs`](../../../apps/desktop/src/connection/lanes.rs) | Support / data |
| [`apps/desktop/src/connection/presence.rs`](../../../apps/desktop/src/connection/presence.rs) | `Context` |
| [`apps/desktop/src/connection/session.rs`](../../../apps/desktop/src/connection/session.rs) | `Context` |
| [`apps/desktop/src/connection/voice.rs`](../../../apps/desktop/src/connection/voice.rs) | `Context` |
| [`apps/desktop/src/credentials.rs`](../../../apps/desktop/src/credentials.rs) | `Context` |
| [`apps/desktop/src/desktop_commands.rs`](../../../apps/desktop/src/desktop_commands.rs) | `Level` |
| [`apps/desktop/src/desktop_events.rs`](../../../apps/desktop/src/desktop_events.rs) | `Context`, `ThemePreference`, `Variant`, `set_variant` |
| [`apps/desktop/src/desktop_frame.rs`](../../../apps/desktop/src/desktop_frame.rs) | `Context`, `Ui`, `TITLE_BAR_HEIGHT`, `paint_backdrop` |
| [`apps/desktop/src/desktop_history.rs`](../../../apps/desktop/src/desktop_history.rs) | `accept_cached_history_page` |
| [`apps/desktop/src/desktop_preferences.rs`](../../../apps/desktop/src/desktop_preferences.rs) | `Context`, `Theme`, `primary_color`, `set_primary_color` |
| [`apps/desktop/src/desktop_presence.rs`](../../../apps/desktop/src/desktop_presence.rs) | `Context`, `Level` |
| [`apps/desktop/src/desktop_startup.rs`](../../../apps/desktop/src/desktop_startup.rs) | `install`, `Variant`, `set_variant`, `apply` |
| [`apps/desktop/src/desktop_window.rs`](../../../apps/desktop/src/desktop_window.rs) | `app_menu` |
| [`apps/desktop/src/dm_demo.rs`](../../../apps/desktop/src/dm_demo.rs) | `Context`, `Event`, `RawInput`, `Rect` |
| [`apps/desktop/src/downloads.rs`](../../../apps/desktop/src/downloads.rs) | `Context` |
| [`apps/desktop/src/emoji_upload.rs`](../../../apps/desktop/src/emoji_upload.rs) | `ColorImage`, `Context` |
| [`apps/desktop/src/extension_app.rs`](../../../apps/desktop/src/extension_app.rs) | `messaging_settings_snapshot` |
| [`apps/desktop/src/extension_bridge.rs`](../../../apps/desktop/src/extension_bridge.rs) | `cancel_previews` |
| [`apps/desktop/src/extension_data_events.rs`](../../../apps/desktop/src/extension_data_events.rs) | Support / data |
| [`apps/desktop/src/extension_events.rs`](../../../apps/desktop/src/extension_events.rs) | Support / data |
| [`apps/desktop/src/extension_forum_data.rs`](../../../apps/desktop/src/extension_forum_data.rs) | Support / data |
| [`apps/desktop/src/extension_member_details.rs`](../../../apps/desktop/src/extension_member_details.rs) | Support / data |
| [`apps/desktop/src/extension_message_content.rs`](../../../apps/desktop/src/extension_message_content.rs) | Support / data |
| [`apps/desktop/src/extensions.rs`](../../../apps/desktop/src/extensions.rs) | `load_preview`, `cached_preview`, `remember_preview`, `decode_preview` |
| [`apps/desktop/src/font_import.rs`](../../../apps/desktop/src/font_import.rs) | `Context`, `RawInput` |
| [`apps/desktop/src/force_sync.rs`](../../../apps/desktop/src/force_sync.rs) | `older_history_pages_ignore_newer_edits_and_keep_continuation_fences` |
| [`apps/desktop/src/game_activity.rs`](../../../apps/desktop/src/game_activity.rs) | `Context` |
| [`apps/desktop/src/game_activity/custom.rs`](../../../apps/desktop/src/game_activity/custom.rs) | Support / data |
| [`apps/desktop/src/game_mentions.rs`](../../../apps/desktop/src/game_mentions.rs) | `Context` |
| [`apps/desktop/src/gpu.rs`](../../../apps/desktop/src/gpu.rs) | Support / data |
| [`apps/desktop/src/gpu_video.rs`](../../../apps/desktop/src/gpu_video.rs) | `mutex`, `TextureId`, `TextureHandle`, `Vec2` |
| [`apps/desktop/src/group_demo.rs`](../../../apps/desktop/src/group_demo.rs) | Support / data |
| [`apps/desktop/src/group_icon.rs`](../../../apps/desktop/src/group_icon.rs) | `stale_picker_retains_single_slot_until_completion_and_discards_result` |
| [`apps/desktop/src/inbox_demo.rs`](../../../apps/desktop/src/inbox_demo.rs) | Support / data |
| [`apps/desktop/src/interaction_uploads.rs`](../../../apps/desktop/src/interaction_uploads.rs) | `Context` |
| [`apps/desktop/src/main.rs`](../../../apps/desktop/src/main.rs) | `ui`, `disk_cache_does_not_replace_resident_previews_or_deleted_positions`, `an_adopted_preload_page_is_saved_like_the_clicks_own`, `attachment_sends_stay_allowed_while_the_latest_page_reloads` |
| [`apps/desktop/src/media_output.rs`](../../../apps/desktop/src/media_output.rs) | Support / data |
| [`apps/desktop/src/member_cache_search.rs`](../../../apps/desktop/src/member_cache_search.rs) | Support / data |
| [`apps/desktop/src/message_speech.rs`](../../../apps/desktop/src/message_speech.rs) | `Context` |
| [`apps/desktop/src/notification_runtime.rs`](../../../apps/desktop/src/notification_runtime.rs) | `Context` |
| [`apps/desktop/src/notification_sounds.rs`](../../../apps/desktop/src/notification_sounds.rs) | `Context` |
| [`apps/desktop/src/offline_checks.rs`](../../../apps/desktop/src/offline_checks.rs) | `debug_slider_check`, `Context`, `apply`, `ThemePreference` |
| [`apps/desktop/src/ogg_opus.rs`](../../../apps/desktop/src/ogg_opus.rs) | Support / data |
| [`apps/desktop/src/onboarding_demo.rs`](../../../apps/desktop/src/onboarding_demo.rs) | Support / data |
| [`apps/desktop/src/pointer.rs`](../../../apps/desktop/src/pointer.rs) | Support / data |
| [`apps/desktop/src/polls_demo.rs`](../../../apps/desktop/src/polls_demo.rs) | Support / data |
| [`apps/desktop/src/post_menu_demo.rs`](../../../apps/desktop/src/post_menu_demo.rs) | `Shape`, `Rect`, `Context`, `apply` |
| [`apps/desktop/src/profile_settings_demo.rs`](../../../apps/desktop/src/profile_settings_demo.rs) | Support / data |
| [`apps/desktop/src/proxy_auth.rs`](../../../apps/desktop/src/proxy_auth.rs) | `Context` |
| [`apps/desktop/src/reading_settings.rs`](../../../apps/desktop/src/reading_settings.rs) | Support / data |
| [`apps/desktop/src/registered_games.rs`](../../../apps/desktop/src/registered_games.rs) | `Context` |
| [`apps/desktop/src/rendering_demo.rs`](../../../apps/desktop/src/rendering_demo.rs) | `show`, `paint` |
| [`apps/desktop/src/screen.rs`](../../../apps/desktop/src/screen.rs) | `share_settings` |
| [`apps/desktop/src/server_settings_demo.rs`](../../../apps/desktop/src/server_settings_demo.rs) | Support / data |
| [`apps/desktop/src/sign_in.rs`](../../../apps/desktop/src/sign_in.rs) | `restoring_screen`, `sign_in_screen`, `sign_in_header`, `confirm_forget_dialog` |
| [`apps/desktop/src/slash_demo.rs`](../../../apps/desktop/src/slash_demo.rs) | Support / data |
| [`apps/desktop/src/sound_upload.rs`](../../../apps/desktop/src/sound_upload.rs) | `Context` |
| [`apps/desktop/src/soundboard.rs`](../../../apps/desktop/src/soundboard.rs) | `Context` |
| [`apps/desktop/src/spotify.rs`](../../../apps/desktop/src/spotify.rs) | `Context` |
| [`apps/desktop/src/startup.rs`](../../../apps/desktop/src/startup.rs) | `Context` |
| [`apps/desktop/src/sticker_upload.rs`](../../../apps/desktop/src/sticker_upload.rs) | `ColorImage`, `Context` |
| [`apps/desktop/src/toggle_setting.rs`](../../../apps/desktop/src/toggle_setting.rs) | Support / data |
| [`apps/desktop/src/tray_window.rs`](../../../apps/desktop/src/tray_window.rs) | `show`, `ui` |
| [`apps/desktop/src/updater.rs`](../../../apps/desktop/src/updater.rs) | `Context` |
| [`apps/desktop/src/updater_delta.rs`](../../../apps/desktop/src/updater_delta.rs) | Support / data |
| [`apps/desktop/src/updater_install.rs`](../../../apps/desktop/src/updater_install.rs) | `appimage_header` |
| [`apps/desktop/src/uploads.rs`](../../../apps/desktop/src/uploads.rs) | `decode_preview`, `select_pasted_with_previews`, `open_picker_blocks_competing_selections_until_its_count_is_known`, `cancelled_open_picker_blocks_new_selections_until_it_returns` |
| [`apps/desktop/src/video.rs`](../../../apps/desktop/src/video.rs) | `paint` |
| [`apps/desktop/src/video/fallback.rs`](../../../apps/desktop/src/video/fallback.rs) | Support / data |
| [`apps/desktop/src/video/media.wgsl`](../../../apps/desktop/src/video/media.wgsl) | Support / data |
| [`apps/desktop/src/video/output.rs`](../../../apps/desktop/src/video/output.rs) | Support / data |
| [`apps/desktop/src/video/source.rs`](../../../apps/desktop/src/video/source.rs) | `web_page` |
| [`apps/desktop/src/video/source/social.rs`](../../../apps/desktop/src/video/source/social.rs) | Support / data |
| [`apps/desktop/src/video_capabilities.rs`](../../../apps/desktop/src/video_capabilities.rs) | `cancellation_while_settings_remain_visible_also_restarts_detection` |
| [`apps/desktop/src/voice.rs`](../../../apps/desktop/src/voice.rs) | `stop_mic_preview`, `poll_mic_preview`, `camera_preview_allowed`, `debug_mic_preview_check` |
| [`apps/desktop/src/voice_messages.rs`](../../../apps/desktop/src/voice_messages.rs) | `Context` |
| [`apps/desktop/src/watch.rs`](../../../apps/desktop/src/watch.rs) | `Context` |

### crates/extensions

| File | Source signals (sample) |
| --- | --- |
| [`crates/extensions/Cargo.toml`](../../../crates/extensions/Cargo.toml) | Support / data |
| [`crates/extensions/examples/benchmark.rs`](../../../crates/extensions/examples/benchmark.rs) | Support / data |
| [`crates/extensions/examples/legacy_sdk_check.rs`](../../../crates/extensions/examples/legacy_sdk_check.rs) | Support / data |
| [`crates/extensions/examples/manifest_check.rs`](../../../crates/extensions/examples/manifest_check.rs) | Support / data |
| [`crates/extensions/examples/panel_api.rs`](../../../crates/extensions/examples/panel_api.rs) | Support / data |
| [`crates/extensions/examples/sdk_check.rs`](../../../crates/extensions/examples/sdk_check.rs) | Support / data |
| [`crates/extensions/examples/theme_import.rs`](../../../crates/extensions/examples/theme_import.rs) | Support / data |
| [`crates/extensions/src/app.rs`](../../../crates/extensions/src/app.rs) | `server_settings_patch` |
| [`crates/extensions/src/channel_metadata.rs`](../../../crates/extensions/src/channel_metadata.rs) | Support / data |
| [`crates/extensions/src/conversation_activity.rs`](../../../crates/extensions/src/conversation_activity.rs) | Support / data |
| [`crates/extensions/src/discovery.rs`](../../../crates/extensions/src/discovery.rs) | Support / data |
| [`crates/extensions/src/extended.rs`](../../../crates/extensions/src/extended.rs) | Support / data |
| [`crates/extensions/src/forum_data.rs`](../../../crates/extensions/src/forum_data.rs) | Support / data |
| [`crates/extensions/src/lib.rs`](../../../crates/extensions/src/lib.rs) | Support / data |
| [`crates/extensions/src/member_details.rs`](../../../crates/extensions/src/member_details.rs) | Support / data |
| [`crates/extensions/src/message_content.rs`](../../../crates/extensions/src/message_content.rs) | Support / data |
| [`crates/extensions/src/rich_presence.rs`](../../../crates/extensions/src/rich_presence.rs) | Support / data |
| [`crates/extensions/src/runtime.rs`](../../../crates/extensions/src/runtime.rs) | Support / data |
| [`crates/extensions/src/validation.rs`](../../../crates/extensions/src/validation.rs) | Support / data |
| [`crates/extensions/src/voice_messages.rs`](../../../crates/extensions/src/voice_messages.rs) | Support / data |
| [`crates/extensions/tests/api_proxy.rs`](../../../crates/extensions/tests/api_proxy.rs) | Support / data |
| [`crates/extensions/tests/app_contract.rs`](../../../crates/extensions/tests/app_contract.rs) | `local_settings_accept_smaller_zoom_and_reject_below_supported_range` |
| [`crates/extensions/tests/sandbox.rs`](../../../crates/extensions/tests/sandbox.rs) | `rejects_panel_complexity_duplicate_ids_and_unknown_actions`, `catalog_preview_metadata_is_optional_and_bounded` |
| [`crates/extensions/tests/sdk.rs`](../../../crates/extensions/tests/sdk.rs) | `every_sdk_panel_and_appearance_field_is_accepted_by_the_host` |
| [`crates/extensions/tests/voice_messages.rs`](../../../crates/extensions/tests/voice_messages.rs) | `native_configuration_requires_opt_in_and_panel_or_activation`, `committed_plugin_runs_only_inside_bounded_sandbox_and_returns_settings` |

### crates/platform

| File | Source signals (sample) |
| --- | --- |
| [`crates/platform/Cargo.toml`](../../../crates/platform/Cargo.toml) | Support / data |
| [`crates/platform/build.rs`](../../../crates/platform/build.rs) | Support / data |
| [`crates/platform/src/app_menu.rs`](../../../crates/platform/src/app_menu.rs) | `submenu` |
| [`crates/platform/src/badge.rs`](../../../crates/platform/src/badge.rs) | Support / data |
| [`crates/platform/src/captcha-linux-bridge.js`](../../../crates/platform/src/captcha-linux-bridge.js) | Support / data |
| [`crates/platform/src/captcha.html`](../../../crates/platform/src/captcha.html) | Support / data |
| [`crates/platform/src/captcha.js`](../../../crates/platform/src/captcha.js) | Support / data |
| [`crates/platform/src/captcha.rs`](../../../crates/platform/src/captcha.rs) | `verification_storage_requires_the_expected_page_and_hcaptcha`, `ipc_is_restricted_to_the_local_verification_page`, `native_local_page_and_ipc` |
| [`crates/platform/src/captcha_linux.rs`](../../../crates/platform/src/captcha_linux.rs) | `handoff_requires_local_page_capability_and_live_single_use_session` |
| [`crates/platform/src/compositor.rs`](../../../crates/platform/src/compositor.rs) | `show` |
| [`crates/platform/src/compositor/kwin.rs`](../../../crates/platform/src/compositor/kwin.rs) | Support / data |
| [`crates/platform/src/embed_player.html`](../../../crates/platform/src/embed_player.html) | Support / data |
| [`crates/platform/src/embed_player.rs`](../../../crates/platform/src/embed_player.rs) | `player_events_are_bounded_and_page_load_does_not_establish_playback` |
| [`crates/platform/src/embed_player_linux.rs`](../../../crates/platform/src/embed_player_linux.rs) | Support / data |
| [`crates/platform/src/embed_player_macos.rs`](../../../crates/platform/src/embed_player_macos.rs) | Support / data |
| [`crates/platform/src/game_activity.rs`](../../../crates/platform/src/game_activity.rs) | Support / data |
| [`crates/platform/src/heic.rs`](../../../crates/platform/src/heic.rs) | Support / data |
| [`crates/platform/src/hotkeys.rs`](../../../crates/platform/src/hotkeys.rs) | Support / data |
| [`crates/platform/src/lib.rs`](../../../crates/platform/src/lib.rs) | Support / data |
| [`crates/platform/src/login-handoff.js`](../../../crates/platform/src/login-handoff.js) | Support / data |
| [`crates/platform/src/login-linux-bridge.js`](../../../crates/platform/src/login-linux-bridge.js) | Support / data |
| [`crates/platform/src/login_keyring.rs`](../../../crates/platform/src/login_keyring.rs) | Support / data |
| [`crates/platform/src/login_linux.rs`](../../../crates/platform/src/login_linux.rs) | Support / data |
| [`crates/platform/src/memory.rs`](../../../crates/platform/src/memory.rs) | Support / data |
| [`crates/platform/src/message_speech.rs`](../../../crates/platform/src/message_speech.rs) | Support / data |
| [`crates/platform/src/notifications.rs`](../../../crates/platform/src/notifications.rs) | `show`, `missing_windows_settings_entry_allows_delivery_but_denials_do_not` |
| [`crates/platform/src/plaintext_login.rs`](../../../crates/platform/src/plaintext_login.rs) | Support / data |
| [`crates/platform/src/pointer.rs`](../../../crates/platform/src/pointer.rs) | Support / data |
| [`crates/platform/src/processes.rs`](../../../crates/platform/src/processes.rs) | Support / data |
| [`crates/platform/src/proxy_credentials.rs`](../../../crates/platform/src/proxy_credentials.rs) | Support / data |
| [`crates/platform/src/save.rs`](../../../crates/platform/src/save.rs) | Support / data |
| [`crates/platform/src/single_instance.rs`](../../../crates/platform/src/single_instance.rs) | `take_show_request` |
| [`crates/platform/src/speech.rs`](../../../crates/platform/src/speech.rs) | Support / data |
| [`crates/platform/src/startup.rs`](../../../crates/platform/src/startup.rs) | Support / data |
| [`crates/platform/src/startup/macos.rs`](../../../crates/platform/src/startup/macos.rs) | Support / data |
| [`crates/platform/src/system_theme.rs`](../../../crates/platform/src/system_theme.rs) | `gsettings_dark` |
| [`crates/platform/src/tray.rs`](../../../crates/platform/src/tray.rs) | Support / data |
| [`crates/platform/src/tray/linux.rs`](../../../crates/platform/src/tray/linux.rs) | Support / data |
| [`crates/platform/src/tray/macos.rs`](../../../crates/platform/src/tray/macos.rs) | `show` |
| [`crates/platform/src/video/apple.rs`](../../../crates/platform/src/video/apple.rs) | Support / data |
| [`crates/platform/src/video/capture.rs`](../../../crates/platform/src/video/capture.rs) | `serein_capture_frame_preview` |
| [`crates/platform/src/video/capture_windows.c`](../../../crates/platform/src/video/capture_windows.c) | Support / data |
| [`crates/platform/src/video/capture_windows.h`](../../../crates/platform/src/video/capture_windows.h) | Support / data |
| [`crates/platform/src/video/denoise.rs`](../../../crates/platform/src/video/denoise.rs) | Support / data |
| [`crates/platform/src/video/denoise/camera_filter.wgsl`](../../../crates/platform/src/video/denoise/camera_filter.wgsl) | Support / data |
| [`crates/platform/src/video/denoise/gpu.rs`](../../../crates/platform/src/video/denoise/gpu.rs) | Support / data |
| [`crates/platform/src/video/ffmpeg.rs`](../../../crates/platform/src/video/ffmpeg.rs) | `serein_decode_header` |
| [`crates/platform/src/video/ffmpeg/audio.rs`](../../../crates/platform/src/video/ffmpeg/audio.rs) | Support / data |
| [`crates/platform/src/video/ffmpeg/decode.rs`](../../../crates/platform/src/video/ffmpeg/decode.rs) | Support / data |
| [`crates/platform/src/video/ffmpeg/media.rs`](../../../crates/platform/src/video/ffmpeg/media.rs) | Support / data |
| [`crates/platform/src/video/gst.rs`](../../../crates/platform/src/video/gst.rs) | Support / data |
| [`crates/platform/src/video/live_apple.rs`](../../../crates/platform/src/video/live_apple.rs) | Support / data |
| [`crates/platform/src/video/live_gst.rs`](../../../crates/platform/src/video/live_gst.rs) | Support / data |
| [`crates/platform/src/video/live_windows.rs`](../../../crates/platform/src/video/live_windows.rs) | Support / data |
| [`crates/platform/src/video/media_foundation.rs`](../../../crates/platform/src/video/media_foundation.rs) | Support / data |
| [`crates/platform/src/video/mod.rs`](../../../crates/platform/src/video/mod.rs) | Support / data |
| [`crates/platform/src/video/mp4.rs`](../../../crates/platform/src/video/mp4.rs) | `parse_header_times`, `truncated_avcc_header_is_rejected` |
| [`crates/platform/src/video/picture.rs`](../../../crates/platform/src/video/picture.rs) | Support / data |
| [`crates/platform/src/video/video_decode_ffmpeg.c`](../../../crates/platform/src/video/video_decode_ffmpeg.c) | Support / data |
| [`crates/platform/src/video/video_denoise.c`](../../../crates/platform/src/video/video_denoise.c) | Support / data |
| [`crates/platform/src/video/video_encode_ffmpeg.c`](../../../crates/platform/src/video/video_encode_ffmpeg.c) | Support / data |
| [`crates/platform/src/video/video_encode_ffmpeg.h`](../../../crates/platform/src/video/video_encode_ffmpeg.h) | Support / data |
| [`crates/platform/src/video/video_gpu.c`](../../../crates/platform/src/video/video_gpu.c) | Support / data |
| [`crates/platform/src/video/video_query.c`](../../../crates/platform/src/video/video_query.c) | Support / data |
| [`crates/platform/src/video/video_query_amf.cpp`](../../../crates/platform/src/video/video_query_amf.cpp) | Support / data |
| [`crates/platform/src/window.rs`](../../../crates/platform/src/window.rs) | Support / data |
| [`crates/platform/src/window_effects.rs`](../../../crates/platform/src/window_effects.rs) | Support / data |
| [`crates/platform/src/window_effects/dwm.rs`](../../../crates/platform/src/window_effects/dwm.rs) | Support / data |
| [`crates/platform/src/window_effects/macos.rs`](../../../crates/platform/src/window_effects/macos.rs) | Support / data |
| [`crates/platform/src/window_effects/wayland.rs`](../../../crates/platform/src/window_effects/wayland.rs) | Support / data |
| [`crates/platform/src/window_effects/x11.rs`](../../../crates/platform/src/window_effects/x11.rs) | Support / data |

### crates/ui

| File | Source signals (sample) |
| --- | --- |
| [`crates/ui/Cargo.toml`](../../../crates/ui/Cargo.toml) | Support / data |
| [`crates/ui/examples/account_presence.rs`](../../../crates/ui/examples/account_presence.rs) | `Shape`, `Context`, `RawInput`, `Rect` |
| [`crates/ui/examples/autoscroll.rs`](../../../crates/ui/examples/autoscroll.rs) | `Shape`, `Rect`, `Pos2`, `FullOutput` |
| [`crates/ui/examples/chat_first_paint.rs`](../../../crates/ui/examples/chat_first_paint.rs) | `Shape`, `Context`, `RawInput`, `Rect` |
| [`crates/ui/examples/composer_focus.rs`](../../../crates/ui/examples/composer_focus.rs) | `Context`, `FullOutput`, `RawInput`, `Rect` |
| [`crates/ui/examples/composer_scroll.rs`](../../../crates/ui/examples/composer_scroll.rs) | `Context`, `apply`, `RawInput`, `Rect` |
| [`crates/ui/examples/copy_feedback.rs`](../../../crates/ui/examples/copy_feedback.rs) | Support / data |
| [`crates/ui/examples/direct_message_navigation.rs`](../../../crates/ui/examples/direct_message_navigation.rs) | Support / data |
| [`crates/ui/examples/friend_message.rs`](../../../crates/ui/examples/friend_message.rs) | `Context`, `RawInput`, `Rect`, `vec2` |
| [`crates/ui/examples/friends_idle.rs`](../../../crates/ui/examples/friends_idle.rs) | `Context`, `apply`, `RawInput`, `Rect` |
| [`crates/ui/examples/gif_hover.rs`](../../../crates/ui/examples/gif_hover.rs) | `Context`, `vec2`, `Event`, `Shape` |
| [`crates/ui/examples/image_resolution.rs`](../../../crates/ui/examples/image_resolution.rs) | `Context`, `RawInput`, `Rect`, `Pos2` |
| [`crates/ui/examples/language_time.rs`](../../../crates/ui/examples/language_time.rs) | `Context`, `apply`, `RawInput`, `Rect` |
| [`crates/ui/examples/member_flicker.rs`](../../../crates/ui/examples/member_flicker.rs) | `Shape`, `Pos2`, `Context`, `RawInput` |
| [`crates/ui/examples/member_paging.rs`](../../../crates/ui/examples/member_paging.rs) | `Context`, `RawInput`, `Rect`, `Pos2` |
| [`crates/ui/examples/member_voice_status.rs`](../../../crates/ui/examples/member_voice_status.rs) | Support / data |
| [`crates/ui/examples/profile_board.rs`](../../../crates/ui/examples/profile_board.rs) | `vec2`, `Context`, `apply`, `Id` |
| [`crates/ui/examples/profile_layout.rs`](../../../crates/ui/examples/profile_layout.rs) | `Context`, `apply`, `RawInput`, `Rect` |
| [`crates/ui/examples/reaction_loading.rs`](../../../crates/ui/examples/reaction_loading.rs) | `Shape`, `Context`, `RawInput`, `Rect` |
| [`crates/ui/examples/role_mentions.rs`](../../../crates/ui/examples/role_mentions.rs) | Support / data |
| [`crates/ui/examples/solo_speaking.rs`](../../../crates/ui/examples/solo_speaking.rs) | `Context`, `apply`, `RawInput`, `Rect` |
| [`crates/ui/examples/theme_api.rs`](../../../crates/ui/examples/theme_api.rs) | `Context`, `set_extension_theme`, `window_effects`, `set_window_effects` |
| [`crates/ui/examples/thread_member_groups.rs`](../../../crates/ui/examples/thread_member_groups.rs) | `Shape`, `Context`, `apply`, `Rect` |
| [`crates/ui/examples/thread_navigation.rs`](../../../crates/ui/examples/thread_navigation.rs) | Support / data |
| [`crates/ui/examples/unread_navigation.rs`](../../../crates/ui/examples/unread_navigation.rs) | Support / data |
| [`crates/ui/examples/voice_chat.rs`](../../../crates/ui/examples/voice_chat.rs) | `Context`, `Event`, `Rect`, `RawInput` |
| [`crates/ui/locales/cs/main.ftl`](../../../crates/ui/locales/cs/main.ftl) | Support / data |
| [`crates/ui/locales/de/main.ftl`](../../../crates/ui/locales/de/main.ftl) | Support / data |
| [`crates/ui/locales/en-US/main.ftl`](../../../crates/ui/locales/en-US/main.ftl) | Support / data |
| [`crates/ui/locales/es/main.ftl`](../../../crates/ui/locales/es/main.ftl) | Support / data |
| [`crates/ui/locales/fr/main.ftl`](../../../crates/ui/locales/fr/main.ftl) | Support / data |
| [`crates/ui/locales/it/main.ftl`](../../../crates/ui/locales/it/main.ftl) | Support / data |
| [`crates/ui/locales/ja/main.ftl`](../../../crates/ui/locales/ja/main.ftl) | Support / data |
| [`crates/ui/locales/pl/main.ftl`](../../../crates/ui/locales/pl/main.ftl) | Support / data |
| [`crates/ui/locales/pt-BR/main.ftl`](../../../crates/ui/locales/pt-BR/main.ftl) | Support / data |
| [`crates/ui/locales/ru/main.ftl`](../../../crates/ui/locales/ru/main.ftl) | Support / data |
| [`crates/ui/locales/tr/main.ftl`](../../../crates/ui/locales/tr/main.ftl) | Support / data |
| [`crates/ui/locales/zh-CN/main.ftl`](../../../crates/ui/locales/zh-CN/main.ftl) | Support / data |
| [`crates/ui/locales/zh-TW/main.ftl`](../../../crates/ui/locales/zh-TW/main.ftl) | Support / data |
| [`crates/ui/src/account_badge.rs`](../../../crates/ui/src/account_badge.rs) | `palette`, `FontId`, `medium_family`, `paint` |
| [`crates/ui/src/account_menu.rs`](../../../crates/ui/src/account_menu.rs) | `account_menu`, `account_menu_contents`, `presence_menu`, `account_menu_opens_applies_clears_and_closes_across_themes_and_sizes` |
| [`crates/ui/src/account_settings.rs`](../../../crates/ui/src/account_settings.rs) | `connection_settings` |
| [`crates/ui/src/activity_alerts.rs`](../../../crates/ui/src/activity_alerts.rs) | `show` |
| [`crates/ui/src/announcements.rs`](../../../crates/ui/src/announcements.rs) | `show` |
| [`crates/ui/src/appearance_transparency_tests.rs`](../../../crates/ui/src/appearance_transparency_tests.rs) | `appearance_settings_and_mode_previews_keep_their_colors_at_every_window_opacity` |
| [`crates/ui/src/archives.rs`](../../../crates/ui/src/archives.rs) | `show`, `archive_search_distinguishes_no_matches_from_an_empty_page` |
| [`crates/ui/src/attachments.rs`](../../../crates/ui/src/attachments.rs) | `show`, `media_context_menu`, `embed_context_menu`, `media_menu` |
| [`crates/ui/src/attachments/context_tests.rs`](../../../crates/ui/src/attachments/context_tests.rs) | `select_media_menu`, `image_menus_copy_the_owning_message_id_in_the_card_and_viewer`, `image_video_and_viewer_context_menus_copy_or_save_the_attachment`, `media_context_menus_disable_transfers_in_demo_and_while_busy` |
| [`crates/ui/src/audio.rs`](../../../crates/ui/src/audio.rs) | `show`, `a_voice_message_drawn_twice_has_no_widget_id_clash` |
| [`crates/ui/src/avatars.rs`](../../../crates/ui/src/avatars.rs) | `warm_preview`, `show_guide_header`, `paint`, `show` |
| [`crates/ui/src/avatars/cosmetics.rs`](../../../crates/ui/src/avatars/cosmetics.rs) | `ColorImage`, `Mesh`, `Context`, `Ui` |
| [`crates/ui/src/avatars/media.rs`](../../../crates/ui/src/avatars/media.rs) | `offscreen_still_expires_and_can_be_requested_again` |
| [`crates/ui/src/badges.rs`](../../../crates/ui/src/badges.rs) | `show`, `customize_menu`, `saving_locks_badge_edits_and_reopened_menu_receives_confirmation` |
| [`crates/ui/src/browse_channels.rs`](../../../crates/ui/src/browse_channels.rs) | `show`, `open_menu`, `browse_rows_reuse_personal_menus_and_dispatch_the_exact_channel_once`, `browse_menu_rechecks_channel_access_before_hiding` |
| [`crates/ui/src/categories.rs`](../../../crates/ui/src/categories.rs) | `category_header`, `category_header_preserves_original_case` |
| [`crates/ui/src/channel_marks.rs`](../../../crates/ui/src/channel_marks.rs) | `paint` |
| [`crates/ui/src/channel_menu.rs`](../../../crates/ui/src/channel_menu.rs) | `preview_settings`, `show`, `leaving_channel_settings_retires_its_inflight_read`, `settings_pages_keep_one_modal_size_and_align_permission_toggles` |
| [`crates/ui/src/channel_permissions.rs`](../../../crates/ui/src/channel_permissions.rs) | `show` |
| [`crates/ui/src/channel_pill.rs`](../../../crates/ui/src/channel_pill.rs) | `show` |
| [`crates/ui/src/channel_welcome_tests.rs`](../../../crates/ui/src/channel_welcome_tests.rs) | `direct_message_introduction_shows_a_known_username_only_when_it_differs` |
| [`crates/ui/src/command_permissions.rs`](../../../crates/ui/src/command_permissions.rs) | `show` |
| [`crates/ui/src/community.rs`](../../../crates/ui/src/community.rs) | `show`, `stage_dialog` |
| [`crates/ui/src/community_admin.rs`](../../../crates/ui/src/community_admin.rs) | `show`, `channel_picker`, `role_picker`, `emoji_picker` |
| [`crates/ui/src/components.rs`](../../../crates/ui/src/components.rs) | `show` |
| [`crates/ui/src/composer.rs`](../../../crates/ui/src/composer.rs) | `retire_menu_paste` |
| [`crates/ui/src/composer_text.rs`](../../../crates/ui/src/composer_text.rs) | `paint`, `cached_server_nickname_is_drawn_while_the_draft_keeps_the_user_id` |
| [`crates/ui/src/contact_editor.rs`](../../../crates/ui/src/contact_editor.rs) | `show` |
| [`crates/ui/src/design.rs`](../../../crates/ui/src/design.rs) | `theme_preview_palette`, `page_header` |
| [`crates/ui/src/design/busy.rs`](../../../crates/ui/src/design/busy.rs) | `ui` |
| [`crates/ui/src/dialog.rs`](../../../crates/ui/src/dialog.rs) | `show`, `toolbar_header`, `enter_after_shown_frame`, `long_toolbar_title_leaves_room_for_filter_below` |
| [`crates/ui/src/embeds.rs`](../../../crates/ui/src/embeds.rs) | `image_preview`, `browser_video_preview`, `show`, `image_previews_dispatch_copy_and_save_without_opening_links` |
| [`crates/ui/src/emoji.rs`](../../../crates/ui/src/emoji.rs) | `ColorImage`, `Color32`, `TextureOptions`, `TextureFilter` |
| [`crates/ui/src/emoji_details.rs`](../../../crates/ui/src/emoji_details.rs) | `show` |
| [`crates/ui/src/emoji_picker.rs`](../../../crates/ui/src/emoji_picker.rs) | `show`, `loading_clip_tiles_leave_the_picker_placeholder_without_embed_labels`, `current_server_is_the_first_clickable_picker_rail_entry`, `custom_picker_frame_benchmark` |
| [`crates/ui/src/emoticons.rs`](../../../crates/ui/src/emoticons.rs) | Support / data |
| [`crates/ui/src/extension_account_actions.rs`](../../../crates/ui/src/extension_account_actions.rs) | `extension_audio_settings` |
| [`crates/ui/src/extension_actions.rs`](../../../crates/ui/src/extension_actions.rs) | `query_and_account_settings_reuse_native_bounded_state` |
| [`crates/ui/src/extension_admin_actions.rs`](../../../crates/ui/src/extension_admin_actions.rs) | Support / data |
| [`crates/ui/src/extension_app.rs`](../../../crates/ui/src/extension_app.rs) | `extension_local_settings`, `extension_notification_settings` |
| [`crates/ui/src/extension_server_actions.rs`](../../../crates/ui/src/extension_server_actions.rs) | Support / data |
| [`crates/ui/src/extensions_ui.rs`](../../../crates/ui/src/extensions_ui.rs) | `stop_theme_preview`, `begin_gallery_preview`, `theme_preview_bar`, `theme_editor_toolbar` |
| [`crates/ui/src/external_upload.rs`](../../../crates/ui/src/external_upload.rs) | `show`, `failed_upload_dismisses_like_any_dialog_but_a_link_does_not` |
| [`crates/ui/src/fonts.rs`](../../../crates/ui/src/fonts.rs) | `show` |
| [`crates/ui/src/force_sync.rs`](../../../crates/ui/src/force_sync.rs) | `force_sync_settings` |
| [`crates/ui/src/formatting.rs`](../../../crates/ui/src/formatting.rs) | `text`, `Context` |
| [`crates/ui/src/forum.rs`](../../../crates/ui/src/forum.rs) | `show`, `sort_menu`, `forum_list_and_gallery_show_mention_counts_even_for_long_titles`, `post_media_box_counts_files_and_hover_shows_each_with_remove` |
| [`crates/ui/src/forum_settings.rs`](../../../crates/ui/src/forum_settings.rs) | `show`, `reaction_preview` |
| [`crates/ui/src/forwarding.rs`](../../../crates/ui/src/forwarding.rs) | `show` |
| [`crates/ui/src/friends.rs`](../../../crates/ui/src/friends.rs) | `add_friend_page`, `friend_requests_page`, `friends_page`, `add_friend_page_scrolls_to_its_last_action_in_a_short_viewport` |
| [`crates/ui/src/gif_favorite_tests.rs`](../../../crates/ui/src/gif_favorite_tests.rs) | `Shape`, `Context`, `apply`, `RawInput` |
| [`crates/ui/src/group_members.rs`](../../../crates/ui/src/group_members.rs) | `show` |
| [`crates/ui/src/group_menu.rs`](../../../crates/ui/src/group_menu.rs) | `show`, `group_menu_edit_mute_leave_and_stale_picker` |
| [`crates/ui/src/guild_folders.rs`](../../../crates/ui/src/guild_folders.rs) | `empty_and_left_server_folders_disappear_without_changing_settings` |
| [`crates/ui/src/highlight.rs`](../../../crates/ui/src/highlight.rs) | Support / data |
| [`crates/ui/src/i18n.rs`](../../../crates/ui/src/i18n.rs) | `welcome_screen_uses_the_new_product_name_in_every_language` |
| [`crates/ui/src/icons.rs`](../../../crates/ui/src/icons.rs) | `paint` |
| [`crates/ui/src/inbox.rs`](../../../crates/ui/src/inbox.rs) | `ignored_inbox_previews_hide_identity_and_body_until_revealed`, `mentions_pages_bound_cursors_reject_stale_results_and_clear_on_close`, `narrow_inbox_actions_remain_inside_the_dialog_and_do_not_stack_letters`, `mentions_dialog_opens_target_message_and_escape_clears_its_page` |
| [`crates/ui/src/invites.rs`](../../../crates/ui/src/invites.rs) | `show` |
| [`crates/ui/src/join_server.rs`](../../../crates/ui/src/join_server.rs) | `open_picker`, `show`, `show_picker`, `join_dialog_checks_then_confirms_once_and_clears_on_session_change` |
| [`crates/ui/src/keybinds.rs`](../../../crates/ui/src/keybinds.rs) | `show` |
| [`crates/ui/src/lib.rs`](../../../crates/ui/src/lib.rs) | `preview_channel_settings`, `preview_screen_share`, `preview_account_menu`, `preview_sticker_picker` |
| [`crates/ui/src/local_time.rs`](../../../crates/ui/src/local_time.rs) | `format_header`, `headers_show_the_time_today_yesterday_and_a_locale_date_before` |
| [`crates/ui/src/local_time/zone.rs`](../../../crates/ui/src/local_time/zone.rs) | `calendar_headers_use_the_selected_zones_day_and_preserve_explicit_timestamp_instants` |
| [`crates/ui/src/markdown.rs`](../../../crates/ui/src/markdown.rs) | `show`, `append_inline_preview`, `inline_preview_text`, `paint_inline_preview_emojis` |
| [`crates/ui/src/member_guide.rs`](../../../crates/ui/src/member_guide.rs) | `show` |
| [`crates/ui/src/member_search.rs`](../../../crates/ui/src/member_search.rs) | `Context`, `RawInput` |
| [`crates/ui/src/mentions.rs`](../../../crates/ui/src/mentions.rs) | `show`, `game_keyboard_selection_and_catalog_arrival_refresh_the_picker` |
| [`crates/ui/src/message_menu.rs`](../../../crates/ui/src/message_menu.rs) | `report_dialog`, `reminder_dialog`, `channel_and_account_changes_retire_menu_payloads_and_private_drafts` |
| [`crates/ui/src/messaging_permissions.rs`](../../../crates/ui/src/messaging_permissions.rs) | `messaging_permissions_settings` |
| [`crates/ui/src/mfa.rs`](../../../crates/ui/src/mfa.rs) | `show`, `secret_input_is_retired_when_session_changes_even_without_rendering_a_dialog` |
| [`crates/ui/src/mod_view.rs`](../../../crates/ui/src/mod_view.rs) | `show`, `members_context_menu_opens_an_actionable_drawer_above_the_conversation` |
| [`crates/ui/src/moderation_bulk.rs`](../../../crates/ui/src/moderation_bulk.rs) | `Ui`, `Checkbox`, `Button`, `ScrollArea` |
| [`crates/ui/src/name_styles.rs`](../../../crates/ui/src/name_styles.rs) | `effects_show_on_name_or_row_hover_or_always` |
| [`crates/ui/src/notification_settings.rs`](../../../crates/ui/src/notification_settings.rs) | `notification_controls_change_real_state_and_emit_preview_requests`, `notification_settings` |
| [`crates/ui/src/notifications.rs`](../../../crates/ui/src/notifications.rs) | `Vec2`, `vec2`, `Ui`, `Pos2` |
| [`crates/ui/src/onboarding.rs`](../../../crates/ui/src/onboarding.rs) | `show` |
| [`crates/ui/src/owned_cosmetics.rs`](../../../crates/ui/src/owned_cosmetics.rs) | `owned_cosmetic_settings` |
| [`crates/ui/src/panels.rs`](../../../crates/ui/src/panels.rs) | `channel_header`, `channel_header_row` |
| [`crates/ui/src/pending.rs`](../../../crates/ui/src/pending.rs) | `show` |
| [`crates/ui/src/pending_tests.rs`](../../../crates/ui/src/pending_tests.rs) | `Context`, `Event`, `Rect`, `Color32` |
| [`crates/ui/src/polls.rs`](../../../crates/ui/src/polls.rs) | `show` |
| [`crates/ui/src/polls_tests.rs`](../../../crates/ui/src/polls_tests.rs) | `selection_preview_and_pending_cards_preserve_vote_actions`, `answer_emoji_comes_from_the_picker_as_twemoji_and_can_be_cleared` |
| [`crates/ui/src/post_menu.rs`](../../../crates/ui/src/post_menu.rs) | `show` |
| [`crates/ui/src/preload.rs`](../../../crates/ui/src/preload.rs) | `Response`, `Context`, `Rect`, `RawInput` |
| [`crates/ui/src/privacy_settings.rs`](../../../crates/ui/src/privacy_settings.rs) | `account_preferences_header`, `privacy_settings`, `accessibility_settings` |
| [`crates/ui/src/profile_edit.rs`](../../../crates/ui/src/profile_edit.rs) | `show` |
| [`crates/ui/src/profiles.rs`](../../../crates/ui/src/profiles.rs) | `more_menu`, `mutuals_panel`, `debug_activity_panel`, `full_panel` |
| [`crates/ui/src/proxy_auth.rs`](../../../crates/ui/src/proxy_auth.rs) | `show` |
| [`crates/ui/src/reactions.rs`](../../../crates/ui/src/reactions.rs) | `show` |
| [`crates/ui/src/reading.rs`](../../../crates/ui/src/reading.rs) | `layout_settings`, `chat_reading_settings` |
| [`crates/ui/src/recovery_demo.rs`](../../../crates/ui/src/recovery_demo.rs) | `Context`, `RawInput`, `Rect`, `vec2` |
| [`crates/ui/src/registered_games.rs`](../../../crates/ui/src/registered_games.rs) | `registered_games_settings`, `process_picker` |
| [`crates/ui/src/restricted.rs`](../../../crates/ui/src/restricted.rs) | `show` |
| [`crates/ui/src/rich_presence.rs`](../../../crates/ui/src/rich_presence.rs) | `Ui`, `ButtonKind`, `button`, `hint` |
| [`crates/ui/src/role_names.rs`](../../../crates/ui/src/role_names.rs) | `role_name_color`, `text`, `Context`, `luminance` |
| [`crates/ui/src/role_preview.rs`](../../../crates/ui/src/role_preview.rs) | `show`, `role_preview_virtualizes_channels_and_reuses_unchanged_permission_results`, `role_preview_is_read_only_and_retires_after_permission_or_account_change` |
| [`crates/ui/src/screen.rs`](../../../crates/ui/src/screen.rs) | `show`, `audio_controls_describe_the_selected_source_without_widening_picker_scope`, `unknown_picker_geometry_is_deferred_and_source_uses_native_pixels` |
| [`crates/ui/src/scroll.rs`](../../../crates/ui/src/scroll.rs) | `paint` |
| [`crates/ui/src/search.rs`](../../../crates/ui/src/search.rs) | `invalid_queries_show_an_error_and_recover_after_correction` |
| [`crates/ui/src/search_filters.rs`](../../../crates/ui/src/search_filters.rs) | `user_picker`, `channel_picker`, `show` |
| [`crates/ui/src/search_navigation_tests.rs`](../../../crates/ui/src/search_navigation_tests.rs) | `result_jump_and_numeric_pager_keep_scope_and_allow_explicit_retry` |
| [`crates/ui/src/select.rs`](../../../crates/ui/src/select.rs) | `open_menu`, `show` |
| [`crates/ui/src/server_access.rs`](../../../crates/ui/src/server_access.rs) | `show`, `channel_picker` |
| [`crates/ui/src/server_admin.rs`](../../../crates/ui/src/server_admin.rs) | `show`, `member_menu` |
| [`crates/ui/src/server_audit_log.rs`](../../../crates/ui/src/server_audit_log.rs) | `show` |
| [`crates/ui/src/server_automod.rs`](../../../crates/ui/src/server_automod.rs) | `show` |
| [`crates/ui/src/server_integrations.rs`](../../../crates/ui/src/server_integrations.rs) | `show`, `webhook_cards_copy_once_in_both_settings_scopes_and_fit_small_widths` |
| [`crates/ui/src/server_invite.rs`](../../../crates/ui/src/server_invite.rs) | `show`, `invite_picker_search_send_settings_and_failure` |
| [`crates/ui/src/server_invites.rs`](../../../crates/ui/src/server_invites.rs) | `show` |
| [`crates/ui/src/server_menu.rs`](../../../crates/ui/src/server_menu.rs) | `show`, `shared_server_items_hide_settings_without_access_and_leave_for_owners`, `server_menu_requires_explicit_invite_and_leave_confirmation` |
| [`crates/ui/src/server_moderation.rs`](../../../crates/ui/src/server_moderation.rs) | `show` |
| [`crates/ui/src/server_notification_tests.rs`](../../../crates/ui/src/server_notification_tests.rs) | `open_menu`, `ordinary_members_open_notification_settings_from_header_and_rail`, `narrow_notification_dialog_keeps_close_visible_and_scrolls_controls` |
| [`crates/ui/src/server_notifications.rs`](../../../crates/ui/src/server_notifications.rs) | `mute_menu`, `show` |
| [`crates/ui/src/server_profile.rs`](../../../crates/ui/src/server_profile.rs) | `show`, `image_picker_results_are_bound_to_guild_generation_and_editor_revision` |
| [`crates/ui/src/server_roles.rs`](../../../crates/ui/src/server_roles.rs) | `show` |
| [`crates/ui/src/server_settings.rs`](../../../crates/ui/src/server_settings.rs) | `navigate_away_from_server_settings`, `take_server_sound_preview`, `has_server_settings_changes`, `preview_server_settings` |
| [`crates/ui/src/server_sounds.rs`](../../../crates/ui/src/server_sounds.rs) | `stop_preview`, `take_preview`, `upload_dialog`, `show` |
| [`crates/ui/src/server_stickers.rs`](../../../crates/ui/src/server_stickers.rs) | `show` |
| [`crates/ui/src/settings.rs`](../../../crates/ui/src/settings.rs) | `translated_control_labels_find_their_real_page_and_target`, `typing_preserves_the_current_page_and_drafts_until_a_result_is_activated`, `indexed_settings_anchors_reach_their_production_controls`, `tiny_settings_modal_keeps_its_accessible_close_button_on_screen` |
| [`crates/ui/src/shortcuts.rs`](../../../crates/ui/src/shortcuts.rs) | Support / data |
| [`crates/ui/src/slash_builtin.rs`](../../../crates/ui/src/slash_builtin.rs) | `Level`, `Context` |
| [`crates/ui/src/slash_commands.rs`](../../../crates/ui/src/slash_commands.rs) | `show`, `localized_builtin_picker_refreshes_and_option_help_uses_typed_bounds`, `busy_command_shows_an_actionable_notice_without_losing_arguments`, `command_files_reindex_without_losing_arguments_and_keyboard_opens_the_picker` |
| [`crates/ui/src/stickers.rs`](../../../crates/ui/src/stickers.rs) | `show`, `other_server_sticker_card_shows_artwork_name_and_compact_retry` |
| [`crates/ui/src/switcher.rs`](../../../crates/ui/src/switcher.rs) | `show` |
| [`crates/ui/src/test_ui.rs`](../../../crates/ui/src/test_ui.rs) | `Shape`, `Rect` |
| [`crates/ui/src/text_menu.rs`](../../../crates/ui/src/text_menu.rs) | `selection_for_menu`, `show`, `pending_menu_focus_expires_and_yields_to_new_navigation`, `pending_menu_focus_preserves_macos_word_navigation` |
| [`crates/ui/src/theme_editor.rs`](../../../crates/ui/src/theme_editor.rs) | `show`, `map_selects_the_member_panel_and_save_points_to_required_basics` |
| [`crates/ui/src/thread_create.rs`](../../../crates/ui/src/thread_create.rs) | `show` |
| [`crates/ui/src/thread_members.rs`](../../../crates/ui/src/thread_members.rs) | `show` |
| [`crates/ui/src/thumbhash.rs`](../../../crates/ui/src/thumbhash.rs) | `vec2` |
| [`crates/ui/src/timeline.rs`](../../../crates/ui/src/timeline.rs) | `compact_header`, `fill_header_line`, `clamp_hover_toolbar`, `show_fullscreen_video` |
| [`crates/ui/src/timeline/row.rs`](../../../crates/ui/src/timeline/row.rs) | `Rect`, `Response`, `Ui`, `palette` |
| [`crates/ui/src/title_bar_tests.rs`](../../../crates/ui/src/title_bar_tests.rs) | `Context`, `RawInput`, `Rect`, `Pos2` |
| [`crates/ui/src/toasts.rs`](../../../crates/ui/src/toasts.rs) | `show` |
| [`crates/ui/src/typing.rs`](../../../crates/ui/src/typing.rs) | `Ui`, `Rect`, `palette`, `Sense` |
| [`crates/ui/src/updates.rs`](../../../crates/ui/src/updates.rs) | `updates_menu`, `update_settings` |
| [`crates/ui/src/user_direct.rs`](../../../crates/ui/src/user_direct.rs) | `Context`, `Level` |
| [`crates/ui/src/user_menu.rs`](../../../crates/ui/src/user_menu.rs) | `show`, `application_menu`, `user_menu_mouse_keyboard_and_actions_in_both_themes`, `click_menu_action` |
| [`crates/ui/src/verification.rs`](../../../crates/ui/src/verification.rs) | `show` |
| [`crates/ui/src/video.rs`](../../../crates/ui/src/video.rs) | `exit_fullscreen`, `take_fullscreen_request`, `is_fullscreen`, `show_fullscreen` |
| [`crates/ui/src/view.rs`](../../../crates/ui/src/view.rs) | `show` |
| [`crates/ui/src/voice.rs`](../../../crates/ui/src/voice.rs) | `stream_preview_watch_state`, `exit_voice_fullscreen`, `is_voice_fullscreen`, `voice_fullscreen_channel` |
| [`crates/ui/src/voice_connection.rs`](../../../crates/ui/src/voice_connection.rs) | `participant_selector_shows_attributed_media_and_retires_removed_people`, `debug_defers_escape_to_settings_and_releases_data_without_rendering_a_window` |
| [`crates/ui/src/voice_messages.rs`](../../../crates/ui/src/voice_messages.rs) | `show` |
| [`crates/ui/tests/activity_sharing.rs`](../../../crates/ui/tests/activity_sharing.rs) | `Context`, `ThemePreference`, `apply`, `Event` |
| [`crates/ui/tests/components.rs`](../../../crates/ui/tests/components.rs) | `Context`, `Pos2`, `apply`, `Event` |
| [`crates/ui/tests/own_activity.rs`](../../../crates/ui/tests/own_activity.rs) | `own_activity_panel_and_setting_render_and_clear` |
| [`crates/ui/tests/registered_games.rs`](../../../crates/ui/tests/registered_games.rs) | `one_page_shares_activity_and_lists_detected_games_that_can_be_hidden` |
| [`crates/ui/tests/settings_modals.rs`](../../../crates/ui/tests/settings_modals.rs) | `server_settings_pages_share_one_modal_rect` |
| [`crates/ui/tests/startup.rs`](../../../crates/ui/tests/startup.rs) | `Context`, `ThemePreference`, `apply`, `RawInput` |
| [`crates/ui/tests/startup_memory.rs`](../../../crates/ui/tests/startup_memory.rs) | `Context`, `FontId`, `Color32`, `pos2` |

### examples/extensions

| File | Source signals (sample) |
| --- | --- |
| [`examples/extensions/Cargo.toml`](../../../examples/extensions/Cargo.toml) | Support / data |
| [`examples/extensions/app-actions/Cargo.toml`](../../../examples/extensions/app-actions/Cargo.toml) | Support / data |
| [`examples/extensions/app-actions/src/lib.rs`](../../../examples/extensions/app-actions/src/lib.rs) | Support / data |
| [`examples/extensions/app-toolbox/Cargo.toml`](../../../examples/extensions/app-toolbox/Cargo.toml) | Support / data |
| [`examples/extensions/app-toolbox/src/lib.rs`](../../../examples/extensions/app-toolbox/src/lib.rs) | Support / data |
| [`examples/extensions/conversation-inspector/Cargo.toml`](../../../examples/extensions/conversation-inspector/Cargo.toml) | Support / data |
| [`examples/extensions/conversation-inspector/src/lib.rs`](../../../examples/extensions/conversation-inspector/src/lib.rs) | `offline_fixtures_show_loaded_data_and_older_host_absence` |
| [`examples/extensions/guild-inspector/Cargo.toml`](../../../examples/extensions/guild-inspector/Cargo.toml) | Support / data |
| [`examples/extensions/guild-inspector/src/lib.rs`](../../../examples/extensions/guild-inspector/src/lib.rs) | Support / data |
| [`examples/extensions/message-counter/Cargo.toml`](../../../examples/extensions/message-counter/Cargo.toml) | Support / data |
| [`examples/extensions/message-counter/src/lib.rs`](../../../examples/extensions/message-counter/src/lib.rs) | Support / data |
| [`examples/extensions/rgb-cycle/Cargo.toml`](../../../examples/extensions/rgb-cycle/Cargo.toml) | Support / data |
| [`examples/extensions/rgb-cycle/src/lib.rs`](../../../examples/extensions/rgb-cycle/src/lib.rs) | `settings_panel`, `load_settings`, `save_settings` |

### extensions/Cargo.toml

| File | Source signals (sample) |
| --- | --- |
| [`extensions/Cargo.toml`](../../../extensions/Cargo.toml) | Support / data |

### extensions/plugins

| File | Source signals (sample) |
| --- | --- |
| [`extensions/plugins/api-proxy/Cargo.toml`](../../../extensions/plugins/api-proxy/Cargo.toml) | Support / data |
| [`extensions/plugins/api-proxy/src/lib.rs`](../../../extensions/plugins/api-proxy/src/lib.rs) | Support / data |
| [`extensions/plugins/custom-rpc/Cargo.toml`](../../../extensions/plugins/custom-rpc/Cargo.toml) | Support / data |
| [`extensions/plugins/custom-rpc/src/lib.rs`](../../../extensions/plugins/custom-rpc/src/lib.rs) | Support / data |
| [`extensions/plugins/message-delete-protector/Cargo.toml`](../../../extensions/plugins/message-delete-protector/Cargo.toml) | Support / data |
| [`extensions/plugins/message-delete-protector/src/lib.rs`](../../../extensions/plugins/message-delete-protector/src/lib.rs) | Support / data |
| [`extensions/plugins/voice-messages/Cargo.toml`](../../../extensions/plugins/voice-messages/Cargo.toml) | Support / data |
| [`extensions/plugins/voice-messages/src/lib.rs`](../../../extensions/plugins/voice-messages/src/lib.rs) | `corrupt_saved_settings_fail_closed_until_explicit_repair` |

### extensions/sdk

| File | Source signals (sample) |
| --- | --- |
| [`extensions/sdk/Cargo.toml`](../../../extensions/sdk/Cargo.toml) | Support / data |
| [`extensions/sdk/src/app.rs`](../../../extensions/sdk/src/app.rs) | Support / data |
| [`extensions/sdk/src/channel_metadata.rs`](../../../extensions/sdk/src/channel_metadata.rs) | Support / data |
| [`extensions/sdk/src/conversation_activity.rs`](../../../extensions/sdk/src/conversation_activity.rs) | Support / data |
| [`extensions/sdk/src/discovery.rs`](../../../extensions/sdk/src/discovery.rs) | Support / data |
| [`extensions/sdk/src/extended.rs`](../../../extensions/sdk/src/extended.rs) | Support / data |
| [`extensions/sdk/src/forum_data.rs`](../../../extensions/sdk/src/forum_data.rs) | Support / data |
| [`extensions/sdk/src/lib.rs`](../../../extensions/sdk/src/lib.rs) | Support / data |
| [`extensions/sdk/src/manifest.rs`](../../../extensions/sdk/src/manifest.rs) | Support / data |
| [`extensions/sdk/src/member_details.rs`](../../../extensions/sdk/src/member_details.rs) | Support / data |
| [`extensions/sdk/src/message_content.rs`](../../../extensions/sdk/src/message_content.rs) | Support / data |
| [`extensions/sdk/src/rich_presence.rs`](../../../extensions/sdk/src/rich_presence.rs) | Support / data |
| [`extensions/sdk/src/voice_messages.rs`](../../../extensions/sdk/src/voice_messages.rs) | Support / data |
| [`extensions/sdk/tests/authoring.rs`](../../../extensions/sdk/tests/authoring.rs) | Support / data |

### tools/tray-debug

| File | Source signals (sample) |
| --- | --- |
| [`tools/tray-debug/src/main.rs`](../../../tools/tray-debug/src/main.rs) | `Context`, `RawInput`, `ViewportEvent` |

</details>
