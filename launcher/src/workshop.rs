//! Browsing the Stonehearth Workshop without a Steam Web API key: the public
//! browse page carries its results as JSON for the page's own scripts, and
//! GetPublishedFileDetails answers for any item without a key.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::game::APP_ID;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Trending,
    Popular,
    Updated,
    Newest,
}

impl Sort {
    pub const ALL: [Sort; 4] = [Sort::Trending, Sort::Popular, Sort::Updated, Sort::Newest];

    pub fn label(self) -> &'static str {
        match self {
            Sort::Trending => "Trending",
            Sort::Popular => "Most popular",
            Sort::Updated => "Recently updated",
            Sort::Newest => "Newest",
        }
    }

    fn query(self) -> &'static str {
        match self {
            Sort::Trending => "browsesort=trend&days=90",
            Sort::Popular => "browsesort=totaluniquesubscribers",
            Sort::Updated => "browsesort=lastupdated",
            Sort::Newest => "browsesort=mostrecent",
        }
    }
}

#[derive(Clone, Default)]
pub struct Item {
    pub id: u64,
    pub title: String,
    pub summary: String,
    pub preview_url: String,
    pub subscriptions: u64,
    pub file_size: u64,
    pub time_updated: u64,
    pub stars: Option<u8>,
    pub tags: Vec<String>,
    /// Workshop items this one needs (Steam's "required items")
    pub requires: Vec<u64>,
}

pub struct Page {
    pub items: Vec<Item>,
    pub page: u32,
    pub total_pages: u32,
    pub total_count: u64,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .user_agent(concat!("ReHearth/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn num(v: &Value) -> u64 {
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap_or(0)
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().trim().to_string()
}

fn item_from(v: &Value) -> Option<Item> {
    let id = num(v.get("publishedfileid")?);
    if id == 0 {
        return None;
    }
    let tags = v
        .get("tags")
        .and_then(Value::as_array)
        .map(|t| {
            t.iter()
                .filter_map(|t| t.get("display_name").or_else(|| t.get("tag")).and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let requires = v
        .get("children")
        .and_then(Value::as_array)
        .map(|c| c.iter().filter_map(|c| c.get("publishedfileid")).map(num).filter(|&id| id != 0).collect())
        .unwrap_or_default();
    // star_rating is 0-5, or -1 when there aren't enough votes yet
    let stars = v.get("star_rating").and_then(Value::as_i64).filter(|&s| s >= 0).map(|s| s as u8);
    let summary = v.get("short_description").or_else(|| v.get("description")).map(text).unwrap_or_default();
    Some(Item {
        id,
        title: v.get("title").map(text).unwrap_or_else(|| format!("Item {id}")),
        summary: strip_bbcode(&summary),
        preview_url: v.get("preview_url").map(text).unwrap_or_default(),
        subscriptions: v.get("subscriptions").map(num).unwrap_or(0),
        file_size: v.get("file_size").map(num).unwrap_or(0),
        time_updated: v.get("time_updated").map(num).unwrap_or(0),
        stars,
        tags,
        requires,
    })
}

/// Finds the first object anywhere under `v` that has both `results` and
/// `total_count`. Steam nests JSON documents inside JSON strings, so strings
/// that look like documents are opened up too.
fn find_results(v: &Value) -> Option<Value> {
    match v {
        Value::Object(map) => {
            if map.contains_key("results") && map.contains_key("total_count") {
                return Some(v.clone());
            }
            map.values().find_map(find_results)
        }
        Value::Array(list) => list.iter().find_map(find_results),
        Value::String(s) if s.contains("total_count") && (s.starts_with('{') || s.starts_with('[')) => {
            serde_json::from_str::<Value>(s).ok().as_ref().and_then(find_results)
        }
        _ => None,
    }
}

fn parse_browse(html: &str) -> Result<Page> {
    let start_tag = html.find("id=\"valve-ssr-data\"").context("the Workshop page changed shape (no data block)")?;
    let body_start = start_tag + html[start_tag..].find('>').context("bad data block")? + 1;
    let body_end = body_start + html[body_start..].find("</script>").context("bad data block")?;
    let outer: Value = serde_json::from_str(&html[body_start..body_end]).context("couldn't read the Workshop data")?;
    let data = find_results(&outer).context("the Workshop page had no results")?;
    let items = data
        .get("results")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(item_from).collect())
        .unwrap_or_default();
    Ok(Page {
        items,
        page: data.get("current_page").map(num).unwrap_or(1) as u32,
        total_pages: data.get("total_pages").map(num).unwrap_or(1) as u32,
        total_count: data.get("total_count").map(num).unwrap_or(0),
    })
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// One page (30 items) of mods, newest Steam data. Blocking.
pub fn browse(sort: Sort, search: &str, page: u32) -> Result<Page> {
    let search = search.trim();
    let order = if search.is_empty() { sort.query().to_string() } else { format!("browsesort=textsearch&searchtext={}", encode(search)) };
    let url = format!(
        "https://steamcommunity.com/workshop/browse/?appid={APP_ID}&section=readytouseitems&numperpage=30&p={page}&{order}"
    );
    let html = agent().get(&url).call().context("couldn't reach the Steam Workshop")?.body_mut().read_to_string()?;
    parse_browse(&html)
}

/// Full details (long description included) for specific items. Blocking.
pub fn details(ids: &[u64]) -> Result<Vec<(Item, String)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut form: Vec<(String, String)> = vec![("itemcount".into(), ids.len().to_string())];
    for (i, id) in ids.iter().enumerate() {
        form.push((format!("publishedfileids[{i}]"), id.to_string()));
    }
    let body = agent()
        .post("https://api.steampowered.com/ISteamRemoteStorage/GetPublishedFileDetails/v1/")
        .send_form(form.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .context("couldn't reach Steam")?
        .body_mut()
        .read_to_string()?;
    let body: Value = serde_json::from_str(&body).context("Steam sent bad details")?;
    let list = body.pointer("/response/publishedfiledetails").and_then(Value::as_array).context("Steam sent no details")?;
    Ok(list
        .iter()
        .filter(|d| d.get("result").and_then(Value::as_i64) == Some(1))
        .filter_map(|d| {
            let mut item = item_from(d)?;
            let long = strip_bbcode(d.get("description").and_then(Value::as_str).unwrap_or_default());
            item.summary = long.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().trim().to_string();
            Some((item, long))
        })
        .collect())
}

fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"));
    base.join("rehearth/thumbs")
}

/// A square preview image, from the disk cache when we have it. Blocking.
pub fn thumbnail(id: u64, preview_url: &str, size: u32) -> Result<Vec<u8>> {
    if preview_url.is_empty() {
        bail!("no preview");
    }
    let dir = cache_dir();
    // the URL changes when the author uploads a new preview, so key the cache on it
    let key: u64 = preview_url.bytes().fold(0xcbf29ce484222325, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    let path = dir.join(format!("{id}-{key:016x}-{size}"));
    if let Ok(bytes) = fs::read(&path) {
        return Ok(bytes);
    }
    let sep = if preview_url.contains('?') { '&' } else { '?' };
    let url = format!("{preview_url}{sep}imw={size}&imh={size}&ima=fit&impolicy=Letterbox&imcolor=%23000000&letterbox=false");
    let bytes = agent().get(&url).call()?.body_mut().with_config().limit(8 << 20).read_to_vec()?;
    let _ = fs::create_dir_all(&dir);
    let _ = fs::write(&path, &bytes);
    Ok(bytes)
}

/// Workshop descriptions are BBCode; keep the words, drop the markup and images.
pub fn strip_bbcode(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    let mut skip_until: Option<&str> = None;
    while let Some(open) = rest.find('[') {
        let (before, after) = rest.split_at(open);
        if skip_until.is_none() {
            out.push_str(before);
        }
        let Some(close) = after.find(']') else {
            if skip_until.is_none() {
                out.push_str(after);
            }
            rest = "";
            break;
        };
        let tag = after[1..close].to_ascii_lowercase();
        let name = tag.trim_start_matches('/').split(['=', ' ']).next().unwrap_or_default().to_string();
        let known = matches!(
            name.as_str(),
            "b" | "i" | "u" | "s" | "h1" | "h2" | "h3" | "url" | "img" | "list" | "olist" | "*" | "quote" | "code"
                | "spoiler" | "noparse" | "hr" | "table" | "tr" | "td" | "th" | "strike" | "previewyoutube" | "p"
        );
        if !known {
            if skip_until.is_none() {
                out.push('[');
            }
            rest = &after[1..];
            continue;
        }
        match (skip_until, tag.as_str()) {
            (Some(end), t) if t == end => skip_until = None,
            (Some(_), _) => {}
            (None, "img") => skip_until = Some("/img"),
            (None, t) if t.starts_with("previewyoutube") => skip_until = Some("/previewyoutube"),
            (None, "*") => out.push_str("\n• "),
            (None, "hr") | (None, "/h1") | (None, "/h2") | (None, "/h3") | (None, "/p") => out.push('\n'),
            _ => {}
        }
        rest = &after[close + 1..];
    }
    if skip_until.is_none() {
        out.push_str(rest);
    }
    // collapse runs of blank lines
    let mut cleaned = String::new();
    let mut blank = 0;
    for line in out.replace("\r\n", "\n").lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        cleaned.push_str(line);
        cleaned.push('\n');
    }
    cleaned.trim().to_string()
}

pub fn human_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.0} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

pub fn human_count(n: u64) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 10_000 => format!("{:.0}k", n as f64 / 1e3),
        n if n >= 1_000 => format!("{:.1}k", n as f64 / 1e3),
        n => n.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bbcode_keeps_words_and_drops_images() {
        let s = "[h1]ACE[/h1]\n[b]Big[/b] mod.[img]https://x/y.png[/img]\n[list][*]one[*]two[/list]\n[url=https://a]link[/url] [notatag]";
        let out = strip_bbcode(s);
        assert!(out.starts_with("ACE"), "{out}");
        assert!(out.contains("Big mod."));
        assert!(!out.contains("y.png"));
        assert!(out.contains("• one") && out.contains("• two"));
        assert!(out.contains("link [notatag]"));
    }

    #[test]
    fn parses_ssr_page() {
        let inner = serde_json::json!({
            "queries": [{"state": {"data": {"eresult": 1, "current_page": 2, "total_pages": 44, "total_count": 1309,
                "results": [{"publishedfileid": "1577375188", "title": "ACE", "short_description": "[b]big[/b]",
                    "preview_url": "https://p", "subscriptions": 146479, "file_size": "444188147", "time_updated": 1,
                    "star_rating": 5, "children": [{"publishedfileid": "123"}], "tags": [{"tag": "Mod", "display_name": "Mod"}]}]}}}]
        });
        let outer = serde_json::json!({ "loaderData": [ "{\"header\":{}}", inner.to_string() ] });
        let html = format!("<html><script type=\"application/json\" id=\"valve-ssr-data\" nonce=\"x\">{outer}</script></html>");
        let page = parse_browse(&html).unwrap();
        assert_eq!((page.page, page.total_pages, page.total_count), (2, 44, 1309));
        let ace = &page.items[0];
        assert_eq!(ace.id, 1577375188);
        assert_eq!(ace.summary, "big");
        assert_eq!(ace.file_size, 444188147);
        assert_eq!(ace.requires, vec![123]);
        assert_eq!(ace.stars, Some(5));
    }
}
