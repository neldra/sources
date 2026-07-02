use crate::helpers::{build_novel_url, cover_url};
use aidoku::{
	Manga,
	alloc::{String, Vec},
};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct SearchResponse {
	pub data: Vec<SearchItem>,
}

#[derive(Deserialize)]
pub struct SearchItem {
	pub title: String,
	pub slug: String,
	pub image: String,
}

impl From<SearchItem> for Manga {
	fn from(item: SearchItem) -> Self {
		Manga {
			url: Some(build_novel_url(&item.slug)),
			cover: Some(cover_url(&item.image)),
			key: item.slug,
			title: item.title,
			..Default::default()
		}
	}
}

#[derive(Deserialize)]
pub struct ChapterIndexResponse {
	pub data: Vec<ChapterIndexRow>,
}

#[derive(Deserialize)]
pub struct ChapterIndexRow {
	pub n_sort: i64,
	pub title: String,
}
