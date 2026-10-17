//! One session-only draft for the account's selected server profile.
use crate::{avatars::Avatars, design};
use client_core::{Command, State};
use model::{
	Id, Patch,
	server_profile::{Edit, Snapshot, Values},
};

#[derive(Clone, PartialEq, Eq)]
struct Draft {
	values: Values,
	edit: Edit,
}
impl Draft {
	fn from_snapshot(snapshot: &Snapshot) -> Self {
		Self {
			values: snapshot.values.clone(),
			edit: Edit::default(),
		}
	}
	fn refresh(&mut self, saved: &Values) {
		fn change<T: Clone + PartialEq>(before: &Option<T>, after: &Option<T>) -> Patch<T> {
			if before == after {
				Patch::Absent
			} else {
				after.clone().map_or(Patch::Null, Patch::Value)
			}
		}
		self.edit = Edit {
			nick: change(&saved.nick, &self.values.nick),
			bio: change(&saved.bio, &self.values.bio),
			pronouns: change(&saved.pronouns, &self.values.pronouns),
			accent_color: change(&saved.accent_color, &self.values.accent_color),
			theme_colors: change(&saved.theme_colors, &self.values.theme_colors),
			name_style: change(&saved.name_style, &self.values.name_style),
			avatar: std::mem::take(&mut self.edit.avatar),
			banner: std::mem::take(&mut self.edit.banner),
		};
	}
	fn dirty(&self, saved: &Values) -> bool {
		self.values.nick != saved.nick
			|| self.values.bio != saved.bio
			|| self.values.pronouns != saved.pronouns
			|| self.values.accent_color != saved.accent_color
			|| self.values.theme_colors != saved.theme_colors
			|| self.values.name_style != saved.name_style
			|| !matches!(self.edit.avatar, Patch::Absent)
			|| !matches!(self.edit.banner, Patch::Absent)
	}
	fn rebase(&mut self, before: &Values, fresh: &Values) {
		macro_rules! field {
			($field:ident) => {
				if self.values.$field == before.$field {
					self.values.$field = fresh.$field.clone();
				}
			};
		}
		field!(nick);
		field!(bio);
		field!(pronouns);
		field!(accent_color);
		field!(theme_colors);
		field!(name_style);
		self.values.avatar.clone_from(&fresh.avatar);
		self.values.banner.clone_from(&fresh.banner);
	}
}

#[derive(Default)]
pub(super) struct Editor {
	generation: Option<u64>,
	guild: Option<Id>,
	pending_guild: Option<Id>,
	draft: Option<Draft>,
	baseline: Option<Values>,
	requested: bool,
	submitted: Option<u64>,
	saved: bool,
	revision: u64,
	choosing: Option<(u64, bool)>,
	/// Native picker scope is (generation, guild, editor revision); bool selects banner.
	pub image_request: Option<((u64, Id, u64), bool)>,
	pictures: [Option<egui::TextureHandle>; 2],
	saved_pictures: [Option<(Option<String>, egui::TextureHandle)>; 2],
	opening: Option<String>,
}
impl Editor {
	fn switch(&mut self, state: &mut State, guild: Id, commands: &mut Vec<Command>) {
		if state.own_server_profile.saving {
			return;
		}
		state.close_server_profile();
		*self = Self {
			generation: Some(state.generation),
			guild: Some(guild),
			revision: self.revision.wrapping_add(1),
			requested: true,
			..Self::default()
		};
		if let Some(command) = state.load_server_profile(guild) {
			commands.push(command);
		}
	}
	pub fn accept_image(
		&mut self,
		ctx: &egui::Context,
		scope: (u64, Id, u64),
		result: Result<Option<(String, egui::ColorImage)>, &'static str>,
	) -> Result<(), &'static str> {
		let Some((revision, banner)) = self.choosing else {
			return Ok(());
		};
		if self.generation != Some(scope.0) || self.guild != Some(scope.1) || revision != scope.2 {
			return Ok(());
		}
		self.choosing = None;
		let Some(draft) = self.draft.as_mut() else {
			return Ok(());
		};
		if let Some((uri, image)) = result? {
			if !model::valid_avatar_uri(&uri)
				|| image.size[0] > 512
				|| image.size[1] > 512
				|| image.size.contains(&0)
			{
				return Err("That picture is too large; choose a simpler image");
			}
			let index = usize::from(banner);
			if banner {
				draft.edit.banner = Patch::Value(uri);
			} else {
				draft.edit.avatar = Patch::Value(uri);
			}
			self.pictures[index] = Some(ctx.load_texture(
				if banner {
					"server-profile-banner-preview"
				} else {
					"server-profile-avatar-preview"
				},
				image,
				egui::TextureOptions::LINEAR,
			));
			self.saved = false;
		}
		Ok(())
	}
	pub fn show(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		avatars: &mut Avatars,
		commands: &mut Vec<Command>,
	) {
		if self.generation != Some(state.generation) {
			*self = Self {
				generation: Some(state.generation),
				revision: self.revision.wrapping_add(1),
				..Self::default()
			};
		}
		if self
			.guild
			.is_some_and(|guild| !state.can_edit_server_profile(guild))
		{
			*self = Self {
				generation: Some(state.generation),
				revision: self.revision.wrapping_add(1),
				..Self::default()
			};
		}
		let initial = self
			.guild
			.or_else(|| {
				state
					.selected
					.and_then(|id| state.channel(id))
					.and_then(|channel| channel.guild)
					.filter(|guild| state.can_edit_server_profile(*guild))
			})
			.or_else(|| {
				state
					.guilds
					.iter()
					.find(|guild| state.can_edit_server_profile(guild.id))
					.map(|guild| guild.id)
			});
		let Some(guild) = initial else {
			design::hint(ui, "Join a server to customize your server profile.");
			return;
		};
		if self.guild != Some(guild) {
			self.switch(state, guild, commands);
		}
		let dirty = self
			.draft
			.as_ref()
			.zip(state.own_server_profile.data.as_ref())
			.is_some_and(|(draft, snapshot)| draft.dirty(&snapshot.values));
		let mut selected = guild;
		ui.add_enabled_ui(
			!state.own_server_profile.saving && self.choosing.is_none(),
			|ui| {
				let label = ui.label(design::eyebrow(ui, "Server", design::palette(ui).muted));
				egui::ComboBox::from_id_salt("own-server-profile-guild")
					.selected_text(
						state
							.guild(guild)
							.map_or("Server", |guild| guild.name.as_str()),
					)
					.width(ui.available_width().min(340.0))
					.height(280.0)
					.show_ui(ui, |ui| {
						for guild in &state.guilds {
							if state.can_edit_server_profile(guild.id) {
								ui.selectable_value(&mut selected, guild.id, &guild.name);
							}
						}
					})
					.response
					.labelled_by(label.id);
			},
		);
		if selected != guild {
			if dirty {
				self.pending_guild = Some(selected);
			} else {
				self.switch(state, selected, commands);
			}
		}
		if let Some(next) = self.pending_guild {
			design::notice(
				ui,
				design::Level::Warning,
				"Switching servers will discard this server's unsaved changes.",
			);
			ui.horizontal_wrapped(|ui| {
				if design::button(ui, "Discard and switch", design::ButtonKind::Danger).clicked() {
					self.switch(state, next, commands);
				}
				if design::button(ui, "Keep editing", design::ButtonKind::Neutral).clicked() {
					self.pending_guild = None;
				}
			});
		}
		if state.own_server_profile.data.is_none()
			&& !state.own_server_profile.pending
			&& state.own_server_profile.error.is_none()
		{
			self.requested = false;
		}
		if !self.requested && !state.own_server_profile.pending {
			if let Some(command) = state.load_server_profile(guild) {
				commands.push(command);
			}
			self.requested = true;
		}
		if state.own_server_profile.pending {
			ui.horizontal(|ui| {
				ui.add(egui::Spinner::new().size(14.0));
				ui.label(if state.own_server_profile.saving {
					"Saving server profile…"
				} else {
					"Loading server profile…"
				});
			});
		}
		if let Some(error) = state.own_server_profile.error {
			design::notice(ui, design::Level::Error, error);
			if !state.own_server_profile.pending
				&& design::text_action(ui, "Reload server profile").clicked()
				&& let Some(command) = state.load_server_profile(self.guild.unwrap_or(guild))
			{
				commands.push(command);
			}
		}
		let Some(snapshot) = state
			.own_server_profile
			.data
			.as_ref()
			.filter(|snapshot| snapshot.guild() == self.guild)
		else {
			return;
		};
		if self.submitted == Some(state.own_server_profile.request)
			&& !state.own_server_profile.pending
		{
			self.submitted = None;
			if state.own_server_profile.error.is_none() {
				for (index, hash) in [
					snapshot.values.avatar.clone(),
					snapshot.values.banner.clone(),
				]
				.into_iter()
				.enumerate()
				{
					if let Some(texture) = self.pictures[index].take() {
						self.saved_pictures[index] = Some((hash, texture));
					}
				}
				self.draft = Some(Draft::from_snapshot(snapshot));
				self.saved = true;
			}
		}
		if let (Some(draft), Some(before)) = (&mut self.draft, &self.baseline) {
			draft.rebase(before, &snapshot.values);
		}
		self.baseline = Some(snapshot.values.clone());
		let draft = self
			.draft
			.get_or_insert_with(|| Draft::from_snapshot(snapshot));
		let before = (
			draft.values.clone(),
			std::mem::discriminant(&draft.edit.avatar),
			std::mem::discriminant(&draft.edit.banner),
		);
		let editable = !state.own_server_profile.pending;
		let nitro = state.premium_type == 2;
		let nickname = state.can_change_own_nickname(guild);
		ui.add_space(12.0);
		if !nitro {
			design::hint(
				ui,
				"Full Nitro is required to set a server avatar, banner, bio or profile theme. You can still change pronouns, change your nickname where permitted, or remove existing overrides.",
			);
		}
		let mut pick = None;
		ui.add_enabled_ui(editable, |ui| {
			pick = form(
				ui,
				draft,
				snapshot,
				nitro,
				nickname,
				self.choosing.is_some(),
			);
		});
		if let Some(banner) = pick {
			self.revision = self.revision.wrapping_add(1);
			self.choosing = Some((self.revision, banner));
			self.image_request = Some(((state.generation, guild, self.revision), banner));
		}
		if (
			draft.values.clone(),
			std::mem::discriminant(&draft.edit.avatar),
			std::mem::discriminant(&draft.edit.banner),
		) != before
		{
			self.saved = false;
		}
		for (index, edit) in [&draft.edit.avatar, &draft.edit.banner]
			.into_iter()
			.enumerate()
		{
			if !matches!(edit, Patch::Value(_)) {
				self.pictures[index] = None;
			}
		}
		draft.refresh(&snapshot.values);
		let edit = &draft.edit;
		let dirty = !edit.is_empty();
		if !edit.valid() {
			design::notice(
				ui,
				design::Level::Error,
				"Check the character limits and remove control characters before saving.",
			);
		}
		ui.add_space(12.0);
		let mut reset = false;
		ui.horizontal_wrapped(|ui| {
			ui.add_enabled_ui(state.can_save_server_profile(edit), |ui| {
				if design::button(ui, "Save changes", design::ButtonKind::Primary).clicked()
					&& let Some(command) = state.save_server_profile(edit.clone())
				{
					self.submitted = Some(state.own_server_profile.request);
					self.saved = false;
					commands.push(command);
				}
			});
			ui.add_enabled_ui(dirty && editable, |ui| {
				if design::button(ui, "Reset changes", design::ButtonKind::Neutral).clicked() {
					reset = true;
				}
			});
			if self.saved {
				ui.colored_label(
					design::palette(ui).positive,
					if state.demo {
						"Saved in preview"
					} else {
						"Server profile saved"
					},
				);
			} else if dirty {
				ui.weak("You have unsaved changes");
			}
		});
		if reset {
			self.draft = state
				.own_server_profile
				.data
				.as_ref()
				.map(Draft::from_snapshot);
			self.pictures = [None, None];
			self.revision = self.revision.wrapping_add(1);
			self.choosing = None;
			self.saved = false;
		}
		if let (Some(draft), Some(snapshot)) = (&self.draft, &state.own_server_profile.data) {
			ui.add_space(20.0);
			preview(
				ui,
				draft,
				snapshot,
				(&self.pictures, &self.saved_pictures),
				avatars,
				(state.demo, &state.guilds),
				&mut self.opening,
			);
		}
		crate::markdown::confirm_external_link(ui.ctx(), &mut self.opening, true);
	}
}

fn form(
	ui: &mut egui::Ui,
	draft: &mut Draft,
	snapshot: &Snapshot,
	nitro: bool,
	nickname: bool,
	choosing: bool,
) -> Option<bool> {
	let mut pick = None;
	text_override(
		ui,
		"Server nickname",
		"own-server-profile-nick",
		&mut draft.values.nick,
		&snapshot.profile.user.name,
		32,
		false,
		(nickname, nickname),
	);
	if !nickname {
		design::hint(
			ui,
			"You need Change Nickname permission to edit your nickname in this server.",
		);
	}
	text_override(
		ui,
		"Pronouns",
		"own-server-profile-pronouns",
		&mut draft.values.pronouns,
		&snapshot.profile.pronouns,
		40,
		false,
		(true, true),
	);
	text_override(
		ui,
		"About me",
		"own-server-profile-bio",
		&mut draft.values.bio,
		&snapshot.profile.bio,
		190,
		true,
		(nitro, true),
	);
	for (index, (label, change, present)) in [
		(
			"Server avatar",
			&mut draft.edit.avatar,
			snapshot.values.avatar.is_some(),
		),
		(
			"Server banner",
			&mut draft.edit.banner,
			snapshot.values.banner.is_some(),
		),
	]
	.into_iter()
	.enumerate()
	{
		design::card(ui, |ui| {
			ui.label(design::semibold(ui, label, 14.0));
			ui.horizontal_wrapped(|ui| {
				if ui
					.add_enabled(nitro && !choosing, egui::Button::new("Change image"))
					.clicked()
				{
					pick = Some(index == 1);
				}
				if (present || matches!(change, Patch::Value(_)))
					&& design::text_action(ui, "Use my user profile").clicked()
				{
					*change = Patch::Null;
				}
				if !matches!(change, Patch::Absent) && design::text_action(ui, "Undo").clicked() {
					*change = Patch::Absent;
				}
			});
			ui.weak(match change {
				Patch::Value(_) => "New image chosen. Save changes to upload it.",
				Patch::Null => "The user profile image will be used after saving.",
				Patch::Absent => {
					if present {
						"Using this server's image."
					} else {
						"Using your user profile image."
					}
				}
			});
		});
	}
	ui.add_space(8.0);
	let mut theme = draft.values.theme_colors.is_some();
	ui.add_enabled_ui(nitro || theme, |ui| {
		if ui
			.checkbox(&mut theme, "Customize server profile theme")
			.changed()
		{
			draft.values.theme_colors = theme.then_some(
				snapshot
					.profile
					.theme_colors
					.unwrap_or([0x5865f2, 0x232428]),
			);
		}
	});
	if let Some(colors) = &mut draft.values.theme_colors {
		ui.add_enabled_ui(nitro, |ui| {
			theme_colors(ui, colors);
		});
	}
	name_style_controls(
		ui,
		&mut draft.values.name_style,
		nitro,
		"Customize server display name",
	);
	pick
}

pub(super) fn name_style_controls(
	ui: &mut egui::Ui,
	value: &mut Option<model::ProfileNameStyle>,
	nitro: bool,
	label: &str,
) {
	ui.add_space(10.0);
	let mut enabled = value.is_some();
	ui.add_enabled_ui(nitro || enabled, |ui| {
		if ui.checkbox(&mut enabled, label).changed() {
			*value = enabled.then_some(model::ProfileNameStyle {
				font: 11,
				effect: 1,
				colors: vec![0xffffff],
			});
		}
	});
	let Some(style) = value else {
		return;
	};
	if !style.valid() {
		design::hint(
			ui,
			"This style uses unsupported values. Choose supported options to replace it, or turn customization off to remove it.",
		);
	}
	ui.add_enabled_ui(nitro, |ui| {
		let fonts = [
			(11, "Default"),
			(3, "Sakura"),
			(4, "Jellybean"),
			(6, "Modern"),
			(7, "Medieval"),
			(8, "8Bit"),
			(10, "Vampyre"),
			(12, "Tempo"),
			(13, "Monkey Bars"),
			(14, "Mainframe"),
			(15, "Headbang"),
			(16, "Journal"),
		];
		ui.horizontal_wrapped(|ui| {
			ui.label("Font");
			egui::ComboBox::from_id_salt((label, "font"))
				.selected_text(
					fonts
						.iter()
						.find(|(id, _)| *id == style.font)
						.map_or("Unsupported font", |(_, name)| *name),
				)
				.show_ui(ui, |ui| {
					for (id, name) in fonts {
						ui.selectable_value(&mut style.font, id, name);
					}
				});
			ui.label("Effect");
			let effects = [
				(1, "Solid"),
				(2, "Gradient"),
				(3, "Neon"),
				(4, "Toon"),
				(5, "Pop"),
				(6, "Glow"),
				(7, "Prism"),
				(8, "Gummy"),
			];
			egui::ComboBox::from_id_salt((label, "effect"))
				.selected_text(
					effects
						.iter()
						.find(|(id, _)| *id == style.effect)
						.map_or("Unsupported effect", |(_, name)| *name),
				)
				.show_ui(ui, |ui| {
					for (id, name) in effects {
						if ui.selectable_value(&mut style.effect, id, name).changed()
							&& !matches!(id, 2 | 6 | 7 | 8)
						{
							style.colors.truncate(1);
						}
					}
				});
		});
		ui.horizontal_wrapped(|ui| {
			for (index, color) in style.colors.iter_mut().take(2).enumerate() {
				ui.push_id((label, index), |ui| {
					let mut rgb = [(*color >> 16) as u8, (*color >> 8) as u8, *color as u8];
					if design::color_edit(ui, &mut rgb).changed() {
						*color = u32::from_be_bytes([0, rgb[0], rgb[1], rgb[2]]);
					}
				});
			}
		});
		ui.horizontal_wrapped(|ui| {
			if style.colors.len() < 2
				&& matches!(style.effect, 2 | 6 | 7 | 8)
				&& design::text_action(ui, "Add second color").clicked()
			{
				style.colors.push(0x5865f2);
			}
			if style.colors.len() > 1 && design::text_action(ui, "Use one color").clicked() {
				style.colors.truncate(1);
			}
		});
	});
}

#[allow(clippy::too_many_arguments)]
fn text_override(
	ui: &mut egui::Ui,
	label: &str,
	id: &str,
	value: &mut Option<String>,
	fallback: &str,
	limit: usize,
	multiline: bool,
	permissions: (bool, bool),
) {
	ui.add_space(10.0);
	let heading = ui.label(design::eyebrow(ui, label, design::palette(ui).muted));
	let mut inherit = value.is_none();
	ui.add_enabled_ui(permissions.0 || (permissions.1 && value.is_some()), |ui| {
		if ui.checkbox(&mut inherit, "Use my user profile").changed() {
			*value = (!inherit).then(|| fallback.chars().take(limit).collect());
		}
	});
	if let Some(value) = value {
		ui.add_enabled_ui(permissions.0, |ui| {
			let edit = if multiline {
				egui::TextEdit::multiline(value).desired_rows(3)
			} else {
				egui::TextEdit::singleline(value)
			};
			design::input(ui, edit.id(egui::Id::unique(id)).char_limit(limit))
				.labelled_by(heading.id);
		});
	}
}

pub(super) fn theme_colors(ui: &mut egui::Ui, colors: &mut [u32; 2]) {
	ui.horizontal_wrapped(|ui| {
		for (index, color) in colors.iter_mut().enumerate() {
			ui.push_id(index, |ui| {
				ui.label(if index == 0 { "Primary" } else { "Accent" });
				let mut rgb = [(*color >> 16) as u8, (*color >> 8) as u8, *color as u8];
				if design::color_edit(ui, &mut rgb).changed() {
					*color = u32::from_be_bytes([0, rgb[0], rgb[1], rgb[2]]);
				}
			});
		}
	});
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn preview(
	ui: &mut egui::Ui,
	draft: &Draft,
	snapshot: &Snapshot,
	pictures: (
		&[Option<egui::TextureHandle>; 2],
		&[Option<(Option<String>, egui::TextureHandle)>; 2],
	),
	avatars: &mut Avatars,
	media: (bool, &[model::Guild]),
	opening: &mut Option<String>,
) {
	let mut profile = snapshot.profile.clone();
	let Some(member) = profile.guild.as_mut() else {
		return;
	};
	member.nick.clone_from(&draft.values.nick);
	member.avatar = if matches!(draft.edit.avatar, Patch::Null) {
		None
	} else {
		snapshot.values.avatar.clone()
	};
	member.banner = if matches!(draft.edit.banner, Patch::Null) {
		None
	} else {
		snapshot.values.banner.clone()
	};
	member.bio = draft
		.values
		.bio
		.clone()
		.unwrap_or_else(|| profile.bio.clone());
	member.pronouns = draft
		.values
		.pronouns
		.clone()
		.unwrap_or_else(|| profile.pronouns.clone());
	profile.theme_colors = draft.values.theme_colors.or(profile.theme_colors);
	if let Some(style) = draft
		.values
		.name_style
		.clone()
		.or_else(|| model::ProfileNameStyle::from_user(&profile.user))
	{
		profile
			.user
			.cosmetics
			.get_or_insert_with(Default::default)
			.name_style = Some(style.into_display());
	}
	let name = member
		.nick
		.clone()
		.unwrap_or_else(|| profile.user.name.clone());
	let bio = member.bio.clone();
	let pronouns = member.pronouns.clone();
	let texture = |index: usize| -> Option<&egui::TextureHandle> {
		let edit = if index == 0 {
			&draft.edit.avatar
		} else {
			&draft.edit.banner
		};
		if matches!(edit, Patch::Value(_)) {
			pictures.0[index].as_ref()
		} else if matches!(edit, Patch::Absent) {
			let hash = if index == 0 {
				&snapshot.values.avatar
			} else {
				&snapshot.values.banner
			};
			pictures.1[index]
				.as_ref()
				.filter(|(saved, _)| saved.is_some() && saved == hash)
				.map(|(_, texture)| texture)
		} else {
			None
		}
	};
	ui.label(design::eyebrow(
		ui,
		"Server profile preview",
		design::palette(ui).muted,
	));
	ui.scope(|ui| {
		ui.set_width(ui.available_width().min(360.0));
		design::card(ui, |ui| {
			let (banner, _) = ui
				.allocate_exact_size(egui::vec2(ui.available_width(), 96.0), egui::Sense::hover());
			if let Some(texture) = texture(1) {
				egui::Image::new(texture)
					.fit_to_exact_size(banner.size())
					.corner_radius(8)
					.paint_at(ui, banner);
			} else {
				avatars.paint_banner(ui, &profile, banner, egui::CornerRadius::same(8), media.0);
			}
			ui.add_space(8.0);
			if let Some(texture) = texture(0) {
				ui.add(
					egui::Image::new(texture)
						.fit_to_exact_size(egui::Vec2::splat(72.0))
						.corner_radius(36),
				);
			} else {
				avatars.show_profile_avatar(ui, &profile, 72.0, media.0);
			}
			crate::name_styles::label(
				ui,
				profile
					.user
					.cosmetics
					.as_deref()
					.and_then(|cosmetics| cosmetics.name_style.as_ref()),
				&name,
				egui::FontId::proportional(20.0),
				None,
				design::palette(ui).raised,
				design::palette(ui).text_strong,
				egui::Sense::hover(),
				true,
			);
			ui.label(&profile.username);
			if !pronouns.is_empty() {
				ui.weak(pronouns);
			}
			if !bio.is_empty() {
				ui.separator();
				let mut mentions = crate::profiles::ProfileSession::default();
				crate::markdown::Formatted::parse(&bio).show_with_images(
					ui,
					opening,
					&[],
					None,
					&mut mentions,
					(avatars, media.0, media.1),
					design::MessageCardSurface::Opaque,
				);
			}
		});
	});
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn drafts_keep_inheritance_empty_text_and_failed_images_separate() {
		let mut profile = crate::profiles::synthetic(
			test_support::demo_state().user.as_ref().unwrap(),
			Some(Id(10)),
		);
		profile.limited = false;
		let snapshot = Snapshot {
			profile,
			values: Values {
				bio: Some("before".into()),
				..Values::default()
			},
		};
		let mut draft = Draft::from_snapshot(&snapshot);
		draft.values.bio = Some(String::new());
		draft.values.pronouns = Some("they/them".into());
		let mut fresh = snapshot.values.clone();
		fresh.nick = Some("Fresh nickname".into());
		fresh.bio = Some("another edit".into());
		draft.rebase(&snapshot.values, &fresh);
		assert_eq!(draft.values.nick.as_deref(), Some("Fresh nickname"));
		assert_eq!(draft.values.bio, Some(String::new()));
		draft.refresh(&fresh);
		assert!(matches!(&draft.edit.bio, Patch::Value(value) if value.is_empty()));
		draft.values.bio = None;
		draft.refresh(&fresh);
		assert!(matches!(draft.edit.bio, Patch::Null));
	}
	#[test]
	fn image_picker_results_are_bound_to_guild_generation_and_editor_revision() {
		let mut editor = Editor {
			generation: Some(4),
			guild: Some(Id(10)),
			revision: 2,
			choosing: Some((2, false)),
			..Editor::default()
		};
		let ctx = egui::Context::default();
		for scope in [(3, Id(10), 2), (4, Id(11), 2), (4, Id(10), 1)] {
			assert!(editor.accept_image(&ctx, scope, Err("old result")).is_ok());
			assert!(editor.choosing.is_some());
		}
		assert!(editor.accept_image(&ctx, (4, Id(10), 2), Ok(None)).is_ok());
		assert!(editor.choosing.is_none());
	}
}
