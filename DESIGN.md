# treesmith — Engineering Design (binding contracts for the v0 build)

This document turns `treesmith-architecture-spec.md` (the product spec) into concrete engineering
contracts. **Builder agents: public APIs, JSON shapes, and codec rules defined here are binding** —
downstream crates are built against them. If you must deviate, keep the deviation minimal, make the
consuming side obvious, and report it in your final summary. Code that already exists in `crates/`
is truth for anything this document leaves open.

Spec invariants referenced throughout: I1 (working tree is truth), I2 (byte-identical round-trip),
I3 (schema-aware writes), I4 (agent surface first), I5 (deterministic gates), I6 (formats behind a
trait), I7 (one static binary), I8 (standalone IP).

---

## 1. Ground rules

- Rust edition 2021, toolchain 1.93 installed. Workspace root `D:\github\treesmith` (in Git Bash: `/d/github/treesmith`).
- Already written by the orchestrator: root `Cargo.toml` (workspace + root `treesmith` binary package),
  `src/main.rs`, `.gitignore`, `.gitattributes`. Do not restructure these; the scaffold phase adds the rest.
- Dependency direction (spec §2 rule 4):
  `types ← format ← graph ← template ← presentation ← gate ← kernel ← {cli, mcp} ← root binary`.
  No cycles. `cli` and `mcp` never import each other; the root binary bridges them via
  `CliOutcome::LaunchMcp`.
- **Structure amendment (documented deviation):** spec §2 lists eight crates; we add
  `crates/treesmith-kernel`. It realizes the "query / mutation API" node that spec §3.1's diagram
  places between template/presentation and the surfaces, so that `cli` and `mcp` stay thin (spec §2
  rule 3) without importing each other. Record this in the README.
- Determinism: no wall clock, network, or randomness anywhere in parse/resolve/gate paths (I5).
  Exceptions: `census` reports elapsed wall time (it is a benchmark, not a gate); `forge` generates
  a random v4 GUID unless `--id` is supplied.
- Every crate: `license = "MIT OR Apache-2.0"` (root binary package is `Apache-2.0`),
  `version/edition/rust-version/repository` inherited via `.workspace = true`.
- Definition of done per crate: `cargo fmt -p <crate>` applied, `cargo clippy -p <crate>
  --all-targets -- -D warnings` clean, `cargo test -p <crate>` green, plus whatever the phase task
  names. Windows: prefer the Bash tool; cargo/git are on PATH.

### Crate dependency lists (manifests written in the scaffold phase)

| crate | internal deps | external deps |
|---|---|---|
| treesmith-types | — | uuid (v4), serde, thiserror |
| treesmith-format | types | serde, thiserror, walkdir |
| treesmith-graph | types, format | serde, thiserror, walkdir, rayon |
| treesmith-template | types, format, graph | serde, thiserror |
| treesmith-presentation | types, format, graph, template | serde, thiserror |
| treesmith-gate | types, format, graph, template, presentation | serde, serde_json, thiserror |
| treesmith-kernel | all of the above | serde, serde_json, toml, thiserror |
| treesmith-cli | kernel | clap (derive), serde_json |
| treesmith-mcp | kernel | serde_json, notify, thiserror |

All external versions come from `[workspace.dependencies]` in the root manifest.

---

## 2. treesmith-types

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Guid(uuid::Uuid);

impl Guid {
    /// Accepts hyphenated, braced `{...}`, and 32-hex-digit forms, any case.
    pub fn parse(s: &str) -> Result<Guid, GuidError>;
    /// Rainbow file form: lowercase hyphenated, no braces. Also the `Display` impl.
    pub fn rainbow(&self) -> String;
    /// Sitecore field-value form: `{UPPERCASE-HYPHENATED}`.
    pub fn braced_upper(&self) -> String;
    pub fn new_random() -> Guid; // uuid v4 — used only by forge
    /// Splits a raw field value on `|`, `\r`, `\n`; trims; skips empties.
    /// Returns (parsed guids, invalid tokens).
    pub fn parse_list(s: &str) -> (Vec<Guid>, Vec<String>);
}
// serde: Serialize as rainbow() string; Deserialize via parse().
```

`SectionKind { Shared, Unversioned, Versioned }` with `as_str(&self) -> &'static str`
(`"shared" | "unversioned" | "versioned"`), Serialize as that string.

`pub mod wellknown` — platform GUID vocabulary (see §12). Interpretation of I6 (record in README):
I6 quarantines *serialization dialect* knowledge in `treesmith-format`; well-known platform GUIDs
are the domain vocabulary the whole kernel resolves against and live in `types::wellknown` with
neutral constant names.

---

## 3. treesmith-format

Modules: `yaml` (codec), `item` (typed view + mutations), `rainbow`, `scs`, `census`, `valuefmt`.

### 3.1 Codec model (`yaml`)

Lexical preservation is the I2 strategy: the parser records every presentation detail the emitter
needs, so `emit(parse(bytes)) == bytes` holds **by construction** for any file that parses. Files
outside the subset are `ParseFault`s (exit-3 class; census failure classes), never silently skipped
(spec §3.4).

```rust
pub enum Newline { Lf, CrLf }

pub struct YamlDocument {
    pub bom: bool,               // leading UTF-8 BOM present
    pub newline: Newline,        // detected from first newline; mixed => ParseFault
    pub trailing_newline: bool,  // source ends with a newline
    pub root: Vec<Entry>,
}
pub struct Entry { pub key: String, pub value: Value }
pub enum Value { Scalar(Scalar), Map(Vec<Entry>), List(Vec<Vec<Entry>>) }
pub enum Scalar {
    Plain(String),   // `K: txt` — txt is everything after ": ", verbatim (may be "")
    PlainBare,       // `K:` — no trailing space, empty value
    Quoted(String),  // `K: "txt"` — txt is the inner text verbatim, no escape processing
    Block { header: String, lines: Vec<String> }, // `K: |` — header verbatim (`|`, `|-`, ...), lines stored without base indent
}
impl Scalar { pub fn value(&self) -> String } // Block => lines.join("\n"); others => text

pub struct ParseFault { pub kind: FaultKind, pub line: usize, pub message: String }
pub enum FaultKind { Utf8, MissingDocMarker, TabIndent, BadIndent, UnexpectedBlank,
                     BadListItem, BadStructure, MixedNewlines }

pub fn parse(bytes: &[u8]) -> Result<YamlDocument, ParseFault>;
pub fn emit(doc: &YamlDocument) -> Vec<u8>;
pub fn scalar_for_new_value(v: &str) -> Scalar; // write-rule table below
```

**Parse rules** (the Rainbow writer's output grammar; it is *not* general YAML — e.g. plain values
run verbatim to end of line, so `Value: a: b` is legal):

1. Strip optional UTF-8 BOM (record). Detect newline style from the first newline; a `CrLf` file
   containing a bare `\n`, or an `Lf` file containing any `\r`, is `MixedNewlines`.
2. First line must be exactly `---` (`MissingDocMarker` otherwise). One document per file.
3. Indentation is exactly 2 spaces per level. Tabs in indentation → `TabIndent`. An indent that is
   not a valid level for its context → `BadIndent`.
4. Map entry at indent `I`: `key: rest` or `key:` or `key: ` (key = chars up to first `": "` or a
   trailing `:`). Value forms:
   - `rest` starts with `"` and ends with `"` (len ≥ 2) → `Quoted(inner)`.
   - `rest` starts with `|` or `>` → `Block`: header = rest verbatim; content lines follow while
     blank or indent ≥ `I + 2`; each stored with `I + 2` spaces stripped (blank → `""`). A blank
     line is included only if a later content line at ≥ `I + 2` follows, or only blanks remain to
     EOF; otherwise → `UnexpectedBlank`.
   - other non-empty `rest` → `Plain(rest)` verbatim (including any trailing spaces).
   - empty remainder: look at the next line — same indent starting with `"- "` → `List`; indent
     `I + 2` with a `key:` → `Map`; otherwise `PlainBare` (`K:`) or `Plain("")` (`K: `).
5. List at indent `I`: items start `- ` at indent `I` (dash sits at the *key's* indent — Rainbow
   style). The text after `- ` is the item's first entry parsed at logical indent `I + 2`;
   subsequent entries of the item are lines at indent `I + 2`. A dash line without an inline
   `key: value` first entry → `BadListItem`.
6. Blank lines anywhere else → `UnexpectedBlank`. Unknown keys are ordinary entries — preserved
   in order, round-tripped verbatim (spec §3.4 tolerant-read).

**Emit rules:** exact inverse. Lines joined with `doc.newline`; final newline iff
`trailing_newline`; BOM iff `bom`. `Plain(t)` → `K: {t}` (note `Plain("")` → `K: ` with trailing
space, `PlainBare` → `K:`); `Quoted(t)` → `K: "{t}"`; `Block` → `K: {header}` then each line at
base indent (blank lines emitted empty, no padding); `Map`/`List` per the indent rules above.

**Write-rule table** — `scalar_for_new_value` decides the style for *new or changed* values only
(existing scalars keep their parsed style). Mirrors observed Rainbow writer output; each rule
tagged VERIFY-P0 must be checked against real client repos in the P0 census (spec Appendix).

1. contains `\n` → `Block { header: "|", lines: v.split('\n') }` (callers reject/normalize `\r` first)
2. contains `\\` or `"` → `Block` (Rainbow never escapes — e.g. `sitecore\admin` is a block literal)
3. empty → `Quoted("")` (VERIFY-P0)
4. first char is one of `{ [ ' & * # ? | > % @ \` -` or a space, or contains `: `, or ends with
   space or `:` → `Quoted` (VERIFY-P0; makes braced-GUID values come out as `"{...}"`, as observed)
5. otherwise → `Plain`

GUID-keyed entries (`ID`, `Parent`, `Template`, `BranchID`, `BlobID`) are always
`Quoted(guid.rainbow())`. `Language`, `DB`, `Hint`, `Type`, `Path`, `Version` are `Plain`.

### 3.2 Item view + mutations (`item`)

```rust
pub struct ParsedItem { pub doc: YamlDocument }

pub struct FieldRef { pub id: Guid, pub hint: Option<String>, pub type_hint: Option<String>,
                      pub blob_id: Option<Guid>, pub value: String }
pub struct LanguageBlock { pub language: String, pub unversioned: Vec<FieldRef>,
                           pub versions: Vec<(u32, Vec<FieldRef>)> }
#[derive(Clone, Debug, PartialEq)]
pub enum FieldSlot { Shared, Unversioned { language: String },
                     Versioned { language: String, version: u32 } }

impl ParsedItem {
    pub fn id(&self) -> Option<Guid>;
    pub fn parent_id(&self) -> Option<Guid>;
    pub fn template_id(&self) -> Option<Guid>;
    pub fn path(&self) -> Option<String>;
    pub fn db(&self) -> Option<String>;
    pub fn shared_fields(&self) -> Vec<FieldRef>;
    pub fn languages(&self) -> Vec<LanguageBlock>;
    /// Deterministic first match: shared, then languages alphabetically
    /// (unversioned first, then versions ascending).
    pub fn find_field(&self, id: Guid) -> Option<(FieldSlot, FieldRef)>;
    pub fn max_version(&self, language: &str) -> Option<u32>;

    pub fn set_field(&mut self, slot: &FieldSlot, id: Guid, hint: Option<&str>,
                     type_hint: Option<&str>, value: &str);
    pub fn remove_field(&mut self, slot: &FieldSlot, id: Guid) -> bool;
    pub fn ensure_version(&mut self, language: &str, version: u32);
    pub fn set_path(&mut self, path: &str);
    pub fn set_parent(&mut self, parent: Guid);
}
```

Rainbow item schema (canonical key orders — used when *inserting*; existing order is preserved):

- Top level: `ID`, `Parent`, `Template`, `Path`, `DB`, `BranchID`, `SharedFields`, `Languages`.
- Language item: `Language`, `Fields`, `Versions`.
- Version item: `Version`, `Fields`.
- Field item: `ID`, `Hint`, `Type`, `BlobID`, `Value`.

Insert sorting (VERIFY-P0, mirrors Rainbow's deterministic output): fields sorted by GUID string
ascending (insert before the first existing field with a greater id); languages alphabetically;
versions numerically. `set_field` on an existing field replaces its `Value` scalar (style via
`scalar_for_new_value`) and updates `Type` if a `type_hint` is supplied. Lists/sections that
become empty on `remove_field` are removed entirely (Rainbow omits empty sections).

### 3.3 Format trait + registry (`lib.rs`, `rainbow`, `scs`)

```rust
pub trait SerializationFormat: Send + Sync {
    fn key(&self) -> &'static str;                       // "rainbow" | "scs"
    fn sniff_file_name(&self, name: &str) -> bool;       // *.yml
    fn sniff_head(&self, head: &[u8]) -> bool;           // BOM? + "---" line + next line "ID: "
    fn parse(&self, bytes: &[u8]) -> Result<ParsedItem, ParseFault>;
    fn emit(&self, item: &ParsedItem) -> Vec<u8>;
    /// Physical convention shared by Unicorn and SCS: children live in a folder
    /// named after the parent file stem: `.../Home.yml` -> `.../Home/<child>.yml`.
    fn child_file_path(&self, parent_file: &Path, child_name: &str) -> PathBuf;
}
pub fn detect(root: &Path) -> &'static dyn SerializationFormat; // any *.module.json under root => scs, else rainbow
pub fn by_key(key: &str) -> Option<&'static dyn SerializationFormat>;
```

SCS item files use the same codec as Rainbow (VERIFY-P0); the two implementations differ only in
`key()` and, later, discovery nuances. Nothing outside this crate names Sitecore/Unicorn/Rainbow/SCS (I6).

### 3.4 Census (`census`) — the P0 fidelity harness

```rust
pub struct Census { pub files: usize, pub items: usize, pub ok: usize,
                    pub faults: Vec<CensusFault>,        // { file, kind, line, message }
                    pub mismatches: Vec<CensusMismatch>, // { file, first_diff_line, expected, actual }
                    pub elapsed_ms: u64 }
pub fn round_trip_census(root: &Path, fmt: &dyn SerializationFormat) -> Census;
```

Walks `root` (skip `.git`, `target`, `node_modules`, `bin`, `obj`), sniffs `*.yml`, parses, emits,
byte-compares. Deterministic ordering (sort by path) apart from `elapsed_ms`.

### 3.5 Field formatter table (`valuefmt`)

Rainbow field formatters change how values are *stored* in YAML vs the raw platform value, and
stamp `Type:` on the field so the transform is reversible (VERIFY-P0):

- Multilist family — `Checklist`, `Multilist`, `Multilist with Search`, `Treelist`, `TreelistEx`,
  `tree list`: stored one braced-uppercase GUID per line (block literal). Raw form is `|`-separated.
- XML family — `Layout`, `Tracking`, `Rules`: stored as-is (Rainbow pretty-prints; we do not
  re-pretty-print on write — diff churn only, still valid).

`pub fn is_multilist_type(t: &str) -> bool`, `pub fn is_xml_type(t: &str) -> bool`
(case-insensitive), `pub fn normalize_guid_list(raw: &str) -> Result<String, String>` (accept `|`
or newline separated, any GUID form; emit braced-upper newline-joined; Err names the bad token).
Writers emit `Type: <field type>` for these families.

---

## 4. treesmith-graph

```rust
pub struct Graph { /* root, format, items, indexes, faults, repo_files */ }
pub struct ItemNode { pub id: Guid, pub file: PathBuf, pub item: ParsedItem, pub meta: ItemMeta }
pub struct ItemMeta { pub id: Guid, pub parent: Option<Guid>, pub template: Option<Guid>,
                      pub path: String, pub name: String, pub db: Option<String>,
                      pub languages: Vec<(String, Vec<u32>)> }
pub struct TreeFault { pub file: PathBuf, pub kind: String, pub message: String }
                      // kinds: "parse" | "missing-id" | "duplicate-id"

impl Graph {
    pub fn build(root: &Path) -> Graph;          // format via treesmith_format::detect
    pub fn format(&self) -> &'static dyn SerializationFormat;
    pub fn root(&self) -> &Path;
    pub fn rebuild(&mut self);
    pub fn refresh_paths(&mut self, paths: &[PathBuf]); // re-parse changed, drop deleted
    pub fn get(&self, id: Guid) -> Option<&ItemNode>;
    pub fn find_path(&self, path: &str) -> Vec<Guid>;   // case-insensitive; may be multiple
    pub fn children(&self, id: Guid) -> Vec<Guid>;      // sorted by (name, id)
    pub fn by_template(&self, template: Guid) -> Vec<Guid>; // sorted by path
    pub fn ids_by_path(&self) -> Vec<Guid>;             // all items, sorted by (path, id)
    pub fn faults(&self) -> &[TreeFault];               // sorted by file
    pub fn repo_files(&self) -> &RepoFiles;
    pub fn file_of(&self, id: Guid) -> Option<&Path>;
}

pub struct RepoFiles { pub all: Vec<String> } // forward-slash repo-relative paths, sorted
impl RepoFiles {
    /// Case-insensitive suffix match after normalizing `\` -> `/` and trimming leading `/` or `~/`.
    pub fn find_suffix(&self, virtual_path: &str) -> Vec<&str>;
    pub fn with_extension(&self, ext: &str) -> Vec<&str>;
}
```

Build: walk (same exclusions as census), sniff, parse in parallel with rayon, assemble
deterministically (files sorted by path; duplicate GUID keeps the lexically-first file and records
`duplicate-id`). Parse faults are recorded, never dropped (§3.4); the kernel refuses non-census
ops on a faulted tree (exit-3 class). Everything derived and rebuildable (I1).

Query engine (`query` module):

```rust
pub struct Query { /* terms */ }
pub fn parse_query(expr: &str) -> Result<Query, String>;
impl Query { pub fn matches(&self, graph: &Graph, node: &ItemNode) -> bool }
pub fn glob_match(pattern: &str, text: &str) -> bool;
```

Terms are whitespace-separated `key:value` (value may be `"quoted"` to include spaces); all terms
must match (AND). Keys: `path:` (glob, case-insensitive; `**` crosses `/`, `*` within a segment,
`?` one non-`/` char), `name:` (glob on last path segment), `template:` (GUID in any form, or exact
template-item name, case-insensitive), `field:` (`field:Name=Value` exact value in any slot, or
`field:Name` existence; Name matches field hint case-insensitively or field GUID). Bare terms are a
usage error. Results are always ordered by (path, id).

---

## 5. treesmith-template

Semantic core (spec §4). Template items are items whose `Template` GUID equals
`wellknown::TEMPLATE_TEMPLATE`; sections are their children with `TEMPLATE_SECTION`; field
definitions are section children with `TEMPLATE_FIELD`.

```rust
pub struct TemplateIndex { /* defs by id, name index */ }
pub struct TemplateDef { pub id: Guid, pub name: String, pub path: String, pub bases: Vec<Guid>,
                         pub fields: Vec<FieldDef>, pub standard_values: Option<Guid> }
pub struct FieldDef { pub id: Guid, pub name: String, pub field_type: String,
                      pub section: SectionKind, pub section_name: String }
pub struct EffectiveTemplate { pub id: Guid, pub name: String, pub chain: Vec<Guid>,
                               pub unresolved_bases: Vec<Guid>, pub fields: Vec<EffectiveField> }
pub struct EffectiveField { pub id: Guid, pub name: String, pub field_type: String,
                            pub section: SectionKind, pub section_name: String, pub defined_by: Guid }

impl TemplateIndex {
    pub fn build(graph: &Graph) -> TemplateIndex;
    pub fn get(&self, id: Guid) -> Option<&TemplateDef>;
    pub fn find_by_name(&self, name: &str) -> Vec<Guid>;           // case-insensitive
    pub fn resolve(&self, id: Guid) -> Option<EffectiveTemplate>;  // None if id unknown
    /// Standard-values item ids along the chain, derived-first (self, then bases in chain order).
    pub fn std_values_chain(&self, template: Guid) -> Vec<Guid>;
}
impl EffectiveTemplate {
    pub fn field_by_id(&self, id: Guid) -> Option<&EffectiveField>;
    pub fn field_by_name(&self, name: &str) -> Option<&EffectiveField>; // case-insensitive, first in chain order
}
pub fn validate_value(field_type: &str, value: &str) -> Result<(), String>; // shared by kernel + G6
```

- Field-definition values read with deterministic precedence: shared → unversioned (languages
  alphabetical) → versioned (languages alphabetical, highest version).
- `section`: Shared if the `Shared` checkbox field is `"1"`, else Unversioned if `Unversioned` is
  `"1"`, else Versioned.
- Base chain: DFS, self first, bases in listed order, dedup keep-first, cycle-guarded; unknown base
  GUIDs land in `unresolved_bases`. Effective fields: walk the chain, first definition of a field
  ID wins; `field_by_name` returns the first by chain order.
- `standard_values`: the `STANDARD_VALUES` field on the template item, else a child named
  `__Standard Values`.
- `validate_value` (used for I3 write validation and G6): `Checkbox` → `"1"` or `""`;
  `Integer`/`Number` → optional `-` + digits or empty; `Date`/`Datetime` → empty or
  `^\d{8}T\d{6}Z?$`; multilist family → every token a GUID; `Droplink`/`Droptree`/`Reference`/
  `Grouped Droplink` → empty or a single GUID; anything else → accepted.

---

## 6. treesmith-presentation

Modules: `layoutxml` (mini parser), `delta` (merge), `codemap`, `lib` (resolve).

### 6.1 Layout XML subset

Hand-rolled parser (no XML dependency): elements, attributes (single/double quoted), self-closing
tags, ignorable text/whitespace, XML declaration and comments skipped, entities `&lt; &gt; &amp;
&quot; &apos; &#N; &#xN;` decoded in attribute values.

```rust
pub struct XmlEl { pub name: String, pub attrs: Vec<(String, String)>, pub children: Vec<XmlEl> }
impl XmlEl { pub fn attr(&self, name: &str) -> Option<&str> }
pub fn parse_xml(s: &str) -> Result<XmlEl, XmlError>; // XmlError { message, offset }
```

### 6.2 Delta merge (`apply_delta`) — Sitecore final-renderings semantics, T3 posture

```rust
pub fn apply_delta(base: &XmlEl, delta: &XmlEl) -> (XmlEl, Vec<DeltaNote>);
pub enum DeltaNote { UnknownUid { device: String, uid: String },
                     BadPositionRef { device: String, expr: String },
                     DeviceWithoutLayout { device: String } }
```

Deterministic rules (document verbatim in code):

1. Result devices = base devices in order. For each delta `<d id=...>`:
   - No base device with that id → append the delta device as given (note `DeviceWithoutLayout`
     if it lacks `l=`).
   - **Replace mode** if the delta device has `l=`, no `p:*` attribute anywhere within it, and
     every `<r>` in it has both `id=` and `ph=` → the delta device replaces the base device wholesale.
   - **Patch mode** otherwise: overlay non-`p:` device attributes; for each delta `<r>`:
     matching base `uid` → overlay its non-`p:` attributes; no match but has `id=` → insert
     (position from `p:before` / `p:after` with selector `r[@uid='{UID}']`; unparseable or
     unknown selector → `BadPositionRef`, append); no match and no `id=` → `UnknownUid`,
     skipped. Base renderings absent from the delta are kept.
2. Notes surface through `resolve` and gate G2; nothing panics on weird deltas (T3: report, don't crash).

### 6.3 Resolution

```rust
pub fn resolve(graph: &Graph, templates: &TemplateIndex, item: Guid,
               language: Option<&str>, version: Option<u32>)
               -> Result<ResolvedPresentation, PresentationError>;
```

Layout stacking, base-most first (each layer applied with `apply_delta`):
standard-values items along `std_values_chain` in *reverse* (base-most template's std values first)
contribute their shared `LAYOUT_FIELD` values; then the item's own shared `LAYOUT_FIELD`; then
final-renderings (`FINAL_LAYOUT_FIELD`) of std-values items (requested language, highest version ≤
requested), then the item's own final renderings for the language/version. Defaults: language =
first language with versions on the item, else `en`; version = max for that language.

Output model (all `Serialize`, camelCase JSON):

```text
ResolvedPresentation { itemId, itemPath, language, version,
                       devices: [ { deviceId, layout: {id, path?}|null, layoutCodeFiles: [..],
                                    renderings: [ResolvedRendering], notes: [..] } ] }
ResolvedRendering    { uid?, renderingId?, renderingName?, placeholder, placeholderLeaf,
                       datasource: DatasourceResolution, parameters: {k: v},
                       codeFiles: [..], source: "shared"|"final" }
DatasourceResolution — tagged: {"kind":"contextItem"} | {"kind":"item","raw","id","path"}
                     | {"kind":"missing","raw"} | {"kind":"dynamic","raw","scheme"}  // query:/code:/...
```

Datasource resolution: empty → contextItem; GUID form → graph lookup; `local:X` → page path + `X`;
`/sitecore/...` path → `find_path`; `scheme:rest` → dynamic; anything else unresolvable → missing.
`placeholderLeaf` = segment after the last `/`, with a dynamic-placeholder suffix
(`-{36-hex-guid}-<digits>`, braces optional) stripped. Parameters: split `&` then `=`, `%XX`-decoded.

### 6.4 Code map + placeholder scan (`codemap`)

```rust
pub enum CodeKind { View, Controller, Layout }
pub struct CodeRef { pub kind: CodeKind, pub raw: String, pub files: Vec<String> }
pub fn rendering_code(graph: &Graph, templates: &TemplateIndex, node: &ItemNode) -> Option<CodeRef>;
pub struct PlaceholderScan { pub exposed: BTreeSet<String>, pub files_scanned: usize }
pub fn scan_placeholders(root: &Path, files: &RepoFiles) -> PlaceholderScan;
```

Classification: item template == `VIEW_RENDERING` / `LAYOUT` → field `Path` (`.cshtml` virtual
path) matched via `RepoFiles::find_suffix`; == `CONTROLLER_RENDERING` → field `Controller`, short
type name matched against `class <Name>` in `.cs` files (line scan, no regex dep needed if simple
`contains` logic is exact about word boundaries). Field lookup precedence everywhere: resolved
template field by name, else serialized-field hint match (real repos rarely serialize system
templates — hints are the robust path). Placeholder scan: every `.cshtml`, patterns
`.Placeholder("NAME")` and `DynamicPlaceholder("NAME")` (case-insensitive method match, first
string argument); collect NAME set.

---

## 7. treesmith-gate

```rust
pub enum Severity { Error, Warning, Info } // serialize lowercase
pub struct Finding { pub gate: &'static str, pub code: String, pub severity: Severity,
                     pub item: Option<Guid>, pub item_path: Option<String>, pub file: Option<String>,
                     pub message: String, pub details: serde_json::Value }
pub struct GateConfig { pub disabled: BTreeSet<String>,             // gate keys "G1".."G7"
                        pub required_languages: Option<Vec<String>>,
                        pub language_paths: Vec<String> }           // default ["/sitecore/content"]
pub struct GateCtx<'a> { pub graph: &'a Graph, pub templates: &'a TemplateIndex,
                         pub placeholders: &'a PlaceholderScan, pub config: &'a GateConfig }
pub struct GateReport { pub findings: Vec<Finding>, pub skipped: Vec<(String, String)> }
pub fn run_all(ctx: &GateCtx) -> GateReport;
pub fn run_some(ctx: &GateCtx, gates: &[String]) -> Result<GateReport, String>; // unknown gate name -> Err (usage)
pub const GATES: &[&str] = &["G1","G2","G3","G4","G5","G6","G7"];
```

Findings sorted by `(gate, item_path, code, message)`. Identical tree → identical report (I5).
Reason codes (`code` values) per gate:

| Gate | codes (severity) |
|---|---|
| G1 datasources | `g1.missing-datasource` (error), `g1.dynamic-datasource` (info) |
| G2 layout XML | `g2.malformed-xml` (error), `g2.unknown-uid` (error), `g2.bad-position-ref` (error), `g2.device-without-layout` (warning) |
| G3 code files | `g3.missing-view` (error), `g3.missing-controller` (warning), `g3.empty-path` (warning) |
| G4 placeholders | `g4.placeholder-not-exposed` (warning) |
| G5 field refs | `g5.broken-reference` (error), `g5.invalid-guid-token` (error) |
| G6 conformance | `g6.unknown-field` (error), `g6.wrong-section` (error), `g6.duplicate-field` (error), `g6.invalid-value` (error), `g6.unresolved-template` (warning), `g6.unresolved-base` (warning) |
| G7 languages | `g7.missing-language` (error) |

Notes: G1/G2/G4 evaluate every layout value in the stack (shared + final, all languages/versions)
via presentation; `query:`/`code:` datasources are `g1.dynamic-datasource` info, not failures.
G3/G4 severity split reflects static-analysis confidence. G5 checks fields whose resolved type is
in the reference family (multilist family + droplink/droptree/reference/grouped droplink) plus
General Link `id=` attributes when `linktype="internal"`. G6 runs `validate_value` plus
section-placement and unknown-field checks against the effective template. G7 is skipped with
reason `no language policy configured` when `required_languages` is `None`; otherwise items under
`language_paths` (case-insensitive prefix) with ≥ 1 version in any language must have ≥ 1 version
in every required language.

Config file (loaded by the kernel from `<root>/treesmith.toml`, absent = defaults):

```toml
[gates]
disabled = []                      # e.g. ["G4"]
[gates.language-policy]
required = ["en"]                  # presence enables G7
paths = ["/sitecore/content"]
```

---

## 8. treesmith-kernel

The query/mutation API both surfaces call (spec §3.1's "API" node).

```rust
pub struct Workspace { /* root, graph, templates, gate config */ }
impl Workspace {
    pub fn open(root: &Path) -> Result<Workspace, KernelError>;
    pub fn refresh_paths(&mut self, paths: &[PathBuf]); // rebuilds template index too
    pub fn rebuild(&mut self);

    // Every op returns the exact serde_json::Value the surfaces print/return (I4: 1:1).
    pub fn query(&self, expr: &str) -> Result<serde_json::Value, KernelError>;
    pub fn get(&self, item: &str) -> Result<serde_json::Value, KernelError>;
    pub fn set_field(&mut self, req: &SetFieldRequest) -> Result<serde_json::Value, KernelError>;
    pub fn forge(&mut self, req: &ForgeRequest) -> Result<serde_json::Value, KernelError>;
    pub fn move_item(&mut self, req: &MoveRequest) -> Result<serde_json::Value, KernelError>;
    pub fn resolve_presentation(&self, item: &str, language: Option<&str>, version: Option<u32>)
        -> Result<serde_json::Value, KernelError>;
    pub fn validate(&self, gates: Option<&[String]>) -> Result<(serde_json::Value, bool), KernelError>; // bool = has errors
    pub fn census(root: &Path) -> serde_json::Value; // works on faulted trees by design
}
pub struct SetFieldRequest { pub item: String, pub field: String, pub value: String,
                             pub language: Option<String>, pub version: Option<u32>,
                             pub create_version: bool } // default true
pub struct ForgeRequest { pub template: String, pub parent: String, pub name: String,
                          pub id: Option<Guid>, pub language: Option<String> }
pub struct MoveRequest { pub item: String, pub new_parent: String, pub name: Option<String> }

pub enum KernelError { Usage(String), Validation { code: String, message: String, details: serde_json::Value },
                       TreeFault(Vec</*graph faults*/>), Io(String) }
impl KernelError { pub fn class(&self) -> &'static str;   // "usage"|"validation"|"tree-fault"|"io"
                   pub fn exit_code(&self) -> u8;         // 2 | 1 | 3 | 1
                   pub fn to_json(&self) -> serde_json::Value }
```

- Item designators: a GUID in any form, else a `/sitecore/...` path (ambiguous path → Usage error
  listing candidates; unknown → Usage).
- Tree-fault policy: `open` succeeds with faults recorded, but every op except `census` returns
  `TreeFault` if `graph.faults()` is non-empty — faults are never silently skipped (spec §3.4);
  scripts distinguish exit 3 from gate exits (spec §3.2).
- **Write path (spec §5), shared by set-field/forge/move:**
  1. Resolve the field through the effective template — field IDs never guessed from names (name →
     `field_by_name`, GUID accepted directly but must exist in the effective template unless it is
     a well-known system field).
  2. Slot from the *field definition's* section kind, never from where a value currently sits:
     Shared → reject `--language/--version`; Unversioned → language (default `en`); Versioned →
     language + version (default: max existing; none and `create_version` → create version 1).
  3. Validate the value (`validate_value`), normalize multilist input via `normalize_guid_list`,
     stamp `Type:` for formatter-covered types, layout fields must `parse_xml` cleanly. Rejections
     are `Validation` errors with machine-readable codes (I3/I5), e.g. `unknown-field`,
     `wrong-slot-for-section`, `invalid-value`, `malformed-layout-xml`, `blob-unsupported`.
  4. Self-check before any disk write (I3 operational): emit candidate bytes → re-parse →
     re-emit must equal candidate bytes; the mutated slot must read back exactly the requested
     value. Failure → `Validation { code: "self-check-failed" }` and nothing is written.
  5. Write files, then `refresh_paths` so the graph mirrors disk (I1).
- forge: parent must exist; new file at `format.child_file_path(parent_file, name)`; collision on
  path+name or target file → Validation. Minimal item (ID/Parent/Template/Path/DB inherited from
  parent). Template designator must resolve to a known template.
- move: item + subtree `Path` fields rewritten, files/dirs relocated via `child_file_path`
  convention; path-form datasources (`ds=`) and path-valued reference fields equal to or under the
  old path are rewritten graph-wide. Name collision under new parent → Validation.
- JSON shapes (camelCase; exactly these keys):

```text
query   {"ok":true,"count":N,"items":[ItemSummary]}
get     {"ok":true,"item":ItemDetail}
mutate  {"ok":true,"changedFiles":["rel/path.yml"],"selfCheck":"ok","item":ItemDetail}
validate{"ok":bool,"errors":N,"warnings":N,"infos":N,
         "findings":[{"gate","code","severity","itemId","itemPath","file","message","details"}],
         "skipped":[{"gate","reason"}]}
census  {"ok":bool,"files":N,"items":N,"roundTripOk":N,
         "faults":[{"file","kind","line","message"}],
         "mismatches":[{"file","firstDiffLine","expected","actual"}],"elapsedMs":N}
resolve-presentation: ResolvedPresentation wrapped as {"ok":true, ...fields...}
errors  {"ok":false,"error":{"class","code","message","details"}}

ItemSummary {"id","path","name","template":{"id","name"|null}|null,"db",
             "languages":[{"language","versions":[..]}],"file"}
ItemDetail  = ItemSummary + {"templateChain":[..],"sharedFields":[FieldOut],
             "languages":[{"language","unversioned":[FieldOut],
                           "versions":[{"version","fields":[FieldOut]}]}],
             "fieldsNotInTemplate":[{"id","hint","slot"}]}
FieldOut    {"id","name","type","section","value","definedBy"|null}
             // name: effective-template name, else hint, else id; type: template type, else Type: hint, else null
```

---

## 9. treesmith-cli

```rust
pub enum CliOutcome { Exit(u8), LaunchMcp { root: PathBuf } }
pub fn run() -> CliOutcome; // parses std::env::args itself
```

Verbs exactly per spec §3.2 (`query`, `get`, `set-field`, `forge`, `move`,
`resolve-presentation`, `validate [--gate NAME ...]`, `mcp`) plus `census` (the P0 harness).
Global flags: `--root DIR` (default `.`), `--json`.

Output contract (spec §3.2): JSON (pretty) on stdout when stdout is not a TTY
(`std::io::IsTerminal`) or `--json`; human-readable lines otherwise; diagnostics to stderr only.
Exit codes: 0 success · 1 gate/validation failure (validate with errors; rejected writes) · 2
usage (clap errors already exit 2; kernel Usage errors map to 2) · 3 tree unreadable (kernel
TreeFault; census with faults/mismatches also exits 3). Human mode for validate prints one line
per finding (`G1 error g1.missing-datasource /sitecore/... — message`) plus a summary line.
`mcp` verb → `CliOutcome::LaunchMcp` (root binary bridges; no cli→mcp dependency).

Integration tests live in the **root package** `tests/cli_integration.rs` using
`env!("CARGO_BIN_EXE_treesmith")` against `fixtures/` (copy fixture repos into a temp dir for
mutation tests; never mutate `fixtures/` in place).

---

## 10. treesmith-mcp

**O6 spike outcome (record in README):** hand-rolled JSON-RPC 2.0 over stdio instead of `rmcp` —
zero async runtime, no API-drift risk, protocol surface is 4 methods; `rmcp` remains a clean swap
later because this crate is the only MCP-aware code. Newline-delimited JSON-RPC on stdio:

- `initialize` → `{"protocolVersion": <echo client's if in {"2024-11-05","2025-03-26","2025-06-18"}, else "2025-06-18">,
  "capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"treesmith","version":env!("CARGO_PKG_VERSION")}}`
- `notifications/initialized` → ignored. `ping` → `{}`. Unknown method → error `-32601`;
  malformed JSON → `-32700`; notifications (no `id`) never get responses. stdin EOF → clean exit.
- `tools/list` → tools mirroring CLI verbs 1:1 (spec §3.3), names:
  `query`, `get`, `set_field`, `forge`, `move`, `resolve_presentation`, `validate`, `census`.
  Input schemas (JSON Schema, camelCase properties): query `{expr}`; get `{item}`; set_field
  `{item,field,value,language?,version?,createVersion?}`; forge `{template,parent,name,id?,language?}`;
  move `{item,newParent,name?}`; resolve_presentation `{item,language?,version?}`; validate
  `{gates?:[string]}`; census `{}`.
- `tools/call` → `{"content":[{"type":"text","text": <the same JSON string the CLI prints>}],
  "isError": <true for kernel errors and validate-with-errors>}`. Kernel errors return their
  `to_json()` payload as the text — machine-readable, never a protocol error.

`pub fn serve(root: &Path) -> Result<(), Box<dyn std::error::Error>>` — owns
`Workspace` behind a `Mutex`, plus a `notify` watcher thread collecting dirty paths into a shared
set; each `tools/call` drains the dirty set and `refresh_paths` first (warm graph, spec §3.3). If
the watcher fails to start: log to stderr once, rebuild the workspace before every call instead.
Integration test in root package `tests/mcp_integration.rs`: spawn the binary, drive
initialize → tools/list → tools/call (query + validate) over pipes, assert shapes.

---

## 11. Root binary

Already written (`src/main.rs`): dispatches `CliOutcome`. Root package also hosts the integration
tests (`tests/`) since it can see `CARGO_BIN_EXE_treesmith`.

---

## 12. Well-known GUIDs (`types::wellknown`)

Canonical lowercase-hyphenated. Constants are `Guid`s built via a `const`-friendly helper or
`Guid::parse` in a `once`/`LazyLock` table — implementer's choice, but expose plain
`pub fn`/`pub static` accessors with these names:

| const | guid | meaning |
|---|---|---|
| TEMPLATE_TEMPLATE | ab86861a-6030-46c5-b394-e8f99e8b87db | template definition item |
| TEMPLATE_SECTION | e269fbb5-3750-427a-9149-7aa950b49301 | template section |
| TEMPLATE_FIELD | 455a3e98-a627-4b40-8035-e683a0331ac7 | template field |
| TEMPLATE_FOLDER | 0437fee2-44c9-46a6-abe9-28858d9fee8c | template folder |
| STANDARD_TEMPLATE | 1930bbeb-7805-471a-a3be-4858ac7cf696 | standard template |
| FOLDER | a87a00b1-e6db-45ab-8b54-636fec3b5523 | common folder |
| BASE_TEMPLATE_FIELD | 12c33f3f-86c5-43a5-aeb4-5598cec45116 | __Base template |
| STANDARD_VALUES_FIELD | f7d48a55-2158-4f02-9356-756654404f73 | template's std-values pointer |
| FIELD_TYPE_FIELD | ab162cc0-dc80-4abf-8871-998ee5d7ba32 | `Type` on a template field |
| FIELD_SHARED_FIELD | be351a73-fcb0-4213-93fa-c302d8ab4f51 | `Shared` checkbox |
| FIELD_UNVERSIONED_FIELD | 39847666-389d-409b-95bd-f2016f11eed5 | `Unversioned` checkbox |
| LAYOUT_FIELD | f1a1fe9e-a60c-4ddb-a3a0-bb5b29fe732e | __Renderings (shared) |
| FINAL_LAYOUT_FIELD | 04bf00db-f5fb-41f7-8ab7-22408372a981 | __Final Renderings (versioned) |
| DISPLAY_NAME_FIELD | b5e02ad9-d56f-4c41-a065-a133db87bdeb | __Display name |
| SORTORDER_FIELD | ba3f86a2-4a1c-4d78-b63d-91c2779c1b5e | __Sortorder |
| CREATED_FIELD | 25bed78c-4957-4165-998a-ca1b52f67497 | __Created |
| CREATED_BY_FIELD | 5dd74568-4d4b-44c1-b513-0af5f4cda34f | __Created by |
| VIEW_RENDERING | 99f8905d-4a87-4eb8-9f8b-a9bebfb3add6 | view rendering template |
| CONTROLLER_RENDERING | 2a3e91a0-7987-44b5-ab34-35c2d9de83b9 | controller rendering template |
| LAYOUT | 3a45a723-64ee-4919-9d41-02fd40fd1466 | layout template |
| PLACEHOLDER_SETTINGS | 5c547d4e-7111-4995-95b0-6b561751bf2e | placeholder settings template |
| DEFAULT_DEVICE | fe5d7fdf-89c0-4d99-9aa3-b5fbd009c9f3 | Default device |
| LAYOUT_PATH_FIELD | 07aa88dc-3b4b-4e85-91f2-a4cc5261c6d4 | `Path` on layout (VERIFY-P0) |

Code must not *depend* on VERIFY-P0 ids alone — name/hint fallback is the primary resolution path
for `Path`/`Controller` (see §6.4).

---

## 13. Fixtures

`fixtures/` is the I2 corpus and the gate test bed. Everything must round-trip byte-identical
(LF newlines except where the corpus deliberately varies; `.gitattributes` already pins
`fixtures/** -text`). Layout:

```text
fixtures/
├── corpus/                     # codec edge cases, authored in the Format phase
│   (quoted values, braced-guid value, backslash block literal, empty-quoted, unknown keys,
│    Type-before-Hint order, plain value containing ": ", CRLF file, BOM file,
│    no-trailing-newline file, block literal with interior blank line, bare `Key:`)
├── rainbow/
│   ├── basic/                  # healthy mini repo — authored in the Fixtures phase
│   │   ├── serialization/...   # items (tree below)
│   │   └── src/                # Views/*.cshtml + Controllers/*.cs
│   └── broken/                 # one deliberate violation per gate + treesmith.toml (G7 policy)
```

`basic` content tree (GUID register — suffix each with `-4000-8000-0000000000NN` pattern shown):

| item | guid | notes |
|---|---|---|
| templates: Project folder | 7c1e1c2a-0000-4000-8000-000000000000 | template folder |
| Page template | 7c1e1c2a-0001-…01 | sections/fields below |
| Content section | 7c1e1c2a-0002-…02 | |
| Title field | 7c1e1c2a-0003-…03 | Single-Line Text, versioned |
| Body field | 7c1e1c2a-0004-…04 | Rich Text, versioned |
| NavTitle field | 7c1e1c2a-0005-…05 | Single-Line Text, **unversioned** |
| RelatedPages field | 7c1e1c2a-0006-…06 | Treelist, **shared** |
| Meta template | 7c1e1c2a-0010-…10 | |
| SEO section | 7c1e1c2a-0011-…11 | |
| Keywords field | 7c1e1c2a-0012-…12 | Single-Line Text, shared |
| ArticlePage template | 7c1e1c2a-0020-…20 | __Base template = Page \| Meta |
| Page _Standard Values | 7c1e1c2a-0030-…30 | shared __Renderings (full layout XML) |
| MainLayout | 9a11aaaa-0001-…01 | layout item, Path=/Views/Shared/MainLayout.cshtml |
| Hero | 9a11aaaa-0002-…02 | view rendering, Path=/Views/Hero.cshtml |
| NavBar | 9a11aaaa-0003-…03 | controller rendering, Controller=NavBarController |
| PromoBanner | 9a11aaaa-0004-…04 | view rendering, Path=/Views/PromoBanner.cshtml |
| Home | c0ffee00-0001-…01 | ArticlePage; final-renderings delta on en v1 (ds swap to `local:/Data/HeroData` + PromoBanner inserted `p:after` Hero); languages en (2 versions) + da (1) |
| HeroData | c0ffee00-0002-…02 | under Home/Data |
| About | c0ffee00-0003-…03 | Page; en only (G7 exercise in broken, healthy here) |
| Data folder | c0ffee00-0004-…04 | FOLDER template |

Home's parent (`aaaaaaaa-0000-4000-8000-0000000000aa`) is deliberately unserialized (partial-tree
root case). Physical layout mirrors the item tree (`Home.yml` + `Home/About.yml` +
`Home/Data/HeroData.yml`, …) so `child_file_path` conventions hold. Standard-values layout binds
NavBar + Hero (ds = HeroData guid) in placeholder `main`; `MainLayout.cshtml` exposes
`@Html.Sitecore().Placeholder("main")`.

`broken` is a *small* standalone repo (own minimal template `Simple` with a Droplink field `Link`
and its std values layout) containing exactly one violation per gate: G1 missing-datasource GUID;
G2 malformed layout XML in one item and an unknown-uid delta in another; G3 view rendering with
missing `.cshtml`; G4 layout referencing placeholder `sidebar` never exposed; G5 `Link` value →
nonexistent GUID; G6 an item with a field GUID not in its template *and* a shared-declared field
serialized under `Versions`; G7 `treesmith.toml` requiring `["en","da"]` with an item having only
`en` versions. Keep every file within the emit rules of §3.1 so the corpus walker stays green.

---

## 14. Testing strategy

- **format**: unit tests for every parse/emit rule + mutation behaviors; a corpus walker test that
  round-trips every sniffable `.yml` under `fixtures/` (workspace root located via
  `CARGO_MANIFEST_DIR/../..`; skips silently if `fixtures/` is absent so the Format phase can land
  before the Fixtures phase).
- **graph**: unit tests against tempdir-authored mini-trees (do not depend on `fixtures/`).
- **template / presentation / gate / kernel**: tests against `fixtures/rainbow/basic` and
  `broken` (read-only; copy to a tempdir for any mutation test).
- **root package**: `tests/cli_integration.rs` (exit codes 0/1/2/3, JSON shapes, set-field on a
  tempdir copy incl. round-trip stability) and `tests/mcp_integration.rs` (handshake, tools/list,
  two tools/call round-trips) — written in the CLI and MCP phases respectively.
- Gates and census must be bit-for-bit deterministic across runs (assert in at least one test by
  running twice and comparing JSON with `elapsedMs` stripped).

## 15. Assumption log (verify against real repos in P0)

All tagged VERIFY-P0 above, gathered: emitter style rules 3–4 (§3.1), field insert-sort by GUID
string / language alpha / version numeric (§3.2), `Type:` stamping set + multilist storage form
(§3.5), braced-upper normalization for reference values, SCS-item-codec == Rainbow, layout `Path`
field GUID (§12), plus the spec Appendix's own unverified claims. The census (`treesmith census`)
is the falsifier once a real client repo is available; `fixtures/` encode these assumptions
self-consistently until then.

Post-build review additions: the item head sniff requires `ID: ` as the first key after `---` —
an item serialized with a non-canonical leading key would be silently ignored rather than
surfaced (VERIFY-P0: extend the sniff and census if a real repo exhibits one). Values ending in
`\n` are rejected on write with `trailing-newline-unsupported` — they encode as a block scalar
with a trailing blank line, which round-trips only at end-of-document, so acceptance would be
position-dependent.
