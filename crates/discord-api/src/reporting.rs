//! Unofficial V3 message reports. Only the fixed first-party endpoint may receive a report.
use crate::{DiscordApi, Failure};
use client_core::reporting::{Outcome, Request};
use model::reporting::{MAX_STEPS, MAX_WIRE};
use reqwest::Method;

impl DiscordApi {
	pub(super) async fn report(&self, request: &Request) -> Result<Outcome, Failure> {
		if request.target.channel.0 == 0 || request.target.message.0 == 0 {
			return Err(Failure::Protocol);
		}
		let Some(submission) = &request.submission else {
			let bytes = self
				.request_limited(Method::GET, "/reporting/menu/message", None, MAX_WIRE)
				.await?;
			return discord_protocol::reporting::menu(&bytes)
				.map(|menu| Outcome::Menu(Box::new(menu)))
				.map_err(Failure::ProtocolAt);
		};
		if submission.version != "1.0"
			|| submission.variant.len() > 256
			|| submission.language.len() > 32
			|| submission.breadcrumbs.is_empty()
			|| submission.breadcrumbs.len() > MAX_STEPS
			|| submission
				.breadcrumbs
				.iter()
				.enumerate()
				.any(|(i, id)| submission.breadcrumbs[..i].contains(id))
			|| submission.elements.len() > 64
			|| submission.bytes() > 32 * 1024
			|| submission.elements.iter().any(|(name, values)| {
				name.is_empty()
					|| name.len() > 128
					|| values.len() > 64
					|| values
						.iter()
						.any(|text| text.len() > 16 * 1024 || text.contains('\0'))
			}) {
			return Err(Failure::Protocol);
		}
		let body = serde_json::json!({"name":"message", "version":submission.version, "variant":submission.variant, "language":submission.language, "breadcrumbs":submission.breadcrumbs, "elements":submission.elements, "channel_id":request.target.channel, "message_id":request.target.message});
		let bytes = self
			.request_limited(Method::POST, "/reporting/message", Some(body), 4096)
			.await
			.map_err(|failure| match failure {
				Failure::Network | Failure::Protocol | Failure::Capacity => Failure::Ambiguous,
				other => other,
			})?;
		#[derive(serde::Deserialize)]
		struct Confirmation {
			report_id: model::Id,
		}
		let confirmation: Confirmation =
			discord_protocol::decode(&bytes).map_err(|_| Failure::Ambiguous)?;
		if confirmation.report_id.0 == 0 {
			return Err(Failure::Ambiguous);
		}
		Ok(Outcome::Sent(confirmation.report_id))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::SessionSecret;
	use std::{sync::Arc, time::Duration};
	use tokio::{
		io::{AsyncReadExt, AsyncWriteExt},
		net::TcpListener,
	};
	#[tokio::test]
	async fn reports_use_fixed_routes_exact_targets_and_never_repeat_uncertain_submissions() {
		tokio::time::timeout(Duration::from_secs(10), async {
			let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
			let mut api = DiscordApi::new(Arc::new(SessionSecret::from_owner_input("SYNTHETIC_REPORT_TOKEN".into()).unwrap())).unwrap();
			api.base = format!("http://{}", listener.local_addr().unwrap());
			let server = tokio::spawn(async move {
				for step in 0..3 {
					let (mut socket,_) = listener.accept().await.unwrap();
					let mut received = Vec::new();
					let (header_end, length) = loop {
						let mut buffer = [0;4096]; let count = socket.read(&mut buffer).await.unwrap(); assert_ne!(count,0); received.extend_from_slice(&buffer[..count]); assert!(received.len() < 32768);
						if let Some(end) = received.windows(4).position(|part| part == b"\r\n\r\n") {
							let headers = String::from_utf8_lossy(&received[..end]);
							let length = headers.lines().find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length:").and_then(|value| value.trim().parse::<usize>().ok())).unwrap_or(0);
							if received.len() >= end+4+length { break (end+4,length); }
						}
					};
					let headers = String::from_utf8_lossy(&received[..header_end]);
					let body = if step == 0 {
						assert!(headers.starts_with("GET /reporting/menu/message "));
						serde_json::json!({"name":"message","version":"1.0","variant":"test","root_node_id":1,"postback_url":"/api/reporting/message","nodes":{"1":{"header":"Confirm","children":[],"button":{"type":"submit"},"elements":[]}}}).to_string()
					} else {
						assert!(headers.starts_with("POST /reporting/message "));
						let body: serde_json::Value = serde_json::from_slice(&received[header_end..header_end+length]).unwrap();
						assert_eq!(body["channel_id"], "17"); assert_eq!(body["message_id"], "18"); assert_eq!(body["breadcrumbs"], serde_json::json!([1]));
						if step == 2 { drop(socket); assert!(tokio::time::timeout(Duration::from_millis(200),listener.accept()).await.is_err()); continue; }
						"{\"report_id\":\"99\"}".into()
					};
					socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
				}
			});
			let target = model::reporting::Target { channel: model::Id(17), message: model::Id(18) };
			let Outcome::Menu(menu) = api.report(&Request { target, request: 1, submission: None }).await.unwrap() else { panic!() };
			let submission = model::reporting::Submission { version: menu.version, variant: menu.variant, language: menu.language, breadcrumbs: vec![1], elements: Default::default() };
			let request = Request { target, request: 2, submission: Some(Box::new(submission)) };
			assert!(matches!(api.report(&request).await, Ok(Outcome::Sent(model::Id(99)))));
			assert!(matches!(api.report(&request).await, Err(Failure::Ambiguous)));
			server.await.unwrap();
		}).await.unwrap();
	}
}
