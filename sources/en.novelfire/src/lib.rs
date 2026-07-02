#![no_std]
use aidoku::{
	Chapter, DeepLinkHandler, DeepLinkResult, FilterValue, Home, HomeComponent, HomeComponentValue,
	HomeLayout, Listing, ListingProvider, Manga, MangaPageResult, Page, PageContent, Result,
	Source,
	alloc::{String, Vec, vec},
	helpers::uri::encode_uri_component,
	imports::{
		net::{Request, TimeUnit, set_rate_limit},
		std::send_partial_result,
	},
	prelude::*,
};

mod helpers;
mod models;

use helpers::{
	build_browse_url, build_chapter_url, build_novel_url, extract_chapter_text,
	fetch_chapter_index, fill_manga_details, has_next_page, parse_home_section, parse_novel_items,
	post_id, request_html, slug_from_url,
};
use models::SearchResponse;

pub const BASE_URL: &str = "https://novelfire.net";

struct NovelFire;

impl Source for NovelFire {
	fn new() -> Self {
		set_rate_limit(12, 10, TimeUnit::Seconds);
		Self
	}

	fn get_search_manga_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		if let Some(query) = query {
			if page > 1 {
				return Ok(MangaPageResult::default());
			}
			let url = format!(
				"{BASE_URL}/ajax/searchLive?keyword={}&type=title",
				encode_uri_component(query)
			);
			let response = Request::get(&url)?.json_owned::<SearchResponse>()?;
			return Ok(MangaPageResult {
				entries: response.data.into_iter().map(Into::into).collect(),
				has_next_page: false,
			});
		}
		let url = build_browse_url(&filters, page);
		let html = request_html(&url)?;
		Ok(MangaPageResult {
			entries: parse_novel_items(html.select("li.novel-item")),
			has_next_page: has_next_page(&html, page),
		})
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		let html = request_html(&build_novel_url(&manga.key))?;
		if needs_details {
			manga = fill_manga_details(&html, manga)?;
			if needs_chapters {
				send_partial_result(&manga);
			}
		}
		if needs_chapters {
			let Some(id) = post_id(&html) else {
				bail!("post id not found");
			};
			manga.chapters = Some(fetch_chapter_index(&manga.key, &id)?);
		}
		Ok(manga)
	}

	fn get_page_list(&self, manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		let url = build_chapter_url(&manga.key, &chapter.key);
		let html = request_html(&url)?;
		let text = extract_chapter_text(&html)?;
		Ok(vec![Page {
			content: PageContent::text(text),
			..Default::default()
		}])
	}
}

impl Home for NovelFire {
	fn get_home(&self) -> Result<HomeLayout> {
		let html = request_html(&format!("{BASE_URL}/home"))?;
		let mut components = Vec::new();
		let mut push_scroller = |title: &str, mut entries: Vec<Manga>| {
			if entries.is_empty() {
				return;
			}
			components.push(HomeComponent {
				title: Some(title.into()),
				subtitle: None,
				value: HomeComponentValue::Scroller {
					entries: entries.drain(..).map(Into::into).collect(),
					listing: None,
				},
			});
		};
		push_scroller("Recommends", parse_home_section(&html, "Recommends"));
		push_scroller("Latest Novels", parse_home_section(&html, "Latest Novels"));
		push_scroller(
			"Completed Stories",
			parse_home_section(&html, "Completed Stories"),
		);
		Ok(HomeLayout { components })
	}
}

impl ListingProvider for NovelFire {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		let url = format!("{BASE_URL}/ranking/{}?page={page}", listing.id);
		let html = request_html(&url)?;
		Ok(MangaPageResult {
			entries: parse_novel_items(html.select("li.novel-item")),
			has_next_page: has_next_page(&html, page),
		})
	}
}

impl DeepLinkHandler for NovelFire {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		let Some(slug) = slug_from_url(&url) else {
			return Ok(None);
		};
		let chapter_key = url
			.split("/book/")
			.nth(1)
			.and_then(|rest| rest.split(['?', '#']).next())
			.and_then(|path| path.split('/').nth(1))
			.filter(|seg| seg.starts_with("chapter-"));
		Ok(Some(match chapter_key {
			Some(key) => DeepLinkResult::Chapter {
				manga_key: slug,
				key: key.into(),
			},
			None => DeepLinkResult::Manga { key: slug },
		}))
	}
}

register_source!(NovelFire, Home, ListingProvider, DeepLinkHandler);

#[cfg(test)]
mod tests {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn search_returns_results() {
		let source = NovelFire;
		let result = source
			.get_search_manga_list(Some("shadow slave".into()), 1, Vec::new())
			.expect("search failed");
		assert!(!result.entries.is_empty());
		let hit = result
			.entries
			.iter()
			.find(|m| m.title.eq_ignore_ascii_case("shadow slave"))
			.expect("expected 'Shadow Slave' in results");
		assert_eq!(hit.key, "shadow-slave");
		assert!(
			hit.cover
				.as_deref()
				.is_some_and(|c| c.starts_with("https://")),
			"expected absolute cover url"
		);
	}

	#[aidoku_test]
	fn browse_returns_entries_with_pagination() {
		let source = NovelFire;
		let result = source
			.get_search_manga_list(None, 1, Vec::new())
			.expect("browse failed");
		assert!(result.entries.len() >= 20, "got {}", result.entries.len());
		assert!(result.has_next_page);
		let first = &result.entries[0];
		assert!(!first.key.is_empty() && !first.title.is_empty());
		assert!(
			first
				.cover
				.as_deref()
				.is_some_and(|c| c.starts_with("https://")),
			"lazy-loaded cover must come from data-src"
		);
	}

	#[aidoku_test]
	fn details_parse_shadow_slave() {
		let source = NovelFire;
		let manga = Manga {
			key: "shadow-slave".into(),
			..Default::default()
		};
		let manga = source
			.get_manga_update(manga, true, false)
			.expect("get_manga_update failed");
		assert_eq!(manga.title, "Shadow Slave");
		assert_eq!(manga.authors.as_deref(), Some(&["Guiltythree".into()][..]));
		assert_eq!(manga.status, aidoku::MangaStatus::Ongoing);
		assert!(manga.tags.as_deref().is_some_and(|t| !t.is_empty()));
		assert!(manga.description.is_some());
		assert!(
			manga
				.cover
				.as_deref()
				.is_some_and(|c| c.contains("shadow-slave"))
		);
	}

	#[aidoku_test]
	fn chapter_index_loads_all_titles_in_one_request() {
		// The chapter index endpoint returns every chapter with its title in
		// a single request — including deep-backlog chapters in both of the
		// site's title formats.
		let source = NovelFire;
		let manga = Manga {
			key: "shadow-slave".into(),
			..Default::default()
		};
		let manga = source
			.get_manga_update(manga, false, true)
			.expect("get_manga_update failed");
		let chapters = manga.chapters.expect("no chapters returned");
		assert!(chapters.len() > 3000, "got {}", chapters.len());
		// Newest-first ordering with no gaps.
		let first = chapters.first().and_then(|c| c.chapter_number).unwrap();
		let last = chapters.last().and_then(|c| c.chapter_number).unwrap();
		assert!(first > last, "expected newest first ({first} vs {last})");
		assert_eq!(chapters.len(), first as usize, "gaps in chapter list");
		// Old dash format, mid-backlog.
		let ch = chapters.iter().find(|c| c.key == "chapter-2490").unwrap();
		assert_eq!(ch.title.as_deref(), Some("Heal Thyself"));
		// New no-separator format.
		let ch = chapters.iter().find(|c| c.key == "chapter-2821").unwrap();
		assert_eq!(ch.title.as_deref(), Some("Above Deck"));
		assert_eq!(
			chapters.last().unwrap().title.as_deref(),
			Some("Nightmare Begins")
		);
	}

	#[aidoku_test]
	fn page_list_returns_text() {
		let source = NovelFire;
		let manga = Manga {
			key: "shadow-slave".into(),
			..Default::default()
		};
		let chapter = Chapter {
			key: "chapter-2479".into(),
			..Default::default()
		};
		let pages = source.get_page_list(manga, chapter).expect("no pages");
		assert_eq!(pages.len(), 1);
		let PageContent::Text(text) = &pages[0].content else {
			panic!("expected text page");
		};
		// Verified opening line of chapter 2479 in the HAR capture.
		assert!(
			text.contains("Mordret"),
			"unexpected content: {}",
			&text[..200.min(text.len())]
		);
		assert!(
			text.len() > 2000,
			"suspiciously short chapter: {}",
			text.len()
		);
	}

	#[aidoku_test]
	fn chapter_titles_clean_both_site_formats() {
		use helpers::clean_chapter_title;
		// Old format (chapters <= 2820): number duplicated after a dash.
		assert_eq!(
			clean_chapter_title("Chapter 2820 - 2820: Throne of Blood", "2820").as_deref(),
			Some("Throne of Blood")
		);
		assert_eq!(
			clean_chapter_title("Chapter 1 - 1: Nightmare Begins", "1").as_deref(),
			Some("Nightmare Begins")
		);
		// New format (chapters >= 2821): no separator at all.
		assert_eq!(
			clean_chapter_title("Chapter 2821 Above Deck", "2821").as_deref(),
			Some("Above Deck")
		);
		// Title starting with digits survives.
		assert_eq!(
			clean_chapter_title("Chapter 5 - 5: 100 Days of Night", "5").as_deref(),
			Some("100 Days of Night")
		);
		// Bare heading with no title yields None.
		assert_eq!(clean_chapter_title("Chapter 2900", "2900"), None);
	}

	#[aidoku_test]
	fn deep_link_novel_and_chapter() {
		let source = NovelFire;
		assert!(matches!(
			source
				.handle_deep_link("https://novelfire.net/book/shadow-slave".into())
				.unwrap(),
			Some(DeepLinkResult::Manga { key }) if key == "shadow-slave"
		));
		assert!(matches!(
			source
				.handle_deep_link("https://novelfire.net/book/shadow-slave/chapter-2479".into())
				.unwrap(),
			Some(DeepLinkResult::Chapter { manga_key, key })
				if manga_key == "shadow-slave" && key == "chapter-2479"
		));
		// The chapter-list URL is not a chapter.
		assert!(matches!(
			source
				.handle_deep_link("https://novelfire.net/book/shadow-slave/chapters".into())
				.unwrap(),
			Some(DeepLinkResult::Manga { key }) if key == "shadow-slave"
		));
		assert!(
			source
				.handle_deep_link("https://novelfire.net/ranking".into())
				.unwrap()
				.is_none()
		);
	}

	#[aidoku_test]
	fn most_read_listing_returns_entries() {
		let source = NovelFire;
		let listing = Listing {
			id: "most-read".into(),
			..Default::default()
		};
		let result = source.get_manga_list(listing, 1).expect("listing failed");
		assert!(!result.entries.is_empty());
	}

	#[aidoku_test]
	fn home_has_sections() {
		let source = NovelFire;
		let layout = source.get_home().expect("home failed");
		assert!(
			layout.components.len() >= 2,
			"got {}",
			layout.components.len()
		);
	}
}
