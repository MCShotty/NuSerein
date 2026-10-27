//! Sparse writes to Discord's unofficial normal-account guild settings route.
use crate::{DiscordApi, Failure};
use client_core::preferences_sync::Request;
use model::{
	Patch,
	guild_preferences::{Fields, Update},
};
use reqwest::Method;
use serde_json::{Value, json};

fn fields_body(fields: &Fields) -> Result<Value, Failure> {
	fn insert<T: serde::Serialize>(body: &mut Value, name: &str, value: &Patch<T>) {
		match value {
			Patch::Absent => {}
			Patch::Null => body[name] = Value::Null,
			Patch::Value(value) => body[name] = json!(value),
		}
	}
	let mut body = json!({});
	insert(&mut body, "flags", &fields.flags);
	insert(&mut body, "collapsed", &fields.collapsed);
	insert(&mut body, "muted", &fields.muted);
	insert(&mut body, "message_notifications", &fields.level);
	insert(&mut body, "suppress_everyone", &fields.suppress_everyone);
	insert(&mut body, "suppress_roles", &fields.suppress_roles);
	insert(&mut body, "hide_muted_channels", &fields.hide_muted);
	match fields.mute_until {
		Patch::Absent => {}
		Patch::Null => body["mute_config"] = json!({"end_time":null,"selected_time_window":-1}),
		Patch::Value(until) => {
			let end = discord_protocol::pins::format_cursor(i128::from(until) * 1_000_000_000)
				.map_err(|_| Failure::Protocol)?;
			body["mute_config"] = json!({"end_time":end,"selected_time_window":-1});
		}
	}
	Ok(body)
}
impl DiscordApi {
	pub(super) async fn channel_preferences(&self, request: &Request) -> Result<Update, Failure> {
		if request.guild.0 == 0 || !request.patch.valid() {
			return Err(Failure::Protocol);
		}
		let mut body = fields_body(&request.patch.fields)?;
		if !request.patch.channels.is_empty() {
			let mut rows = serde_json::Map::new();
			for row in &request.patch.channels {
				rows.insert(row.channel.to_string(), fields_body(&row.fields)?);
			}
			body["channel_overrides"] = Value::Object(rows);
		}
		let refresh = body.as_object().is_some_and(serde_json::Map::is_empty);
		let bytes = self
			.request_limited(
				Method::PATCH,
				&format!("/users/@me/guilds/{}/settings", request.guild),
				Some(body),
				512 * 1024,
			)
			.await
			.map_err(|error| {
				if !refresh
					&& matches!(
						error,
						Failure::Network | Failure::Protocol | Failure::Capacity
					) {
					Failure::Ambiguous
				} else {
					error
				}
			})?;
		let record: discord_protocol::notifications::Setting =
			discord_protocol::decode(&bytes).map_err(|_| Failure::Ambiguous)?;
		if record.guild_id != Some(request.guild) {
			return Err(Failure::Ambiguous);
		}
		// A refresh asks for the complete representation. Ordinary acknowledgements can be sparse.
		let full = refresh;
		let update = record.into_update(full);
		if !update.valid() {
			return Err(Failure::Ambiguous);
		}
		Ok(update)
	}
}
#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn sparse_body_keeps_unknown_bits_and_absent_fields() {
		let fields = Fields {
			flags: Patch::Value((1 << 55) | 4096),
			collapsed: Patch::Value(false),
			..Default::default()
		};
		assert_eq!(
			fields_body(&fields).unwrap(),
			json!({"flags":(1u64<<55)|4096,"collapsed":false})
		);
		assert_eq!(fields_body(&Fields::default()).unwrap(), json!({}));
		let fields = Fields {
			muted: Patch::Value(false),
			mute_until: Patch::Null,
			..Default::default()
		};
		assert_eq!(
			fields_body(&fields).unwrap(),
			json!({"muted":false,"mute_config":{"end_time":null,"selected_time_window":-1}})
		);
	}
	#[tokio::test]
	async fn preference_route_is_sparse_refreshes_with_empty_patch_and_never_retries_uncertainty() {
		use client_core::{Command, Event, auth::SessionSecret};
		use model::{
			Id,
			guild_preferences::{Row, Write},
		};
		use std::{sync::Arc, time::Duration};
		use tokio::net::TcpListener;
		tokio::time::timeout(Duration::from_secs(10),async{
			let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
			let mut api=DiscordApi::new(Arc::new(SessionSecret::from_owner_input("SYNTHETIC_CHANNEL_PREFERENCES_TOKEN".into()).unwrap())).unwrap();api.base=format!("http://{}",listener.local_addr().unwrap());
			let server=tokio::spawn(async move{
				let body=crate::channel_actions::tests::reply(&listener,"PATCH /users/@me/guilds/2/settings",200,json!({"guild_id":"2","version":4,"channel_overrides":[{"channel_id":"3","flags":(1u64<<55)|4096}],"hide_muted_channels":true})).await;
				assert_eq!(body,json!({"hide_muted_channels":true,"channel_overrides":{"3":{"flags":(1u64<<55)|4096}}}));
				let body=crate::channel_actions::tests::reply(&listener,"PATCH /users/@me/guilds/2/settings",200,json!({"guild_id":"2","channel_overrides":[]})).await;assert_eq!(body,json!({}));
				let body=crate::channel_actions::tests::reply(&listener,"PATCH /users/@me/guilds/2/settings",200,json!({"guild_id":"9","hide_muted_channels":false})).await;assert_eq!(body,json!({"hide_muted_channels":false}));
				assert!(tokio::time::timeout(Duration::from_millis(100),listener.accept()).await.is_err(),"an uncertain mutation must never be replayed");
			});
			let request=Request{generation:3,guild:Id(2),request:1,patch:Write{fields:Fields{hide_muted:Patch::Value(true),..Default::default()},channels:vec![Row{channel:Id(3),fields:Fields{flags:Patch::Value((1<<55)|4096),..Default::default()}}]}};
			let Event::ChannelPreferences(result)=api.execute(Command::ChannelPreferences(Box::new(request))).await else{panic!()};let update=result.result.unwrap();assert!(!update.full);assert_eq!(update.fields.muted,Patch::Absent);assert_eq!(update.version,Some(4));
			let update=api.channel_preferences(&Request{generation:3,guild:Id(2),request:2,patch:Write::default()}).await.unwrap();assert!(update.full);assert_eq!(update.fields.muted,Patch::Value(false));assert_eq!(update.channels,Some(vec![]));
			let failure=api.channel_preferences(&Request{generation:3,guild:Id(2),request:3,patch:Write{fields:Fields{hide_muted:Patch::Value(false),..Default::default()},..Default::default()}}).await;assert_eq!(failure,Err(Failure::Ambiguous));server.await.unwrap();
		}).await.unwrap();
	}
}
