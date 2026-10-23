//! The native reader belongs to this app and never participates in voice transmission.
use crate::Desktop;
use eframe::egui;

impl Desktop {
	pub(crate) fn sync_message_speech(&mut self, ctx: &egui::Context) {
		if let Some((generation, channel, message)) = self.message_speech_target
			&& !self
				.messaging
				.message_speech_allowed(&self.state, generation, channel, message)
		{
			if let Some(speaker) = &self.message_speaker {
				let _ = speaker.replace(None);
			}
			self.message_speech_target = None;
			self.messaging.finish_message_speech(None);
		}
		if let Some(request) = self.messaging.take_message_speech() {
			if self.fixture_only || self.state.demo {
				self.messaging.finish_message_speech(Some(
					"System speech is disabled in this synthetic preview.",
				));
				return;
			}
			if request.text.is_some()
				&& !self.messaging.message_speech_allowed(
					&self.state,
					request.generation,
					request.channel,
					request.message,
				) {
				return;
			}
			if self.message_speaker.is_none() && request.text.is_some() {
				let wake = ctx.clone();
				match platform::message_speech::Speaker::new(move || wake.request_repaint()) {
					Ok(speaker) => self.message_speaker = Some(speaker),
					Err(error) => {
						self.messaging.finish_message_speech(Some(error));
						return;
					}
				}
			}
			self.message_speech_target = request
				.text
				.as_ref()
				.map(|_| (request.generation, request.channel, request.message));
			if let Some(speaker) = &self.message_speaker
				&& let Err(error) = speaker.replace(request.text)
			{
				self.message_speech_target = None;
				self.messaging.finish_message_speech(Some(error));
			}
		}
		if let Some(result) = self
			.message_speaker
			.as_ref()
			.and_then(|speaker| speaker.poll())
		{
			self.message_speech_target = None;
			self.messaging.finish_message_speech(result.err());
		}
	}
}
