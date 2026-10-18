//! Bounded group icon encoding shared by the native picker and HTTP boundary.
use base64::{Engine as _, engine::general_purpose::STANDARD};

#[derive(serde::Deserialize)]
pub struct MembersDto {
	pub id: model::Id,
	#[serde(rename = "type")]
	pub kind: u8,
	#[serde(default)]
	pub guild_id: Option<model::Id>,
	pub owner_id: model::Id,
	pub recipients: Vec<crate::UserDto>,
}

pub const MAX_GROUP_ICON_PNG_BYTES: usize = 256 * 1024;
pub const MAX_ICON_DATA_URI: usize = 22 + 4 * MAX_GROUP_ICON_PNG_BYTES.div_ceil(3);

fn valid_png(png: &[u8], edge: u32) -> bool {
	(1..=512).contains(&edge)
		&& png.len() >= 33
		&& png.len() <= MAX_GROUP_ICON_PNG_BYTES
		&& png.starts_with(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR")
		&& [16, 20].into_iter().all(|offset| {
			(1..=edge).contains(&u32::from_be_bytes(
				png[offset..offset + 4].try_into().unwrap(),
			))
		})
}

pub fn icon_data_uri(png: &[u8]) -> Option<String> {
	image_data_uri(png, 256)
}

/// Native Guide/profile artwork uses the same byte bound with an explicit prepared edge.
pub fn image_data_uri(png: &[u8], edge: u32) -> Option<String> {
	valid_png(png, edge).then(|| {
		let mut uri = format!("data:image/png;base64,{}", STANDARD.encode(png));
		uri.shrink_to_fit();
		uri
	})
}

pub fn valid_icon_data_uri(uri: &str) -> bool {
	valid_image_data_uri(uri, 256)
}

pub fn valid_image_data_uri(uri: &str, edge: u32) -> bool {
	uri.len() <= MAX_ICON_DATA_URI
		&& uri
			.strip_prefix("data:image/png;base64,")
			.is_some_and(|data| STANDARD.decode(data).is_ok_and(|png| valid_png(&png, edge)))
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn group_icon_encoding_is_bounded_and_requires_png_dimensions() {
		let mut png =
			b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x80\0\0\0\x80\0\0\0\0\0\0\0\0\0".to_vec();
		let uri = icon_data_uri(&png).unwrap();
		assert!(valid_icon_data_uri(&uri));
		assert!(!valid_icon_data_uri("data:image/png;base64,bad!"));
		png[18] = 2;
		assert!(icon_data_uri(&png).is_none());
		assert!(icon_data_uri(&vec![0; MAX_GROUP_ICON_PNG_BYTES + 1]).is_none());
	}
	#[test]
	fn guide_artwork_accepts_512_pixels_without_weakening_group_icon_bounds() {
		let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
		png.extend_from_slice(&512u32.to_be_bytes());
		png.extend_from_slice(&170u32.to_be_bytes());
		png.resize(33, 0);
		assert!(icon_data_uri(&png).is_none());
		let uri = image_data_uri(&png, 512).unwrap();
		assert!(valid_image_data_uri(&uri, 512));
		assert!(!valid_icon_data_uri(&uri));
		assert!(image_data_uri(&png, 513).is_none());
		png[16..20].copy_from_slice(&513u32.to_be_bytes());
		assert!(image_data_uri(&png, 512).is_none());
	}
}
