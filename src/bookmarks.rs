use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookmarkItem {
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookmarkFolder {
    pub title: String,
    pub children: Vec<BookmarkNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BookmarkNode {
    Url(BookmarkItem),
    Folder(BookmarkFolder),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookmarkRootKind {
    Bar,
    Other,
    Custom,
}

#[derive(Debug, Clone, Default)]
pub struct BookmarkStore {
    pub source_label: String,
    pub bar: Vec<BookmarkNode>,
    pub other: Vec<BookmarkNode>,
    pub custom: Vec<BookmarkItem>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum PersistedBookmarks {
    Tree {
        #[serde(default)]
        source_label: String,
        #[serde(default)]
        bar: Vec<BookmarkNode>,
        #[serde(default)]
        other: Vec<BookmarkNode>,
        #[serde(default)]
        custom: Vec<BookmarkItem>,
    },
    LegacyCustom(Vec<BookmarkItem>),
}

#[derive(Debug, Deserialize)]
struct ChromeBookmarksFile {
    roots: ChromeRoots,
}

#[derive(Debug, Deserialize)]
struct ChromeRoots {
    bookmark_bar: Option<ChromeNode>,
    other: Option<ChromeNode>,
    synced: Option<ChromeNode>,
}

#[derive(Debug, Deserialize)]
struct ChromeNode {
    #[serde(default)]
    name: String,
    #[serde(rename = "type", default)]
    node_type: String,
    url: Option<String>,
    #[serde(default)]
    children: Vec<ChromeNode>,
}

impl BookmarkStore {
    pub fn load() -> Self {
        if let Some(loaded) = Self::load_persisted_file() {
            match loaded {
                PersistedBookmarks::Tree {
                    source_label,
                    bar,
                    other,
                    custom,
                } if !bar.is_empty() || !other.is_empty() => {
                    return Self {
                        source_label: if source_label.is_empty() {
                            "Chrome".to_string()
                        } else {
                            source_label
                        },
                        bar,
                        other,
                        custom,
                    };
                }
                PersistedBookmarks::Tree { custom, .. } | PersistedBookmarks::LegacyCustom(custom) => {
                    let mut store = Self {
                        source_label: "Empty".to_string(),
                        bar: Vec::new(),
                        other: Vec::new(),
                        custom,
                    };
                    let _ = store.import_from_chrome();
                    return store;
                }
            }
        }
        let mut store = Self {
            source_label: "Empty".to_string(),
            bar: Vec::new(),
            other: Vec::new(),
            custom: Vec::new(),
        };
        let _ = store.import_from_chrome();
        store
    }

    pub fn import_from_chrome(&mut self) -> Result<usize, String> {
        let candidates = find_chrome_bookmark_files();
        for (profile_label, path) in candidates {
            if let Ok((mut bar, mut other)) = parse_chrome_bookmarks(&path) {
                let total = count_urls(&bar) + count_urls(&other);
                if total > 0 || self.bar.is_empty() {
                    merge_extra_urls(&self.bar, &mut bar);
                    merge_extra_urls(&self.other, &mut other);
                    self.bar = bar;
                    self.other = other;
                    self.source_label = format!("Chrome ({profile_label})");
                    self.save_custom_file();
                    if total > 0 {
                        return Ok(total);
                    }
                }
            }
        }
        Err("Chrome 북마크 파일을 찾지 못했습니다.".to_string())
    }

    pub fn total_count(&self) -> usize {
        count_urls(&self.bar) + count_urls(&self.other) + self.custom.len()
    }

    /// Borrows the folder contents instead of cloning the whole bookmark tree.
    pub fn resolve_folder(
        &self,
        kind: BookmarkRootKind,
        path: &[usize],
    ) -> Option<(String, Cow<'_, [BookmarkNode]>)> {
        let (root_title, mut cur): (&str, Cow<'_, [BookmarkNode]>) = match kind {
            BookmarkRootKind::Bar => ("북마크바", Cow::Borrowed(&self.bar[..])),
            BookmarkRootKind::Other => ("기타 북마크", Cow::Borrowed(&self.other[..])),
            BookmarkRootKind::Custom => (
                "이 브라우저에서 추가",
                Cow::Owned(self.custom.iter().cloned().map(BookmarkNode::Url).collect()),
            ),
        };
        let mut cur_title = root_title.to_string();
        for &idx in path {
            let next = match cur.get(idx) {
                Some(BookmarkNode::Folder(folder)) => folder,
                _ => return None,
            };
            cur_title = next.title.clone();
            // Folders only exist under the borrowed Bar/Other roots.
            cur = match cur {
                Cow::Borrowed(nodes) => match &nodes[idx] {
                    BookmarkNode::Folder(f) => Cow::Borrowed(&f.children[..]),
                    _ => unreachable!(),
                },
                Cow::Owned(_) => return None,
            };
        }
        Some((cur_title, cur))
    }

    pub fn search(&self, query: &str) -> Vec<BookmarkItem> {
        let q = query.trim().to_ascii_lowercase();
        let mut out: Vec<BookmarkItem> = self.custom.iter().filter(|i| item_matches(i, &q)).cloned().collect();
        collect_matching(&self.bar, &q, &mut out);
        collect_matching(&self.other, &q, &mut out);
        out
    }

    pub fn contains_url(&self, url: &str) -> bool {
        self.custom.iter().any(|b| b.url == url)
            || tree_contains_url(&self.bar, url)
            || tree_contains_url(&self.other, url)
    }

    pub fn remove_url(&mut self, url: &str) -> bool {
        let before = self.custom.len();
        self.custom.retain(|b| b.url != url);
        let removed = (self.custom.len() != before)
            | remove_url_from_tree(&mut self.bar, url)
            | remove_url_from_tree(&mut self.other, url);
        if removed {
            self.save_custom_file();
        }
        removed
    }

    pub fn toggle_custom(&mut self, title: &str, url: &str) -> bool {
        if let Some(pos) = self.custom.iter().position(|b| b.url == url) {
            self.custom.remove(pos);
            self.save_custom_file();
            false
        } else if remove_url_from_tree(&mut self.bar, url) | remove_url_from_tree(&mut self.other, url) {
            self.save_custom_file();
            false
        } else {
            let clean_title = if title.trim().is_empty() {
                url.to_string()
            } else {
                title.trim().to_string()
            };
            self.custom.insert(
                0,
                BookmarkItem {
                    title: clean_title,
                    url: url.to_string(),
                },
            );
            self.save_custom_file();
            true
        }
    }

    /// Moves or registers a bookmark (`src_spec`: `"custom-0"`, `"bar-0-1"`, or `"url:<url>"`)
    /// into a destination folder (`dst_folder_spec`: `"bar"`, `"bar-0"`, `"other-1"`, `"custom"`).
    /// Returns `(bookmark_title, folder_title)`.
    pub fn move_to_folder(
        &mut self,
        src_spec: &str,
        fallback_title: &str,
        dst_folder_spec: &str,
    ) -> Result<(String, String), String> {
        let res = self.move_to_folder_in_memory(src_spec, fallback_title, dst_folder_spec)?;
        self.save_custom_file();
        Ok(res)
    }

    pub fn move_to_folder_in_memory(
        &mut self,
        src_spec: &str,
        fallback_title: &str,
        dst_folder_spec: &str,
    ) -> Result<(String, String), String> {
        let (dst_kind, dst_path) = parse_folder_spec(dst_folder_spec);
        if dst_kind == BookmarkRootKind::Custom && !dst_path.is_empty() {
            return Err("잘못된 대상 폴더입니다.".to_string());
        }
        let folder_title = self
            .resolve_folder(dst_kind, &dst_path)
            .map(|(t, _)| t)
            .ok_or_else(|| "대상 폴더를 찾을 수 없습니다.".to_string())?;

        if let Some(url) = src_spec.strip_prefix("url:") {
            let clean_title = if fallback_title.trim().is_empty() {
                url
            } else {
                fallback_title.trim()
            };
            let item = BookmarkItem {
                title: clean_title.to_string(),
                url: url.to_string(),
            };
            // If it was in `custom`, move it out of `custom` when placing into a folder.
            if dst_kind != BookmarkRootKind::Custom {
                self.custom.retain(|b| b.url != item.url);
            }
            let bm_title = item.title.clone();
            self.insert_into_folder(dst_kind, &dst_path, item)?;
            return Ok((bm_title, folder_title));
        }

        let (src_kind, src_path) = parse_folder_spec(src_spec);
        if src_path.is_empty() {
            return Err("이동할 북마크 항목이 아닙니다.".to_string());
        }
        let src_parent = &src_path[..src_path.len() - 1];
        let src_idx = src_path[src_path.len() - 1];

        if src_kind == dst_kind && src_parent == dst_path.as_slice() {
            let item = self.peek_item(src_kind, src_parent, src_idx)?;
            return Ok((item.title, folder_title));
        }

        let item = self.peek_item(src_kind, src_parent, src_idx)?;
        let bm_title = item.title.clone();

        if src_kind == dst_kind && src_parent.len() < dst_path.len() {
            self.insert_into_folder(dst_kind, &dst_path, item.clone())?;
            if let Err(e) = self.remove_item(src_kind, src_parent, src_idx) {
                // Roll back the insert so a failed move can't duplicate the item.
                let _ = self.remove_item(dst_kind, &dst_path, self.resolve_folder(dst_kind, &dst_path).map(|(_, n)| n.len().saturating_sub(1)).unwrap_or(0));
                let _ = item;
                return Err(e);
            }
        } else {
            let _ = self.remove_item(src_kind, src_parent, src_idx)?;
            self.insert_into_folder(dst_kind, &dst_path, item)?;
        }

        Ok((bm_title, folder_title))
    }

    fn peek_item(&self, kind: BookmarkRootKind, parent: &[usize], idx: usize) -> Result<BookmarkItem, String> {
        match kind {
            BookmarkRootKind::Custom => self
                .custom
                .get(idx)
                .cloned()
                .ok_or_else(|| "북마크를 찾을 수 없습니다.".to_string()),
            BookmarkRootKind::Bar | BookmarkRootKind::Other => {
                let (_, nodes) = self
                    .resolve_folder(kind, parent)
                    .ok_or_else(|| "원본 폴더를 찾을 수 없습니다.".to_string())?;
                match nodes.get(idx) {
                    Some(BookmarkNode::Url(item)) => Ok(item.clone()),
                    _ => Err("북마크 URL 항목이 아닙니다.".to_string()),
                }
            }
        }
    }

    fn remove_item(&mut self, kind: BookmarkRootKind, parent: &[usize], idx: usize) -> Result<BookmarkItem, String> {
        match kind {
            BookmarkRootKind::Custom => {
                if idx < self.custom.len() {
                    Ok(self.custom.remove(idx))
                } else {
                    Err("북마크를 찾을 수 없습니다.".to_string())
                }
            }
            BookmarkRootKind::Bar => remove_node_at(&mut self.bar, parent, idx),
            BookmarkRootKind::Other => remove_node_at(&mut self.other, parent, idx),
        }
    }

    fn insert_into_folder(&mut self, kind: BookmarkRootKind, path: &[usize], item: BookmarkItem) -> Result<(), String> {
        match kind {
            BookmarkRootKind::Custom => {
                self.custom.retain(|b| b.url != item.url);
                self.custom.insert(0, item);
                Ok(())
            }
            BookmarkRootKind::Bar => insert_node_at(&mut self.bar, path, item),
            BookmarkRootKind::Other => insert_node_at(&mut self.other, path, item),
        }
    }

    fn custom_file_path() -> Option<PathBuf> {
        Some(app_data_dir()?.join("bookmarks.json"))
    }

    fn load_persisted_file() -> Option<PersistedBookmarks> {
        let path = Self::custom_file_path()?;
        let raw = fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    fn save_custom_file(&self) {
        let Some(path) = Self::custom_file_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let persisted = PersistedBookmarks::Tree {
            source_label: self.source_label.clone(),
            bar: self.bar.clone(),
            other: self.other.clone(),
            custom: self.custom.clone(),
        };
        if let Ok(json) = serde_json::to_string_pretty(&persisted) {
            let tmp = path.with_extension("tmp");
            if fs::write(&tmp, json).is_ok() {
                let _ = fs::rename(&tmp, &path);
            }
        }
    }
}

pub fn parse_folder_spec(spec: &str) -> (BookmarkRootKind, Vec<usize>) {
    let mut parts = spec.split('-');
    let kind = match parts.next() {
        Some("other") => BookmarkRootKind::Other,
        Some("custom") => BookmarkRootKind::Custom,
        _ => BookmarkRootKind::Bar,
    };
    let path = parts.filter_map(|p| p.parse::<usize>().ok()).collect();
    (kind, path)
}

fn folder_children_mut<'a>(
    root: &'a mut Vec<BookmarkNode>,
    path: &[usize],
) -> Result<&'a mut Vec<BookmarkNode>, String> {
    let mut cur = root;
    for &idx in path {
        cur = match cur.get_mut(idx) {
            Some(BookmarkNode::Folder(f)) => &mut f.children,
            _ => return Err("폴더 경로가 올바르지 않습니다.".to_string()),
        };
    }
    Ok(cur)
}

fn remove_node_at(root: &mut Vec<BookmarkNode>, parent: &[usize], idx: usize) -> Result<BookmarkItem, String> {
    let list = folder_children_mut(root, parent)?;
    if idx >= list.len() {
        return Err("북마크 인덱스가 범위를 벗어났습니다.".to_string());
    }
    match list.remove(idx) {
        BookmarkNode::Url(item) => Ok(item),
         other => {
            list.insert(idx, other);
            Err("폴더는 이동할 수 없습니다.".to_string())
        }
    }
}

fn insert_node_at(root: &mut Vec<BookmarkNode>, path: &[usize], item: BookmarkItem) -> Result<(), String> {
    let list = folder_children_mut(root, path)?;
    list.retain(|n| !matches!(n, BookmarkNode::Url(u) if u.url == item.url));
    list.push(BookmarkNode::Url(item));
    Ok(())
}

fn remove_url_from_tree(nodes: &mut Vec<BookmarkNode>, url: &str) -> bool {
    let before = nodes.len();
    nodes.retain(|n| !matches!(n, BookmarkNode::Url(u) if u.url == url));
    let mut removed = nodes.len() != before;
    for node in nodes {
        if let BookmarkNode::Folder(f) = node {
            removed |= remove_url_from_tree(&mut f.children, url);
        }
    }
    removed
}

/// Preserves user-added URLs inside matching folders when re-importing Chrome's bookmark tree.
fn merge_extra_urls(old_nodes: &[BookmarkNode], new_nodes: &mut Vec<BookmarkNode>) {
    for old in old_nodes {
        match old {
            BookmarkNode::Url(item) => {
                if !new_nodes.iter().any(|n| matches!(n, BookmarkNode::Url(u) if u.url == item.url)) {
                    new_nodes.push(BookmarkNode::Url(item.clone()));
                }
            }
            BookmarkNode::Folder(old_f) => {
                if let Some(BookmarkNode::Folder(new_f)) = new_nodes
                    .iter_mut()
                    .find(|n| matches!(n, BookmarkNode::Folder(f) if f.title == old_f.title))
                {
                    merge_extra_urls(&old_f.children, &mut new_f.children);
                }
            }
        }
    }
}

/// Per-user app data directory:
/// Windows `%LOCALAPPDATA%/rust-browser`, macOS `~/Library/Application Support/rust-browser`,
/// Linux `$XDG_DATA_HOME/rust-browser` (or `~/.local/share/rust-browser`).
pub fn app_data_dir() -> Option<PathBuf> {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?
    } else if cfg!(target_os = "macos") {
        std::env::home_dir()?.join("Library").join("Application Support")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| Some(std::env::home_dir()?.join(".local").join("share")))?
    };
    Some(base.join("rust-browser"))
}

/// Chrome's "User Data" directory for the current platform.
fn chrome_user_data_dir() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
        Some(local.join("Google").join("Chrome").join("User Data"))
    } else if cfg!(target_os = "macos") {
        Some(std::env::home_dir()?.join("Library/Application Support/Google/Chrome"))
    } else {
        Some(std::env::home_dir()?.join(".config/google-chrome"))
    }
}

fn find_chrome_bookmark_files() -> Vec<(String, PathBuf)> {
    let mut results = Vec::new();
    let Some(chrome_user_data) = chrome_user_data_dir() else {
        return results;
    };
    if !chrome_user_data.is_dir() {
        return results;
    }

    let default_path = chrome_user_data.join("Default").join("Bookmarks");
    if default_path.is_file() {
        results.push(("Default".to_string(), default_path));
    }

    if let Ok(entries) = fs::read_dir(&chrome_user_data) {
        let mut profiles: Vec<(String, PathBuf)> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with("Profile ") {
                    let bm = entry.path().join("Bookmarks");
                    if bm.is_file() {
                        return Some((name, bm));
                    }
                }
                None
            })
            .collect();
        profiles.sort_by(|a, b| a.0.cmp(&b.0));
        results.extend(profiles);
    }

    results
}

fn parse_chrome_bookmarks(path: &Path) -> Result<(Vec<BookmarkNode>, Vec<BookmarkNode>), String> {
    let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let parsed: ChromeBookmarksFile = serde_json::from_str(&raw).map_err(|e| e.to_string())?;

    let bar = parsed
        .roots
        .bookmark_bar
        .map(|n| convert_chrome_children(&n.children))
        .unwrap_or_default();

    let mut other = Vec::new();
    if let Some(o) = parsed.roots.other {
        other.extend(convert_chrome_children(&o.children));
    }
    if let Some(s) = parsed.roots.synced {
        let synced = convert_chrome_children(&s.children);
        if !synced.is_empty() {
            other.push(BookmarkNode::Folder(BookmarkFolder {
                title: "Synced Bookmarks".to_string(),
                children: synced,
            }));
        }
    }

    Ok((bar, other))
}

fn convert_chrome_children(nodes: &[ChromeNode]) -> Vec<BookmarkNode> {
    let mut out = Vec::with_capacity(nodes.len());
    for node in nodes {
        match node.node_type.as_str() {
            "url" => {
                if let Some(url) = &node.url {
                    if !url.is_empty() {
                        out.push(BookmarkNode::Url(BookmarkItem {
                            title: if node.name.trim().is_empty() {
                                url.clone()
                            } else {
                                node.name.clone()
                            },
                            url: url.clone(),
                        }));
                    }
                }
            }
            "folder" => {
                let children = convert_chrome_children(&node.children);
                if !children.is_empty() {
                    out.push(BookmarkNode::Folder(BookmarkFolder {
                        title: if node.name.trim().is_empty() {
                            "Folder".to_string()
                        } else {
                            node.name.clone()
                        },
                        children,
                    }));
                }
            }
            _ => {}
        }
    }
    out
}

pub fn count_urls(nodes: &[BookmarkNode]) -> usize {
    nodes
        .iter()
        .map(|n| match n {
            BookmarkNode::Url(_) => 1,
            BookmarkNode::Folder(f) => count_urls(&f.children),
        })
        .sum()
}

fn tree_contains_url(nodes: &[BookmarkNode], target: &str) -> bool {
    nodes.iter().any(|n| match n {
        BookmarkNode::Url(item) => item.url == target,
        BookmarkNode::Folder(f) => tree_contains_url(&f.children, target),
    })
}

fn item_matches(item: &BookmarkItem, q_lower: &str) -> bool {
    q_lower.is_empty()
        || item.title.to_ascii_lowercase().contains(q_lower)
        || item.url.to_ascii_lowercase().contains(q_lower)
}

fn collect_matching(nodes: &[BookmarkNode], q_lower: &str, out: &mut Vec<BookmarkItem>) {
    for node in nodes {
        match node {
            BookmarkNode::Url(item) if item_matches(item, q_lower) => out.push(item.clone()),
            BookmarkNode::Url(_) => {}
            BookmarkNode::Folder(folder) => collect_matching(&folder.children, q_lower, out),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chrome_bookmarks_json_tree() {
        let sample = r#"{
            "roots": {
                "bookmark_bar": {
                    "type": "folder",
                    "name": "Bookmarks bar",
                    "children": [
                        { "type": "url", "name": "Rust", "url": "https://www.rust-lang.org/" },
                        {
                            "type": "folder",
                            "name": "Docs",
                            "children": [
                                { "type": "url", "name": "Docs.rs", "url": "https://docs.rs/" }
                            ]
                        }
                    ]
                },
                "other": { "type": "folder", "name": "Other", "children": [] },
                "synced": { "type": "folder", "name": "Mobile", "children": [] }
            }
        }"#;

        let parsed: ChromeBookmarksFile = serde_json::from_str(sample).unwrap();
        let bar = convert_chrome_children(&parsed.roots.bookmark_bar.unwrap().children);
        assert_eq!(count_urls(&bar), 2);
        assert!(tree_contains_url(&bar, "https://docs.rs/"));
        assert!(!tree_contains_url(&bar, "https://missing.test/"));

        let store = BookmarkStore {
            source_label: "Test".into(),
            bar,
            other: Vec::new(),
            custom: Vec::new(),
        };
        let (title, nodes) = store.resolve_folder(BookmarkRootKind::Bar, &[1]).unwrap();
        assert_eq!(title, "Docs");
        assert_eq!(nodes.len(), 1);
        assert_eq!(store.search("docs.rs").len(), 1);

        let mut store = store;
        store.custom.push(BookmarkItem {
            title: "Astryx".into(),
            url: "https://github.com/meta/astryx".into(),
        });
        let (bm_t, folder_t) = store
            .move_to_folder_in_memory("custom-0", "Astryx", "bar-1")
            .unwrap();
        assert_eq!(bm_t, "Astryx");
        assert_eq!(folder_t, "Docs");
        assert!(store.custom.is_empty());
        let (_, docs_nodes) = store.resolve_folder(BookmarkRootKind::Bar, &[1]).unwrap();
        assert_eq!(docs_nodes.len(), 2);

        // Moving a top-level bar item ("bar-0") into its sibling folder ("bar-1") handles index shift.
        let (bm_t2, folder_t2) = store
            .move_to_folder_in_memory("bar-0", "Rust", "bar-1")
            .unwrap();
        assert_eq!(bm_t2, "Rust");
        assert_eq!(folder_t2, "Docs");
        let (_, docs_nodes2) = store.resolve_folder(BookmarkRootKind::Bar, &[0]).unwrap();
        assert_eq!(docs_nodes2.len(), 3);
    }
}
