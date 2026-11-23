//! Standalone synthetic integration test for crates/discord-protocol/tests/.
//! No account, environment configuration, file IO or networking. Run with --nocapture.
use discord_protocol::{MessageDto, history};
use serde_json::{Value, json};

fn row(id: u32) -> Value {
	json!({"id":id.to_string(),"channel_id":"7","author":{"id":"2","username":"fixture"},"content":"synthetic"})
}

fn trace(label: &str, fields: Value) {
	let mut middle = row(12);
	middle
		.as_object_mut()
		.unwrap()
		.extend(fields.as_object().unwrap().clone());
	let raw = serde_json::to_vec(&middle).unwrap();
	let strict = serde_path_to_error::deserialize::<_, MessageDto>(
		&mut serde_json::Deserializer::from_slice(&raw),
	);
	// Only hard-coded synthetic inputs reach this standalone test, never live traffic.
	let strict = strict
		.err()
		.map_or_else(|| "accepted".to_string(), |e| e.path().to_string());
	let bytes = serde_json::to_vec(&[row(13), middle, row(11)]).unwrap();
	match history::page(&bytes) {
		Ok(rows) => {
			assert_eq!(
				rows.iter().map(|r| r.id.0).collect::<Vec<_>>(),
				[13, 12, 11]
			);
			let m = &rows[1];
			println!(
				"{label}: strict={strict}; page=3; attachments={}; embeds={}; limited_cards={}; deleted={}",
				m.attachments.len(),
				m.embeds.len(),
				m.embeds.iter().filter(|e| e.limited).count(),
				m.reply_deleted
			);
		}
		Err(category) => println!("{label}: strict={strict}; page={category:?}; rows_returned=0"),
	}
}

#[test]
fn synthetic_history_media_trace() {
	let file = json!({"id":"4","filename":"SPOILER_fixture.png","flags":8,"width":32});
	trace("control", json!({"attachments":[file.clone()]}));
	for (label, key, value) in [
		("nullable width", "width", Value::Null),
		("string width", "width", json!("unknown")),
		("negative width", "width", json!(-1)),
		("null size", "size", Value::Null),
		("string flags", "flags", json!("8")),
		("null filename", "filename", Value::Null),
		("unknown extension field", "fixture_extra", json!({"v":1})),
		("invalid attachment identity", "id", json!(0)),
	] {
		let mut changed = file.clone();
		changed[key] = value;
		trace(label, json!({"attachments":[changed]}));
	}
	for n in [10u32, 11] {
		let files = (0..n)
			.map(|i| json!({"id":(100+i).to_string(),"filename":"ok.png"}))
			.collect::<Vec<_>>();
		trace(
			if n == 10 {
				"10 attachments"
			} else {
				"11 attachments"
			},
			json!({"attachments":files}),
		);
		trace(
			if n == 10 { "10 embeds" } else { "11 embeds" },
			json!({"embeds":vec![json!({"title":"ok"}); n as usize]}),
		);
	}
	trace("null attachment list", json!({"attachments":null}));
	trace("null embed list", json!({"embeds":null}));
	trace("non-list embeds", json!({"embeds":{}}));
	trace(
		"bad card between good cards",
		json!({"embeds":[{"title":"ok"},{"video":{"height":"unknown"}},{"title":"ok"}]}),
	);
	let bad = json!({"id":"4","filename":"ok.png","width":"unknown"});
	trace(
		"bad file between good files",
		json!({"attachments":[{"id":"3"},bad.clone(),{"id":"5"}]}),
	);
	trace(
		"bad file only in quote",
		json!({"type":19,"message_reference":{"message_id":"8","channel_id":"7"},"referenced_message":{"id":"8","channel_id":"7","author":{"id":"2","username":"fixture"},"attachments":[bad.clone()]}}),
	);
	trace(
		"bad file only in forward",
		json!({"message_reference":{"type":1,"message_id":"8","channel_id":"7"},"message_snapshots":[{"message":{"type":0,"attachments":[bad]}}]}),
	);
	trace("invalid message identity", json!({"id":"0"}));
}
