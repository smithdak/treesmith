//! Query engine over the item graph (DESIGN.md §4).
//!
//! Grammar: whitespace-separated `key:value` terms; a value may contain
//! spaces when wrapped in double quotes (quotes may enclose the whole
//! token or just part of it — they toggle whitespace literalness). All
//! terms must match (AND). Bare terms and unknown keys are usage errors.
//!
//! Keys:
//! - `path:` — glob over the full item path, case-insensitive; `**`
//!   crosses `/`, `*` stays within a segment, `?` is one non-`/` char.
//! - `name:` — glob over the last path segment.
//! - `template:` — GUID in any form, or exact template-item name
//!   (case-insensitive).
//! - `field:` — `field:Name=Value` (exact value in any slot) or
//!   `field:Name` (existence). `Name` matches the field hint
//!   case-insensitively, or the field GUID in any form.
//!
//! Results are always ordered by (path, id) — see [`Query::run`].

use crate::{Graph, ItemNode};
use treesmith_format::FieldRef;
use treesmith_types::Guid;

/// A parsed query: an AND of terms.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    terms: Vec<Term>,
}

#[derive(Clone, Debug, PartialEq)]
enum Term {
    Path(String),
    Name(String),
    Template(TemplateTerm),
    Field {
        name: FieldName,
        value: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
enum TemplateTerm {
    Id(Guid),
    Name(String),
}

#[derive(Clone, Debug, PartialEq)]
struct FieldName {
    raw: String,
    as_guid: Option<Guid>,
}

/// Parses a query expression. `Err` carries a usage message naming the
/// offending term (spec §3.2 exit-2 class).
pub fn parse_query(expr: &str) -> Result<Query, String> {
    let mut terms = Vec::new();
    for token in tokenize(expr)? {
        let Some((key, value)) = token.split_once(':') else {
            return Err(format!(
                "bare term `{token}`: expected key:value (keys: path, name, template, field)"
            ));
        };
        if value.is_empty() {
            return Err(format!("empty value in term `{token}`"));
        }
        let term = match key {
            "path" => Term::Path(value.to_string()),
            "name" => Term::Name(value.to_string()),
            "template" => Term::Template(match Guid::parse(value) {
                Ok(g) => TemplateTerm::Id(g),
                Err(_) => TemplateTerm::Name(value.to_string()),
            }),
            "field" => {
                let (name, val) = match value.split_once('=') {
                    Some((n, v)) => (n, Some(v.to_string())),
                    None => (value, None),
                };
                if name.is_empty() {
                    return Err(format!("empty field name in term `{token}`"));
                }
                Term::Field {
                    name: FieldName {
                        raw: name.to_string(),
                        as_guid: Guid::parse(name).ok(),
                    },
                    value: val,
                }
            }
            other => {
                return Err(format!(
                "unknown query key `{other}` in term `{token}` (keys: path, name, template, field)"
            ))
            }
        };
        terms.push(term);
    }
    Ok(Query { terms })
}

/// Splits on whitespace; a `"` toggles quoting (whitespace becomes
/// literal, quote chars are dropped). Unclosed quote → Err.
fn tokenize(expr: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut saw_any = false;
    for c in expr.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                saw_any = true;
            }
            c if c.is_whitespace() && !in_quote => {
                if saw_any {
                    tokens.push(std::mem::take(&mut current));
                    saw_any = false;
                }
            }
            c => {
                current.push(c);
                saw_any = true;
            }
        }
    }
    if in_quote {
        return Err("unclosed quote in query expression".to_string());
    }
    if saw_any {
        tokens.push(current);
    }
    Ok(tokens)
}

impl Query {
    /// Whether every term matches this node.
    pub fn matches(&self, graph: &Graph, node: &ItemNode) -> bool {
        self.terms.iter().all(|t| term_matches(t, graph, node))
    }

    /// All matching item ids, in the canonical (path, id) order.
    pub fn run(&self, graph: &Graph) -> Vec<Guid> {
        graph
            .ids_by_path()
            .into_iter()
            .filter(|id| graph.get(*id).is_some_and(|n| self.matches(graph, n)))
            .collect()
    }
}

fn term_matches(term: &Term, graph: &Graph, node: &ItemNode) -> bool {
    match term {
        Term::Path(glob) => glob_match(glob, &node.meta.path),
        Term::Name(glob) => glob_match(glob, &node.meta.name),
        Term::Template(TemplateTerm::Id(g)) => node.meta.template == Some(*g),
        Term::Template(TemplateTerm::Name(name)) => node
            .meta
            .template
            .and_then(|t| graph.get(t))
            .is_some_and(|tn| tn.meta.name.eq_ignore_ascii_case(name)),
        Term::Field { name, value } => field_matches(node, name, value.as_deref()),
    }
}

/// Any slot: shared, then every language's unversioned + versioned fields.
fn field_matches(node: &ItemNode, name: &FieldName, value: Option<&str>) -> bool {
    let hit = |f: &FieldRef| -> bool {
        let name_ok = f
            .hint
            .as_deref()
            .is_some_and(|h| h.eq_ignore_ascii_case(&name.raw))
            || name.as_guid == Some(f.id);
        name_ok && value.is_none_or(|v| f.value == v)
    };
    if node.item.shared_fields().iter().any(&hit) {
        return true;
    }
    node.item.languages().iter().any(|lang| {
        lang.unversioned.iter().any(&hit) || lang.versions.iter().any(|(_, fs)| fs.iter().any(&hit))
    })
}

/// Glob match, ASCII case-insensitive. `**` matches any run including
/// `/`; `*` matches a run of non-`/` chars; `?` matches one non-`/`
/// char; everything else is literal.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_ascii_lowercase().chars().collect();
    let t: Vec<char> = text.to_ascii_lowercase().chars().collect();
    glob(&p, &t)
}

fn glob(p: &[char], t: &[char]) -> bool {
    match p.first() {
        None => t.is_empty(),
        Some('*') if p.get(1) == Some(&'*') => {
            let rest = &p[2..];
            (0..=t.len()).any(|i| glob(rest, &t[i..]))
        }
        Some('*') => {
            let rest = &p[1..];
            for i in 0..=t.len() {
                if glob(rest, &t[i..]) {
                    return true;
                }
                if t.get(i) == Some(&'/') {
                    break; // `*` never consumes a `/`
                }
                if i == t.len() {
                    break;
                }
            }
            false
        }
        Some('?') => t.first().is_some_and(|c| *c != '/') && glob(&p[1..], &t[1..]),
        Some(c) => t.first() == Some(c) && glob(&p[1..], &t[1..]),
    }
}
