# Contributing

treesmith is a nine-crate Rust workspace with one hard architectural rule and one hard behavioral
invariant. Read this page, then [docs/architecture.md](docs/architecture.md) for the layout, and
[`DESIGN.md`](DESIGN.md) when you touch anything with a documented contract — public APIs, JSON
shapes, and codec rules defined there are **binding**.

## Toolchain and build

Rust 1.85+ (edition 2021), stable channel. No other dependencies — the workspace builds to a
single static binary.

```sh
cargo build
cargo test --workspace
```

## Definition of done

CI (`.github/workflows/ci.yml`) runs the same three checks on Ubuntu and Windows; run them locally
before pushing:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All three must be clean. Clippy warnings are errors; there is no allowlist.

## The rules that are not up for debate

1. **Dependency direction.** `types ← format ← graph ← template ← presentation ← gate ← kernel ←
   {cli, mcp} ← root binary`. No cycles; `treesmith-cli` and `treesmith-mcp` never import each
   other (the root binary bridges them). If your change needs an upward dependency, the change is
   in the wrong crate.
2. **Quarantines.** Nothing outside `treesmith-format` names Rainbow/SCS/Unicorn/YAML; nothing
   outside `treesmith-mcp` knows MCP exists.
3. **Determinism.** No wall clock, network, or randomness in parse/resolve/gate paths. The only
   exceptions are `census`'s `elapsedMs` and `forge`'s random GUID. New outputs must be
   deterministically ordered.
4. **Round-trip fidelity.** `emit(parse(bytes)) == bytes` for every file that parses. Any codec
   change must keep the corpus walker green, and any newly supported syntax needs a corpus file
   proving it round-trips.
5. **Fail loudly.** Unparseable files are recorded faults, never silently skipped. New error paths
   get a machine-readable `code`, not just a message.

## Working with fixtures

`fixtures/` is the I2 corpus and the gate test bed — synthetic by design, treated as read-only at
runtime:

- **Never mutate `fixtures/` in a test.** Copy the fixture repo to a temp dir first (existing
  mutation tests show the pattern).
- Every fixture file must round-trip byte-identically; the corpus walker test enforces this over
  all sniffable `.yml` files under `fixtures/`.
- Line endings are load-bearing (some files are deliberately CRLF or BOM'd);
  `.gitattributes` pins `fixtures/** -text` so git never normalizes them. Don't let your editor
  "fix" them either.
- Found a real-world serialization pattern the codec mishandles? That's a P0-class finding: add a
  minimal corpus file reproducing it and note which `DESIGN.md` §15 assumption it falsifies.
- `fixtures/rainbow/broken` contains **exactly one violation per gate** — keep it that way; gate
  tests count on it.

## Tests

| Layer | Where | Pattern |
|---|---|---|
| Codec rules | `crates/treesmith-format` | Unit tests + the corpus walker |
| Graph | `crates/treesmith-graph` | Tempdir-authored mini-trees, no fixture dependency |
| Semantics, gates, kernel | respective crates | Against `fixtures/rainbow/basic` and `broken` |
| Binary behavior | `tests/` (root package) | Drives the compiled binary: exit codes, JSON shapes, MCP handshake |

Determinism-sensitive features should assert it: run twice, compare JSON with `elapsedMs`
stripped.

## Docs

User-facing behavior changes update the matching page under [docs/](docs/) — command examples
there are captured from real runs, so re-run and re-paste rather than hand-editing outputs.
Contract changes (JSON shapes, codec rules, exit codes) update [`DESIGN.md`](DESIGN.md) first,
since that document is what the code is built against. Deviations from the product spec are
recorded in the [README's decisions log](README.md#decisions--deviations).

## License

Library crates are dual-licensed MIT OR Apache-2.0; the `treesmith` binary is Apache-2.0. Unless
you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work
by you shall be dual-licensed as above, without any additional terms or conditions.
