use crate::BASE_URL;
use crate::models::ChapterIndexResponse;
use aidoku::{
	Chapter, ContentRating, FilterValue, Manga, MangaStatus, Result,
	alloc::{String, Vec, string::ToString, vec},
	helpers::uri::QueryParameters,
	imports::{
		html::{Document, Element, Kind},
		net::Request,
	},
	prelude::*,
};

pub fn request_html(url: &str) -> Result<Document> {
	Ok(Request::get(url)?.html()?)
}

pub fn cover_url(image_path: &str) -> String {
	format!("{BASE_URL}/{image_path}")
}

pub fn build_novel_url(slug: &str) -> String {
	format!("{BASE_URL}/book/{slug}")
}

pub fn build_browse_url(filters: &[FilterValue], page: i32) -> String {
	let mut genre = "all";
	let mut sort = "new";
	let mut status = "all";
	let mut lang = "all";
	for filter in filters {
		match filter {
			FilterValue::Select { id, value } => match id.as_str() {
				"genre" => genre = value,
				"status" => status = value,
				"language" => lang = value,
				_ => {}
			},
			FilterValue::Sort { id, index, .. } if id == "sort" => {
				sort = match index {
					1 => "popular",
					2 => "latest-release",
					_ => "new",
				};
			}
			_ => {}
		}
	}
	format!("{BASE_URL}/genre-{genre}/sort-{sort}/status-{status}/{lang}-novel?page={page}")
}

/// Extract `{slug}` from `https://novelfire.net/book/{slug}[/...]`.
pub fn slug_from_url(url: &str) -> Option<String> {
	let rest = url.split("/book/").nth(1)?;
	let slug = rest.split(['/', '?', '#']).next()?;
	(!slug.is_empty()).then(|| slug.to_string())
}

/// Parse `li.novel-item` entries (browse, ranking, and home sections share
/// this markup). Covers are lazy-loaded, so prefer `data-src` over `src`.
/// Pass the result of `select("li.novel-item")` on a [Document] or [Element].
pub fn parse_novel_items<I>(items: Option<I>) -> Vec<Manga>
where
	I: Iterator<Item = Element>,
{
	let mut entries = Vec::new();
	let Some(items) = items else {
		return entries;
	};
	for item in items {
		let Some(link) = item.select_first("a[href*='/book/']") else {
			continue;
		};
		let Some(url) = link.attr("abs:href") else {
			continue;
		};
		let Some(slug) = slug_from_url(&url) else {
			continue;
		};
		let Some(title) = link
			.attr("title")
			.or_else(|| item.select_first("h4.novel-title").and_then(|h| h.text()))
			.map(|t| t.trim().to_string())
			.filter(|t| !t.is_empty())
		else {
			continue;
		};
		let cover = item
			.select_first("img")
			.and_then(|img| img.attr("abs:data-src").or_else(|| img.attr("abs:src")));
		entries.push(Manga {
			key: slug,
			title,
			cover,
			url: Some(url),
			..Default::default()
		});
	}
	entries
}

pub fn has_next_page(html: &Document, page: i32) -> bool {
	html.select_first(format!("a[href*='page={}']", page + 1))
		.is_some()
}

pub fn content_rating_from_tags(tags: &[String]) -> ContentRating {
	const NSFW_TAGS: &[&str] = &["Adult", "Mature", "Smut"];
	const LITE_TAGS: &[&str] = &["Ecchi", "Yaoi", "Yuri", "Harem"];
	if tags.iter().any(|tag| NSFW_TAGS.contains(&tag.as_str())) {
		ContentRating::NSFW
	} else if tags.iter().any(|tag| LITE_TAGS.contains(&tag.as_str())) {
		ContentRating::Suggestive
	} else {
		ContentRating::Safe
	}
}

/// Reads the value in `.header-stats` for the given label, e.g.
/// `<span><strong>Ongoing</strong><small>Status</small></span>`.
fn header_stat(html: &Document, label: &str) -> Option<String> {
	let spans = html.select(".header-stats span")?;
	for span in spans {
		let matches = span
			.select_first("small")
			.and_then(|s| s.text())
			.is_some_and(|t| t.trim() == label);
		if matches {
			return span
				.select_first("strong")
				.and_then(|s| s.text())
				.map(|t| t.trim().to_string());
		}
	}
	None
}

pub fn fill_manga_details(html: &Document, mut manga: Manga) -> Result<Manga> {
	let Some(title) = html
		.select_first("h1.novel-title")
		.and_then(|el| el.text())
		.map(|t| t.trim().to_string())
		.filter(|t| !t.is_empty())
	else {
		bail!("title not found");
	};
	manga.title = title;
	manga.cover = html
		.select_first("meta[property='og:image']")
		.and_then(|el| el.attr("content"));
	manga.url = Some(build_novel_url(&manga.key));
	manga.authors = html
		.select_first(".author a span[itemprop='author']")
		.and_then(|el| el.text())
		.map(|a| vec![a.trim().to_string()]);
	manga.description = html
		.select_first(".summary .content")
		.and_then(|el| el.text())
		.map(|d| d.trim().to_string())
		.filter(|d| !d.is_empty());
	manga.tags = html.select(".categories li a").map(|els| {
		els.filter_map(|el| el.text())
			.map(|t| t.trim().to_string())
			.filter(|t| !t.is_empty())
			.collect()
	});
	manga.content_rating = manga
		.tags
		.as_deref()
		.map(content_rating_from_tags)
		.unwrap_or(ContentRating::Unknown);
	manga.status = match header_stat(html, "Status").as_deref() {
		Some("Ongoing") => MangaStatus::Ongoing,
		Some("Completed") => MangaStatus::Completed,
		_ => MangaStatus::Unknown,
	};
	Ok(manga)
}

pub fn build_chapter_url(slug: &str, chapter_key: &str) -> String {
	format!("{BASE_URL}/book/{slug}/{chapter_key}")
}

/// Reduce a raw chapter heading to the bare title. The site uses two formats:
/// older chapters read `"Chapter 2820 - 2820: Throne of Blood"` and newer ones
/// `"Chapter 2821 Above Deck"`, so strip the `Chapter {n}` prefix, then any
/// separator, then a duplicated `{n}:`/`{n} -` prefix.
pub fn clean_chapter_title(raw: &str, number: &str) -> Option<String> {
	let mut rest = raw.trim();
	if let Some(r) = rest.strip_prefix("Chapter") {
		rest = r.trim_start();
		rest = rest.strip_prefix(number).unwrap_or(rest).trim_start();
	}
	if let Some(r) = rest.strip_prefix(['-', ':']) {
		rest = r.trim_start();
	}
	// Only strip a repeated chapter number when a separator follows it, so
	// titles that genuinely start with digits stay intact.
	if let Some(r) = rest.strip_prefix(number) {
		let r = r.trim_start();
		if let Some(r) = r.strip_prefix(['-', ':']) {
			rest = r.trim_start();
		}
	}
	(!rest.is_empty()).then(|| rest.to_string())
}

/// The numeric post id embedded on the book page, needed by the chapter
/// index endpoint.
pub fn post_id(html: &Document) -> Option<String> {
	html.select_first("a#novel-report")
		.and_then(|el| el.attr("report-post_id"))
}

/// Fetch the complete chapter index (every chapter with its title) in a
/// single request, newest-first. This is the endpoint the site's own
/// chapter-list modal uses (a server-side DataTable); `length=-1` returns
/// all rows, so even 3000-chapter novels load in one round trip.
pub fn fetch_chapter_index(slug: &str, post_id: &str) -> Result<Vec<Chapter>> {
	let mut qs = QueryParameters::new();
	qs.push("draw", Some("1"));
	qs.push("columns[0][data]", Some("n_sort"));
	qs.push("columns[0][name]", Some("cmm_posts_detail.n_sort"));
	qs.push("columns[0][searchable]", Some("true"));
	qs.push("columns[0][orderable]", Some("true"));
	qs.push("columns[0][search][value]", Some(""));
	qs.push("columns[0][search][regex]", Some("false"));
	qs.push("order[0][column]", Some("0"));
	qs.push("order[0][dir]", Some("asc"));
	qs.push("start", Some("0"));
	qs.push("length", Some("-1"));
	qs.push("search[value]", Some(""));
	qs.push("search[regex]", Some("false"));
	qs.push("post_id", Some(post_id));
	qs.push("user_id", Some(""));
	qs.push("only_bookmark", Some("false"));
	let url = format!("{BASE_URL}/ajax/listChapterDataAjax?{qs}");
	let response = Request::get(&url)?.json_owned::<ChapterIndexResponse>()?;
	Ok(response
		.data
		.into_iter()
		.rev()
		.map(|row| {
			let number = row.n_sort.to_string();
			let key = format!("chapter-{number}");
			Chapter {
				url: Some(build_chapter_url(slug, &key)),
				title: clean_chapter_title(&row.title, &number),
				key,
				chapter_number: Some(row.n_sort as f32),
				..Default::default()
			}
		})
		.collect())
}

fn convert_element_to_markdown(element: &Element, output: &mut String) {
	let nodes = element.child_nodes();

	for node in nodes {
		match node.kind() {
			Kind::TextNode => {
				if let Some(text) = node.text() {
					if text.len() >= 3 && text.replace("-", "").trim().is_empty() {
						output.push_str(&text);
					} else {
						output.push_str(&text.replace("*", r"\*").replace("-", r"\-"));
					}
				}
			}
			Kind::Element => {
				if let Ok(el) = Element::try_from(node) {
					convert_tag_to_markdown(&el, output);
				}
			}
			_ => (),
		}
	}
}

fn convert_tag_to_markdown(element: &Element, output: &mut String) {
	let tag = element.tag_name().unwrap_or_default();

	match tag.as_str() {
		"p" => {
			convert_element_to_markdown(element, output);
			output.push_str("\n\n");
		}
		"br" => {
			output.push_str("  \n");
		}
		"h1" => {
			output.push_str("# ");
			convert_element_to_markdown(element, output);
			output.push_str("\n\n");
		}
		"h2" => {
			output.push_str("## ");
			convert_element_to_markdown(element, output);
			output.push_str("\n\n");
		}
		"h3" => {
			output.push_str("### ");
			convert_element_to_markdown(element, output);
			output.push_str("\n\n");
		}
		"h4" => {
			output.push_str("#### ");
			convert_element_to_markdown(element, output);
			output.push_str("\n\n");
		}
		"h5" => {
			output.push_str("##### ");
			convert_element_to_markdown(element, output);
			output.push_str("\n\n");
		}
		"h6" => {
			output.push_str("###### ");
			convert_element_to_markdown(element, output);
			output.push_str("\n\n");
		}
		"strong" | "b" => {
			output.push_str("**");
			convert_element_to_markdown(element, output);
			output.push_str("**");
		}
		"em" | "i" => {
			output.push('*');
			convert_element_to_markdown(element, output);
			output.push('*');
		}
		"u" => {
			output.push_str("__");
			convert_element_to_markdown(element, output);
			output.push_str("__");
		}
		"s" | "strike" | "del" => {
			output.push_str("~~");
			convert_element_to_markdown(element, output);
			output.push_str("~~");
		}
		_ => {
			convert_element_to_markdown(element, output);
		}
	}
}

/// Entries from the home-page `<section>` whose `.section-header h3` matches
/// `heading`.
pub fn parse_home_section(html: &Document, heading: &str) -> Vec<Manga> {
	let Some(sections) = html.select("section.container") else {
		return Vec::new();
	};
	for section in sections {
		let matches = section
			.select_first(".section-header h3")
			.and_then(|h| h.text())
			.is_some_and(|t| t.trim() == heading);
		if matches {
			return parse_novel_items(section.select("li.novel-item"));
		}
	}
	Vec::new()
}

pub fn extract_chapter_text(html: &Document) -> Result<String> {
	let mut text = String::new();
	if let Some(container) = html.select_first("#content") {
		// Ads and UI junk are injected as non-prose elements inside the content div.
		if let Some(junk) = container.select("div, script, iframe, ul, nav, dialog, style") {
			junk.for_each(Element::remove);
		}
		convert_element_to_markdown(&container, &mut text);
		text = text.replace("****", "");
	}
	if text.trim().is_empty() {
		bail!("chapter text not found");
	}
	Ok(text)
}
