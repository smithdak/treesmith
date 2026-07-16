//! Item graph and indexes derived from the serialized working tree —
//! always rebuildable from disk, never authoritative (spec I1).
//!
//! [`Graph::build`] walks a repo root (skipping `.git`, `target`,
//! `node_modules`, `bin`, `obj`), sniffs item files via the detected
//! [`SerializationFormat`], parses them in parallel with rayon, and
//! assembles items + indexes deterministically (files sorted by path;
//! a duplicated GUID keeps the lexically-first file and records a
//! `duplicate-id` [`TreeFault`]). Parse faults are recorded, never
//! dropped (spec §3.4); consumers decide whether a faulted tree blocks
//! their operation (exit-3 class).

pub mod query;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::ser::SerializeStruct;
use treesmith_format::{ParsedItem, SerializationFormat};
use treesmith_types::Guid;

/// Directory names never descended into (same set as the census walk).
const EXCLUDED_DIRS: &[&str] = &[".git", "target", "node_modules", "bin", "obj"];

fn is_excluded_dir(name: &str) -> bool {
    EXCLUDED_DIRS.iter().any(|d| name.eq_ignore_ascii_case(d))
}

/// One serialized item in the graph.
#[derive(Clone, Debug)]
pub struct ItemNode {
    /// The item's GUID.
    pub id: Guid,
    /// Absolute path of the backing file.
    pub file: PathBuf,
    /// The lossless parsed item.
    pub item: ParsedItem,
    /// Extracted metadata used by indexes and queries.
    pub meta: ItemMeta,
}

/// Cheap metadata extracted from a parsed item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemMeta {
    /// The item's GUID.
    pub id: Guid,
    /// The `Parent` GUID, if present.
    pub parent: Option<Guid>,
    /// The `Template` GUID, if present.
    pub template: Option<Guid>,
    /// The `Path` value (empty string when the file omits it).
    pub path: String,
    /// Last `/`-separated segment of `path` (empty when `path` is empty).
    pub name: String,
    /// The `DB` value, if present.
    pub db: Option<String>,
    /// `(language, version numbers ascending)`, languages sorted
    /// alphabetically. A language with only unversioned fields appears
    /// with an empty version list.
    pub languages: Vec<(String, Vec<u32>)>,
}

/// A problem discovered while assembling the graph. Never dropped.
///
/// `kind` is one of `"parse"`, `"missing-id"`, `"duplicate-id"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeFault {
    /// Root-relative path of the offending file.
    pub file: PathBuf,
    /// `"parse" | "missing-id" | "duplicate-id"`.
    pub kind: String,
    /// Human-readable detail (parse faults include the 1-based line).
    pub message: String,
}

impl serde::Serialize for TreeFault {
    /// Serializes as `{file, kind, message}` with a forward-slash file path.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut s = serializer.serialize_struct("TreeFault", 3)?;
        s.serialize_field("file", &self.file.to_string_lossy().replace('\\', "/"))?;
        s.serialize_field("kind", &self.kind)?;
        s.serialize_field("message", &self.message)?;
        s.end()
    }
}

/// Every file in the repo (not just items), for code-file resolution.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoFiles {
    /// Forward-slash repo-relative paths, sorted.
    pub all: Vec<String>,
}

impl RepoFiles {
    /// Case-insensitive suffix match after normalizing `\` -> `/` and
    /// trimming a leading `/` or `~/`. Matches whole path segments only
    /// (`Views/Hero.cshtml` does not match `Views/NotHero.cshtml`).
    /// Results keep the sorted order of [`RepoFiles::all`].
    pub fn find_suffix(&self, virtual_path: &str) -> Vec<&str> {
        let norm = virtual_path.replace('\\', "/");
        let norm = norm.strip_prefix('~').unwrap_or(&norm);
        let needle = norm.trim_start_matches('/').to_ascii_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        self.all
            .iter()
            .filter(|f| {
                let lf = f.to_ascii_lowercase();
                lf.ends_with(&needle)
                    && (lf.len() == needle.len()
                        || lf.as_bytes()[lf.len() - needle.len() - 1] == b'/')
            })
            .map(String::as_str)
            .collect()
    }

    /// Files with the given extension (leading `.` optional,
    /// case-insensitive). Results keep the sorted order of `all`.
    pub fn with_extension(&self, ext: &str) -> Vec<&str> {
        let ext = ext.trim_start_matches('.').to_ascii_lowercase();
        if ext.is_empty() {
            return Vec::new();
        }
        let suffix = format!(".{ext}");
        self.all
            .iter()
            .filter(|f| f.to_ascii_lowercase().ends_with(&suffix))
            .map(String::as_str)
            .collect()
    }
}

/// Per-file scan outcome retained between assemblies so refreshes stay
/// incremental and duplicate-id resolution stays deterministic.
#[derive(Clone, Debug)]
enum FileRecord {
    Item(ParsedItem),
    ParseFault { message: String },
}

/// The derived item graph over one serialized repo (spec I1).
pub struct Graph {
    root: PathBuf,
    format: &'static dyn SerializationFormat,
    /// Absolute item-file path -> latest scan outcome (BTreeMap: path order).
    files: BTreeMap<PathBuf, FileRecord>,
    /// All repo files, root-relative forward-slash, sorted.
    repo_set: BTreeSet<String>,
    // ---- derived on every assemble ----
    items: HashMap<Guid, ItemNode>,
    children: HashMap<Guid, Vec<Guid>>,
    by_path: HashMap<String, Vec<Guid>>,
    by_template: HashMap<Guid, Vec<Guid>>,
    ids_by_path: Vec<Guid>,
    faults: Vec<TreeFault>,
    repo_files: RepoFiles,
}

impl std::fmt::Debug for Graph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Graph")
            .field("root", &self.root)
            .field("format", &self.format.key())
            .field("items", &self.items.len())
            .field("faults", &self.faults.len())
            .finish_non_exhaustive()
    }
}

impl Graph {
    /// Builds the graph for `root`; the format comes from
    /// [`treesmith_format::detect`]. Faults are recorded, never fatal.
    pub fn build(root: &Path) -> Graph {
        let mut graph = Graph {
            root: root.to_path_buf(),
            format: treesmith_format::detect(root),
            files: BTreeMap::new(),
            repo_set: BTreeSet::new(),
            items: HashMap::new(),
            children: HashMap::new(),
            by_path: HashMap::new(),
            by_template: HashMap::new(),
            ids_by_path: Vec::new(),
            faults: Vec::new(),
            repo_files: RepoFiles::default(),
        };
        graph.rebuild();
        graph
    }

    /// The detected serialization format.
    pub fn format(&self) -> &'static dyn SerializationFormat {
        self.format
    }

    /// The repo root the graph was built from.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Full rescan from disk (I1: everything derived and rebuildable).
    pub fn rebuild(&mut self) {
        self.scan();
        self.assemble();
    }

    /// Re-parses the given changed paths and drops deleted ones, then
    /// reassembles indexes. Paths may be absolute or root-relative; a
    /// directory path refreshes everything under it (and drops entries
    /// under a deleted directory).
    pub fn refresh_paths(&mut self, paths: &[PathBuf]) {
        for p in paths {
            let abs = if p.is_absolute() {
                p.clone()
            } else {
                self.root.join(p)
            };
            self.refresh_one(&abs);
        }
        self.assemble();
    }

    /// The item with this GUID, if present.
    pub fn get(&self, id: Guid) -> Option<&ItemNode> {
        self.items.get(&id)
    }

    /// Items whose `Path` equals `path` case-insensitively. Duplicate
    /// paths are real (multiple ids may share one path); results are
    /// sorted by (path, id).
    pub fn find_path(&self, path: &str) -> Vec<Guid> {
        self.by_path
            .get(&path.to_lowercase())
            .cloned()
            .unwrap_or_default()
    }

    /// Children of `id` (via `Parent` links), sorted by (name, id).
    /// Works for unserialized parents too (partial-tree roots).
    pub fn children(&self, id: Guid) -> Vec<Guid> {
        self.children.get(&id).cloned().unwrap_or_default()
    }

    /// Items using this template, sorted by (path, id).
    pub fn by_template(&self, template: Guid) -> Vec<Guid> {
        self.by_template.get(&template).cloned().unwrap_or_default()
    }

    /// All item ids, sorted by (path, id) — the canonical result order.
    pub fn ids_by_path(&self) -> Vec<Guid> {
        self.ids_by_path.clone()
    }

    /// All faults, sorted by (file, kind, message).
    pub fn faults(&self) -> &[TreeFault] {
        &self.faults
    }

    /// Every repo file (root-relative, forward slashes, sorted).
    pub fn repo_files(&self) -> &RepoFiles {
        &self.repo_files
    }

    /// The absolute backing-file path of an item.
    pub fn file_of(&self, id: Guid) -> Option<&Path> {
        self.items.get(&id).map(|n| n.file.as_path())
    }

    // ---- internals -----------------------------------------------------

    /// Full walk: repo-file census + parallel parse of sniffed item files.
    fn scan(&mut self) {
        self.files.clear();
        self.repo_set.clear();
        let mut candidates: Vec<PathBuf> = Vec::new();
        let walker = walkdir::WalkDir::new(&self.root)
            .into_iter()
            .filter_entry(|e| {
                !(e.file_type().is_dir() && e.file_name().to_str().is_some_and(is_excluded_dir))
            });
        for entry in walker.flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            let abs = entry.into_path();
            self.repo_set.insert(rel_string(&self.root, &abs));
            if abs
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| self.format.sniff_file_name(n))
            {
                candidates.push(abs);
            }
        }
        let fmt = self.format;
        let parsed: Vec<(PathBuf, FileRecord)> = candidates
            .into_par_iter()
            .filter_map(|p| parse_file(fmt, &p).map(|rec| (p, rec)))
            .collect();
        self.files.extend(parsed); // BTreeMap: insertion order irrelevant
    }

    /// Refreshes one absolute path: deleted → drop (recursively for
    /// directories), directory → rescan its files, file → re-parse.
    fn refresh_one(&mut self, abs: &Path) {
        // Never track anything inside an excluded directory.
        if rel_string(&self.root, abs).split('/').any(is_excluded_dir) {
            return;
        }
        match std::fs::metadata(abs) {
            Err(_) => {
                // Deleted file or directory: drop it and anything under it.
                self.files.retain(|k, _| !k.starts_with(abs));
                let rel = rel_string(&self.root, abs);
                let prefix = format!("{rel}/");
                self.repo_set
                    .retain(|f| f != &rel && !f.starts_with(&prefix));
            }
            Ok(m) if m.is_dir() => {
                self.files.retain(|k, _| !k.starts_with(abs));
                let prefix = format!("{}/", rel_string(&self.root, abs));
                self.repo_set.retain(|f| !f.starts_with(&prefix));
                let walker = walkdir::WalkDir::new(abs).into_iter().filter_entry(|e| {
                    !(e.file_type().is_dir() && e.file_name().to_str().is_some_and(is_excluded_dir))
                });
                for entry in walker.flatten() {
                    if entry.file_type().is_file() {
                        self.refresh_file(&entry.into_path());
                    }
                }
            }
            Ok(_) => self.refresh_file(abs),
        }
    }

    /// Refreshes a single existing file: tracked in the repo census, and
    /// re-parsed if it sniffs as an item file.
    fn refresh_file(&mut self, abs: &Path) {
        self.repo_set.insert(rel_string(&self.root, abs));
        let is_item_name = abs
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| self.format.sniff_file_name(n));
        if !is_item_name {
            return;
        }
        match parse_file(self.format, abs) {
            Some(rec) => {
                self.files.insert(abs.to_path_buf(), rec);
            }
            None => {
                // No longer sniffs as an item (or unreadable): drop it.
                self.files.remove(abs);
            }
        }
    }

    /// Deterministic assembly: iterate files in path order, extract
    /// metadata, index. Duplicate GUID keeps the lexically-first file and
    /// records a `duplicate-id` fault against every later file.
    fn assemble(&mut self) {
        self.items.clear();
        self.children.clear();
        self.by_path.clear();
        self.by_template.clear();
        self.ids_by_path.clear();
        self.faults.clear();

        for (file, record) in &self.files {
            let rel = rel_path(&self.root, file);
            match record {
                FileRecord::ParseFault { message } => self.faults.push(TreeFault {
                    file: rel,
                    kind: "parse".to_string(),
                    message: message.clone(),
                }),
                FileRecord::Item(item) => {
                    let Some(id) = item.id() else {
                        self.faults.push(TreeFault {
                            file: rel,
                            kind: "missing-id".to_string(),
                            message: "item has no parseable ID".to_string(),
                        });
                        continue;
                    };
                    if let Some(existing) = self.items.get(&id) {
                        let kept = rel_path(&self.root, &existing.file)
                            .to_string_lossy()
                            .replace('\\', "/");
                        self.faults.push(TreeFault {
                            file: rel,
                            kind: "duplicate-id".to_string(),
                            message: format!("duplicate item id {id}; kept {kept}"),
                        });
                        continue;
                    }
                    let meta = meta_of(id, item);
                    self.items.insert(
                        id,
                        ItemNode {
                            id,
                            file: file.clone(),
                            item: item.clone(),
                            meta,
                        },
                    );
                }
            }
        }

        // Indexes, all derived from the winning items, sorted by (path, id).
        let mut keyed: Vec<(String, Guid, Option<Guid>, Option<Guid>)> = self
            .items
            .values()
            .map(|n| (n.meta.path.clone(), n.id, n.meta.parent, n.meta.template))
            .collect();
        keyed.sort();
        for (path, id, parent, template) in &keyed {
            self.ids_by_path.push(*id);
            self.by_path
                .entry(path.to_lowercase())
                .or_default()
                .push(*id);
            if let Some(t) = template {
                self.by_template.entry(*t).or_default().push(*id);
            }
            if let Some(p) = parent {
                self.children.entry(*p).or_default().push(*id);
            }
        }
        // children re-sorted by (name, id) rather than (path, id).
        for kids in self.children.values_mut() {
            kids.sort_by(|a, b| {
                let na = &self.items[a].meta.name;
                let nb = &self.items[b].meta.name;
                (na, a).cmp(&(nb, b))
            });
        }

        self.faults
            .sort_by(|a, b| (&a.file, &a.kind, &a.message).cmp(&(&b.file, &b.kind, &b.message)));
        self.repo_files = RepoFiles {
            all: self.repo_set.iter().cloned().collect(),
        };
    }
}

/// Reads + sniffs + parses one file. `None` means "not an item file"
/// (unreadable, or the head does not sniff) — not a fault, per the
/// census semantics.
fn parse_file(fmt: &dyn SerializationFormat, path: &Path) -> Option<FileRecord> {
    let bytes = std::fs::read(path).ok()?;
    let head_len = bytes.len().min(512);
    if !fmt.sniff_head(&bytes[..head_len]) {
        return None;
    }
    Some(match fmt.parse(&bytes) {
        Ok(item) => FileRecord::Item(item),
        Err(f) => FileRecord::ParseFault {
            message: format!("line {}: {}", f.line, f.message),
        },
    })
}

fn meta_of(id: Guid, item: &ParsedItem) -> ItemMeta {
    let path = item.path().unwrap_or_default();
    let name = path.rsplit('/').next().unwrap_or("").to_string();
    let mut languages: Vec<(String, Vec<u32>)> = item
        .languages()
        .iter()
        .map(|l| {
            let mut versions: Vec<u32> = l.versions.iter().map(|(n, _)| *n).collect();
            versions.sort_unstable();
            (l.language.clone(), versions)
        })
        .collect();
    languages.sort();
    ItemMeta {
        id,
        parent: item.parent_id(),
        template: item.template_id(),
        path,
        name,
        db: item.db(),
        languages,
    }
}

fn rel_path(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

fn rel_string(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests;
