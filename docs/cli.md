# CLI reference

Every capability of the kernel is a CLI verb. This page documents each verb with its exact
signature, semantics, and a real run against the bundled fixture repo
(`fixtures/rainbow/basic` unless noted). All examples on this page were captured from actual
`treesmith 0.1.0` runs — outputs are trimmed where marked with `…`, never altered.

- [Invocation & global flags](#invocation--global-flags)
- [Output contract](#output-contract)
- [Item designators](#item-designators)
- [Exit codes](#exit-codes)
- [`query`](#query) · [the query language](#the-query-language)
- [`get`](#get)
- [`set-field`](#set-field)
- [`forge`](#forge)
- [`move`](#move)
- [`resolve-presentation`](#resolve-presentation)
- [`validate`](#validate)
- [`census`](#census)
- [`mcp`](#mcp)
- [Error payloads](#error-payloads)
- [Configuration: `treesmith.toml`](#configuration-treesmithtoml)

## Invocation & global flags

```text
treesmith [--root DIR] [--json] <VERB> [ARGS...]
```

| Flag | Meaning |
|---|---|
| `--root DIR` | Repository root to operate on (default: current directory) |
| `--json` | Force JSON output even when stdout is a TTY |

Global flags may appear before or after the verb. The root is the directory that contains your
serialized items — for Unicorn/Rainbow, the parent of `serialization/`; for SCS, the directory
whose `*.module.json` files describe the item folders. The format is auto-detected (any
`*.module.json` under the root selects SCS, otherwise Rainbow); both use the same item codec.

> [!TIP]
> On Windows under Git Bash (MSYS), `/sitecore/...` arguments get rewritten by the shell's path
> conversion into `C:/Program Files/Git/sitecore/...` before treesmith sees them. Prefix the
> command with `MSYS_NO_PATHCONV=1`, or use the item's GUID instead of its path. PowerShell and
> cmd.exe are unaffected.

## Output contract

- **JSON (pretty) on stdout** when stdout is *not* a TTY — piped or redirected, no flag needed —
  or when `--json` is passed.
- **Human-readable lines on stdout** when stdout is a TTY.
- **Diagnostics always go to stderr**, never stdout, so `treesmith ... | jq` is always safe.

The JSON shapes are camelCase and stable; they are contracts, defined in
[`DESIGN.md` §8](../DESIGN.md). Object keys serialize in alphabetical order.

## Item designators

Wherever a verb takes `<ITEM>`, `<PARENT>`, or `<NEW_PARENT>`, you may pass:

| Form | Example |
|---|---|
| Hyphenated GUID | `c0ffee00-0001-4000-8000-000000000001` |
| Braced GUID | `{C0FFEE00-0001-4000-8000-000000000001}` |
| 32 hex digits | `c0ffee0000014000800000000000001` (any case) |
| Item path | `/sitecore/content/Home` (case-insensitive) |

`<TEMPLATE>` additionally accepts a template *name* (case-insensitive). A path matching more than
one item is an `ambiguous-path` usage error that lists the candidate GUIDs; pass a GUID to
disambiguate.

## Exit codes

| Code | Class | Meaning |
|---|---|---|
| `0` | success | The operation succeeded (`validate`: no error-severity findings) |
| `1` | validation | Gate failure, or a rejected write (schema-invalid mutation) |
| `2` | usage | Bad arguments, unknown item/path/gate/template, malformed designator |
| `3` | tree-fault | The tree is unreadable: parse fault, duplicate/missing ID (`census` with faults or mismatches also exits 3) |

Exit 1 and exit 3 are deliberately distinct failure classes: **exit 3 = the tree is unreadable;
exit 1 = the tree parses but violates a gate or rejected a write.** Every verb except `census`
refuses to run on a faulted tree (exit 3) rather than silently skipping unparseable files.

---

## `query`

```text
treesmith query <EXPR>
```

Evaluates a [query expression](#the-query-language) over the item graph and returns matching items
as summaries, always ordered by `(path, id)`.

```sh
treesmith --root fixtures/rainbow/basic query 'path:/sitecore/content/**' --json
```

```json
{
  "count": 4,
  "items": [
    {
      "db": "master",
      "file": "serialization/content/Home.yml",
      "id": "c0ffee00-0001-4000-8000-000000000001",
      "languages": [
        { "language": "da", "versions": [1] },
        { "language": "en", "versions": [1, 2] }
      ],
      "name": "Home",
      "path": "/sitecore/content/Home",
      "template": {
        "id": "7c1e1c2a-0020-4000-8000-000000000020",
        "name": "ArticlePage"
      }
    },
    …3 more items: /sitecore/content/Home/About, …/Home/Data, …/Home/Data/HeroData
  ],
  "ok": true
}
```

### The query language

An expression is one or more whitespace-separated `key:value` terms; **all terms must match**
(AND). A value containing spaces is quoted: `name:"Press Release"`. Bare terms (no `key:`) are a
usage error.

| Term | Matches | Example |
|---|---|---|
| `path:` | Item path against a glob, case-insensitive | `path:/sitecore/content/**` |
| `name:` | Last path segment against a glob | `name:Hero*` |
| `template:` | Template GUID (any form) or exact template-item name, case-insensitive | `template:ArticlePage` |
| `field:Name=Value` | Field named/GUID `Name` has exactly `Value` in any slot | `field:NavTitle=Hjem` |
| `field:Name` | Field exists with a non-empty value in any slot | `field:Keywords` |

Glob semantics: `**` crosses `/` boundaries, `*` matches within one segment, `?` matches one
non-`/` character. `field:` names match the field's hint case-insensitively, or a field GUID.

Real results against the fixture, one line each:

```sh
treesmith --root fixtures/rainbow/basic query 'template:Page'
treesmith --root fixtures/rainbow/basic query 'name:Hero*'
treesmith --root fixtures/rainbow/basic query 'field:NavTitle=Hjem'
treesmith --root fixtures/rainbow/basic query 'path:/sitecore/content/** field:Keywords'
```

```text
3 items: /sitecore/content/Home/About, /sitecore/content/Home/Data/HeroData,
         /sitecore/templates/Sample/Page/__Standard Values
2 items: /sitecore/content/Home/Data/HeroData, /sitecore/layout/Renderings/Hero
1 item:  /sitecore/content/Home
1 item:  /sitecore/content/Home
```

Note `template:Page` matches *everything whose Template GUID is the Page template* — including the
template's own `__Standard Values` item. Filter with a `path:` term when you only want content.

## `get`

```text
treesmith get <ITEM>
```

Returns one item in full detail: every field in every slot, with the field's **effective-template
resolution** attached — its resolved name, type, section, and the template that defined it
(`definedBy`). Fields present on disk but absent from the effective template are surfaced
separately in `fieldsNotInTemplate` instead of being guessed at.

```sh
treesmith --root fixtures/rainbow/basic get /sitecore/content/Home --json
```

```json
{
  "item": {
    "db": "master",
    "fieldsNotInTemplate": [
      {
        "hint": "__Final Renderings",
        "id": "04bf00db-f5fb-41f7-8ab7-22408372a981",
        "slot": "versioned:en:1"
      }
    ],
    "file": "serialization/content/Home.yml",
    "id": "c0ffee00-0001-4000-8000-000000000001",
    "languages": [
      {
        "language": "da",
        "unversioned": [
          {
            "definedBy": "7c1e1c2a-0001-4000-8000-000000000001",
            "id": "7c1e1c2a-0005-4000-8000-000000000005",
            "name": "NavTitle",
            "section": "unversioned",
            "type": "Single-Line Text",
            "value": "Hjem"
          }
        ],
        "versions": [ …1 version with Title ]
      },
      …language "en" with 2 versions
    ],
    "name": "Home",
    "path": "/sitecore/content/Home",
    "sharedFields": [ …RelatedPages (Treelist), Keywords ],
    "template": { "id": "7c1e1c2a-0020-4000-8000-000000000020", "name": "ArticlePage" },
    "templateChain": [ …ArticlePage, Page, Meta ]
  },
  "ok": true
}
```

`templateChain` is the item's full base-template resolution order — the same chain the write path
uses to resolve field names, so what `get` shows is exactly what `set-field` will accept.

## `set-field`

```text
treesmith set-field <ITEM> <FIELD> <VALUE> [--language L] [--version N] [--no-create-version]
```

Writes one field value, resolved through the item's effective template. `<FIELD>` is a field name
(resolved case-insensitively through the template chain) or a field GUID. The **slot is decided by
the field definition's section**, never by where a value currently sits or by what flags you pass:

| Field section | `--language` / `--version` | Default slot |
|---|---|---|
| shared | rejected | the shared block |
| unversioned | `--language` only | language `en` |
| versioned | both allowed | language `en`, its highest existing version |

For a versioned field with no versions in the target language, a version 1 is created unless
`--no-create-version` is passed. Values are validated against the field type before writing
(checkbox `1`/empty, integer digits, ISO timestamps, GUID lists for multilists and droplinks —
see [docs/formats.md](formats.md)); multilist input accepts `|`- or newline-separated GUIDs in any
form and is normalized to Rainbow's storage form; layout fields must parse as layout XML.

Before anything touches disk, the candidate bytes are **self-checked**: emitted, re-parsed,
re-emitted, byte-compared, and the mutated slot read back and compared to the requested value. Only
then is the file written and the in-memory graph refreshed.

```sh
treesmith --root fixtures/rainbow/basic set-field /sitecore/content/Home Title "Welcome to Acme" --json
```

```json
{
  "changedFiles": ["serialization/content/Home.yml"],
  "item": { …full ItemDetail, as `get` would return it… },
  "ok": true,
  "selfCheck": "ok"
}
```

The resulting `git diff` is exactly one changed line — see the
[README walkthrough](../README.md#what-a-write-looks-like). A misspelled field name is rejected
with the template's actual field list and a nearest-match suggestion:

```json
{
  "error": {
    "class": "validation",
    "code": "unknown-field",
    "details": {
      "available": ["Body", "Keywords", "NavTitle", "RelatedPages", "Title"],
      "didYouMean": "Title",
      "field": "Titel"
    },
    "message": "field `Titel` is not in the item's effective template — did you mean `Title`?"
  },
  "ok": false
}
```

## `forge`

```text
treesmith forge <TEMPLATE> <PARENT> <NAME> [--id GUID] [--language L]
```

Creates a new item from a template. `<TEMPLATE>` must resolve to a known template (GUID or name);
`<PARENT>` must exist. The new file lands where the serialization convention dictates —
`.../Parent.yml` gets children at `.../Parent/<Name>.yml` — and the item ID is a fresh random v4
GUID unless `--id` pins it (the only source of randomness in the tool). `--language L` additionally
creates version 1 in that language. A name collision under the parent, or an existing file at the
target path, is a validation error.

```sh
treesmith --root fixtures/rainbow/basic forge ArticlePage /sitecore/content/Home "Press Release" --id 0f0f0f0f-0000-4000-8000-00000000000f
```

```json
{
  "changedFiles": ["serialization/content/Home/Press Release.yml"],
  "item": { …ItemDetail for the new item… },
  "ok": true,
  "selfCheck": "ok"
}
```

The created file is minimal and canonical — ID, Parent, Template, Path, DB (inherited from the
parent), nothing invented:

```yaml
---
ID: "0f0f0f0f-0000-4000-8000-00000000000f"
Parent: "c0ffee00-0001-4000-8000-000000000001"
Template: "7c1e1c2a-0020-4000-8000-000000000020"
Path: /sitecore/content/Home/Press Release
DB: master
```

## `move`

```text
treesmith move <ITEM> <NEW_PARENT> [--name NAME]
```

Relocates an item (optionally renaming it) with the whole-tree bookkeeping done for you:

- The item's and every descendant's `Path` field is rewritten.
- Files and directories are relocated following the `Parent/<Child>.yml` convention.
- **Path-form references graph-wide are rewritten**: `ds=` datasources in layout XML and
  path-valued reference fields that equal or fall under the old path.

A name collision under the new parent is a validation error; nothing is half-moved.

```sh
treesmith --root fixtures/rainbow/basic move "/sitecore/content/Home/Press Release" /sitecore/content/Home/Data
```

```text
git status --porcelain now shows:
 M serialization/content/Home.yml
?? serialization/content/Home/Data/Press Release.yml
```

and the moved file's `Parent:` and `Path:` reflect the new location.

## `resolve-presentation`

```text
treesmith resolve-presentation <ITEM> [--language L] [--version N]
```

Answers "what actually renders on this page?" — the full layout stack resolved the way the
platform resolves it: standard-values shared layouts base-most first, the item's shared layout,
then final-renderings deltas merged on top for the requested language/version. Each rendering
comes back with its datasource *resolved* (context item, concrete item, dynamic `query:`/`code:`
scheme, or missing) and the code files it binds to, located in the repo.

Defaults: language = the item's first language with versions (alphabetically), version = the
highest for that language. Pass `--language en` explicitly when the item is multilingual.

```sh
treesmith --root fixtures/rainbow/basic resolve-presentation /sitecore/content/Home --json
```

```json
{
  "devices": [
    {
      "deviceId": "fe5d7fdf-89c0-4d99-9aa3-b5fbd009c9f3",
      "layout": {
        "id": "9a11aaaa-0001-4000-8000-000000000001",
        "path": "/sitecore/layout/Layouts/MainLayout"
      },
      "layoutCodeFiles": ["src/Views/Shared/MainLayout.cshtml"],
      "notes": [],
      "renderings": [
        {
          "codeFiles": ["src/Controllers/NavBarController.cs"],
          "datasource": { "kind": "contextItem" },
          "parameters": {},
          "placeholder": "main",
          "placeholderLeaf": "main",
          "renderingId": "9a11aaaa-0003-4000-8000-000000000003",
          "renderingName": "NavBar",
          "source": "shared",
          "uid": "11111111-1111-4111-8111-111111111101"
        },
        {
          "codeFiles": ["src/Views/Hero.cshtml"],
          "datasource": {
            "id": "c0ffee00-0002-4000-8000-000000000002",
            "kind": "item",
            "path": "/sitecore/content/Home/Data/HeroData",
            "raw": "{C0FFEE00-0002-4000-8000-000000000002}"
          },
          "parameters": {},
          "placeholder": "main",
          "placeholderLeaf": "main",
          "renderingId": "9a11aaaa-0002-4000-8000-000000000002",
          "renderingName": "Hero",
          "source": "shared",
          "uid": "11111111-1111-4111-8111-111111111102"
        }
      ]
    }
  ],
  "itemId": "c0ffee00-0001-4000-8000-000000000001",
  "itemPath": "/sitecore/content/Home",
  "language": "da",
  "ok": true,
  "version": 1
}
```

(The example resolved language `da` — the fixture item's alphabetically-first language with
versions. `--language en --version 1` would additionally show the final-renderings delta: the Hero
datasource swapped to `local:/Data/HeroData` and a PromoBanner inserted after it, each tagged
`"source": "final"`.)

`datasource.kind` is a tagged union an agent can branch on directly:

| kind | Meaning | Extra fields |
|---|---|---|
| `contextItem` | Empty datasource — renders against the page item | — |
| `item` | Resolved to a concrete item | `raw`, `id`, `path` |
| `dynamic` | `query:` / `code:` scheme, resolvable only at runtime | `raw`, `scheme` |
| `missing` | Set but unresolvable | `raw` |

## `validate`

```text
treesmith validate [--gate G1 --gate G5 ...]
```

Runs the deterministic gate engine — all seven gates by default, or only the ones named by
repeated `--gate` flags. Every finding carries a machine-readable reason code; the full catalog
with real payloads is in [docs/gates.md](gates.md).

```sh
treesmith --root fixtures/rainbow/broken validate --json
```

```json
{
  "errors": 8,
  "findings": [
    {
      "code": "g1.missing-datasource",
      "details": {
        "datasource": "{D0000000-0000-4000-8000-0000000000D0}",
        "device": "fe5d7fdf-89c0-4d99-9aa3-b5fbd009c9f3",
        "renderingId": "b0000000-0011-4000-8000-000000000011",
        "uid": "22222222-2222-4222-8222-222222222201"
      },
      "file": "serialization/content/Alpha.yml",
      "gate": "G1",
      "itemId": "b0000000-0020-4000-8000-000000000020",
      "itemPath": "/sitecore/content/Alpha",
      "message": "rendering SideView in device fe5d7fdf-89c0-4d99-9aa3-b5fbd009c9f3 has an unresolvable datasource `{D0000000-0000-4000-8000-0000000000D0}`",
      "severity": "error"
    },
    …8 more findings covering g2–g7
  ],
  "infos": 0,
  "ok": false,
  "skipped": [],
  "warnings": 1
}
```

Exit code is `1` when any finding has severity `error`, `0` otherwise — which is exactly the
pre-commit-hook contract. On a TTY, each finding prints as one line
(`G1 error g1.missing-datasource /sitecore/content/Alpha — message`) plus a summary. Naming an
unknown gate is a usage error (exit 2) listing the valid gates.

## `census`

```text
treesmith census
```

The P0 fidelity harness: parses every serialized item under the root, re-emits it, and
byte-compares. This is the falsifier for the byte-identical round-trip invariant and must be run
against a real client repo before trusting any write path on it.

```sh
treesmith --root fixtures/rainbow/basic census --json
```

```json
{
  "elapsedMs": 2,
  "faults": [],
  "files": 20,
  "items": 20,
  "mismatches": [],
  "ok": true,
  "roundTripOk": 20
}
```

`faults` are files that failed to parse (with kind, line, message); `mismatches` are files that
parsed but re-emitted differently (with the first differing line and both byte sequences). Either
being non-empty makes `ok: false` and **exit 3**. Census is the only verb that runs on a faulted
tree — that is its job.

> [!NOTE]
> Census measures *fidelity*, not *policy*: a repo full of gate violations that still round-trips
> byte-identically gets `ok: true`, exit 0. `fixtures/rainbow/broken` demonstrates the split —
> `census` passes it, `validate` fails it with all seven gates firing.

## `mcp`

```text
treesmith mcp [--root DIR]
```

Launches the persistent MCP server on stdio — a warm graph plus filesystem watcher, exposing all
verbs as MCP tools. Full protocol documentation and a captured session: [docs/mcp.md](mcp.md).

---

## Error payloads

Every error is `{"ok": false, "error": {class, code, message, details}}` on stdout, plus a
one-line human diagnostic on stderr. `class` maps 1:1 to an exit code; `code` is the stable
machine identifier an agent should branch on.

| class | exit | codes |
|---|---|---|
| `usage` | 2 | `unknown-path`, `unknown-item`, `invalid-designator`, `ambiguous-path`, `unknown-template`, `unknown-gate`, bad query expressions |
| `validation` | 1 | `unknown-field`, `wrong-slot-for-section`, `invalid-value`, `malformed-layout-xml`, `blob-unsupported`, `trailing-newline-unsupported`, `self-check-failed`, forge/move collisions |
| `tree-fault` | 3 | the graph has parse faults / duplicate or missing IDs; `details` lists the offending files |
| `io` | 1 | filesystem errors during a write |

Two real examples:

```json
{
  "error": {
    "class": "usage",
    "code": "unknown-path",
    "details": { "path": "/sitecore/content/Nope" },
    "message": "unknown item path `/sitecore/content/Nope`"
  },
  "ok": false
}
```

```json
{
  "error": {
    "class": "usage",
    "code": "unknown-gate",
    "details": null,
    "message": "unknown gate `G9` (expected one of G1, G2, G3, G4, G5, G6, G7)"
  },
  "ok": false
}
```

## Configuration: `treesmith.toml`

An optional `<root>/treesmith.toml` configures the gate engine; absence means defaults (all gates
on, no language policy):

```toml
[gates]
disabled = []                      # e.g. ["G4"] to turn a gate off

[gates.language-policy]
required = ["en", "da"]            # presence of this table enables G7
paths    = ["/sitecore/content"]   # scope of the language requirement
```

Details per gate: [docs/gates.md](gates.md).
