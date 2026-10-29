//! Official onboarding transport. Screening/Server Guide use unofficial normal-account routes.
use crate::{DiscordApi, Failure};
use discord_protocol::{community_admin as wire, server_roles as roles_wire};
use model::{
	Id, Patch,
	community_admin::{Action, Data, Role, Section, Snapshot},
};
use reqwest::Method;
use serde_json::{Value, json};
struct Metadata {
	roles: model::server_roles::Catalog,
	owner: Option<Id>,
	header: Option<String>,
}
impl DiscordApi {
	async fn community_metadata(&self, guild: Id) -> Result<Metadata, Failure> {
		let bytes = self
			.request_limited(
				Method::GET,
				&format!("/guilds/{guild}"),
				None,
				wire::MAX_WIRE,
			)
			.await?;
		let raw = wire::raw(&bytes).map_err(|_| Failure::Protocol)?;
		let roles = roles_wire::catalog(&bytes, guild)
			.map_err(|_| Failure::ProtocolAt("Community role metadata was unsupported"))?;
		let header = raw["home_header"].as_str().map(str::to_owned);
		if header
			.as_ref()
			.is_some_and(|s| !model::valid_avatar_hash(s))
		{
			return Err(Failure::Protocol);
		}
		let owner = raw["owner_id"].as_str().and_then(|s| s.parse::<Id>().ok());
		Ok(Metadata {
			roles,
			owner,
			header,
		})
	}
	async fn community_raw(&self, guild: Id, section: Section) -> Result<Value, Failure> {
		// A missing configuration is an editable empty draft. 403 remains permission denied.
		let path = format!("/guilds/{guild}/{}", section.path());
		match self.request_optional_limited(&path, wire::MAX_WIRE).await? {
			Some(bytes) if !bytes.is_empty() => wire::raw(&bytes)
				.map_err(|_| Failure::ProtocolAt("Community settings response was unsupported")),
			_ => Ok(json!({})),
		}
	}
	fn community_projection(
		guild: Id,
		section: Section,
		raw: &Value,
		metadata: &Metadata,
	) -> Result<Snapshot, Failure> {
		let enabled = metadata
			.roles
			.features
			.iter()
			.any(|f| f == "MEMBER_VERIFICATION_GATE_ENABLED");
		let manual = metadata
			.roles
			.features
			.iter()
			.any(|f| f == "MEMBER_VERIFICATION_MANUAL_APPROVAL");
		let mut snapshot = wire::parse(guild, section, raw, enabled, manual)
			.map_err(|_| Failure::ProtocolAt("Community settings response was unsupported"))?;
		if let Data::Guide(guide) = &mut snapshot.data {
			guide.header.clone_from(&metadata.header);
			guide.header_available = metadata.roles.features.iter().any(|f| f == "BANNER");
		}
		snapshot.roles = metadata
			.roles
			.items
			.iter()
			.map(|r| Role {
				id: r.id,
				name: r.name.clone(),
				bits: r.permissions,
				position: r.position,
				managed: r.managed,
			})
			.collect();
		if !snapshot.valid() {
			return Err(Failure::ResponseLimit);
		}
		Ok(snapshot)
	}
	async fn validate_guide_authors(
		&self,
		guild: Id,
		data: &Data,
		metadata: &Metadata,
	) -> Result<(), Failure> {
		let Data::Guide(guide) = data else {
			return Ok(());
		};
		// At most ten authors, sequential bounded requests; the service remains authoritative.
		for author in &guide.authors {
			if metadata.owner == Some(*author) {
				continue;
			}
			let member = self.admin_member(guild, *author).await?;
			let mut permissions = 0;
			for role in std::iter::once(guild).chain(member.roles.iter().copied()) {
				permissions |= metadata
					.roles
					.items
					.iter()
					.find(|r| r.id == role)
					.ok_or(Failure::Forbidden)?
					.permissions;
			}
			if permissions
				& (model::permissions::ADMINISTRATOR
					| model::permissions::MANAGE_GUILD
					| model::permissions::MANAGE_ROLES)
				== 0
			{
				return Err(Failure::Forbidden);
			}
		}
		Ok(())
	}
	async fn community_artwork(
		&self,
		guild: Id,
		action: &Action,
		current: Snapshot,
	) -> Result<Snapshot, Failure> {
		let Action::Artwork {
			channel,
			resource,
			before,
			image,
		} = action
		else {
			return Err(Failure::Protocol);
		};
		let Data::Guide(guide) = &current.data else {
			return Err(Failure::Protocol);
		};
		let old = if let Some(channel) = channel {
			let rows = if *resource {
				&guide.resources
			} else {
				&guide.actions
			};
			&rows
				.iter()
				.find(|r| r.channel == *channel)
				.ok_or(Failure::Forbidden)?
				.icon
		} else {
			if !guide.header_available {
				return Err(Failure::Forbidden);
			}
			&guide.header
		};
		if old != before {
			return Err(Failure::ProtocolAt(
				"Guide artwork changed; reload before saving",
			));
		}
		let data = match image {
			Patch::Null => Value::Null,
			Patch::Value(uri)
				if discord_protocol::group_actions::valid_image_data_uri(uri, 512) =>
			{
				json!(uri)
			}
			_ => return Err(Failure::Protocol),
		};
		let (method, path, body) = if let Some(channel) = channel {
			(
				if *resource {
					Method::PUT
				} else {
					Method::PATCH
				},
				format!(
					"/guilds/{guild}/{}/{channel}",
					if *resource {
						"resource-channels"
					} else {
						"new-member-actions"
					}
				),
				json!({"icon":data}),
			)
		} else {
			(
				Method::PATCH,
				format!("/guilds/{guild}"),
				json!({"home_header":data}),
			)
		};
		self.request_limited(method, &path, Some(body), wire::MAX_WIRE)
			.await
			.map_err(community_write_failure)?;
		// A successful write followed by failed reconciliation is uncertain and is never retried.
		let metadata = self
			.community_metadata(guild)
			.await
			.map_err(community_reconcile_failure)?;
		let raw = self
			.community_raw(guild, Section::Guide)
			.await
			.map_err(community_reconcile_failure)?;
		let result = Self::community_projection(guild, Section::Guide, &raw, &metadata)
			.map_err(community_reconcile_failure)?;
		let Data::Guide(guide) = &result.data else {
			return Err(Failure::Ambiguous);
		};
		let hash = if let Some(channel) = channel {
			let rows = if *resource {
				&guide.resources
			} else {
				&guide.actions
			};
			&rows
				.iter()
				.find(|r| r.channel == *channel)
				.ok_or(Failure::Ambiguous)?
				.icon
		} else {
			&guide.header
		};
		if matches!(image, Patch::Null) && hash.is_some()
			|| matches!(image, Patch::Value(_)) && (hash.is_none() || hash == before)
		{
			return Err(Failure::Ambiguous);
		}
		Ok(result)
	}
	pub(super) async fn community_admin(
		&self,
		guild: Id,
		action: &Action,
	) -> Result<Snapshot, Failure> {
		if guild.0 == 0 || !action.valid() {
			return Err(Failure::Protocol);
		}
		let metadata = self.community_metadata(guild).await?;
		let fresh = self.community_raw(guild, action.section()).await?;
		let before_write = Self::community_projection(guild, action.section(), &fresh, &metadata)?;
		if matches!(action, Action::Artwork { .. }) {
			return self.community_artwork(guild, action, before_write).await;
		}
		let Action::Edit {
			before,
			after,
			pending,
		} = action
		else {
			return Ok(before_write);
		};
		self.validate_guide_authors(guild, after, &metadata).await?;
		// Revalidate assigned roles against fresh metadata even if a gateway role-delete raced.
		if let Data::Onboarding(onboarding) = &**after {
			for role in onboarding
				.prompts
				.iter()
				.flat_map(|p| &p.options)
				.flat_map(|o| &o.roles)
			{
				if !metadata
					.roles
					.items
					.iter()
					.any(|r| r.id == *role && r.id != guild && !r.managed)
				{
					return Err(Failure::Forbidden);
				}
			}
		}
		let (enabled, manual) = match &before_write.data {
			Data::Screening(s) => (s.enabled, s.manual_approval),
			_ => (false, false),
		};
		let body =
			wire::edit(guild, &fresh, before, after, *pending, enabled, manual).map_err(|_| {
				Failure::ProtocolAt("Community settings changed; reload before saving these edits")
			})?;
		let method = if action.section() == Section::Screening {
			Method::PATCH
		} else {
			Method::PUT
		};
		let bytes = self
			.request_limited(
				method,
				&format!("/guilds/{guild}/{}", action.section().path()),
				Some(body),
				wire::MAX_WIRE,
			)
			.await
			.map_err(community_write_failure)?;
		let raw = wire::raw(&bytes).map_err(|_| Failure::Ambiguous)?;
		// Screening enablement lives in guild features, not the verification response.
		let after_metadata = if action.section() == Section::Screening {
			self.community_metadata(guild)
				.await
				.map_err(community_reconcile_failure)?
		} else {
			metadata
		};
		let result = Self::community_projection(guild, action.section(), &raw, &after_metadata)
			.map_err(community_reconcile_failure)?;
		if !wire::confirms(before, after, &result.data) {
			return Err(Failure::Ambiguous);
		}
		Ok(result)
	}
}
fn community_write_failure(f: Failure) -> Failure {
	match f {
		Failure::Network
		| Failure::Protocol
		| Failure::Capacity
		| Failure::ProtocolAt(_)
		| Failure::CapacityAt(_) => Failure::Ambiguous,
		other => other,
	}
}
fn community_reconcile_failure(f: Failure) -> Failure {
	if f.ends_session() {
		f
	} else {
		Failure::Ambiguous
	}
}

#[cfg(test)]
pub(crate) mod tests {
	use super::*;
	use client_core::auth::SessionSecret;
	use std::{
		sync::{Arc, Mutex},
		time::Duration,
	};
	use tokio::{
		io::{AsyncReadExt, AsyncWriteExt},
		net::TcpListener,
	};
	type Captured = Arc<Mutex<Vec<(String, Value)>>>;
	const METADATA: &str = r#"{"id":"1","owner_id":"7","roles":[{"id":"1","name":"@everyone","permissions":"0","position":0,"hoist":false,"mentionable":false,"managed":false}],"features":[]}"#;
	pub(crate) async fn fixture(
		responses: Vec<(&'static str, String)>,
	) -> (DiscordApi, Captured, tokio::task::JoinHandle<()>) {
		let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let mut api = DiscordApi::new(Arc::new(
			SessionSecret::from_owner_input("SYNTHETIC_COMMUNITY_TOKEN".into()).unwrap(),
		))
		.unwrap();
		api.base = format!("http://{}", listener.local_addr().unwrap());
		let captured = Arc::new(Mutex::new(vec![]));
		let capture = captured.clone();
		let server = tokio::spawn(async move {
			for (status, body) in responses {
				let (mut stream, _) = listener.accept().await.unwrap();
				let mut header = Vec::new();
				while !header.ends_with(b"\r\n\r\n") {
					assert!(header.len() < 16384);
					header.push(stream.read_u8().await.unwrap());
				}
				let header = String::from_utf8(header).unwrap();
				let len = header
					.lines()
					.find_map(|line| {
						line.to_ascii_lowercase()
							.strip_prefix("content-length:")
							.and_then(|v| v.trim().parse::<usize>().ok())
					})
					.unwrap_or(0);
				assert!(len <= wire::MAX_WIRE);
				let mut request_body = vec![0; len];
				stream.read_exact(&mut request_body).await.unwrap();
				capture.lock().unwrap().push((
					header.lines().next().unwrap().into(),
					serde_json::from_slice(&request_body).unwrap_or(Value::Null),
				));
				stream
					.write_all(
						format!(
							"HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
							body.len()
						)
						.as_bytes(),
					)
					.await
					.unwrap();
			}
		});
		(api, captured, server)
	}
	fn mode_edit() -> Action {
		let before = Data::Onboarding(model::community_admin::Onboarding::default());
		let mut after = before.clone();
		let Data::Onboarding(o) = &mut after else {
			panic!()
		};
		o.mode = 1;
		Action::Edit {
			before: Box::new(before),
			after: Box::new(after),
			pending: Default::default(),
		}
	}
	#[tokio::test]
	async fn community_malformed_success_is_unknown_and_never_retried() {
		tokio::time::timeout(Duration::from_secs(15), async {
			let (api, captured, server) = fixture(vec![
				("200 OK", METADATA.into()),
				(
					"200 OK",
					r#"{"guild_id":"1","enabled":false,"mode":0,"prompts":[]}"#.into(),
				),
				("200 OK", "invalid json".into()),
			])
			.await;
			assert_eq!(
				api.community_admin(Id(1), &mode_edit()).await.unwrap_err(),
				Failure::Ambiguous
			);
			server.await.unwrap();
			let requests = captured.lock().unwrap();
			assert_eq!(requests.len(), 3);
			assert!(requests[2].0.starts_with("PUT /guilds/1/onboarding "));
			assert_eq!(requests[2].1, json!({"mode":1}));
		})
		.await
		.unwrap();
	}
	#[tokio::test]
	async fn community_fresh_conflict_and_forbidden_load_never_write() {
		tokio::time::timeout(Duration::from_secs(15), async {
			let (api, captured, server) = fixture(vec![
				("200 OK", METADATA.into()),
				(
					"200 OK",
					r#"{"guild_id":"1","enabled":false,"mode":1,"prompts":[]}"#.into(),
				),
				("200 OK", METADATA.into()),
				("403 Forbidden", "{}".into()),
			])
			.await;
			assert!(matches!(
				api.community_admin(Id(1), &mode_edit()).await,
				Err(Failure::ProtocolAt(_))
			));
			assert_eq!(
				api.community_admin(Id(1), &Action::Load(Section::Guide))
					.await
					.unwrap_err(),
				Failure::Forbidden
			);
			server.await.unwrap();
			assert_eq!(captured.lock().unwrap().len(), 4);
		})
		.await
		.unwrap();
	}
	#[tokio::test]
	async fn community_screening_partial_write_preserves_pending_choice_and_requires_reload() {
		tokio::time::timeout(Duration::from_secs(15), async {
			let metadata = METADATA.replace("\"features\":[]", "\"features\":[\"MEMBER_VERIFICATION_GATE_ENABLED\"]");
			let form = json!({"form_fields":[{"field_type":"TERMS","label":"Rules","required":true,"values":["Be kind"],"automations":["keep"]}]});
			let before = wire::parse(Id(1), Section::Screening, &form, true, false).unwrap().data;
			let mut after = before.clone();
			let Data::Screening(s) = &mut after else { panic!() };
			s.enabled = false;
			let action = Action::Edit {
				before: Box::new(before), after: Box::new(after), pending: model::community_admin::PendingApplications::Reject,
			};
			let (api, captured, server) = fixture(vec![
				("200 OK", metadata), ("200 OK", form.to_string()), ("200 OK", form.to_string()), ("503 Service Unavailable", "{}".into()),
			]).await;
			assert_eq!(api.community_admin(Id(1), &action).await.unwrap_err(), Failure::Ambiguous);
			server.await.unwrap();
			let requests = captured.lock().unwrap();
			assert_eq!(requests.len(), 4);
			assert!(requests[2].0.starts_with("PATCH /guilds/1/member-verification "));
			assert_eq!(requests[2].1, json!({"enabled":false,"bulk_action":"REJECTED"}));
		}).await.unwrap();
	}
}
