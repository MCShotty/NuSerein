//! The unofficial message-report menu is data, never an executable action or URL.
use model::reporting::{Button, Element, Kind, MAX_WIRE, Menu, Node};
use serde_json::Value;

fn text(value: Option<&Value>) -> Result<String, &'static str> {
	let text = value
		.filter(|v| !v.is_null())
		.map_or(Ok(""), |v| v.as_str().ok_or("Invalid report text"))?;
	if text.len() > 4096 {
		return Err("Report text exceeds capacity");
	}
	Ok(text.to_owned())
}
fn number(value: Option<&Value>) -> Result<u32, &'static str> {
	value
		.and_then(Value::as_u64)
		.and_then(|n| n.try_into().ok())
		.ok_or("Invalid report node")
}
fn list(value: Option<&Value>) -> Result<&[Value], &'static str> {
	match value {
		None | Some(Value::Null) => Ok(&[]),
		Some(value) => value
			.as_array()
			.map(Vec::as_slice)
			.ok_or("Invalid report list"),
	}
}
fn options(value: Option<&Value>) -> Result<Vec<(String, String)>, &'static str> {
	let values = list(value)?;
	if values.len() > 64 {
		return Err("Too many report choices");
	}
	values
		.iter()
		.map(|v| Ok((text(v.get("value"))?, text(v.get("label"))?)))
		.collect()
}
fn element(value: &Value) -> Result<Option<Element>, &'static str> {
	if value.get("skip_if_unlocalized").and_then(Value::as_bool) == Some(true)
		&& value.get("is_localized").and_then(Value::as_bool) == Some(false)
	{
		return Ok(None);
	}
	let name = text(value.get("name"))?;
	let submit = value
		.get("should_submit_data")
		.and_then(Value::as_bool)
		.unwrap_or(false);
	let data = &value["data"];
	let kind = match value["type"]
		.as_str()
		.ok_or("Missing report element type")?
	{
		"checkbox" => {
			let values = list(Some(data))?;
			if values.len() > 64 {
				return Err("Too many report choices");
			}
			Kind::Checkbox(
				values
					.iter()
					.map(|v| {
						let parts = list(Some(v))?;
						if !(2..=3).contains(&parts.len()) {
							return Err("Invalid report checkbox");
						}
						let label = text(parts.get(1))?;
						let detail = text(parts.get(2))?;
						Ok((
							text(parts.first())?,
							if detail.is_empty() {
								label
							} else {
								format!("{label} — {detail}")
							},
						))
					})
					.collect::<Result<_, _>>()?,
			)
		}
		"dropdown" => Kind::Dropdown {
			title: text(data.get("title"))?,
			options: options(data.get("options"))?,
		},
		"free_text" => {
			// Unrecognized validation patterns must never be silently omitted on submission.
			if data
				.get("pattern")
				.and_then(Value::as_str)
				.is_some_and(|pattern| !pattern.is_empty())
			{
				return Err("This reporting form needs validation available in Discord");
			}
			Kind::Text {
				title: text(data.get("title"))?,
				placeholder: text(data.get("placeholder"))?,
				limit: data
					.get("character_limit")
					.and_then(Value::as_u64)
					.and_then(|n| n.try_into().ok())
					.filter(|n| *n <= 4096)
					.ok_or("Invalid report input capacity")?,
			}
		}
		"text" | "text_line_resource" => Kind::Notice(format!(
			"{}\n{}",
			text(data.get("header").or_else(|| data.get("title")))?,
			text(data.get("body"))?
		)),
		"external_link" => Kind::Link {
			label: text(data.get("link_text"))?,
			url: text(data.get("url"))?,
		},
		"message_preview" => Kind::Preview,
		_ if !submit => return Ok(None),
		_ => return Err("This reporting form is not supported; open the message in Discord"),
	};
	Ok(Some(Element { name, submit, kind }))
}

pub fn menu(bytes: &[u8]) -> Result<Menu, &'static str> {
	if bytes.len() > MAX_WIRE {
		return Err("Report menu exceeds capacity");
	}
	let value: Value = crate::decode(bytes).map_err(|_| "Invalid report menu")?;
	if value["name"] != "message"
		|| !matches!(
			value["postback_url"].as_str(),
			Some(
				"/api/reporting/message"
					| "/reporting/message"
					| "/api/v9/reporting/message"
					| "/api/v10/reporting/message"
			)
		) {
		return Err("Unapproved reporting endpoint");
	}
	let nodes = value["nodes"].as_object().ok_or("Missing report nodes")?;
	if nodes.len() > model::reporting::MAX_NODES {
		return Err("Too many report nodes");
	}
	let mut result = Menu {
		version: text(value.get("version"))?,
		variant: text(value.get("variant"))?,
		language: text(value.get("language"))?,
		root: number(value.get("root_node_id"))?,
		nodes: Default::default(),
	};
	if result.language.is_empty() {
		result.language = "en".into();
	}
	for (key, node) in nodes {
		let id: u32 = key.parse().map_err(|_| "Invalid report node identifier")?;
		if let Some(supplied) = node.get("id")
			&& number(Some(supplied))? != id
		{
			return Err("Mismatched report node identifier");
		}
		let children = list(node.get("children"))?;
		let elements = list(node.get("elements"))?;
		if children.len() > 64 || elements.len() > 16 {
			return Err("Report node exceeds capacity");
		}
		let button = match node["button"]["type"].as_str() {
			Some("next") => Button::Next(number(node["button"].get("target"))?),
			Some("submit") => Button::Submit,
			Some("done") => Button::Done,
			Some("cancel") => Button::Cancel,
			None if node.get("is_auto_submit").and_then(Value::as_bool) == Some(true) => {
				Button::Submit
			}
			None => Button::None,
			_ => return Err("Unsupported report navigation"),
		};
		result.nodes.insert(
			id,
			Node {
				header: text(node.get("header"))?,
				detail: [text(node.get("subheader"))?, text(node.get("info"))?]
					.into_iter()
					.filter(|s| !s.is_empty())
					.collect::<Vec<_>>()
					.join("\n"),
				children: children
					.iter()
					.map(|choice| {
						let pair = list(Some(choice))?;
						if pair.len() != 2 {
							return Err("Invalid report choice");
						}
						Ok((text(pair.first())?, number(pair.get(1))?))
					})
					.collect::<Result<_, _>>()?,
				elements: elements
					.iter()
					.map(element)
					.collect::<Result<Vec<_>, _>>()?
					.into_iter()
					.flatten()
					.collect(),
				button,
				required: node
					.get("is_multi_select_required")
					.and_then(Value::as_bool)
					.unwrap_or(false),
			},
		);
	}
	result
		.valid()
		.then_some(result)
		.ok_or("Unsupported or oversized report menu")
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn validates_the_actual_report_graph_and_rejects_foreign_postbacks_and_broken_nodes() {
		let mut wire = serde_json::json!({"name":"message","version":"1.0","variant":"1","root_node_id":1,"postback_url":"/api/reporting/message","nodes":{"1":{"id":1,"header":"Report","children":[["Spam",2]],"elements":[]},"2":{"id":2,"header":"Confirm","button":{"type":"submit"},"elements":[]}}});
		assert!(menu(&serde_json::to_vec(&wire).unwrap()).is_ok());
		wire["postback_url"] = "https://example.com/collect".into();
		assert!(menu(&serde_json::to_vec(&wire).unwrap()).is_err());
		wire["postback_url"] = "/api/reporting/message".into();
		wire["nodes"]["1"]["children"][0][1] = 3.into();
		assert!(menu(&serde_json::to_vec(&wire).unwrap()).is_err());
	}
}
