//! The query/mutation API both surfaces call — the "API" node of spec
//! §3.1, factored out so `cli` and `mcp` stay thin (DESIGN.md §1 structure
//! amendment).
//!
//! Every operation returns the exact `serde_json::Value` the surfaces
//! print/return (spec I4: 1:1). The write path (DESIGN.md §8 / spec §5)
//! is fully schema-aware: field ids resolved through the effective
//! template, slots derived from field definitions, values validated and
//! normalized, and every candidate file self-checked
//! (emit → re-parse → re-emit byte-equal) before anything touches disk.

mod error;
mod shapes;
mod write;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use treesmith_format::yaml::{Entry, Newline, Scalar, Value as YamlValue, YamlDocument};
use treesmith_format::{FieldSlot, ParsedItem};
use treesmith_gate::{GateConfig, GateCtx, GateReport, Severity};
use treesmith_graph::{Graph, ItemNode};
use treesmith_presentation::{scan_placeholders, PresentationError};
use treesmith_template::TemplateIndex;
use treesmith_types::Guid;

use error::validation;
pub use error::KernelError;
use shapes::{item_detail, item_summary, rel_file};
use write::{prepare_value, read_slot_value, resolve_field, self_check, slot_for};

/// A `set-field` mutation request (DESIGN.md §8).
#[derive(Clone, Debug)]
pub struct SetFieldRequest {
    /// Item designator: GUID in any form, or a `/sitecore/...` path.
    pub item: String,
    /// Field designator: name (resolved via the effective template) or GUID.
    pub field: String,
    /// The raw value to store.
    pub value: String,
    /// Language for unversioned/versioned fields (default `en`).
    pub language: Option<String>,
    /// Version for versioned fields (default: max existing).
    pub version: Option<u32>,
    /// Create version 1 when the language has none (default `true`).
    pub create_version: bool,
}

/// A `forge` (create item) request (DESIGN.md §8).
#[derive(Clone, Debug)]
pub struct ForgeRequest {
    /// Template designator: GUID, `/sitecore/...` path, or template name.
    pub template: String,
    /// Parent item designator (must be serialized).
    pub parent: String,
    /// New item name (one path segment).
    pub name: String,
    /// Explicit GUID; a random v4 GUID when absent.
    pub id: Option<Guid>,
    /// Create version 1 in this language.
    pub language: Option<String>,
}

/// A `move` request (DESIGN.md §8).
#[derive(Clone, Debug)]
pub struct MoveRequest {
    /// Item designator.
    pub item: String,
    /// New parent designator (must be serialized).
    pub new_parent: String,
    /// Rename while moving (default: keep the current name).
    pub name: Option<String>,
}

/// An open serialized tree: graph + template index + gate config,
/// everything derived from disk and rebuildable (spec I1).
pub struct Workspace {
    root: PathBuf,
    graph: Graph,
    templates: TemplateIndex,
    gate_config: GateConfig,
}

impl Workspace {
    /// Opens the tree at `root`. Succeeds even when the tree has faults
    /// (they are recorded; non-census ops then return
    /// [`KernelError::TreeFault`]). Fails on a missing root or an
    /// unreadable/invalid `treesmith.toml`.
    pub fn open(root: &Path) -> Result<Workspace, KernelError> {
        if !root.is_dir() {
            return Err(KernelError::Io(format!(
                "root `{}` is not a directory",
                root.display()
            )));
        }
        let gate_config = load_gate_config(root)?;
        let graph = Graph::build(root);
        let templates = TemplateIndex::build(&graph);
        Ok(Workspace {
            root: root.to_path_buf(),
            graph,
            templates,
            gate_config,
        })
    }

    /// The workspace root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Re-parses changed paths, drops deleted ones, and rebuilds the
    /// template index over the refreshed graph.
    pub fn refresh_paths(&mut self, paths: &[PathBuf]) {
        self.graph.refresh_paths(paths);
        self.templates = TemplateIndex::build(&self.graph);
    }

    /// Full rescan from disk: graph, template index, and gate config
    /// (a config that no longer parses keeps the previous one).
    pub fn rebuild(&mut self) {
        self.graph.rebuild();
        self.templates = TemplateIndex::build(&self.graph);
        if let Ok(config) = load_gate_config(&self.root) {
            self.gate_config = config;
        }
    }

    // ---- read ops -------------------------------------------------------------

    /// `query <expr>` → `{"ok":true,"count":N,"items":[ItemSummary]}`.
    pub fn query(&self, expr: &str) -> Result<Value, KernelError> {
        self.ensure_tree_ok()?;
        let query = treesmith_graph::query::parse_query(expr).map_err(KernelError::Usage)?;
        let items: Vec<Value> = query
            .run(&self.graph)
            .into_iter()
            .filter_map(|id| self.graph.get(id))
            .map(|n| item_summary(&self.root, &self.graph, &self.templates, n))
            .collect();
        Ok(json!({ "ok": true, "count": items.len(), "items": items }))
    }

    /// `get <item>` → `{"ok":true,"item":ItemDetail}`.
    pub fn get(&self, item: &str) -> Result<Value, KernelError> {
        self.ensure_tree_ok()?;
        let node = self.resolve_item(item)?;
        Ok(json!({
            "ok": true,
            "item": item_detail(&self.root, &self.graph, &self.templates, node),
        }))
    }

    /// `resolve-presentation <item>` → `ResolvedPresentation` wrapped as
    /// `{"ok":true, ...fields...}`.
    pub fn resolve_presentation(
        &self,
        item: &str,
        language: Option<&str>,
        version: Option<u32>,
    ) -> Result<Value, KernelError> {
        self.ensure_tree_ok()?;
        let node = self.resolve_item(item)?;
        let resolved = treesmith_presentation::resolve(
            &self.graph,
            &self.templates,
            node.id,
            language,
            version,
        )
        .map_err(|e| match e {
            PresentationError::ItemNotFound(id) => KernelError::Usage(format!("unknown item {id}")),
            PresentationError::MalformedLayoutXml { item, field, error } => validation(
                "malformed-layout-xml",
                format!(
                    "item {item} {field}: {} at offset {}",
                    error.message, error.offset
                ),
                json!({
                    "item": item.rainbow(),
                    "field": field,
                    "error": error.message,
                    "offset": error.offset,
                }),
            ),
        })?;
        let mut value = serde_json::to_value(resolved)
            .map_err(|e| KernelError::Io(format!("serialize presentation: {e}")))?;
        value
            .as_object_mut()
            .expect("presentation serializes as an object")
            .insert("ok".to_string(), Value::Bool(true));
        Ok(value)
    }

    /// `validate [--gate ...]` → the validate JSON shape plus a
    /// has-errors bool (drives exit code 1).
    pub fn validate(&self, gates: Option<&[String]>) -> Result<(Value, bool), KernelError> {
        self.ensure_tree_ok()?;
        let placeholders = scan_placeholders(&self.root, self.graph.repo_files());
        let ctx = GateCtx {
            graph: &self.graph,
            templates: &self.templates,
            placeholders: &placeholders,
            config: &self.gate_config,
        };
        let report: GateReport = match gates {
            None => treesmith_gate::run_all(&ctx),
            Some(names) => treesmith_gate::run_some(&ctx, names).map_err(KernelError::Usage)?,
        };
        let count = |s: Severity| report.findings.iter().filter(|f| f.severity == s).count();
        let (errors, warnings, infos) = (
            count(Severity::Error),
            count(Severity::Warning),
            count(Severity::Info),
        );
        let json = json!({
            "ok": errors == 0,
            "errors": errors,
            "warnings": warnings,
            "infos": infos,
            "findings": report.findings,
            "skipped": report.skipped.iter()
                .map(|(gate, reason)| json!({ "gate": gate, "reason": reason }))
                .collect::<Vec<_>>(),
        });
        Ok((json, errors > 0))
    }

    /// `census` — the P0 fidelity harness. Works on faulted trees by
    /// design (it is how faults are diagnosed).
    pub fn census(root: &Path) -> Value {
        let fmt = treesmith_format::detect(root);
        let census = treesmith_format::census::round_trip_census(root, fmt);
        json!({
            "ok": census.faults.is_empty() && census.mismatches.is_empty(),
            "files": census.files,
            "items": census.items,
            "roundTripOk": census.ok,
            "faults": census.faults,
            "mismatches": census.mismatches,
            "elapsedMs": census.elapsed_ms,
        })
    }

    // ---- write ops -------------------------------------------------------------

    /// `set-field` — the full schema-aware write path (DESIGN.md §8
    /// steps 1–5). Returns the mutate JSON shape.
    pub fn set_field(&mut self, req: &SetFieldRequest) -> Result<Value, KernelError> {
        self.ensure_tree_ok()?;
        let node = self.resolve_item(&req.item)?.clone();

        // 1. Field id via the effective template (never guessed from names).
        let effective = node.meta.template.and_then(|t| self.templates.resolve(t));
        let target = resolve_field(effective.as_ref(), &req.field)?;

        // 2. Slot from the field definition's section kind.
        let slot = slot_for(
            &target,
            &node.item,
            req.language.as_deref(),
            req.version,
            req.create_version,
        )?;

        // 3. Validate + normalize the value; Type: stamping decided here.
        let (value, type_hint) = prepare_value(&target, &req.value)?;

        // Blob-backed fields are not writable through set-field.
        if let Some(existing) = node.item.find_field(target.id) {
            if existing.0 == slot && existing.1.blob_id.is_some() {
                return Err(validation(
                    "blob-unsupported",
                    format!(
                        "field `{}` is blob-backed; set-field cannot write it",
                        target.name
                    ),
                    json!({ "field": target.id.rainbow() }),
                ));
            }
        }

        let mut item = node.item.clone();
        let hint = (!target.name.is_empty()).then_some(target.name.as_str());
        item.set_field(&slot, target.id, hint, type_hint.as_deref(), &value);

        // 4. Self-check before any disk write.
        let bytes = self_check(self.graph.format(), &item)?;
        let reparsed = self
            .graph
            .format()
            .parse(&bytes)
            .expect("self_check proved the bytes parse");
        if read_slot_value(&reparsed, &slot, target.id).as_deref() != Some(value.as_str()) {
            return Err(validation(
                "self-check-failed",
                format!(
                    "slot {} did not read back the requested value",
                    shapes::slot_string(&slot)
                ),
                json!({ "field": target.id.rainbow(), "slot": shapes::slot_string(&slot) }),
            ));
        }

        // 5. Write, then mirror disk into the graph.
        std::fs::write(&node.file, &bytes)
            .map_err(|e| KernelError::Io(format!("write {}: {e}", node.file.display())))?;
        self.refresh_paths(std::slice::from_ref(&node.file));

        self.mutate_result(node.id, vec![rel_file(&self.root, &node.file)])
    }

    /// `forge` — create a new item from a template under a serialized
    /// parent, at `format.child_file_path(parent_file, name)`.
    pub fn forge(&mut self, req: &ForgeRequest) -> Result<Value, KernelError> {
        self.ensure_tree_ok()?;
        let template = self.resolve_template(&req.template)?;
        let parent = self.resolve_item(&req.parent)?.clone();
        validate_name(&req.name)?;

        let id = req.id.unwrap_or_else(Guid::new_random);
        if self.graph.get(id).is_some() {
            return Err(validation(
                "already-exists",
                format!("item {id} already exists"),
                json!({ "id": id.rainbow() }),
            ));
        }
        let collision = self
            .graph
            .children(parent.id)
            .into_iter()
            .filter_map(|c| self.graph.get(c))
            .any(|c| c.meta.name.eq_ignore_ascii_case(&req.name));
        if collision {
            return Err(validation(
                "already-exists",
                format!(
                    "`{}` already has a child named `{}`",
                    parent.meta.path, req.name
                ),
                json!({ "parent": parent.id.rainbow(), "name": req.name }),
            ));
        }
        let file = self.graph.format().child_file_path(&parent.file, &req.name);
        if file.exists() {
            return Err(validation(
                "already-exists",
                format!(
                    "target file `{}` already exists",
                    rel_file(&self.root, &file)
                ),
                json!({ "file": rel_file(&self.root, &file) }),
            ));
        }

        // Minimal canonical item: ID/Parent/Template/Path, DB from parent.
        let path = format!("{}/{}", parent.meta.path, req.name);
        let mut root_entries = vec![
            scalar_entry("ID", Scalar::Quoted(id.rainbow())),
            scalar_entry("Parent", Scalar::Quoted(parent.id.rainbow())),
            scalar_entry("Template", Scalar::Quoted(template.rainbow())),
            scalar_entry("Path", Scalar::Plain(path)),
        ];
        if let Some(db) = &parent.meta.db {
            root_entries.push(scalar_entry("DB", Scalar::Plain(db.clone())));
        }
        let mut item = ParsedItem {
            doc: YamlDocument {
                bom: false,
                newline: Newline::Lf,
                trailing_newline: true,
                root: root_entries,
            },
        };
        if let Some(language) = &req.language {
            item.ensure_version(language, 1);
        }

        let bytes = self_check(self.graph.format(), &item)?;
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| KernelError::Io(format!("create {}: {e}", dir.display())))?;
        }
        std::fs::write(&file, &bytes)
            .map_err(|e| KernelError::Io(format!("write {}: {e}", file.display())))?;
        self.refresh_paths(std::slice::from_ref(&file));

        self.mutate_result(id, vec![rel_file(&self.root, &file)])
    }

    /// `move` — structure-safe relocation: subtree files move along the
    /// `child_file_path` convention, `Path` fields are rewritten, and
    /// path-form datasources / path-valued fields graph-wide are updated.
    pub fn move_item(&mut self, req: &MoveRequest) -> Result<Value, KernelError> {
        self.ensure_tree_ok()?;
        let node = self.resolve_item(&req.item)?.clone();
        let new_parent = self.resolve_item(&req.new_parent)?.clone();
        let name = req.name.clone().unwrap_or_else(|| node.meta.name.clone());
        validate_name(&name)?;

        let old_path = node.meta.path.clone();
        if new_parent.id == node.id || path_is_or_under(&new_parent.meta.path, &old_path) {
            return Err(validation(
                "invalid-move",
                format!("cannot move `{old_path}` under itself"),
                json!({ "item": node.id.rainbow(), "newParent": new_parent.id.rainbow() }),
            ));
        }
        let collision = self
            .graph
            .children(new_parent.id)
            .into_iter()
            .filter_map(|c| self.graph.get(c))
            .any(|c| c.id != node.id && c.meta.name.eq_ignore_ascii_case(&name));
        if collision {
            return Err(validation(
                "already-exists",
                format!(
                    "`{}` already has a child named `{name}`",
                    new_parent.meta.path
                ),
                json!({ "parent": new_parent.id.rainbow(), "name": name }),
            ));
        }
        let new_path = format!("{}/{}", new_parent.meta.path, name);

        // Plan the subtree relocation: id → (old file, new file, new path).
        let mut plan: Vec<(Guid, PathBuf, PathBuf, String)> = Vec::new();
        let root_new_file = self.graph.format().child_file_path(&new_parent.file, &name);
        self.plan_subtree(node.id, &root_new_file, &old_path, &new_path, &mut plan);
        let in_subtree: BTreeSet<Guid> = plan.iter().map(|(id, ..)| *id).collect();
        for (_, old_file, new_file, _) in &plan {
            if new_file != old_file && new_file.exists() {
                return Err(validation(
                    "already-exists",
                    format!(
                        "target file `{}` already exists",
                        rel_file(&self.root, new_file)
                    ),
                    json!({ "file": rel_file(&self.root, new_file) }),
                ));
            }
        }

        // Mutate: subtree Path fields (+ root Parent), then graph-wide
        // path-valued fields and path-form `ds=` datasources.
        let mut writes: Vec<(PathBuf, ParsedItem)> = Vec::new();
        for (id, _, new_file, item_new_path) in &plan {
            let mut item = self
                .graph
                .get(*id)
                .expect("subtree item in graph")
                .item
                .clone();
            item.set_path(item_new_path);
            if *id == node.id {
                item.set_parent(new_parent.id);
            }
            rewrite_path_references(&mut item, &old_path, &new_path);
            writes.push((new_file.clone(), item));
        }
        for id in self.graph.ids_by_path() {
            if in_subtree.contains(&id) {
                continue;
            }
            let other = self.graph.get(id).expect("id from index");
            let mut item = other.item.clone();
            if rewrite_path_references(&mut item, &old_path, &new_path) {
                writes.push((other.file.clone(), item));
            }
        }

        // Self-check every candidate before any disk write (all-or-nothing).
        let mut checked: Vec<(PathBuf, Vec<u8>)> = Vec::new();
        for (file, item) in &writes {
            let bytes = self_check(self.graph.format(), item)?;
            checked.push((file.clone(), bytes));
        }

        // Write everything, then delete vacated files and empty dirs.
        let mut changed: BTreeSet<String> = BTreeSet::new();
        for (file, bytes) in &checked {
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| KernelError::Io(format!("create {}: {e}", dir.display())))?;
            }
            std::fs::write(file, bytes)
                .map_err(|e| KernelError::Io(format!("write {}: {e}", file.display())))?;
            changed.insert(rel_file(&self.root, file));
        }
        let mut refresh: Vec<PathBuf> = checked.iter().map(|(f, _)| f.clone()).collect();
        for (_, old_file, new_file, _) in &plan {
            if old_file != new_file {
                std::fs::remove_file(old_file)
                    .map_err(|e| KernelError::Io(format!("remove {}: {e}", old_file.display())))?;
                changed.insert(rel_file(&self.root, old_file));
                refresh.push(old_file.clone());
            }
        }
        for (_, old_file, new_file, _) in &plan {
            if old_file != new_file {
                remove_empty_dirs(old_file.parent(), &self.root);
            }
        }
        self.refresh_paths(&refresh);

        self.mutate_result(node.id, changed.into_iter().collect())
    }

    // ---- internals -------------------------------------------------------------

    /// Tree-fault policy: every op except `census` refuses a faulted tree.
    fn ensure_tree_ok(&self) -> Result<(), KernelError> {
        let faults = self.graph.faults();
        if faults.is_empty() {
            Ok(())
        } else {
            Err(KernelError::TreeFault(faults.to_vec()))
        }
    }

    /// Item designator: a GUID in any form, else a `/sitecore/...` path.
    /// Ambiguous path → Usage listing candidates; unknown → Usage.
    fn resolve_item(&self, designator: &str) -> Result<&ItemNode, KernelError> {
        if let Ok(id) = Guid::parse(designator) {
            return self
                .graph
                .get(id)
                .ok_or_else(|| KernelError::Usage(format!("unknown item {id}")));
        }
        if designator.starts_with('/') {
            let matches = self.graph.find_path(designator);
            return match matches.len() {
                0 => Err(KernelError::Usage(format!(
                    "unknown item path `{designator}`"
                ))),
                1 => Ok(self.graph.get(matches[0]).expect("id from index")),
                _ => Err(KernelError::Usage(format!(
                    "ambiguous item path `{designator}`: candidates {}",
                    matches
                        .iter()
                        .map(|g| g.rainbow())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))),
            };
        }
        Err(KernelError::Usage(format!(
            "invalid item designator `{designator}`: expected a GUID or a /sitecore/... path"
        )))
    }

    /// Template designator: GUID, item path, or template name — must
    /// resolve to a known template in the index.
    fn resolve_template(&self, designator: &str) -> Result<Guid, KernelError> {
        if let Ok(id) = Guid::parse(designator) {
            return if self.templates.get(id).is_some() {
                Ok(id)
            } else {
                Err(KernelError::Usage(format!("unknown template {id}")))
            };
        }
        if designator.starts_with('/') {
            let node = self.resolve_item(designator)?;
            return if self.templates.get(node.id).is_some() {
                Ok(node.id)
            } else {
                Err(KernelError::Usage(format!(
                    "item `{designator}` is not a template"
                )))
            };
        }
        let matches = self.templates.find_by_name(designator);
        match matches.len() {
            0 => Err(KernelError::Usage(format!(
                "unknown template `{designator}`"
            ))),
            1 => Ok(matches[0]),
            _ => Err(KernelError::Usage(format!(
                "ambiguous template name `{designator}`: candidates {}",
                matches
                    .iter()
                    .map(|g| g.rainbow())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    /// Recursively plans new files/paths for `id` and its serialized
    /// descendants (children in canonical order).
    fn plan_subtree(
        &self,
        id: Guid,
        new_file: &Path,
        old_prefix: &str,
        new_prefix: &str,
        plan: &mut Vec<(Guid, PathBuf, PathBuf, String)>,
    ) {
        let Some(node) = self.graph.get(id) else {
            return;
        };
        let new_path = rewrite_path(&node.meta.path, old_prefix, new_prefix)
            .unwrap_or_else(|| node.meta.path.clone());
        plan.push((id, node.file.clone(), new_file.to_path_buf(), new_path));
        for child in self.graph.children(id) {
            if let Some(child_node) = self.graph.get(child) {
                let child_file = self
                    .graph
                    .format()
                    .child_file_path(new_file, &child_node.meta.name);
                self.plan_subtree(child, &child_file, old_prefix, new_prefix, plan);
            }
        }
    }

    /// The shared mutate response:
    /// `{"ok":true,"changedFiles":[..],"selfCheck":"ok","item":ItemDetail}`.
    fn mutate_result(&self, id: Guid, changed: Vec<String>) -> Result<Value, KernelError> {
        let node = self
            .graph
            .get(id)
            .ok_or_else(|| KernelError::Io(format!("item {id} vanished after write")))?;
        Ok(json!({
            "ok": true,
            "changedFiles": changed,
            "selfCheck": "ok",
            "item": item_detail(&self.root, &self.graph, &self.templates, node),
        }))
    }
}

// ---- config -------------------------------------------------------------------

#[derive(Debug, Default, serde::Deserialize)]
struct ConfigFile {
    #[serde(default)]
    gates: GatesSection,
}

#[derive(Debug, Default, serde::Deserialize)]
struct GatesSection {
    #[serde(default)]
    disabled: Vec<String>,
    #[serde(default, rename = "language-policy")]
    language_policy: Option<LanguagePolicy>,
}

#[derive(Debug, serde::Deserialize)]
struct LanguagePolicy {
    required: Vec<String>,
    #[serde(default)]
    paths: Option<Vec<String>>,
}

/// Loads `<root>/treesmith.toml`; an absent file means the defaults.
fn load_gate_config(root: &Path) -> Result<GateConfig, KernelError> {
    let path = root.join("treesmith.toml");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(GateConfig::default()),
        Err(e) => return Err(KernelError::Io(format!("read {}: {e}", path.display()))),
    };
    let file: ConfigFile =
        toml::from_str(&text).map_err(|e| KernelError::Usage(format!("treesmith.toml: {e}")))?;
    let mut config = GateConfig {
        disabled: file
            .gates
            .disabled
            .iter()
            .map(|g| g.trim().to_ascii_uppercase())
            .collect(),
        ..GateConfig::default()
    };
    if let Some(policy) = file.gates.language_policy {
        config.required_languages = Some(policy.required);
        if let Some(paths) = policy.paths {
            config.language_paths = paths;
        }
    }
    Ok(config)
}

// ---- move helpers ---------------------------------------------------------------

fn scalar_entry(key: &str, scalar: Scalar) -> Entry {
    Entry {
        key: key.to_string(),
        value: YamlValue::Scalar(scalar),
    }
}

/// New-item / new-child name: one non-empty path segment.
fn validate_name(name: &str) -> Result<(), KernelError> {
    if name.trim().is_empty() || name.contains('/') || name.contains('\\') {
        return Err(KernelError::Usage(format!(
            "invalid item name `{name}`: must be a single non-empty path segment"
        )));
    }
    Ok(())
}

/// Case-insensitive: is `path` equal to or under `prefix`?
fn path_is_or_under(path: &str, prefix: &str) -> bool {
    let p = path.to_lowercase();
    let pre = prefix.to_lowercase();
    p == pre || p.starts_with(&format!("{pre}/"))
}

/// Rewrites a path equal to or under `old` onto `new` (case-insensitive
/// prefix match, suffix preserved verbatim). `None` when out of scope.
fn rewrite_path(value: &str, old: &str, new: &str) -> Option<String> {
    let v = value.to_lowercase();
    let o = old.to_lowercase();
    if v == o {
        Some(new.to_string())
    } else if v.starts_with(&format!("{o}/")) {
        Some(format!("{new}{}", &value[old.len()..]))
    } else {
        None
    }
}

/// Rewrites `ds="..."` / `ds='...'` attribute values inside a layout XML
/// string when they are equal to or under `old`. String-level so the
/// surrounding XML round-trips byte-identically. `None` when unchanged.
fn rewrite_ds_attrs(value: &str, old: &str, new: &str) -> Option<String> {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    let mut changed = false;
    while let Some(pos) = rest.find("ds=") {
        // Attribute boundary: start of text or a non-name char before `ds=`.
        let boundary = pos == 0
            || !rest[..pos]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ':');
        let after = &rest[pos + 3..];
        let quote = after.chars().next();
        if !boundary || !matches!(quote, Some('"') | Some('\'')) {
            out.push_str(&rest[..pos + 3]);
            rest = after;
            continue;
        }
        let quote = quote.expect("matched above");
        let inner = &after[1..];
        let Some(end) = inner.find(quote) else {
            break; // unterminated attribute: leave the tail verbatim
        };
        let ds_value = &inner[..end];
        out.push_str(&rest[..pos + 3]);
        out.push(quote);
        match rewrite_path(ds_value, old, new) {
            Some(rewritten) => {
                out.push_str(&rewritten);
                changed = true;
            }
            None => out.push_str(ds_value),
        }
        out.push(quote);
        rest = &inner[end + 1..];
    }
    out.push_str(rest);
    changed.then_some(out)
}

/// Rewrites path-valued fields and path-form `ds=` datasources on one
/// item. Returns whether anything changed.
fn rewrite_path_references(item: &mut ParsedItem, old: &str, new: &str) -> bool {
    let mut edits: Vec<(FieldSlot, Guid, String)> = Vec::new();
    let mut collect = |slot: FieldSlot, fields: &[treesmith_format::FieldRef]| {
        for f in fields {
            // Whole-value path (path-valued reference fields), or a layout
            // value containing path-form datasources.
            let rewritten = if f.id != treesmith_types::wellknown::LAYOUT_FIELD
                && f.id != treesmith_types::wellknown::FINAL_LAYOUT_FIELD
                && f.value.starts_with('/')
            {
                rewrite_path(&f.value, old, new)
            } else if f.value.contains("ds=") {
                rewrite_ds_attrs(&f.value, old, new)
            } else {
                None
            };
            if let Some(v) = rewritten {
                edits.push((slot.clone(), f.id, v));
            }
        }
    };
    collect(FieldSlot::Shared, &item.shared_fields());
    for lang in item.languages() {
        collect(
            FieldSlot::Unversioned {
                language: lang.language.clone(),
            },
            &lang.unversioned,
        );
        for (version, fields) in &lang.versions {
            collect(
                FieldSlot::Versioned {
                    language: lang.language.clone(),
                    version: *version,
                },
                fields,
            );
        }
    }
    let changed = !edits.is_empty();
    for (slot, id, value) in edits {
        item.set_field(&slot, id, None, None, &value);
    }
    changed
}

/// Removes now-empty directories bottom-up, stopping at the repo root or
/// the first non-empty directory.
fn remove_empty_dirs(mut dir: Option<&Path>, root: &Path) {
    while let Some(d) = dir {
        if d == root || std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}
