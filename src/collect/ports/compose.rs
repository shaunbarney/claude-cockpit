//! Static fallback: host ports declared in the repo's *default* Compose files.
//!
//! Only used when none of this repo's containers are running. Deliberately
//! narrow — the files `docker compose up` reads with no `-f`, found via `git
//! ls-files` (so gitignored and vendored trees are skipped) — because variant
//! files (`docker-compose.prod.yml`, `…lan.yml`) describe other deployments
//! and would list ports that some *other* process may well be holding.
//! Dockerfile `EXPOSE` is never consulted: it names container ports, not host
//! ports.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use yaml_rust2::{Yaml, YamlLoader};

use super::DiscoveredPort;
use crate::proc::{stdout_of, DEFAULT_TIMEOUT};

/// A host range wider than this is reported by its first port only.
const MAX_RANGE: u16 = 32;

/// Compose files read by a bare `docker compose up`, plus the override file
/// it merges automatically.
pub fn is_default_compose_file(name: &str) -> bool {
    matches!(
        name,
        "compose.yaml"
            | "compose.yml"
            | "docker-compose.yaml"
            | "docker-compose.yml"
            | "compose.override.yaml"
            | "compose.override.yml"
            | "docker-compose.override.yaml"
            | "docker-compose.override.yml"
    )
}

/// Default Compose files under `root` that git tracks or would track
/// (untracked but not ignored), as absolute paths, sorted.
pub fn find_compose_files(root: &str) -> Vec<PathBuf> {
    if root.is_empty() {
        return Vec::new();
    }
    let out = stdout_of(
        "git",
        &[
            "-C",
            root,
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            "*compose.yml",
            "*compose.yaml",
            "*compose.override.yml",
            "*compose.override.yaml",
        ],
        DEFAULT_TIMEOUT,
    );
    let mut files: Vec<PathBuf> = out
        .split('\0')
        .filter(|p| {
            Path::new(p)
                .file_name()
                .is_some_and(|n| is_default_compose_file(&n.to_string_lossy()))
        })
        .map(|p| Path::new(root).join(p))
        .collect();
    files.sort();
    files.dedup();
    files
}

/// Parse a `.env` file body (`KEY=VALUE`, `export KEY=VALUE`, `# comments`).
pub fn parse_dotenv(text: &str) -> HashMap<String, String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let l = l.strip_prefix("export ").unwrap_or(l);
            let (k, v) = l.split_once('=')?;
            let v = v.trim();
            let v = v
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
                .unwrap_or(v);
            Some((k.trim().to_string(), v.to_string()))
        })
        .collect()
}

/// Expand Compose variable references: `$$`, `$VAR`, `${VAR}`,
/// `${VAR:-default}`, `${VAR-default}`, `${VAR:+alt}`, `${VAR+alt}`,
/// `${VAR:?err}` / `${VAR?err}` (expands to empty when unset). Defaults may
/// themselves contain references.
pub fn interpolate(s: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        if let Some(r) = rest.strip_prefix('$') {
            out.push('$');
            rest = r;
        } else if let Some(r) = rest.strip_prefix('{') {
            let Some(end) = matching_brace(r) else {
                out.push_str("${");
                rest = r;
                continue;
            };
            out.push_str(&expand_braced(&r[..end], lookup));
            rest = &r[end + 1..];
        } else {
            let n = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            if n == 0 {
                out.push('$');
            } else {
                out.push_str(&lookup(&rest[..n]).unwrap_or_default());
            }
            rest = &rest[n..];
        }
    }
    out.push_str(rest);
    out
}

/// Index of the `}` closing a `${`, honouring nested `${…}`.
fn matching_brace(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' if depth == 0 => return Some(i),
            '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

fn expand_braced(body: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let n = body
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(body.len());
    let (name, op) = body.split_at(n);
    let val = lookup(name);
    let set_nonempty = val.as_deref().is_some_and(|v| !v.is_empty());
    let sub = |s: &str| interpolate(s, lookup);
    if let Some(d) = op.strip_prefix(":-") {
        return if set_nonempty { val.unwrap() } else { sub(d) };
    }
    if let Some(d) = op.strip_prefix('-') {
        return val.unwrap_or_else(|| sub(d));
    }
    if let Some(a) = op.strip_prefix(":+") {
        return if set_nonempty { sub(a) } else { String::new() };
    }
    if let Some(a) = op.strip_prefix('+') {
        return if val.is_some() { sub(a) } else { String::new() };
    }
    val.unwrap_or_default()
}

/// Connectable host for a bind address: wildcard binds are probed on loopback.
pub fn connect_host(ip: &str) -> String {
    match ip.trim_matches(|c| c == '[' || c == ']') {
        "" | "0.0.0.0" | "::" | "*" => "127.0.0.1".to_string(),
        other => other.to_string(),
    }
}

/// Expand a host-port spec (`8080` or a range `3000-3002`) to port numbers.
pub fn expand_range(spec: &str) -> Vec<u16> {
    let spec = spec.trim();
    match spec.split_once('-') {
        Some((a, b)) => match (a.trim().parse::<u16>(), b.trim().parse::<u16>()) {
            (Ok(a), Ok(b)) if b >= a && b - a < MAX_RANGE => (a..=b).collect(),
            (Ok(a), _) => vec![a],
            _ => vec![],
        },
        None => spec.parse().ok().into_iter().collect(),
    }
}

/// Host-side `(connect host, port)`s of an (already interpolated) Compose
/// short-syntax entry: `HOST:CTR`, `IP:HOST:CTR`, `[V6]:HOST:CTR`, with
/// optional `/tcp` and ranges. A bare container port (ephemeral host port), an
/// empty host port (`127.0.0.1::80`) or a `/udp` mapping yields nothing.
pub fn parse_short(val: &str) -> Vec<(String, u16)> {
    let val = val.trim().trim_matches(|c| c == '"' || c == '\'');
    let (val, proto) = val.split_once('/').unwrap_or((val, "tcp"));
    if !proto.eq_ignore_ascii_case("tcp") {
        return vec![];
    }
    let (ip, rest) = match val.strip_prefix('[') {
        Some(v6) => match v6.split_once("]:") {
            Some((ip, rest)) => (ip, rest),
            None => return vec![],
        },
        None => match val.matches(':').count() {
            2 => val.split_once(':').unwrap_or(("", val)),
            _ => ("", val),
        },
    };
    let host_spec = match rest.split_once(':') {
        Some((host, _container)) => host,
        None => return vec![], // container port only
    };
    let host = connect_host(ip);
    expand_range(host_spec)
        .into_iter()
        .map(|p| (host.clone(), p))
        .collect()
}

/// A scalar YAML node as text (ports are often unquoted integers).
fn scalar(y: &Yaml) -> Option<String> {
    match y {
        Yaml::String(s) | Yaml::Real(s) => Some(s.clone()),
        Yaml::Integer(i) => Some(i.to_string()),
        _ => None,
    }
}

/// Host ports of one `ports:` list item (short string or long mapping).
fn item_ports(item: &Yaml, lookup: &dyn Fn(&str) -> Option<String>) -> Vec<(String, u16)> {
    match item {
        Yaml::String(s) => parse_short(&interpolate(s, lookup)),
        Yaml::Hash(_) => {
            let get = |k: &str| scalar(&item[k]).map(|s| interpolate(&s, lookup));
            let proto = get("protocol").unwrap_or_else(|| "tcp".into());
            if !proto.eq_ignore_ascii_case("tcp") {
                return vec![];
            }
            let host = connect_host(&get("host_ip").unwrap_or_default());
            get("published")
                .map(|p| expand_range(&p))
                .unwrap_or_default()
                .into_iter()
                .map(|p| (host.clone(), p))
                .collect()
        }
        // A bare integer is a container port with an ephemeral host port.
        _ => vec![],
    }
}

/// A service's `ports`, following a `<<: *anchor` merge when it has none.
fn service_ports(svc: &Yaml) -> Option<&Vec<Yaml>> {
    svc["ports"].as_vec().or_else(|| match &svc["<<"] {
        Yaml::Array(bases) => bases.iter().find_map(|b| b["ports"].as_vec()),
        base => base["ports"].as_vec(),
    })
}

/// Parse host-reachable ports out of a Compose document, labelled by service.
/// `lookup` resolves `${VAR}` references. Invalid YAML yields nothing.
pub fn parse_compose_ports(
    yaml: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Vec<DiscoveredPort> {
    let Ok(docs) = YamlLoader::load_from_str(yaml) else {
        return vec![];
    };
    let mut out = Vec::new();
    for doc in &docs {
        // Legacy v1 files put services at the top level.
        let services = doc["services"].as_hash().or_else(|| doc.as_hash());
        for (name, svc) in services.into_iter().flatten() {
            let (Some(name), Some(ports)) = (name.as_str(), service_ports(svc)) else {
                continue;
            };
            for (host, port) in ports.iter().flat_map(|i| item_ports(i, lookup)) {
                out.push(DiscoveredPort {
                    label: name.to_string(),
                    host,
                    port,
                });
            }
        }
    }
    out
}

/// How deep `include:` chains are followed.
const MAX_INCLUDE_DEPTH: usize = 4;

/// One top-level `include:` entry: file paths plus the env files that
/// supply its variables (both relative to the including file).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Include {
    pub paths: Vec<String>,
    pub env_files: Vec<String>,
}

/// A string or a list of strings.
fn strings(y: &Yaml) -> Vec<String> {
    match y {
        Yaml::String(s) => vec![s.clone()],
        Yaml::Array(v) => v
            .iter()
            .filter_map(|i| i.as_str().map(String::from))
            .collect(),
        _ => vec![],
    }
}

/// Parse a Compose file's top-level `include:` (short `- path` or long
/// `- path: …, env_file: …` forms).
pub fn parse_includes(yaml: &str) -> Vec<Include> {
    let Ok(docs) = YamlLoader::load_from_str(yaml) else {
        return vec![];
    };
    docs.iter()
        .filter_map(|d| d["include"].as_vec())
        .flatten()
        .map(|entry| match entry {
            Yaml::Hash(_) => Include {
                paths: strings(&entry["path"]),
                env_files: strings(&entry["env_file"]),
            },
            other => Include {
                paths: strings(other),
                env_files: vec![],
            },
        })
        .filter(|i| !i.paths.is_empty())
        .collect()
}

fn read_env_files(dir: &Path, names: &[String]) -> HashMap<String, String> {
    let mut env = HashMap::new();
    for n in names {
        if let Ok(t) = std::fs::read_to_string(dir.join(n)) {
            env.extend(parse_dotenv(&t));
        }
    }
    env
}

/// Ports of one Compose file and (recursively) the files it `include:`s.
/// Variables resolve from the process environment, then `dotenv`.
fn discover_file(
    file: &Path,
    dotenv: &HashMap<String, String>,
    depth: usize,
    seen: &mut Vec<PathBuf>,
    out: &mut Vec<DiscoveredPort>,
) {
    let canon = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    if depth > MAX_INCLUDE_DEPTH || seen.contains(&canon) {
        return;
    }
    seen.push(canon);
    let Ok(text) = std::fs::read_to_string(file) else {
        return;
    };
    let lookup = |k: &str| std::env::var(k).ok().or_else(|| dotenv.get(k).cloned());
    out.extend(parse_compose_ports(&text, &lookup));
    let dir = file.parent().unwrap_or(Path::new("."));
    for inc in parse_includes(&text) {
        let env_names = if inc.env_files.is_empty() {
            vec![".env".to_string()]
        } else {
            inc.env_files.clone()
        };
        let env = read_env_files(dir, &env_names);
        for p in &inc.paths {
            discover_file(
                &dir.join(interpolate(p, &lookup)),
                &env,
                depth + 1,
                seen,
                out,
            );
        }
    }
}

/// Ports from every default Compose file under `root` (following
/// `include:`). Variables resolve from the process environment first, then
/// the `.env` beside each file (or an include's `env_file`).
pub fn discover(root: &str) -> Vec<DiscoveredPort> {
    let mut out = Vec::new();
    let mut seen = Vec::new();
    for file in find_compose_files(root) {
        let dir = file.parent().unwrap_or(Path::new("."));
        let dotenv = read_env_files(dir, &[".env".to_string()]);
        discover_file(&file, &dotenv, 0, &mut seen, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| m.get(k).cloned()
    }

    fn pairs(v: &[DiscoveredPort]) -> Vec<(&str, &str, u16)> {
        v.iter()
            .map(|d| (d.label.as_str(), d.host.as_str(), d.port))
            .collect()
    }

    #[test]
    fn interpolation_forms() {
        let e = env(&[("SET", "1234"), ("EMPTY", "")]);
        assert_eq!(interpolate("${SET}", &e), "1234");
        assert_eq!(interpolate("$SET:80", &e), "1234:80");
        assert_eq!(interpolate("${UNSET:-3000}", &e), "3000");
        assert_eq!(interpolate("${EMPTY:-3000}", &e), "3000");
        assert_eq!(interpolate("${EMPTY-3000}", &e), "");
        assert_eq!(interpolate("${SET:-3000}", &e), "1234");
        assert_eq!(interpolate("${UNSET:-${SET}}", &e), "1234");
        assert_eq!(interpolate("${SET:+x}${UNSET:+y}", &e), "x");
        assert_eq!(interpolate("${UNSET:?boom}", &e), "");
        assert_eq!(interpolate("$$HOME", &e), "$HOME");
    }

    #[test]
    fn short_syntax_variants() {
        let lo = |p: u16| ("127.0.0.1".to_string(), p);
        assert_eq!(parse_short("8080:80"), vec![lo(8080)]);
        assert_eq!(parse_short("\"127.0.0.1:5000:5000\""), vec![lo(5000)]);
        assert_eq!(parse_short("0.0.0.0:9000:9000/tcp"), vec![lo(9000)]);
        assert_eq!(parse_short("[::1]:8080:80"), vec![("::1".into(), 8080)]);
        assert_eq!(
            parse_short("192.168.1.5:81:80"),
            vec![("192.168.1.5".into(), 81)]
        );
        assert_eq!(
            parse_short("3000-3002:3000-3002"),
            vec![lo(3000), lo(3001), lo(3002)]
        );
        assert_eq!(parse_short("3000"), vec![]);
        assert_eq!(parse_short("127.0.0.1::80"), vec![]);
        assert_eq!(parse_short("53:53/udp"), vec![]);
    }

    #[test]
    fn compose_document_with_env_flow_and_long_syntax() {
        let yaml = r#"
x-base: &base
  ports: ["7000:7000"]
services:
  web:
    image: nginx
    ports:
      - "${PORT:-3000}:3000"
      - "${FE_PORT}:3000"
      - "127.0.0.1:${P:-80}:80"
      - 9000
    environment:
      FOO: bar
  api:
    ports: ["8080:80", "[::1]:8443:443"]
  db:
    ports:
      - target: 5432
        published: "${DB_PORT:-5433}"
        host_ip: 127.0.0.1
        protocol: tcp
      - target: 53
        published: 5353
        protocol: udp
  inherits:
    <<: *base
"#;
        let e = env(&[("FE_PORT", "4000")]);
        let got = parse_compose_ports(yaml, &e);
        let got = pairs(&got);
        assert!(got.contains(&("web", "127.0.0.1", 3000)), "{got:?}");
        assert!(got.contains(&("web", "127.0.0.1", 4000)), "{got:?}");
        assert!(got.contains(&("web", "127.0.0.1", 80)), "{got:?}");
        assert!(got.contains(&("api", "127.0.0.1", 8080)), "{got:?}");
        assert!(got.contains(&("api", "::1", 8443)), "{got:?}");
        assert!(got.contains(&("db", "127.0.0.1", 5433)), "{got:?}");
        assert!(got.contains(&("inherits", "127.0.0.1", 7000)), "{got:?}");
        assert!(!got.iter().any(|(_, _, p)| *p == 9000 || *p == 5353));
        assert!(!got.iter().any(|(l, _, _)| *l == "environment"));
    }

    #[test]
    fn legacy_top_level_services() {
        let got = parse_compose_ports("api:\n  ports:\n    - 4000:4000\n", &env(&[]));
        assert_eq!(pairs(&got), vec![("api", "127.0.0.1", 4000)]);
    }

    #[test]
    fn invalid_yaml_is_empty() {
        assert!(parse_compose_ports("services: [unclosed", &env(&[])).is_empty());
    }

    #[test]
    fn includes_short_and_long() {
        let yaml = "include:\n  - a.yml\n  - path: b.yml\n    env_file: .env.dev\n  - path: [c.yml, d.yml]\n";
        let inc = parse_includes(yaml);
        assert_eq!(inc.len(), 3);
        assert_eq!(inc[0].paths, vec!["a.yml"]);
        assert_eq!(inc[1].env_files, vec![".env.dev"]);
        assert_eq!(inc[2].paths, vec!["c.yml", "d.yml"]);
    }

    #[test]
    fn discover_follows_includes_with_env_file() {
        let dir = std::env::temp_dir().join(format!("cockpit-compose-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("docker-compose.yml"),
            "include:\n  - path: docker-compose.dev.yml\n    env_file: .env.dev\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("docker-compose.dev.yml"),
            "services:\n  web:\n    ports: [\"${COCKPIT_TEST_WEB_PORT:-1}:80\"]\n",
        )
        .unwrap();
        std::fs::write(dir.join(".env.dev"), "COCKPIT_TEST_WEB_PORT=4242\n").unwrap();
        let mut out = Vec::new();
        let env = HashMap::new();
        discover_file(
            &dir.join("docker-compose.yml"),
            &env,
            0,
            &mut Vec::new(),
            &mut out,
        );
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(pairs(&out), vec![("web", "127.0.0.1", 4242)]);
    }

    #[test]
    fn dotenv_parsing() {
        let m = parse_dotenv("# c\nA=1\nexport B=\"two\"\nC='3'\n\nbad line\n");
        assert_eq!(m.get("A").map(String::as_str), Some("1"));
        assert_eq!(m.get("B").map(String::as_str), Some("two"));
        assert_eq!(m.get("C").map(String::as_str), Some("3"));
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn only_default_compose_files() {
        assert!(is_default_compose_file("docker-compose.yml"));
        assert!(is_default_compose_file("compose.yaml"));
        assert!(is_default_compose_file("docker-compose.override.yml"));
        assert!(!is_default_compose_file("docker-compose.prod.yml"));
        assert!(!is_default_compose_file("docker-compose.observability.yml"));
        assert!(!is_default_compose_file("Dockerfile"));
    }
}
