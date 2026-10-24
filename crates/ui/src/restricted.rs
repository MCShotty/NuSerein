//! Session-only, bounded consent to display blocked/ignored message bodies.
use client_core::{State, user_actions::ContentRestriction};
use model::{Id, Message};
use std::{
	collections::BTreeMap,
	hash::{DefaultHasher, Hash, Hasher},
};

const MAX_REVEALED: usize = 500;

// Avoid egui ID construction here: its debug registry retains source formatting.
fn fingerprint(value: impl Hash) -> u64 {
	let mut hash = DefaultHasher::new();
	value.hash(&mut hash);
	hash.finish()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Key {
	pub id: Id,
	pub author: Id,
	content: u64,
}
impl Key {
	pub fn message(message: &Message) -> Self {
		Self {
			id: message.id,
			author: message.author.id,
			content: fingerprint((
				&message.content,
				&message.embeds,
				&message.attachments,
				&message.components,
				&message.extra_content,
				&message.poll,
				&message.sticker_items,
				message.kind,
				message.edited,
				(
					message.flags,
					message.embeds_suppressed,
					message.unsupported,
					message.forwarded,
				),
				message.interaction.as_ref().map(|interaction| {
					(
						interaction.user.id,
						&interaction.user.name,
						&interaction.user.avatar,
						&interaction.command,
					)
				}),
			)),
		}
	}
	pub fn hit(hit: &model::SearchHit) -> Self {
		Self {
			id: hit.id,
			author: hit.author.id,
			content: fingerprint((&hit.excerpt, &hit.embeds, &hit.attachments)),
		}
	}
}

#[derive(Default)]
pub(super) struct Visibility {
	identity: Option<(u64, u64)>,
	revealed: BTreeMap<Id, Key>,
}
impl Visibility {
	/// Revoke disclosure when the account or its relationship state changes. Store IDs and
	/// hashes only, and retire consent together with the view's bounded message snapshot.
	pub fn sync(&mut self, state: &State, mut current: impl FnMut(Id) -> Option<Key>) -> bool {
		let identity = (state.generation, state.relationship_view());
		let previous = self.revealed.len();
		if self.identity != Some(identity) {
			self.revealed.clear();
			self.identity = Some(identity);
		} else {
			self.revealed
				.retain(|id, key| current(*id).is_some_and(|current| current == *key));
		}
		self.revealed.len() != previous
	}
	pub fn allows(&self, state: &State, key: Key) -> bool {
		state.user_content_restriction(key.author).is_none()
			|| (self.identity == Some((state.generation, state.relationship_view()))
				&& self.revealed.get(&key.id) == Some(&key))
	}
	/// Draw before any author artwork, text, rich content or media renderer. The button is
	/// deliberate consent for this one body, and may be used again to collapse it.
	pub fn show(&mut self, ui: &mut egui::Ui, state: &State, key: Key) -> bool {
		let identity = (state.generation, state.relationship_view());
		if self.identity != Some(identity) {
			self.revealed.clear();
			self.identity = Some(identity);
		}
		let Some(restriction) = state.user_content_restriction(key.author) else {
			return true;
		};
		let visible = self.allows(state, key);
		let changed = ui
			.push_id(("restricted-message", key.id), |ui| {
				ui.horizontal_wrapped(|ui| {
					ui.weak(label(restriction));
					ui.small_button(crate::i18n::translate(if visible {
						"restricted-hide-message"
					} else {
						"restricted-show-message"
					}))
					.clicked()
				})
				.inner
			})
			.inner;
		if changed {
			if visible {
				self.revealed.remove(&key.id);
			} else {
				while self.revealed.len() >= MAX_REVEALED {
					self.revealed.pop_first();
				}
				self.revealed.insert(key.id, key);
			}
			ui.ctx().request_repaint();
		}
		self.allows(state, key)
	}
}

pub(super) fn label(restriction: ContentRestriction) -> String {
	crate::i18n::translate(match restriction {
		ContentRestriction::Blocked => "restricted-blocked-message",
		ContentRestriction::Ignored => "restricted-ignored-message",
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::test_ui::logical_text as collect;
	fn frame(
		ctx: &egui::Context,
		state: &mut State,
		view: &mut crate::timeline::TimelineView,
		avatars: &mut crate::avatars::Avatars,
		width: f32,
		events: Vec<egui::Event>,
	) -> Vec<(String, egui::Rect)> {
		let output = ctx.run_ui(
			egui::RawInput {
				screen_rect: Some(egui::Rect::from_min_size(
					egui::Pos2::ZERO,
					egui::vec2(width, 700.0),
				)),
				events,
				..Default::default()
			},
			|ui| {
				view.show(
					ui,
					state,
					&mut None,
					&mut None,
					(avatars, &mut crate::profiles::ProfileSession::default()),
					None,
				);
			},
		);
		let mut labels = Vec::new();
		for shape in &output.shapes {
			collect(&shape.shape, &mut labels);
		}
		output.drop_without_applying_deltas();
		labels
	}
	#[test]
	fn timeline_reveal_is_explicit_media_stays_hidden_and_changed_bodies_recollapse() {
		for (width, ignored) in [(360.0, false), (900.0, true)] {
			let ctx = egui::Context::default();
			crate::design::apply(&ctx);
			let mut state = test_support::demo_state();
			state.demo = false;
			state.auth = client_core::auth::AuthState::Authenticated;
			state.gateway_connected = true;
			state.freshness = model::Freshness::Fresh;
			state.history_pending = false;
			state.older_exhausted = false;
			let channel = state.selected.unwrap();
			let mut secret = test_support::message(500, channel);
			secret.author.id = Id(900);
			secret.content = "Restricted sensitive caption".into();
			secret.author.avatar = Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into());
			secret.attachments = vec![model::Attachment {
				id: Id(800),
				filename: "private-hidden.png".into(),
				description: None,
				content_type: Some("image/png".into()),
				size: 128,
				spoiler: false,
				duration_ms: None,
				waveform: vec![],
				media: model::EmbedMedia {
					url: Some(
						"https://cdn.discordapp.com/attachments/10/800/private-hidden.png".into(),
					),
					width: 120,
					height: 80,
					..Default::default()
				},
			}];
			let mut reply = test_support::message(501, channel);
			reply.author.id = Id(901);
			reply.content = "Ordinary reply".into();
			reply.reply_to = Some(secret.id);
			state
				.timeline
				.seed_cache(vec![secret.clone(), reply.clone()])
				.unwrap();
			state.revision += 1;
			state
				.channels
				.iter_mut()
				.find(|entry| entry.id == channel)
				.unwrap()
				.last_message = Some(reply.id);
			let relationship = if ignored {
				client_core::user_actions::Event::Restriction {
					user: secret.author.id,
					ignored: Some(true),
					profile: Some((secret.author.clone(), "synthetic".into())),
				}
			} else {
				client_core::user_actions::Event::Relationship {
					user: secret.author.id,
					blocked: true,
				}
			};
			state.apply(client_core::Envelope {
				generation: state.generation,
				event: client_core::Event::UserAction(relationship),
			});
			let mut view = crate::timeline::TimelineView::default();
			let mut avatars = crate::avatars::Avatars::default();
			let mut labels = Vec::new();
			for _ in 0..5 {
				labels = frame(&ctx, &mut state, &mut view, &mut avatars, width, vec![]);
			}
			assert!(
				!labels
					.iter()
					.any(|(label, _)| label.contains("Restricted sensitive caption"))
			);
			assert!(labels.iter().any(|(label, _)| label == "Ordinary reply"));
			assert!(
				!avatars
					.take_requests()
					.iter()
					.any(|url| url.contains("private-hidden") || url.contains("900-"))
			);
			let reveal = labels
				.iter()
				.find(|(label, _)| label == &crate::i18n::translate("restricted-show-message"))
				.expect("native reveal control")
				.1
				.center();
			for pressed in [true, false] {
				frame(
					&ctx,
					&mut state,
					&mut view,
					&mut avatars,
					width,
					vec![
						egui::Event::PointerMoved(reveal),
						egui::Event::PointerButton {
							pos: reveal,
							button: egui::PointerButton::Primary,
							pressed,
							modifiers: egui::Modifiers::NONE,
						},
					],
				);
			}
			for _ in 0..5 {
				labels = frame(&ctx, &mut state, &mut view, &mut avatars, width, vec![]);
			}
			assert!(
				labels
					.iter()
					.any(|(label, _)| label.contains("Restricted sensitive caption")),
				"explicit reveal displays the selected body: {labels:?}"
			);
			assert!(
				avatars
					.take_requests()
					.iter()
					.any(|url| url.contains("private-hidden")),
				"media is requested only after reveal"
			);
			secret.content = "Edited restricted sensitive caption".into();
			state.timeline.seed_cache(vec![secret, reply]).unwrap();
			state.revision += 1;
			for _ in 0..3 {
				labels = frame(&ctx, &mut state, &mut view, &mut avatars, width, vec![]);
			}
			assert!(
				!labels
					.iter()
					.any(|(label, _)| label.contains("sensitive caption")),
				"a changed hidden body needs fresh consent"
			);
		}
	}
}
