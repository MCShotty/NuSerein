//! Explicit owned-equips only: no storefront purchases, claims, or optimistic ownership.
use crate::{DiscordApi, Failure};
use discord_protocol::profile_cosmetics as wire;
use model::{
	Id,
	profile_cosmetics::{Change, Current, Inventory, Snapshot},
};
use reqwest::Method;

impl DiscordApi {
	async fn read_cosmetic_inventory(&self) -> Result<Inventory, Failure> {
		// Individual products avoid inferring entitlement to unreturned grouped variants.
		let bytes = zeroize::Zeroizing::new(
			self.request_limited(
				Method::GET,
				"/users/@me/collectibles-purchases?variants_return_style=1",
				None,
				wire::MAX_INVENTORY_WIRE,
			)
			.await?,
		);
		wire::inventory(&bytes).map_err(|_| {
			Failure::ProtocolAt("Owned cosmetic inventory is unsupported or too large")
		})
	}
	async fn read_equipped_cosmetics(
		&self,
		user: Id,
		guild: Option<Id>,
	) -> Result<Current, Failure> {
		let query = guild.map_or_else(String::new, |guild| format!("guild_id={guild}&"));
		let path = format!(
			"/users/{user}/profile?{query}with_mutual_guilds=false&with_mutual_friends=false&with_mutual_friends_count=false"
		);
		let bytes = zeroize::Zeroizing::new(
			self.request_limited(Method::GET, &path, None, wire::MAX_CURRENT_WIRE)
				.await?,
		);
		wire::current(&bytes, user, guild).map_err(|_| {
			Failure::ProtocolAt("Full equipped cosmetic data is unavailable; reload to continue")
		})
	}
	pub(super) async fn owned_cosmetics(
		&self,
		user: Id,
		guild: Option<Id>,
		change: Option<Change>,
		nitro: bool,
	) -> Result<Box<Snapshot>, Failure> {
		if user.0 == 0 || guild.is_some_and(|guild| guild.0 == 0) {
			return Err(Failure::Protocol);
		}
		// Re-read ownership, expiry, own-profile scope and subscription before each explicit equip.
		let inventory = self.read_cosmetic_inventory().await?;
		let current = self.read_equipped_cosmetics(user, guild).await?;
		let snapshot = Snapshot { inventory, current };
		let Some(change) = change else {
			return Ok(Box::new(snapshot));
		};
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.map_or(0, |duration| duration.as_secs());
		if !change.allowed(&snapshot, nitro, now) {
			return Err(Failure::ProtocolAt(
				"Ownership or subscription changed; reload before equipping",
			));
		}
		if snapshot.current.equipped(change.kind) == change.sku {
			return Ok(Box::new(snapshot));
		}
		let (path, body, expected) =
			wire::encode(&snapshot.current, change).map_err(|_| Failure::Protocol)?;
		// One write attempt. Neither an uncertain reply nor a failed readback replays it.
		let bytes = zeroize::Zeroizing::new(
			self.request_limited(Method::PATCH, &path, Some(body), wire::MAX_CURRENT_WIRE)
				.await?,
		);
		wire::confirm(&bytes, user, guild, change.kind).map_err(|_| Failure::Ambiguous)?;
		let current = self.read_equipped_cosmetics(user, guild).await?;
		if !wire::matches(&expected, &current) {
			return Err(Failure::ProtocolAt(
				"Equipped cosmetic was not confirmed; reload before applying again",
			));
		}
		Ok(Box::new(Snapshot {
			inventory: snapshot.inventory,
			current,
		}))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::server_profile::tests::reply;
	use client_core::auth::SessionSecret;
	use model::profile_cosmetics::Kind;
	use serde_json::json;
	use std::{sync::Arc, time::Duration};
	use tokio::net::TcpListener;
	#[tokio::test]
	async fn equip_rechecks_ownership_preserves_unknown_ids_and_never_retries() {
		tokio::time::timeout(Duration::from_secs(10),async {
			let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
			let make_api=||{let mut api=DiscordApi::new(Arc::new(SessionSecret::from_owner_input("SYNTHETIC_OWNED_COSMETICS_TOKEN".into()).unwrap())).unwrap();api.base=format!("http://{}",listener.local_addr().unwrap());api};
			let inventory="GET /users/@me/collectibles-purchases?variants_return_style=1";
			let profile="GET /users/1/profile?with_mutual_guilds=false&with_mutual_friends=false&with_mutual_friends_count=false";
			let purchases=json!([{"name":"Owned effect","purchase_type":1,"items":[{"type":1,"sku_id":"8"}]}]).to_string();
			let current=json!({"user":{"id":"1"},"user_profile":{"collectibles":[{"type":91,"sku_id":"7"}]}}).to_string();
			let saved=json!({"user":{"id":"1"},"user_profile":{"collectibles":[{"type":91,"sku_id":"7"},{"type":1,"sku_id":"8"}]}}).to_string();
			let change=Change{kind:Kind::Effect,sku:Some(Id(8))};
			let server=async{reply(&listener,inventory,None,200,&purchases).await;reply(&listener,profile,None,200,&current).await;reply(&listener,"PATCH /users/@me/profile",Some(json!({"collectibles_sku_ids":["7","8"]})),200,"{}").await;reply(&listener,profile,None,200,&saved).await;};
			let api=make_api();let(result,())=tokio::join!(api.owned_cosmetics(Id(1),None,Some(change),false),server);assert_eq!(result.unwrap().current.equipped(Kind::Effect),Some(Id(8)));
			for(status,expected)in[(403,Failure::Forbidden),(429,Failure::RateLimited),(500,Failure::Ambiguous)]{
				let api=make_api();let server=async{reply(&listener,inventory,None,200,&purchases).await;reply(&listener,profile,None,200,&current).await;reply(&listener,"PATCH /users/@me/profile",Some(json!({"collectibles_sku_ids":["7","8"]})),status,"{}").await;};
				let(result,())=tokio::join!(api.owned_cosmetics(Id(1),None,Some(change),false),server);assert_eq!(result.err(),Some(expected));
			}
			let api=make_api();let server=async{reply(&listener,inventory,None,200,"[]").await;reply(&listener,profile,None,200,&current).await;};
			let(result,())=tokio::join!(api.owned_cosmetics(Id(1),None,Some(change),false),server);assert!(result.is_err());
			assert!(tokio::time::timeout(Duration::from_millis(20),listener.accept()).await.is_err());
		}).await.unwrap();
	}
}
