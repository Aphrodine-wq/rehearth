//! Finding mods that other mods need on the Workshop.
//!
//! A manifest names its dependencies by namespace ("trapper_plus"), and the
//! Workshop knows nothing about namespaces. The lookup tries, in order:
//! 1. matches confirmed on this machine before,
//! 2. the shared index (`data/namespaces.json`, built into ReHearth and
//!    refreshed from GitHub), of namespaces people have confirmed,
//! 3. the mods linked from the needing mod's own Workshop page (its required
//!    items and description), matched by title,
//! 4. a Workshop search, matched by title.
//! Steps 3 and 4 are guesses, so the app checks each download's real namespace
//! afterwards and takes a wrong one back out (see `App::check_needed`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use anyhow::Result;

use crate::loadorder::{Analysis, KNOWN_WORKSHOP};
use crate::mods::ModInfo;
use crate::workshop::{self, Item, Sort};

const BUNDLED_INDEX: &str = include_str!("../../data/namespaces.json");
const INDEX_URL: &str = "https://raw.githubusercontent.com/Aphrodine-wq/rehearth/main/data/namespaces.json";

/// One namespace and the Workshop item that should provide it, if one was found.
#[derive(Clone)]
pub struct Found {
    pub namespace: String,
    pub item: Option<Item>,
    /// how it was found: "confirmed", "index", "linked from <mod>'s page" or "search"
    pub source: String,
}

impl Found {
    /// True when the match came from a confirmed namespace rather than a title guess.
    pub fn certain(&self) -> bool {
        self.source == "confirmed" || self.source == "index"
    }
}

/// The shared namespace index: the copy built into ReHearth, updated from
/// GitHub when that's reachable.
pub fn index() -> BTreeMap<String, u64> {
    let mut map: BTreeMap<String, u64> = serde_json::from_str(BUNDLED_INDEX).unwrap_or_default();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .user_agent(concat!("ReHearth/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let remote = agent
        .get(INDEX_URL)
        .call()
        .ok()
        .and_then(|mut r| r.body_mut().read_to_string().ok())
        .and_then(|t| serde_json::from_str::<BTreeMap<String, u64>>(&t).ok());
    map.extend(remote.unwrap_or_default());
    map
}

/// For each missing namespace, the Workshop ids of the mods that need it
/// (their pages are where step 3 looks).
pub fn needers(mods: &[ModInfo], analysis: &Analysis) -> BTreeMap<String, Vec<(u64, String)>> {
    analysis
        .missing
        .iter()
        .map(|(ns, who)| {
            let ids = who
                .iter()
                .filter_map(|&i| Some((mods[i].workshop_id.as_deref()?.parse().ok()?, mods[i].name.clone())))
                .collect();
            (ns.clone(), ids)
        })
        .collect()
}

/// Every installed Workshop mod as `namespace -> id`, in the index's format.
pub fn export(mods: &[ModInfo]) -> BTreeMap<String, u64> {
    mods.iter()
        .filter_map(|m| Some((m.namespace.clone(), m.workshop_id.as_deref()?.parse().ok()?)))
        .collect()
}

/// Lowercase letters and digits only, with "+" and "&" spelled out, so
/// "trapper_plus" and "Trapper+" come out the same.
fn norm(s: &str) -> String {
    s.to_lowercase()
        .replace('+', "plus")
        .replace('&', "and")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

/// For mods linked from the needing mod's page, a looser test: most of the
/// namespace's words appear in the title ("furniture_expansion_plus" and
/// "Furniture Expansion (Continuation)").
fn words_match(namespace: &str, title: &str) -> bool {
    let words: Vec<String> = namespace.split('_').map(norm).filter(|w| w.len() >= 3).collect();
    if words.len() < 2 {
        return false;
    }
    let title_words: HashSet<String> = title.split(|c: char| !c.is_alphanumeric()).map(norm).collect();
    let hits = words.iter().filter(|w| title_words.contains(*w)).count();
    hits * 3 >= words.len() * 2
}

/// How well a Workshop title names this namespace: 3 exact, 2 the title
/// contains it, 1 it contains the title, 0 no match.
fn score(namespace: &str, title: &str) -> u8 {
    let (n, t) = (norm(namespace), norm(title));
    if n.is_empty() || t.is_empty() {
        0
    } else if n == t {
        3
    } else if n.len() >= 4 && t.contains(&n) {
        2
    } else if t.len() >= 5 && n.contains(&t) {
        1
    } else {
        0
    }
}

/// The best-matching item among `items`, skipping ones already ruled out.
fn best<'a>(namespace: &str, items: &'a [Item], skip: &HashSet<u64>) -> Option<&'a Item> {
    items
        .iter()
        .filter(|i| !skip.contains(&i.id))
        .map(|i| (score(namespace, &i.title), i))
        .filter(|(s, _)| *s > 0)
        .max_by_key(|(s, i)| (*s, i.subscriptions))
        .map(|(_, i)| i)
}

/// Looks each namespace up. `known` holds matches confirmed on this machine,
/// `wrong` items already found not to be that namespace, `needers` the mods
/// that need each one (see [`needers`]). Blocking.
pub fn find(
    namespaces: &[String],
    known: &BTreeMap<String, u64>,
    wrong: &BTreeMap<String, Vec<u64>>,
    needers: &BTreeMap<String, Vec<(u64, String)>>,
) -> Result<Vec<Found>> {
    let is_wrong = |ns: &str, id: u64| wrong.get(ns).is_some_and(|v| v.contains(&id));
    let shared = index();
    // confirmed and indexed matches first: one details call for all of them
    let mut ids: HashMap<&str, (u64, &str)> = HashMap::new();
    for ns in namespaces {
        let id = known
            .get(ns)
            .map(|&id| (id, "confirmed"))
            .or_else(|| shared.get(ns).map(|&id| (id, "index")))
            .or_else(|| KNOWN_WORKSHOP.iter().find(|(k, ..)| k == ns).map(|(_, id, _)| (*id, "index")));
        if let Some((id, source)) = id.filter(|(id, _)| !is_wrong(ns, *id)) {
            ids.insert(ns.as_str(), (id, source));
        }
    }
    let wanted: Vec<u64> = ids.values().map(|v| v.0).collect();
    let details: HashMap<u64, Item> = workshop::details(&wanted)?.into_iter().map(|(i, _)| (i.id, i)).collect();

    // pages of the mods that need something, read once each
    let mut pages: HashMap<u64, Vec<Item>> = HashMap::new();

    let mut out = Vec::new();
    for ns in namespaces {
        if let Some((source, item)) = ids.get(ns.as_str()).and_then(|(id, source)| Some((*source, details.get(id)?))) {
            out.push(Found { namespace: ns.clone(), item: Some(item.clone()), source: source.to_string() });
            continue;
        }
        let skip: HashSet<u64> = wrong.get(ns).map(|v| v.iter().copied().collect()).unwrap_or_default();

        let mut linked = None;
        for (page_id, page_name) in needers.get(ns).into_iter().flatten() {
            if !pages.contains_key(page_id) {
                let links = workshop::linked_items(*page_id).unwrap_or_default();
                let items = workshop::details(&links).unwrap_or_default().into_iter().map(|(i, _)| i).collect();
                pages.insert(*page_id, items);
            }
            let items = &pages[page_id];
            let hit = best(ns, items, &skip)
                .or_else(|| items.iter().filter(|i| !skip.contains(&i.id)).find(|i| words_match(ns, &i.title)));
            if let Some(item) = hit {
                linked = Some((item.clone(), page_name.clone()));
                break;
            }
        }
        if let Some((item, page_name)) = linked {
            out.push(Found { namespace: ns.clone(), item: Some(item), source: format!("linked from {page_name}'s page") });
            continue;
        }

        let mut results = workshop::browse(Sort::Popular, ns, 1)?.items;
        if ns.contains('_') && best(ns, &results, &skip).is_none() {
            results.extend(workshop::browse(Sort::Popular, &ns.replace('_', " "), 1)?.items);
        }
        let item = best(ns, &results, &skip).cloned();
        out.push(Found { namespace: ns.clone(), item, source: "search".into() });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u64, title: &str, subs: u64) -> Item {
        Item { id, title: title.into(), subscriptions: subs, ..Default::default() }
    }

    #[test]
    fn scores_titles() {
        assert_eq!(score("trapper_plus", "Trapper+"), 3);
        assert_eq!(score("lostems", "LostEms"), 3);
        assert_eq!(score("music_changer", "Music Changer"), 3);
        assert_eq!(score("archipelago_biome", "Fisher Job + Archipelago Biome"), 2);
        assert_eq!(score("swamp_goblins", "Goblin Warfare"), 0);
    }

    #[test]
    fn linked_titles_match_by_words() {
        assert!(words_match("furniture_expansion_plus", "Furniture Expansion (Continuation)"));
        assert!(!words_match("swamp_goblins", "Goblin Warfare"));
        assert!(!words_match("lostems", "LostEms")); // one word: left to `score`
    }

    /// Talks to Steam: `cargo test -- --ignored`
    #[test]
    #[ignore]
    fn finds_furniture_expansion_from_the_needing_mods_page() {
        let needers = BTreeMap::from([("furniture_expansion_plus".to_string(), vec![(3677669187, "Cy's Collection of Furniture".to_string())])]);
        let ns = ["furniture_expansion_plus".to_string()];
        let mut pages = Vec::new();
        for (id, _) in &needers["furniture_expansion_plus"] {
            pages.extend(workshop::details(&workshop::linked_items(*id).unwrap()).unwrap().into_iter().map(|(i, _)| i));
        }
        let hit = pages.iter().find(|i| words_match(&ns[0], &i.title)).map(|i| i.id);
        assert_eq!(hit, Some(1376575173));
    }

    #[test]
    fn bundled_index_parses() {
        let map: BTreeMap<String, u64> = serde_json::from_str(BUNDLED_INDEX).unwrap();
        assert!(map.contains_key("stonehearth_ace"));
    }

    #[test]
    fn picks_exact_title_over_popular_partial() {
        let items = [item(1, "LostEmsCN", 90_000), item(2, "LostEms", 30_000), item(3, "Small House", 1)];
        assert_eq!(best("lostems", &items, &HashSet::new()).map(|i| i.id), Some(2));
    }

    #[test]
    fn skips_ruled_out_items() {
        let items = [item(1, "Trapper+", 10), item(2, "Trapper+ Taxidermy", 5)];
        let skip = HashSet::from([1]);
        assert_eq!(best("trapper_plus", &items, &skip).map(|i| i.id), Some(2));
    }
}
