// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Bookmark sets: a tree of folders and links made once and given to many
//! profiles.
//!
//! Asked for on 10.11.2026 by somebody running a team: the same working
//! links in every profile, in folders, and not the same ones in all of them.
//! A profile's own bookmarks were already kept (they are a file in its
//! directory and travel in its bundle); what was missing was a way to put
//! them there without opening forty browsers.
//!
//! # How a set reaches a browser
//!
//! Written into `Default/Bookmarks` before the browser starts, at the front
//! of the bookmarks bar. Chromium reads that file as it is: measured on core
//! 157 with a file that has no checksum, the tree loads, and a node's
//! `meta_info` survives the browser rewriting the file. That map is where
//! each top node of a set carries the set's id, and it is the whole
//! bookkeeping: on the next launch every node marked with a set this machine
//! knows is taken out and the sets the profile has now are written again.
//!
//! So a set is the operator's and the rest is the profile's. A bookmark made
//! in the browser is never touched. A change made in the browser to a set's
//! own node lasts until the next launch.
//!
//! A node marked with a set this machine has never heard of is left alone:
//! it came in a bundle from a colleague whose set it is, and removing it
//! because the list here does not name it would delete their work.
//!
//! # Where the sets live
//!
//! One JSON file on this machine, like templates and farming plans, for
//! local and team profiles alike: a set names profiles by id. A team does not
//! share the list of sets, only their result, through the bundle.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// The key in a bookmark node's `meta_info` that names the set it came from.
const MARK: &str = "fury_set";
const MAX_DEPTH: usize = 8;
const MAX_NODES: usize = 2000;

/// A link, or a folder of them. A folder is a node with no address.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Set {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub nodes: Vec<Node>,
    /// The profiles that get it, by id.
    #[serde(default)]
    pub profiles: Vec<String>,
    /// Every profile opened on this machine, whatever the list above says.
    #[serde(default)]
    pub all_profiles: bool,
    /// Kept after deletion, so that the next launch of each profile that had
    /// it knows the nodes are ours to take out. Never shown.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deleted: bool,
}

impl Set {
    fn is_for(&self, profile_id: &str) -> bool {
        !self.deleted && (self.all_profiles || self.profiles.iter().any(|p| p == profile_id))
    }

    /// Refuse what the browser would refuse or mangle, and say which.
    fn check(&mut self) -> anyhow::Result<()> {
        self.name = self.name.trim().to_string();
        if self.name.is_empty() {
            anyhow::bail!("the set needs a name");
        }
        let mut count = 0usize;
        fn walk(nodes: &mut [Node], depth: usize, count: &mut usize) -> anyhow::Result<()> {
            if depth > MAX_DEPTH {
                anyhow::bail!("folders are nested more than {MAX_DEPTH} deep");
            }
            for n in nodes {
                *count += 1;
                n.title = n.title.trim().to_string();
                match n.url.as_mut() {
                    Some(url) => {
                        *url = url.trim().to_string();
                        // A bare host is what people type. Anything with
                        // another scheme is refused rather than guessed at: a
                        // javascript: link runs in whatever page it is
                        // clicked on.
                        if !url.contains("://") && !url.is_empty() && !url.contains(':') {
                            *url = format!("https://{url}");
                        }
                        let ok = reqwest::Url::parse(url)
                            .map(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
                            .unwrap_or(false);
                        if !ok {
                            anyhow::bail!("{url:?} is not a web address");
                        }
                        if !n.children.is_empty() {
                            anyhow::bail!("{url:?} is a link and cannot hold other bookmarks");
                        }
                        if n.title.is_empty() {
                            n.title = url.clone();
                        }
                    }
                    None => {
                        if n.title.is_empty() {
                            anyhow::bail!("a folder needs a name");
                        }
                        walk(&mut n.children, depth + 1, count)?;
                    }
                }
            }
            Ok(())
        }
        walk(&mut self.nodes, 1, &mut count)?;
        if count > MAX_NODES {
            anyhow::bail!("{count} bookmarks in one set; the limit is {MAX_NODES}");
        }
        self.profiles.sort();
        self.profiles.dedup();
        Ok(())
    }
}

/// Every set on this machine, as the file holds them.
pub struct Book {
    path: PathBuf,
    pub sets: Vec<Set>,
}

impl Book {
    /// A missing or unreadable file is no sets, not an error: nothing here is
    /// worth refusing to open a browser over.
    pub fn load(path: PathBuf) -> Self {
        let sets = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        Self { path, sets }
    }

    fn write(&self) -> anyhow::Result<()> {
        let body = serde_json::to_string_pretty(&self.sets)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// The sets somebody can see.
    pub fn live(&self) -> Vec<&Set> {
        self.sets.iter().filter(|s| !s.deleted).collect()
    }

    /// Add a set, or replace the one with this id. Returns the id.
    pub fn save(&mut self, mut set: Set) -> anyhow::Result<String> {
        set.check()?;
        set.deleted = false;
        match self.sets.iter_mut().find(|s| !s.deleted && s.id == set.id && !set.id.is_empty()) {
            Some(old) => *old = set.clone(),
            None => {
                set.id = uuid::Uuid::now_v7().to_string();
                self.sets.push(set.clone());
            }
        }
        self.write()?;
        Ok(set.id)
    }

    /// Mark it deleted. Its nodes leave each profile at that profile's next
    /// launch, which is why the id is remembered.
    pub fn delete(&mut self, id: &str) -> anyhow::Result<bool> {
        let Some(set) = self.sets.iter_mut().find(|s| !s.deleted && s.id == id) else {
            return Ok(false);
        };
        set.deleted = true;
        set.nodes.clear();
        set.profiles.clear();
        set.all_profiles = false;
        self.write()?;
        Ok(true)
    }
}

/// A date the way the file spells one: microseconds since 1601, as a string.
fn chrome_now() -> String {
    const TO_1601_MICROS: i64 = 11_644_473_600_000_000;
    (chrono::Utc::now().timestamp_micros() + TO_1601_MICROS).to_string()
}

/// A guid that is the same for the same place in the same set, so that a
/// launch which changes nothing writes the file it read.
fn guid(set: &str, path: &str) -> String {
    use sha2::Digest;
    let h = sha2::Sha256::digest(format!("fury-bookmark\n{set}\n{path}").as_bytes());
    let mut b = [0u8; 16];
    b.copy_from_slice(&h[..16]);
    // The version and variant bits of a random UUID: Chromium rejects a guid
    // that is not a valid one and makes its own.
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(b).to_string()
}

fn max_id(node: &Value) -> u64 {
    let own = node.get("id").and_then(Value::as_str).and_then(|s| s.parse().ok()).unwrap_or(0);
    node.get("children")
        .and_then(Value::as_array)
        .map(|c| c.iter().map(max_id).max().unwrap_or(0))
        .unwrap_or(0)
        .max(own)
}

fn build(node: &Node, set: &str, path: &str, now: &str, next: &mut u64, top: bool) -> Value {
    *next += 1;
    let mut out = Map::new();
    out.insert("date_added".into(), json!(now));
    out.insert("guid".into(), json!(guid(set, path)));
    out.insert("id".into(), json!(next.to_string()));
    out.insert("name".into(), json!(node.title));
    if top {
        out.insert("meta_info".into(), json!({ MARK: set }));
    }
    match &node.url {
        Some(url) => {
            out.insert("type".into(), json!("url"));
            out.insert("url".into(), json!(url));
        }
        None => {
            out.insert("type".into(), json!("folder"));
            out.insert("date_modified".into(), json!(now));
            let children: Vec<Value> = node
                .children
                .iter()
                .enumerate()
                .map(|(i, c)| build(c, set, &format!("{path}/{i}"), now, next, false))
                .collect();
            out.insert("children".into(), Value::Array(children));
        }
    }
    Value::Object(out)
}

fn empty_file(now: &str) -> Value {
    let root = |id: &str, name: &str, guid: &str| {
        json!({ "children": [], "date_added": now, "date_modified": "0", "guid": guid,
                "id": id, "name": name, "type": "folder" })
    };
    // The three guids are Chromium's own, fixed ones for these roots.
    json!({
        "roots": {
            "bookmark_bar": root("1", "Bookmarks bar", "0bc5d13f-2cba-5d74-951f-3f233fe6c908"),
            "other": root("2", "Other bookmarks", "82b081ec-3dd3-529c-8475-ab6c344590dd"),
            "synced": root("3", "Mobile bookmarks", "4cf2e351-0e85-532b-bb37-df045d8f8d0f"),
        },
        "version": 1
    })
}

/// What is compared to decide whether the file needs writing: the nodes
/// without the fields that differ between two builds of the same set.
fn shape(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(k, _)| !matches!(k.as_str(), "id" | "date_added" | "date_modified" | "date_last_used"))
                .map(|(k, v)| (k.clone(), shape(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(shape).collect()),
        other => other.clone(),
    }
}

/// Bring a profile's bookmarks bar in line with the sets it has. Called with
/// the browser not running. True when the file was written.
///
/// A file that does not parse is left exactly as it is: the browser has its
/// own recovery for that (`Bookmarks.bak`) and writing over it would end it.
pub fn apply(profile_dir: &Path, sets: &[Set], profile_id: &str) -> anyhow::Result<bool> {
    let wanted: Vec<&Set> = sets.iter().filter(|s| s.is_for(profile_id) && !s.nodes.is_empty()).collect();
    let known: HashSet<&str> = sets.iter().map(|s| s.id.as_str()).collect();
    let path = profile_dir.join("Default").join("Bookmarks");
    let now = chrome_now();

    let mut file: Value = match std::fs::read(&path) {
        Ok(raw) => serde_json::from_slice(&raw)
            .map_err(|e| anyhow::anyhow!("the profile's bookmarks file does not parse ({e}); left as it is"))?,
        Err(_) if wanted.is_empty() => return Ok(false),
        Err(_) => empty_file(&now),
    };
    let mut next = file.get("roots").map(|r| r.as_object().map(|m| m.values().map(max_id).max().unwrap_or(0)).unwrap_or(0)).unwrap_or(0);
    let Some(bar) = file.pointer_mut("/roots/bookmark_bar/children").and_then(Value::as_array_mut) else {
        anyhow::bail!("the profile's bookmarks file has no bookmarks bar; left as it is");
    };

    let ours = |n: &Value| {
        n.pointer(&format!("/meta_info/{MARK}"))
            .and_then(Value::as_str)
            .is_some_and(|id| known.contains(id))
    };
    let before: Vec<Value> = bar.iter().filter(|n| ours(n)).map(shape).collect();
    let mut fresh = Vec::new();
    for set in &wanted {
        for (i, node) in set.nodes.iter().enumerate() {
            fresh.push(build(node, &set.id, &i.to_string(), &now, &mut next, true));
        }
    }
    if before == fresh.iter().map(shape).collect::<Vec<_>>() {
        return Ok(false);
    }

    let theirs: Vec<Value> = bar.drain(..).filter(|n| !ours(n)).collect();
    bar.extend(fresh);
    bar.extend(theirs);
    if let Some(root) = file.as_object_mut() {
        // It would no longer match, and a file without one loads the same.
        root.remove("checksum");
    }

    std::fs::create_dir_all(path.parent().expect("the file is inside Default"))?;
    let tmp = path.with_extension("fury.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&file)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmp::TempDir;

    fn link(title: &str, url: &str) -> Node {
        Node { title: title.into(), url: Some(url.into()), children: vec![] }
    }
    fn folder(title: &str, children: Vec<Node>) -> Node {
        Node { title: title.into(), url: None, children }
    }
    fn set(id: &str, profiles: &[&str], nodes: Vec<Node>) -> Set {
        Set {
            id: id.into(),
            name: id.into(),
            nodes,
            profiles: profiles.iter().map(|p| p.to_string()).collect(),
            all_profiles: false,
            deleted: false,
        }
    }
    fn bar(dir: &Path) -> Vec<Value> {
        let v: Value = serde_json::from_slice(&std::fs::read(dir.join("Default/Bookmarks")).unwrap()).unwrap();
        v.pointer("/roots/bookmark_bar/children").unwrap().as_array().unwrap().clone()
    }
    fn names(nodes: &[Value]) -> Vec<&str> {
        nodes.iter().map(|n| n["name"].as_str().unwrap()).collect()
    }

    #[test]
    fn a_set_lands_on_the_bar_with_its_folders_and_only_in_its_profiles() {
        let dir = TempDir::new("bm-first");
        let sets = [set(
            "a",
            &["p1"],
            vec![folder("Work", vec![link("Mail", "https://mail.example/"), folder("Deep", vec![link("X", "https://x.example/")])]), link("Top", "https://top.example/")],
        )];
        assert!(apply(&dir, &sets, "p1").unwrap());
        let b = bar(&dir);
        assert_eq!(names(&b), ["Work", "Top"]);
        assert_eq!(b[0]["meta_info"][MARK], "a");
        assert_eq!(b[0]["children"][1]["children"][0]["url"], "https://x.example/");
        // Every id in the file is its own.
        let mut ids = Vec::new();
        fn collect(n: &Value, ids: &mut Vec<String>) {
            if let Some(i) = n["id"].as_str() {
                ids.push(i.to_string());
            }
            for c in n["children"].as_array().into_iter().flatten() {
                collect(c, ids);
            }
        }
        let file: Value = serde_json::from_slice(&std::fs::read(dir.join("Default/Bookmarks")).unwrap()).unwrap();
        for root in file["roots"].as_object().unwrap().values() {
            collect(root, &mut ids);
        }
        let unique: HashSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "{ids:?}");

        // Another profile gets nothing, and no file is made for it.
        let other = TempDir::new("bm-other");
        assert!(!apply(&other, &sets, "p2").unwrap());
        assert!(!other.join("Default/Bookmarks").exists());
    }

    #[test]
    fn the_profiles_own_bookmarks_are_never_touched() {
        let dir = TempDir::new("bm-own");
        std::fs::create_dir_all(dir.join("Default")).unwrap();
        let mut file = empty_file("1");
        file["checksum"] = json!("stale");
        file["roots"]["bookmark_bar"]["children"] = json!([
            { "type": "url", "name": "Mine", "url": "https://mine.example/", "id": "7", "guid": "g", "date_added": "1" },
            // A colleague's set, unknown here: it came in the bundle.
            { "type": "url", "name": "Theirs", "url": "https://theirs.example/", "id": "8", "guid": "h",
              "date_added": "1", "meta_info": { MARK: "elsewhere" } },
        ]);
        std::fs::write(dir.join("Default/Bookmarks"), serde_json::to_vec(&file).unwrap()).unwrap();

        let mut sets = vec![set("a", &["p"], vec![link("Ours", "https://ours.example/")])];
        assert!(apply(&dir, &sets, "p").unwrap());
        assert_eq!(names(&bar(&dir)), ["Ours", "Mine", "Theirs"]);
        let written: Value = serde_json::from_slice(&std::fs::read(dir.join("Default/Bookmarks")).unwrap()).unwrap();
        assert!(written.get("checksum").is_none());
        assert_eq!(bar(&dir)[0]["id"], "9", "numbered after the highest id already there");

        // The same sets again: nothing to write.
        assert!(!apply(&dir, &sets, "p").unwrap());

        // The set changes: its old nodes go, the new ones come, the rest stays.
        sets[0].nodes = vec![link("Newer", "https://newer.example/")];
        assert!(apply(&dir, &sets, "p").unwrap());
        assert_eq!(names(&bar(&dir)), ["Newer", "Mine", "Theirs"]);

        // Taken away from the profile, and then deleted: gone either way.
        sets[0].profiles.clear();
        assert!(apply(&dir, &sets, "p").unwrap());
        assert_eq!(names(&bar(&dir)), ["Mine", "Theirs"]);
    }

    #[test]
    fn a_deleted_set_is_remembered_long_enough_to_be_removed() {
        let home = TempDir::new("bm-book");
        let mut book = Book::load(home.join("bookmarks.json"));
        let mut new = set("", &["p"], vec![link("", "example.com")]);
        new.name = "Work".into();
        let id = book.save(new).unwrap();
        assert!(!id.is_empty());
        // A bare host became an address, and an untitled link is titled by it.
        assert_eq!(book.sets[0].nodes[0].url.as_deref(), Some("https://example.com"));
        assert_eq!(book.sets[0].nodes[0].title, "https://example.com");

        let dir = TempDir::new("bm-deleted");
        assert!(apply(&dir, &book.sets, "p").unwrap());
        assert_eq!(bar(&dir).len(), 1);

        assert!(book.delete(&id).unwrap());
        let again = Book::load(home.join("bookmarks.json"));
        assert!(again.live().is_empty());
        assert!(apply(&dir, &again.sets, "p").unwrap());
        assert!(bar(&dir).is_empty());
    }

    #[test]
    fn what_the_browser_would_mangle_is_refused() {
        let home = TempDir::new("bm-check");
        let mut book = Book::load(home.join("bookmarks.json"));
        let named = |nodes| Set { name: "Work".into(), ..set("", &[], nodes) };
        assert!(book.save(named(vec![link("x", "javascript:alert(1)")])).is_err());
        assert!(book.save(named(vec![link("x", "file:///etc/passwd")])).is_err());
        assert!(book.save(named(vec![folder("", vec![])])).is_err());
        assert!(book.save(named(vec![link("x", "https://ok.example/")])).is_ok());
        book.sets.clear();
        let mut nameless = set("", &[], vec![]);
        nameless.name = "  ".into();
        assert!(book.save(nameless).is_err());
        assert!(book.sets.is_empty());
    }

    #[test]
    fn a_file_that_does_not_parse_is_left_alone() {
        let dir = TempDir::new("bm-broken");
        std::fs::create_dir_all(dir.join("Default")).unwrap();
        std::fs::write(dir.join("Default/Bookmarks"), b"{ not json").unwrap();
        let sets = [set("a", &["p"], vec![link("Ours", "https://ours.example/")])];
        assert!(apply(&dir, &sets, "p").is_err());
        assert_eq!(std::fs::read(dir.join("Default/Bookmarks")).unwrap(), b"{ not json");
    }

    /// Writes a profile directory for a real browser to read, which is the
    /// only way to know the file is one Chromium takes:
    ///
    ///     FURY_TEST_BOOKMARKS_DIR=<empty dir> cargo test -p fury-agent bookmarks -- --ignored
    ///
    /// then start a core on that directory and look at chrome://bookmarks.
    /// Done 10.11.2026 on core 157: both sets on the bar, folders nested, the
    /// marks still in the file after the browser rewrote it.
    #[test]
    #[ignore = "writes a profile for a real core: set FURY_TEST_BOOKMARKS_DIR"]
    fn a_profile_for_a_real_browser_to_read() {
        let dir = PathBuf::from(std::env::var("FURY_TEST_BOOKMARKS_DIR").expect("FURY_TEST_BOOKMARKS_DIR"));
        let sets = [
            set("a", &["p"], vec![folder("Work", vec![link("Mail", "https://mail.example/"), folder("Deep", vec![link("X", "https://x.example/")])])]),
            Set { all_profiles: true, ..set("b", &[], vec![link("Everywhere", "https://everywhere.example/")]) },
        ];
        assert!(apply(&dir, &sets, "p").unwrap());
    }
}
