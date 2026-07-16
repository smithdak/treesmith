# Formats: the codec, round-trip fidelity, and the census

treesmith reads and writes Sitecore serialization YAML — Unicorn/Rainbow and SCS, auto-detected
(any `*.module.json` under the root selects SCS; both use the same item codec). This page explains
the one decision everything else rests on — a hand-written lexical-preservation codec instead of a
YAML library — and the harness that keeps it honest. The normative grammar and API live in
[`DESIGN.md` §3](../DESIGN.md); this page is the working understanding.

## Why not a YAML library

Rainbow's output is **not strict YAML**. Its plain scalars run verbatim to end-of-line, so this is
a legal Rainbow line whose value is `a: b`:

```yaml
Value: a: b
```

An event-stream YAML parser either rejects that or silently reinterprets it — and "silently
reinterprets" is fatal for a tool whose core promise is that a write changes only the lines you
touched. So `treesmith-format` implements the Rainbow writer's actual output grammar: a small,
strict subset parser paired with an emitter that is its exact inverse (decision (b) in the
[README](../README.md#decisions--deviations)).

## Lexical preservation: I2 by construction

The parser records every presentation detail the emitter needs to reproduce the source:

| Recorded | So that |
|---|---|
| UTF-8 BOM presence | A BOM'd file re-emits its BOM |
| Newline style (LF / CRLF) | Line endings survive; *mixed* endings are a parse fault |
| Trailing-newline presence | Files without one stay without one |
| Scalar style per value | `K: v` vs `K: "v"` vs `K: \|` block literals re-emit as parsed |
| Unknown keys, entry order | Keys treesmith doesn't understand round-trip verbatim, in place |

The consequence is the invariant the whole tool is named for: **`emit(parse(bytes)) == bytes`, by
construction, for every file that parses.** There is no normalization pass to get subtly wrong.
Files *outside* the subset (tab indentation, missing `---` marker, stray blank lines, mixed
newlines…) are parse faults — surfaced with a line number and kind, never silently skipped. The
kernel then refuses non-census operations on the tree (exit 3) until the fault is fixed.

Existing values keep their parsed style forever. Only **new or changed** values need a style
decision, made by a fixed write-rule table mirroring observed Rainbow writer output — multiline
and backslash/quote-containing values become block literals (Rainbow never escapes), empty values
become `""`, values starting with YAML-significant characters get quoted, GUID-keyed header
entries (`ID`, `Parent`, `Template`, …) are always quoted. The full table is in
[`DESIGN.md` §3.1](../DESIGN.md); rules derived from observation rather than documentation are
tagged `VERIFY-P0` there (see [the assumption log](#the-assumption-log)).

## What an item looks like

A real item from the fixture corpus (`fixtures/rainbow/basic/serialization/content/Home.yml`,
head):

```yaml
---
ID: "c0ffee00-0001-4000-8000-000000000001"
Parent: "aaaaaaaa-0000-4000-8000-0000000000aa"
Template: "7c1e1c2a-0020-4000-8000-000000000020"
Path: /sitecore/content/Home
DB: master
SharedFields:
- ID: "7c1e1c2a-0006-4000-8000-000000000006"
  Hint: RelatedPages
  Type: Treelist
  Value: |
    {C0FFEE00-0003-4000-8000-000000000003}
- ID: "7c1e1c2a-0012-4000-8000-000000000012"
  Hint: Keywords
  Value: sample, article, treesmith
Languages:
- Language: da
  Fields:
  - ID: "7c1e1c2a-0005-4000-8000-000000000005"
    Hint: NavTitle
    Value: Hjem
  Versions:
  - Version: 1
    Fields:
    - ID: "7c1e1c2a-0003-4000-8000-000000000003"
      Hint: Title
      Value: Hjem
```

Three storage slots per item: `SharedFields`, per-language `Fields` (unversioned), and
per-language per-`Version` `Fields` (versioned). When treesmith *inserts* (it never reorders what
exists), it follows Rainbow's deterministic conventions: canonical key order within an entry,
fields sorted by GUID string, languages alphabetical, versions numeric. Sections emptied by a
field removal are removed entirely, as Rainbow does.

## Field value formatters

Rainbow stores some field types differently from their raw platform value, stamping `Type:` on the
field so the transform is reversible. treesmith honors both directions:

- **Multilist family** (`Checklist`, `Multilist`, `Multilist with Search`, `Treelist`,
  `TreelistEx`, `tree list`): stored one braced-uppercase GUID per line in a block literal (the
  `RelatedPages` field above); the raw platform form is `|`-separated. `set-field` accepts either
  form, in any GUID style, and normalizes to storage form — a bad token is rejected by name.
- **XML family** (`Layout`, `Tracking`, `Rules`): stored as-is. treesmith validates layout XML on
  write but does not re-pretty-print existing values — re-formatting would be pure diff churn.

## The census: trust, then verify

The codec's promise is proven per-repo, not assumed. `treesmith census` walks the root, parses
every sniffable item file, re-emits, and byte-compares:

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

- **`faults`** — files that did not parse: `{file, kind, line, message}`. The file is outside the
  supported subset (or genuinely corrupt).
- **`mismatches`** — files that parsed but re-emitted differently: `{file, firstDiffLine,
  expected, actual}`. Each one is a codec bug or a wrong `VERIFY-P0` assumption — either way, a
  falsified invariant.

Either list being non-empty means `ok: false` and **exit 3**. Census is deliberately the one verb
that runs on a faulted tree, and it measures *fidelity only*: `fixtures/rainbow/broken` fails all
seven gates yet passes census cleanly, because every byte still round-trips.

> [!IMPORTANT]
> **Run `treesmith census` against a real client repo before trusting any write path on it.** A
> clean census — `ok: true`, zero faults, zero mismatches, exit 0 — is the green light for
> mutations on that repo. This is the project's P0: until a real repo has been censused, the
> write rules stand on fixtures that were authored to satisfy them.

## The assumption log

Several codec behaviors mirror *observed* Rainbow writer output rather than any specification:
emitter style rules for empty/leading-special values, field insert-sort order, the `Type:`
stamping set and multilist storage form, braced-uppercase normalization for reference values,
SCS-item-codec equivalence with Rainbow, and the layout `Path` field GUID. Each is tagged
`VERIFY-P0` in [`DESIGN.md`](../DESIGN.md) and gathered in its §15 assumption log. The synthetic
`fixtures/` encode these assumptions self-consistently — which is exactly why they cannot confirm
them. The census against real data is the falsifier; a mismatch points at the specific assumption
to revisit.

Two sharp edges from the log worth knowing up front:

- The item sniff requires `ID: ` as the first key after `---`; an item serialized with a
  non-canonical leading key would be ignored rather than surfaced.
- Values ending in `\n` are rejected on write (`trailing-newline-unsupported`): they encode as a
  block scalar with a trailing blank line, which round-trips only at end-of-document.

## The corpus

`fixtures/corpus/` holds 16 single-file codec edge cases — BOM, CRLF, missing trailing newline,
quoted/empty/backslash values, block literals with interior blank lines, plain values containing
`: `, unknown keys, `Type:`-before-`Hint:` ordering, bare `Key:` — and a workspace test walks
every sniffable `.yml` under `fixtures/` asserting byte-identical round-trip. New edge case
observed in the wild → new corpus file → the walker enforces it forever. Rules for authoring
fixtures are in [CONTRIBUTING.md](../CONTRIBUTING.md).
