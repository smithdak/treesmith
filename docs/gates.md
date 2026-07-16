# Gate reference

`treesmith validate` runs seven deterministic gates over the parsed graph plus a repo scan. The
engine's promise (invariant I5): **identical tree in → identical verdict out** — no network, no
wall clock, no randomness — and every finding carries a machine-readable reason code, so a script
or agent branches on `code`, never on message text.

- [Running the gates](#running-the-gates)
- [Reading a finding](#reading-a-finding)
- [G1 — datasource references](#g1--datasource-references)
- [G2 — layout XML and delta integrity](#g2--layout-xml-and-delta-integrity)
- [G3 — rendering code files](#g3--rendering-code-files)
- [G4 — placeholder exposure](#g4--placeholder-exposure)
- [G5 — field references](#g5--field-references)
- [G6 — template conformance](#g6--template-conformance)
- [G7 — language policy](#g7--language-policy)
- [Configuration](#configuration)
- [Using validate in CI and pre-commit hooks](#using-validate-in-ci-and-pre-commit-hooks)

## Running the gates

```sh
treesmith validate                      # all seven gates
treesmith validate --gate G1 --gate G5  # a subset
```

The report shape (JSON when piped or with `--json`):

```json
{
  "ok": false,
  "errors": 8,
  "warnings": 1,
  "infos": 0,
  "findings": [ …one object per finding, sorted by (gate, itemPath, code, message)… ],
  "skipped": [ …gates that did not run, with a reason — e.g. G7 without a language policy… ]
}
```

Exit code `1` when any finding has severity `error`, `0` otherwise. On a TTY each finding prints
as one line plus a summary. All examples below are real findings from
`treesmith --root fixtures/rainbow/broken validate` — the fixture repo authored with exactly one
violation per gate.

## Reading a finding

```json
{
  "code": "g5.broken-reference",
  "details": {
    "field": "b0000000-0003-4000-8000-000000000003",
    "name": "Link",
    "slot": "shared",
    "target": "e0000000-0000-4000-8000-0000000000e0"
  },
  "file": "serialization/content/Echo.yml",
  "gate": "G5",
  "itemId": "b0000000-0024-4000-8000-000000000024",
  "itemPath": "/sitecore/content/Echo",
  "message": "field `Link` references item e0000000-0000-4000-8000-0000000000e0 which is not serialized",
  "severity": "error"
}
```

| Field | Contract |
|---|---|
| `code` | Stable machine identifier, `g<N>.<kebab-reason>` — branch on this |
| `severity` | `error` (exit 1) · `warning` · `info` |
| `gate` | `G1`…`G7` |
| `itemId`, `itemPath`, `file` | Where to look; each may be `null` when not applicable |
| `details` | Code-specific structured payload (the fields shown per gate below) |
| `message` | Human sentence — informative, **not** a stable contract |

Severity encodes static-analysis confidence: `error` means the graph proves the problem;
`warning` means the evidence is strong but an unscanned artifact could excuse it; `info` is a
fact worth surfacing that is not a defect.

## G1 — datasource references

Every rendering's datasource, across the **entire layout stack** — shared and final renderings,
all languages and versions, standard-values layers included — must resolve to a serialized item.

| Code | Severity | Fires when |
|---|---|---|
| `g1.missing-datasource` | error | Datasource is set but resolves to nothing |
| `g1.dynamic-datasource` | info | Datasource is a `query:`/`code:` scheme, resolvable only at runtime |

```json
{
  "code": "g1.missing-datasource",
  "severity": "error",
  "itemPath": "/sitecore/content/Alpha",
  "message": "rendering SideView in device fe5d7fdf-89c0-4d99-9aa3-b5fbd009c9f3 has an unresolvable datasource `{D0000000-0000-4000-8000-0000000000D0}`",
  "details": {
    "datasource": "{D0000000-0000-4000-8000-0000000000D0}",
    "device": "fe5d7fdf-89c0-4d99-9aa3-b5fbd009c9f3",
    "renderingId": "b0000000-0011-4000-8000-000000000011",
    "uid": "22222222-2222-4222-8222-222222222201"
  }
}
```

## G2 — layout XML and delta integrity

Layout field values must parse as layout XML, and every final-renderings delta must apply cleanly
to the shared layout beneath it.

| Code | Severity | Fires when |
|---|---|---|
| `g2.malformed-xml` | error | A layout value does not parse (`details.offset` points at the failure) |
| `g2.unknown-uid` | error | A delta patches a rendering `uid` that does not exist in the base layout |
| `g2.bad-position-ref` | error | A delta's `p:before`/`p:after` selector cannot be resolved |
| `g2.device-without-layout` | warning | A delta introduces a device with no layout assigned |

```json
{
  "code": "g2.unknown-uid",
  "severity": "error",
  "itemPath": "/sitecore/content/Charlie",
  "message": "final-renderings delta targets uid `{33333333-3333-4333-8333-333333333301}` in device {FE5D7FDF-89C0-4D99-9AA3-B5FBD009C9F3}, but no such rendering exists in the shared layout",
  "details": {
    "device": "{FE5D7FDF-89C0-4D99-9AA3-B5FBD009C9F3}",
    "uid": "{33333333-3333-4333-8333-333333333301}"
  }
}
```

## G3 — rendering code files

Rendering and layout items must point at code files that exist in the repository. View renderings
and layouts are checked by their `Path` field (`.cshtml` virtual path, suffix-matched against the
repo file list); controller renderings by their `Controller` field (short type name matched
against `class <Name>` in `.cs` files).

| Code | Severity | Fires when |
|---|---|---|
| `g3.missing-view` | error | The `.cshtml` the `Path` names does not exist |
| `g3.missing-controller` | warning | No `.cs` file declares the controller class |
| `g3.empty-path` | warning | The rendering item has no path/controller value at all |

The severity split is confidence: a named view file either exists or it doesn't (error), while a
controller class could live in a compiled dependency the repo scan cannot see (warning).

## G4 — placeholder exposure

Every placeholder a rendering binds to must be exposed by some scanned view — treesmith statically
scans all `.cshtml` files for `Placeholder("NAME")` and `DynamicPlaceholder("NAME")` calls and
compares against the placeholder paths referenced in presentation. Dynamic-placeholder suffixes
(`-{guid}-N`) are stripped before comparison.

| Code | Severity | Fires when |
|---|---|---|
| `g4.placeholder-not-exposed` | warning | A bound placeholder name appears in no scanned view |

```json
{
  "code": "g4.placeholder-not-exposed",
  "severity": "warning",
  "itemPath": "/sitecore/content/Dee",
  "message": "rendering SideView binds placeholder `sidebar` (path `sidebar`), which no scanned view exposes",
  "details": {
    "device": "fe5d7fdf-89c0-4d99-9aa3-b5fbd009c9f3",
    "leaf": "sidebar",
    "placeholder": "sidebar",
    "renderingId": "b0000000-0011-4000-8000-000000000011",
    "uid": "22222222-2222-4222-8222-222222222202"
  }
}
```

Warning, not error, because placeholders can be exposed by mechanisms the static scan cannot see
(placeholder settings, code-constructed names). If that is routine in your repo, disable G4 in
[configuration](#configuration).

## G5 — field references

Fields whose resolved type is in the reference family — the multilist family (`Checklist`,
`Multilist`, `Treelist`, …), `Droplink`, `Droptree`, `Reference`, `Grouped Droplink` — plus
General Link `id=` attributes with `linktype="internal"`, must point at serialized items and
contain only well-formed GUID tokens.

| Code | Severity | Fires when |
|---|---|---|
| `g5.broken-reference` | error | A referenced GUID is not a serialized item |
| `g5.invalid-guid-token` | error | A token in a reference value does not parse as a GUID |

(Example payload shown in [Reading a finding](#reading-a-finding).)

## G6 — template conformance

Items must conform to their effective template: only defined fields, each in the section the
definition declares, no duplicates, values valid for their field type. This is the same
`validate_value` logic the write path enforces — G6 catches what was serialized by *other* tools.

| Code | Severity | Fires when |
|---|---|---|
| `g6.unknown-field` | error | A serialized field is not in the item's effective template |
| `g6.wrong-section` | error | A field sits in a different slot than its definition declares |
| `g6.duplicate-field` | error | The same field ID appears twice in one slot |
| `g6.invalid-value` | error | A value fails its field-type validation |
| `g6.unresolved-template` | warning | The item's template is not serialized, so conformance cannot be checked |
| `g6.unresolved-base` | warning | A base template in the chain is not serialized |

```json
{
  "code": "g6.wrong-section",
  "severity": "error",
  "itemPath": "/sitecore/content/Foxtrot",
  "message": "field `Link` is declared shared but serialized in the versioned slot (en #1)",
  "details": {
    "actual": "versioned",
    "declared": "shared",
    "field": "b0000000-0003-4000-8000-000000000003",
    "name": "Link",
    "slot": "en #1"
  }
}
```

> [!NOTE]
> System fields serialized on items (e.g. `__Final Renderings`) whose defining system templates
> are not serialized in the repo do not trip `g6.unknown-field` — they surface through `get` as
> `fieldsNotInTemplate` instead. Real repos rarely serialize the platform's system templates.

## G7 — language policy

Items under the configured paths that have at least one version in *any* language must have at
least one version in *every* required language. Skipped — reported in the report's `skipped`
array with reason `no language policy configured` — unless a policy exists in `treesmith.toml`.

| Code | Severity | Fires when |
|---|---|---|
| `g7.missing-language` | error | A required language has no version on an in-scope item |

```json
{
  "code": "g7.missing-language",
  "severity": "error",
  "itemPath": "/sitecore/content/Golf",
  "message": "item has no version in required language `da`",
  "details": { "language": "da" }
}
```

## Configuration

Optional `<root>/treesmith.toml`; absence means all gates on, no language policy (G7 skipped):

```toml
[gates]
disabled = ["G4"]                  # gate keys "G1".."G7"

[gates.language-policy]
required = ["en", "da"]            # presence of this table enables G7
paths    = ["/sitecore/content"]   # case-insensitive path-prefix scope (this is the default)
```

## Using validate in CI and pre-commit hooks

The exit-code contract makes `validate` a hook body with no wrapper logic:

```sh
#!/bin/sh
# .git/hooks/pre-commit
exec treesmith validate
```

Exit 0 (clean or warnings-only) lets the commit through; exit 1 (error-severity findings) blocks
it; exit 3 means the tree itself is unreadable — fix the parse fault before anything else. In
GitHub Actions the same single line works as a step:

```yaml
- name: Validate content tree
  run: treesmith validate
```

Because the engine is deterministic (I5), a gate verdict is reproducible on any machine at the
same commit — a CI failure always reproduces locally with the same finding codes in the same
order.

> [!TIP]
> `validate` and `census` answer different questions. A repo can pass `census` (every byte
> round-trips) while failing `validate` on all seven gates — `fixtures/rainbow/broken` does
> exactly that by design. Fidelity problems exit 3; policy problems exit 1.
