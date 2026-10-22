//! Bounded, session-only forms for explicitly reporting one accessible message.
use crate::Id;
use std::collections::BTreeMap;

pub const MAX_WIRE: usize = 256 * 1024;
pub const MAX_BYTES: usize = 512 * 1024;
pub const MAX_NODES: usize = 256;
pub const MAX_STEPS: usize = 64;

#[derive(Clone, Debug)]
pub struct Menu {
	pub version: String,
	pub variant: String,
	pub language: String,
	pub root: u32,
	pub nodes: BTreeMap<u32, Node>,
}
#[derive(Clone, Debug)]
pub struct Node {
	pub header: String,
	pub detail: String,
	pub children: Vec<(String, u32)>,
	pub elements: Vec<Element>,
	pub button: Button,
	pub required: bool,
}
#[derive(Clone, Debug)]
pub enum Button {
	Next(u32),
	Submit,
	Done,
	Cancel,
	None,
}
#[derive(Clone, Debug)]
pub struct Element {
	pub name: String,
	pub submit: bool,
	pub kind: Kind,
}
#[derive(Clone, Debug)]
pub enum Kind {
	Checkbox(Vec<(String, String)>),
	Dropdown {
		title: String,
		options: Vec<(String, String)>,
	},
	Text {
		title: String,
		placeholder: String,
		limit: usize,
	},
	Notice(String),
	Link {
		label: String,
		url: String,
	},
	Preview,
}
#[derive(Clone, Debug)]
pub struct Submission {
	pub version: String,
	pub variant: String,
	pub language: String,
	pub breadcrumbs: Vec<u32>,
	pub elements: BTreeMap<String, Vec<String>>,
}

fn strings(bytes: &mut usize, text: &str) -> bool {
	*bytes += text.len();
	text.len() <= 4096 && *bytes <= MAX_WIRE
}
impl Menu {
	pub fn valid(&self) -> bool {
		if self.version != "1.0"
			|| self.variant.len() > 256
			|| self.language.len() > 32
			|| self.nodes.is_empty()
			|| self.nodes.len() > MAX_NODES
			|| !self.nodes.contains_key(&self.root)
			|| self.bytes() > MAX_BYTES
		{
			return false;
		}
		let mut bytes = 0;
		self.nodes.values().all(|node| {
			strings(&mut bytes, &node.header)
				&& strings(&mut bytes, &node.detail)
				&& node.children.len() <= 64
				&& node.elements.len() <= 16
				&& node.children.iter().all(|(label, target)| {
					strings(&mut bytes, label) && self.nodes.contains_key(target)
				}) && match node.button {
				Button::Next(target) => self.nodes.contains_key(&target),
				_ => true,
			} && node.elements.iter().all(|element| {
				strings(&mut bytes, &element.name)
					&& !element.name.is_empty()
					&& element.name.len() <= 128
					&& match &element.kind {
						Kind::Dropdown { title, .. } => strings(&mut bytes, title),
						_ => true,
					} && match &element.kind {
					Kind::Checkbox(options) | Kind::Dropdown { options, .. } => {
						options.len() <= 64
							&& options.iter().all(|(value, label)| {
								strings(&mut bytes, value) && strings(&mut bytes, label)
							})
					}
					Kind::Text {
						title,
						placeholder,
						limit,
					} => {
						*limit > 0
							&& *limit <= 4096 && strings(&mut bytes, title)
							&& strings(&mut bytes, placeholder)
					}
					Kind::Notice(text) => !element.submit && strings(&mut bytes, text),
					Kind::Link { label, url } => {
						!element.submit
							&& strings(&mut bytes, label)
							&& strings(&mut bytes, url)
							&& url.starts_with("https://")
					}
					Kind::Preview => !element.submit,
				}
			})
		})
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.version.capacity()
			+ self.variant.capacity()
			+ self.language.capacity()
			+ self
				.nodes
				.values()
				.map(|node| {
					size_of::<Node>()
						+ 64 + node.header.capacity()
						+ node.detail.capacity()
						+ node.children.capacity() * size_of::<(String, u32)>()
						+ node
							.children
							.iter()
							.map(|(s, _)| s.capacity())
							.sum::<usize>() + node.elements.capacity() * size_of::<Element>()
						+ node
							.elements
							.iter()
							.map(|element| {
								element.name.capacity()
									+ match &element.kind {
										Kind::Checkbox(options) => option_bytes(options),
										Kind::Dropdown { title, options } => {
											title.capacity() + option_bytes(options)
										}
										Kind::Text {
											title, placeholder, ..
										} => title.capacity() + placeholder.capacity(),
										Kind::Notice(text) => text.capacity(),
										Kind::Link { label, url } => {
											label.capacity() + url.capacity()
										}
										Kind::Preview => 0,
									}
							})
							.sum::<usize>()
				})
				.sum::<usize>()
	}
	pub fn valid_path(&self, path: &[u32]) -> bool {
		!path.is_empty()
			&& path.len() <= MAX_STEPS
			&& path[0] == self.root
			&& path
				.iter()
				.enumerate()
				.all(|(i, id)| self.nodes.contains_key(id) && !path[..i].contains(id))
			&& path.windows(2).all(|step| {
				let node = &self.nodes[&step[0]];
				node.children.iter().any(|(_, id)| *id == step[1])
					|| matches!(node.button, Button::Next(id) if id == step[1])
			})
	}
	pub fn validate_values(&self, path: &[u32], values: &BTreeMap<String, Vec<String>>) -> bool {
		if !self.valid_path(path)
			|| values.len() > 64
			|| values
				.iter()
				.map(|(key, values)| key.len() + values.iter().map(String::len).sum::<usize>())
				.sum::<usize>()
				> 16 * 1024
		{
			return false;
		}
		let elements: Vec<_> = path
			.iter()
			.flat_map(|id| self.nodes[id].elements.iter())
			.filter(|e| e.submit)
			.collect();
		if !values
			.keys()
			.all(|name| elements.iter().any(|e| &e.name == name))
		{
			return false;
		}
		path.iter().all(|id| {
			let node = &self.nodes[id];
			node.elements.iter().filter(|e| e.submit).all(|element| {
				let entries = values.get(&element.name).map_or(&[][..], Vec::as_slice);
				match &element.kind {
					Kind::Checkbox(options) => {
						(!node.required || !entries.is_empty())
							&& entries.len() <= options.len()
							&& entries.iter().enumerate().all(|(i, v)| {
								!entries[..i].contains(v)
									&& options.iter().any(|(value, _)| value == v)
							})
					}
					Kind::Dropdown { options, .. } => {
						(entries.is_empty() && !node.required)
							|| (entries.len() == 1
								&& options.iter().any(|(value, _)| value == &entries[0]))
					}
					Kind::Text { limit, .. } => {
						entries.len() <= 1
							&& (!node.required
								|| entries.first().is_some_and(|v| !v.trim().is_empty()))
							&& entries
								.iter()
								.all(|v| v.chars().count() <= *limit && !v.contains('\0'))
					}
					_ => entries.is_empty(),
				}
			})
		})
	}
}
fn option_bytes(options: &Vec<(String, String)>) -> usize {
	options.capacity() * size_of::<(String, String)>()
		+ options
			.iter()
			.map(|(a, b)| a.capacity() + b.capacity())
			.sum::<usize>()
}
impl Submission {
	pub fn valid_for(&self, menu: &Menu) -> bool {
		self.version == menu.version
			&& self.variant == menu.variant
			&& self.language == menu.language
			&& menu.validate_values(&self.breadcrumbs, &self.elements)
			&& self
				.breadcrumbs
				.last()
				.and_then(|id| menu.nodes.get(id))
				.is_some_and(|node| matches!(node.button, Button::Submit))
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.version.capacity()
			+ self.variant.capacity()
			+ self.language.capacity()
			+ self.breadcrumbs.capacity() * size_of::<u32>()
			+ self
				.elements
				.iter()
				.map(|(k, v)| {
					64 + k.capacity()
						+ v.capacity() * size_of::<String>()
						+ v.iter().map(String::capacity).sum::<usize>()
				})
				.sum::<usize>()
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
	pub channel: Id,
	pub message: Id,
}
