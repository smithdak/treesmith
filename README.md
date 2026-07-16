# treesmith

treesmith is a **developer tool, not a CMS**: an agent-first Rust content kernel that gives coding
agents template-aware, GUID-safe, structure-safe read/write access to a client's serialized CMS
content tree (Sitecore Unicorn / Rainbow YAML and SCS YAML), and gives the human operator
verification through tooling already trusted — `git diff`, deterministic CLI query output, and
deterministic gates. It renders nothing, hosts nothing, and syncs nothing: the git working tree is
the only source of truth, and every capability is a plain library function exposed through exactly
two thin surfaces — **CLI verbs** and **MCP tools**.

- **Correctness of the write path** comes first: mutations pass through template resolution (correct
  field IDs, correct shared/unversioned/versioned placement, GUID discipline) and every write is
  re-parsed and graph-compared before it is reported successful. treesmith never emits a file it
  cannot re-parse to an identical graph.
- **Round-trip fidelity is byte-identical** by construction: `parse → emit` with no mutation
  reproduces the original bytes exactly, so a treesmith write shows up in `git diff` as only the
  lines you changed.
- **Gates are deterministic and interrogable**: identical tree in → identical verdict out, with a
  machine-readable reason code for every finding. No network, no wall clock, no randomness.

---

## Quickstart

Build and install the single static binary (no runtime dependencies, no telemetry):

```sh
git clone https://github.com/smithdak/treesmith
cd treesmith
cargo install --path .
```

Point it at a serialized repo (the directory that contains your `serialization/` items — for SCS,
the folders your `*.module.json` files describe; the format is auto-detected):

```sh
# What's in the tree?
treesmith --root /path/to/repo query 'path:/sitecore/content/**'

# One item with its resolved effective fields (template inheritance applied)
treesmith --root /path/to/repo get /sitecore/content/Home

# The presentation tree: renderings, datasources, and the code files they bind to
treesmith --root /path/to/repo resolve-presentation /sitecore/content/Home

# Run the gate engine (pre-commit-hook compatible)
treesmith --root /path/to/repo validate
```

`--root` defaults to the current directory, so from inside a repo you can drop it. Every command
also exists as an MCP tool (see [MCP setup](#mcp-setup-for-a-coding-agent)).

**Output contract:** JSON (pretty) on stdout when stdout is **not** a TTY (i.e. when piped or
redirected — no flag needed); human-readable lines when it is; `--json` forces JSON either way.
Diagnostics always go to stderr, never stdout, so `treesmith ... | jq` is always safe.

---

## Verbs

Note the **positional** argument signatures for the mutating verbs — they are positional, not
`--flags`. (The MCP tools use named JSON arguments; see the [MCP section](#mcp-setup-for-a-coding-agent).)

| Verb | Signature | Purpose |
|---|---|---|
| `query` | `query <EXPR>` | Path/template/field predicates over the graph |
| `get` | `get <ITEM>` | Item with resolved effective fields |
| `set-field` | `set-field <ITEM> <FIELD> <VALUE> [--language L] [--version N] [--no-create-version]` | Single-field mutation, template-validated |
| `forge` | `forge <TEMPLATE> <PARENT> <NAME> [--id GUID] [--language L]` | Create item from template (GUID-safe, section-correct) |
| `move` | `move <ITEM> <NEW_PARENT> [--name NAME]` | Structure-safe relocation, path/reference updates |
| `resolve-presentation` | `resolve-presentation <ITEM> [--language L] [--version N]` | Placeholder/rendering tree with datasources and code files |
| `validate` | `validate [--gate G1 --gate G5 ...]` | Run the gate engine; pre-commit-hook compatible |
| `census` | `census` | Round-trip fidelity census (the P0 harness) |
| `mcp` | `mcp` | Launch the persistent MCP server |

`<ITEM>`/`<PARENT>`/`<NEW_PARENT>` designators are a GUID in any form (hyphenated, `{braced}`, or
32 hex digits) or a `/sitecore/...` path. `<TEMPLATE>` also accepts a template name. Global flags
`--root DIR` and `--json` may appear before or after the verb.

### Exit codes

Gate failures and broken trees are **different failure classes** and scripts can tell them apart:

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | Gate/validation failure, or a rejected write (schema-invalid mutation) |
| `2` | Usage error (bad arguments, unknown item/path/gate, malformed designator) |
| `3` | Tree unreadable — a parse or round-trip **fidelity** fault (`census` with faults/mismatches also exits 3) |

Because fidelity faults (exit 3) and gate violations (exit 1) are distinct classes, `census` on a
repo whose only problems are *gate* violations returns `ok: true` **exit 0** — census measures
byte-level round-trip fidelity only; policy/structure problems surface through `validate`. See
[fixtures](#fixtures--the-census-harness) for a worked example.

### JSON output contracts

All shapes are camelCase and defined precisely in [`DESIGN.md` §8](DESIGN.md). In brief:

```text
query    {"ok":true,"count":N,"items":[ItemSummary]}
get      {"ok":true,"item":ItemDetail}
mutate   {"ok":true,"changedFiles":["rel/path.yml"],"selfCheck":"ok","item":ItemDetail}
validate {"ok":bool,"errors":N,"warnings":N,"infos":N,"findings":[Finding],"skipped":[{gate,reason}]}
census   {"ok":bool,"files":N,"items":N,"roundTripOk":N,"faults":[..],"mismatches":[..],"elapsedMs":N}
error    {"ok":false,"error":{"class","code","message","details"}}
```

Errors carry both a broad `class` (`usage` · `validation` · `tree-fault` · `io`) and a **distinct
machine `code`** so an agent can branch without string-parsing the message — e.g. usage errors
split into `unknown-path`, `unknown-item`, `invalid-designator`, `ambiguous-path`,
`unknown-template`, `unknown-gate`, and a validation `unknown-field` includes the effective
template's `available` field names plus a `didYouMean` nearest match in `details`.

---

## Gates (G1–G7)

`treesmith validate` runs seven deterministic gates, all evaluable from the parsed graph plus a repo
scan. Each finding carries a `code`, a `severity` (`error` / `warning` / `info`), and the offending
item/file.

| Gate | Checks | Reason codes (severity) |
|---|---|---|
| **G1** | Broken or missing datasource reference | `g1.missing-datasource` (error), `g1.dynamic-datasource` (info) |
| **G2** | Malformed layout XML / unresolvable final-renderings delta | `g2.malformed-xml`, `g2.unknown-uid`, `g2.bad-position-ref` (error), `g2.device-without-layout` (warning) |
| **G3** | Rendering item → missing code file | `g3.missing-view` (error), `g3.missing-controller`, `g3.empty-path` (warning) |
| **G4** | Placeholder mismatch (static `.cshtml` scan vs presentation references) | `g4.placeholder-not-exposed` (warning) |
| **G5** | Field reference to a nonexistent item | `g5.broken-reference`, `g5.invalid-guid-token` (error) |
| **G6** | Template conformance on created/mutated items | `g6.unknown-field`, `g6.wrong-section`, `g6.duplicate-field`, `g6.invalid-value` (error), `g6.unresolved-template`, `g6.unresolved-base` (warning) |
| **G7** | Language-version gaps against a required-languages policy | `g7.missing-language` (error) |

G7 is skipped unless a language policy is configured. Configuration lives in an optional
`<root>/treesmith.toml` (absent = defaults):

```toml
[gates]
disabled = []                      # e.g. ["G4"] to turn a gate off
[gates.language-policy]
required = ["en", "da"]            # presence enables G7
paths    = ["/sitecore/content"]  # items under these paths must carry all required languages
```

Use it as a pre-commit hook by running `treesmith validate` and letting exit code 1 block the commit.

---

## MCP setup (for a coding agent)

The `mcp` verb launches a long-running JSON-RPC 2.0 server over stdio that owns a warm in-memory
graph and a filesystem watcher, so agent sessions get sub-command-latency reads. Register it with a
coding-agent MCP client like so:

```json
{
  "mcpServers": {
    "treesmith": {
      "command": "treesmith",
      "args": ["mcp", "--root", "/path/to/repo"]
    }
  }
}
```

`tools/list` exposes eight tools mirroring the CLI verbs 1:1 (note: snake_case where the verb has a
hyphen): `query`, `get`, `set_field`, `forge`, `move`, `resolve_presentation`, `validate`,
`census`. Their arguments are a **named** camelCase object, e.g. `set_field` takes
`{item, field, value, language?, version?, createVersion?}` and `forge` takes
`{template, parent, name, id?, language?}`. `tools/call` returns the same JSON string the CLI would
print as `content[0].text`, with `isError: true` for kernel errors and for `validate` when the gate
report has errors — kernel errors are always returned as machine-readable payloads, never as a
protocol-level error.

---

## Fixtures & the census harness

`fixtures/` is a **synthetic** corpus (`fixtures/rainbow/basic` — a healthy mini repo — and
`fixtures/rainbow/broken` — one deliberate violation per gate), authored to exercise the codec and
gates self-consistently. It is *not* a substitute for real client data:

> **`treesmith census` is the P0 fidelity harness. Run it against a real client repo before trusting
> any write path on that repo.** The census parses every serialized item, re-emits it, and
> byte-compares — the single falsifier for the byte-identical round-trip invariant (I2) and for the
> emitter-style assumptions listed in [`DESIGN.md` §15](DESIGN.md). A clean census (`ok: true`, zero
> faults, zero mismatches, exit 0) is the green light for mutations on that repo.

Note the exit-code split when reading census output: `census` reports **fidelity** only. The
`broken` fixture, for instance, is broken at the *gate* level (missing datasources, malformed
layout XML, etc.) but every file still round-trips byte-identically — so `census` on `broken`
returns `ok: true` exit 0, while `validate` returns exit 1 with all seven gates firing. Branch
scripts on this split: **exit 3 = the tree is unreadable; exit 1 = the tree parses but violates a
gate.**

---

## Decisions & deviations

Facts recorded here are load-bearing for anyone taking the project forward; they are stated verbatim
as resolved (or reopened) during the build.

- **(a) Owning-org name is unavailable; naming reopens as an O-item.** `github.com/treesmith` is
  **TAKEN** — a dormant user account created 2014-06 with zero public repos — so the spec §8 O4
  owning-org name is unavailable. The naming decision reopens as an O-item for the owner
  (options: GitHub's dormant-username process, or an alternate org name). Interim home resolved
  by the owner on 2026-07-16: this repository lives at `github.com/smithdak/treesmith` (the
  owner's personal account) until the org question is settled.
- **(b) O3 resolved in-build: custom Rainbow-subset parser + emitter.** Rainbow output is **not
  strict YAML** — plain scalars run verbatim to end-of-line (e.g. `Value: a: b` is a legal plain
  value), which rules out event-stream YAML parsers. treesmith uses a hand-written
  lexical-preservation parser and emitter (the spec's own pre-committed fallback), giving
  byte-identical round-trip by construction. All YAML-engine knowledge is quarantined in
  `treesmith-format`.
- **(c) O6 resolved in-build: hand-rolled JSON-RPC over stdio instead of `rmcp`.** The MCP surface
  is four methods; a newline-delimited JSON-RPC implementation carries **zero async runtime and no
  API-drift risk**. `treesmith-mcp` is the isolated seam — it is the only MCP-aware code, so `rmcp`
  can be adopted later without touching the kernel or CLI.
- **(d) Structure amendment: `crates/treesmith-kernel` added.** The spec §2 tree lists eight crates;
  we add `treesmith-kernel`, realizing spec §3.1's "query / mutation API" node so that the two
  surfaces (`treesmith-cli`, `treesmith-mcp`) stay thin and **never import each other** — the root
  binary bridges them.
- **(e) Assumption log.** [`DESIGN.md` §15](DESIGN.md) (emitter style rules, field insert-sort
  order, `Type:` stamping set and multilist storage form, braced-upper normalization, SCS-item-codec
  equivalence, layout `Path` field GUID, etc.) is **unverified against real client repos** until a
  P0 census runs on one. The synthetic `fixtures/` encode these assumptions self-consistently; the
  census is what confirms or falsifies them on real data.

---

## License

MIT OR Apache-2.0 on the library crates; Apache-2.0 on the `treesmith` binary. See
[`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE`](LICENSE-APACHE).
