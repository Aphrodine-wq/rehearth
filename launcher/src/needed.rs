//! Finding mods that other mods need on the Workshop.
//!
//! A manifest names its dependencies by namespace ("trapper_plus"), and the
//! Workshop knows nothing about namespaces, so the lookup goes by title: search
//! for the namespace and keep the result whose title matches it best. That's a
//! guess, so the app checks each download's real namespace afterwards and takes
//! a wrong one back out (see `App::check_needed`).

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Result;

use crate::loadorder::KNOWN_WORKSHOP;
use crate::workshop::{self, Item, Sort};

/// One namespace and the Workshop item that should provide it, if one was found.
#[derive(Clone)]
pub struct Found {
    pub namespace: String,
    pub item: Option<Item>,
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

/// Looks each namespace up. `known` holds matches confirmed earlier,
/// `wrong` items already found not to be that namespace. Blocking.
pub fn find(
    namespaces: &[String],
    known: &BTreeMap<String, u64>,
    wrong: &BTreeMap<String, Vec<u64>>,
) -> Result<Vec<Found>> {
    // confirmed and built-in matches first: one details call for all of them
    let mut ids: HashMap<&str, u64> = HashMap::new();
    for ns in namespaces {
        let id = known
            .get(ns)
            .copied()
            .or_else(|| KNOWN_WORKSHOP.iter().find(|(k, ..)| k == ns).map(|(_, id, _)| *id));
        if let Some(id) = id {
            ids.insert(ns.as_str(), id);
        }
    }
    let wanted: Vec<u64> = ids.values().copied().collect();
    let details: HashMap<u64, Item> = workshop::details(&wanted)?.into_iter().map(|(i, _)| (i.id, i)).collect();

    let mut out = Vec::new();
    for ns in namespaces {
        if let Some(item) = ids.get(ns.as_str()).and_then(|id| details.get(id)) {
            out.push(Found { namespace: ns.clone(), item: Some(item.clone()) });
            continue;
        }
        let skip: HashSet<u64> = wrong.get(ns).map(|v| v.iter().copied().collect()).unwrap_or_default();
        let mut results = workshop::browse(Sort::Popular, ns, 1)?.items;
        if ns.contains('_') && best(ns, &results, &skip).is_none() {
            results.extend(workshop::browse(Sort::Popular, &ns.replace('_', " "), 1)?.items);
        }
        let item = best(ns, &results, &skip).cloned();
        out.push(Found { namespace: ns.clone(), item });
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
