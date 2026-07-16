# Rainbow fixture repos (DESIGN.md §13)

Both repos are part of the I2 corpus: every `.yml` here must round-trip byte-identically through
`treesmith-format` (LF newlines, 2-space indent, dash-at-key-indent lists, canonical key order,
guid-sorted fields, quoted lowercase GUIDs, layout XML in `Type: Layout` block literals with
braced-UPPERCASE GUIDs inside values).

## `basic/` — healthy mini repo

Unserialized root (partial-tree case): `aaaaaaaa-0000-4000-8000-0000000000aa` — the `Parent` of
every top-level serialized item (`Sample`, the four layout items, `Home`). It is the only GUID
referenced as a `Parent` that does not resolve inside the repo. Template/section/field/layout
items use the well-known system template GUIDs from `types::wellknown`.

| item | guid | notes |
|---|---|---|
| templates: Sample (project folder) | 7c1e1c2a-0000-4000-8000-000000000000 | TEMPLATE_FOLDER |
| Page template | 7c1e1c2a-0001-4000-8000-000000000001 | |
| Content section | 7c1e1c2a-0002-4000-8000-000000000002 | |
| Title field | 7c1e1c2a-0003-4000-8000-000000000003 | Single-Line Text, versioned |
| Body field | 7c1e1c2a-0004-4000-8000-000000000004 | Rich Text, versioned |
| NavTitle field | 7c1e1c2a-0005-4000-8000-000000000005 | Single-Line Text, unversioned |
| RelatedPages field | 7c1e1c2a-0006-4000-8000-000000000006 | Treelist, shared |
| Meta template | 7c1e1c2a-0010-4000-8000-000000000010 | |
| SEO section | 7c1e1c2a-0011-4000-8000-000000000011 | |
| Keywords field | 7c1e1c2a-0012-4000-8000-000000000012 | Single-Line Text, shared |
| ArticlePage template | 7c1e1c2a-0020-4000-8000-000000000020 | __Base template = Page \| Meta |
| Page __Standard Values | 7c1e1c2a-0030-4000-8000-000000000030 | shared __Renderings: NavBar + Hero (ds = HeroData) in `main` |
| MainLayout | 9a11aaaa-0001-4000-8000-000000000001 | layout, Path=/Views/Shared/MainLayout.cshtml |
| Hero | 9a11aaaa-0002-4000-8000-000000000002 | view rendering, Path=/Views/Hero.cshtml |
| NavBar | 9a11aaaa-0003-4000-8000-000000000003 | controller rendering, Controller=NavBarController |
| PromoBanner | 9a11aaaa-0004-4000-8000-000000000004 | view rendering, Path=/Views/PromoBanner.cshtml |
| Home | c0ffee00-0001-4000-8000-000000000001 | ArticlePage; en x2 + da x1; final-renderings delta on en v1 (Hero ds → `local:/Data/HeroData`, PromoBanner inserted `p:after` Hero) |
| HeroData | c0ffee00-0002-4000-8000-000000000002 | Page, under Home/Data |
| About | c0ffee00-0003-4000-8000-000000000003 | Page, en only |
| Data folder | c0ffee00-0004-4000-8000-000000000004 | FOLDER |

Rendering `uid`s in the layout XML: NavBar `…1101`, Hero `…1102`, PromoBanner `…1103`
(`11111111-1111-4111-8111-1111111111NN`). The view/controller pointer fields on rendering items
use fixture GUIDs `cccccccc-0001…c1` (Path) and `cccccccc-0002…c2` (Controller); resolution is by
hint (DESIGN §6.4), except MainLayout which uses the well-known LAYOUT_PATH_FIELD.

## `broken/` — one deliberate violation per gate

Standalone repo (own `treesmith.toml` arming G7 with `required = ["en", "da"]`). Unserialized
root: `bbbbbbbb-0000-4000-8000-0000000000bb`. Template `Simple`
(`b0000000-0001-4000-8000-000000000001`) with section `Data` (`…0002`), shared Droplink field
`Link` (`…0003`), versioned Single-Line Text field `Title` (`…0005`), and `__Standard Values`
(`…0004`) whose shared `__Renderings` binds SimpleLayout (`…0010`) with no renderings. View
renderings: SideView (`…0011`, /Views/Side.cshtml, exists), BrokenView (`…0012`,
/Views/Missing.cshtml, missing).

The violations below are the ONLY violations in the repo — everything else is healthy:

| gate | item (guid suffix) | violation |
|---|---|---|
| G1 | Alpha (…0020) | rendering ds = nonexistent `{D0000000-0000-4000-8000-0000000000D0}` |
| G2 | Bravo (…0021) | malformed shared layout XML (unclosed `<d>`) |
| G2 | Charlie (…0022) | final-renderings delta `<r>` with unknown uid and no `id=` |
| G3 | BrokenView (…0012) | view rendering, /Views/Missing.cshtml does not exist |
| G4 | Dee (…0023) | rendering in placeholder `sidebar`, never exposed by any .cshtml |
| G5 | Echo (…0024) | shared `Link` = nonexistent `{E0000000-0000-4000-8000-0000000000E0}` |
| G6 | Foxtrot (…0025) | field `f0000000-0001-4000-8000-0000000000f1` not in template; shared-declared `Link` serialized under en v1 Versions (value valid: Dee) |
| G7 | Golf (…0026) | en version only; policy requires en + da |

Charlie and Foxtrot carry both en and da versions so Golf is the sole G7 hit; Alpha, Bravo, Dee
and Echo carry no language versions at all (shared fields only) and are therefore G7-exempt.
