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
        let mut store = Self {
            source_label: "Empty".to_string(),
            bar: Vec::new(),
            other: Vec::new(),
            custom: Self::load_custom_file(),
        };
        let _ = store.import_from_chrome();
        store
    }

    pub fn import_from_chrome(&mut self) -> Result<usize, String> {
        let candidates = find_chrome_bookmark_files();
        for (profile_label, path) in candidates {
            if let Ok((bar, other)) = parse_chrome_bookmarks(&path) {
                let total = count_urls(&bar) + count_urls(&other);
                if total > 0 || self.bar.is_empty() {
                    self.bar = bar;
                    self.other = other;
                    self.source_label = format!("Chrome ({profile_label})");
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
        let mut out = Vec::new();
        for item in &self.custom {
            if q.is_empty()
                || item.title.to_ascii_lowercase().contains(&q)
                || item.url.to_ascii_lowercase().contains(&q)
            {
                out.push(item.clone());
            }
        }
        collect_matching(&self.bar, &q, &mut out);
        collect_matching(&self.other, &q, &mut out);
        out
    }

    pub fn contains_url(&self, url: &str) -> bool {
        self.custom.iter().any(|b| b.url == url)
            || tree_contains_url(&self.bar, url)
            || tree_contains_url(&self.other, url)
    }

    pub fn toggle_custom(&mut self, title: &str, url: &str) -> bool {
        if let Some(pos) = self.custom.iter().position(|b| b.url == url) {
            self.custom.remove(pos);
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

    fn custom_file_path() -> Option<PathBuf> {
        Some(app_data_dir()?.join("bookmarks.json"))
    }

    fn load_custom_file() -> Vec<BookmarkItem> {
        let Some(path) = Self::custom_file_path() else {
            return Vec::new();
        };
        let Ok(raw) = fs::read_to_string(path) else {
            return Vec::new();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    fn save_custom_file(&self) {
        let Some(path) = Self::custom_file_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.custom) {
            let _ = fs::write(path, json);
        }
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Per-user app data directory:
/// Windows `%LOCALAPPDATA%/rust-browser`, macOS `~/Library/Application Support/rust-browser`,
/// Linux `$XDG_DATA_HOME/rust-browser` (or `~/.local/share/rust-browser`).
pub fn app_data_dir() -> Option<PathBuf> {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?
    } else if cfg!(target_os = "macos") {
        home_dir()?.join("Library").join("Application Support")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| Some(home_dir()?.join(".local").join("share")))?
    };
    Some(base.join("rust-browser"))
}

/// Chrome's "User Data" directory for the current platform.
fn chrome_user_data_dir() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
        Some(local.join("Google").join("Chrome").join("User Data"))
    } else if cfg!(target_os = "macos") {
        Some(home_dir()?.join("Library/Application Support/Google/Chrome"))
    } else {
        Some(home_dir()?.join(".config/google-chrome"))
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

fn collect_matching(nodes: &[BookmarkNode], q_lower: &str, out: &mut Vec<BookmarkItem>) {
    for node in nodes {
        match node {
            BookmarkNode::Url(item) => {
                if q_lower.is_empty()
                    || item.title.to_ascii_lowercase().contains(q_lower)
                    || item.url.to_ascii_lowercase().contains(q_lower)
                {
                    out.push(item.clone());
                }
            }
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
    }
}
