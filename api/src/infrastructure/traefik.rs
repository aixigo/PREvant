use crate::config::TraefikVersion;
use crate::models::{AppName, Image};
use pest::Parser;
use serde_value::Value;
use std::collections::{BTreeMap, VecDeque};
use std::{fmt::Display, str::FromStr};
use url::Url;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraefikIngressRoute {
    entry_points: Vec<String>,
    routes: Vec<TraefikRoute>,
    tls: Option<TraefikTLS>,
}

impl TraefikIngressRoute {
    pub fn entry_points(&self) -> &Vec<String> {
        &self.entry_points
    }

    pub fn routes(&self) -> &Vec<TraefikRoute> {
        &self.routes
    }

    pub fn tls(&self) -> &Option<TraefikTLS> {
        &self.tls
    }

    #[cfg(test)]
    pub fn empty() -> Self {
        Self {
            entry_points: Vec::new(),
            routes: Vec::new(),
            tls: None,
        }
    }

    pub fn with_app_only_defaults(app_name: &AppName) -> Self {
        let mut prefixes = BTreeMap::new();
        prefixes.insert(
            Value::String(String::from("prefixes")),
            Value::Seq(vec![Value::String(format!("/{app_name}/",))]),
        );

        let mut middlewares = BTreeMap::new();
        middlewares.insert(
            Value::String(String::from("stripPrefix")),
            Value::Map(prefixes),
        );

        Self {
            entry_points: Vec::new(),
            routes: vec![TraefikRoute {
                rule: TraefikRouterRule::path_prefix_rule([app_name.as_str()]),
                middlewares: vec![TraefikMiddleware {
                    name: format!("{}-middleware", app_name.to_rfc1123_namespace_id()),
                    spec: Value::Map(middlewares),
                }],
            }],
            tls: None,
        }
    }

    pub fn with_defaults(app_name: &AppName, service_name: &str) -> Self {
        Self::with_defaults_and_additional_middleware(app_name, service_name, std::iter::empty())
    }

    pub fn with_defaults_and_additional_middleware<I>(
        app_name: &AppName,
        service_name: &str,
        additional_middlewares: I,
    ) -> Self
    where
        I: IntoIterator<Item = TraefikMiddleware>,
    {
        let mut prefixes = BTreeMap::new();
        prefixes.insert(
            Value::String(String::from("prefixes")),
            Value::Seq(vec![Value::String(format!("/{app_name}/{service_name}/",))]),
        );

        let mut middlewares = BTreeMap::new();
        middlewares.insert(
            Value::String(String::from("stripPrefix")),
            Value::Map(prefixes),
        );

        let mut middlewares = vec![TraefikMiddleware {
            name: format!("{app_name}-{service_name}-middleware"),
            spec: Value::Map(middlewares),
        }];
        middlewares.extend(additional_middlewares);

        Self {
            entry_points: Vec::new(),
            routes: vec![TraefikRoute {
                rule: TraefikRouterRule::path_prefix_rule(&[app_name.as_str(), service_name]),
                middlewares,
            }],
            tls: None,
        }
    }

    pub fn with_rule(rule: TraefikRouterRule) -> Self {
        Self::with_existing_routing_rules(Vec::new(), rule, Vec::new(), None)
    }

    pub fn with_rule_and_middlewares(
        rule: TraefikRouterRule,
        middlewares: Vec<TraefikMiddleware>,
    ) -> Self {
        Self::with_existing_routing_rules(Vec::new(), rule, middlewares, None)
    }

    /// Constructs a new [`TraefikIngressRoute`] that is based on existing list of
    /// [entrypoints](https://doc.traefik.io/traefik/routing/entrypoints/),
    /// [rules and middlewares](https://doc.traefik.io/traefik/routing/routers/), and
    /// existng [TLS cert resolver](https://doc.traefik.io/traefik/routing/routers/#certresolver).
    pub fn with_existing_routing_rules(
        entry_points: Vec<String>,
        rule: TraefikRouterRule,
        middlewares: Vec<TraefikMiddleware>,
        cert_resolver: Option<String>,
    ) -> Self {
        Self {
            entry_points,
            routes: vec![TraefikRoute { rule, middlewares }],
            tls: cert_resolver.map(|cert_resolver| TraefikTLS { cert_resolver }),
        }
    }

    pub fn merge_with(&mut self, other: Self) {
        self.entry_points.extend(other.entry_points);

        // FIXME: at the moment there is no handling of multiple routes which needs to be addessed
        // in the future when it is required.
        match (
            self.routes.iter_mut().next(),
            other.routes.into_iter().next(),
        ) {
            (None, None) => {}
            (None, Some(route)) => {
                self.routes.push(route);
            }
            (Some(_), None) => {}
            (Some(route1), Some(route2)) => {
                route1.rule.merge_with(route2.rule);
                route1.middlewares.extend(route2.middlewares);
            }
        };

        self.tls = match (self.tls.take(), other.tls) {
            (None, None) => None,
            (Some(tls), None) => Some(tls),
            (None, Some(tls)) => Some(tls),
            (Some(_), Some(tls)) => Some(tls),
        };
    }

    pub fn to_url(&self) -> Option<Url> {
        let mut domain = None;
        let mut path = None;

        match self.routes.first() {
            Some(route) => {
                let rule = &route.rule;
                for m in &rule.matches {
                    match m {
                        Matcher::Host { domains } => {
                            domain = Some(&domains[0]);
                        }
                        Matcher::PathPrefix { paths } => {
                            path = Some(&paths[0]);
                        }
                        _ => {}
                    }
                }
            }
            None => return None,
        }

        let scheme = if self.tls.is_some()
            || self
                .entry_points
                .iter()
                .any(|entry_point| entry_point == "websecure")
        {
            "https"
        } else {
            "http"
        };

        Url::parse(&format!(
            "{scheme}://{}{}",
            domain?,
            path.as_ref().map(|p| p.as_str()).unwrap_or_default()
        ))
        .ok()
    }
}

impl From<Url> for TraefikIngressRoute {
    fn from(url: Url) -> Self {
        Self::from(&url)
    }
}

impl From<&Url> for TraefikIngressRoute {
    fn from(url: &Url) -> Self {
        let mut matches = Vec::with_capacity(2);
        matches.push(Matcher::Host {
            domains: vec![match (url.host(), url.port()) {
                (None, _) => panic!("URLs in the context of PREvant should have an host entry"),
                (Some(host), None) => host.to_string(),
                (Some(host), Some(port)) => format!("{host}:{port}"),
            }],
        });

        if url.path() != "/" {
            matches.push(Matcher::PathPrefix {
                paths: vec![url.path().to_string()],
            });
        }

        Self {
            entry_points: match url.scheme() {
                "https" => vec![String::from("websecure")],
                _ => Vec::new(),
            },
            routes: vec![TraefikRoute {
                rule: TraefikRouterRule { matches },
                middlewares: Vec::new(),
            }],
            tls: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraefikRoute {
    rule: TraefikRouterRule,
    middlewares: Vec<TraefikMiddleware>,
}

impl TraefikRoute {
    pub fn rule(&self) -> &TraefikRouterRule {
        &self.rule
    }

    pub fn middlewares(&self) -> &Vec<TraefikMiddleware> {
        &self.middlewares
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TraefikRouterRule {
    matches: Vec<Matcher>,
}

impl TraefikRouterRule {
    pub fn path_prefix_from_segments<S>(segments: S) -> String
    where
        S: IntoIterator,
        S::Item: AsRef<str>,
    {
        let mut base = Url::parse("https://example.com").expect(
            "This should never happen: the URL should be parsebale because it is fixed string",
        );

        let segments = segments.into_iter().flat_map(|seg| {
            Url::parse(&format!(
                "https://example.com/{}",
                seg.as_ref().trim_matches('/')
            ))
            .unwrap()
            .path_segments()
            .unwrap()
            .map(|s| s.to_owned())
            .collect::<Vec<_>>()
        });

        base.path_segments_mut()
            .expect("URL shoud be a base")
            .extend(segments);

        format!("/{}/", base.path().trim_matches('/'))
    }

    #[cfg(test)]
    pub fn host_rule(domains: Vec<String>) -> Self {
        Self {
            matches: vec![Matcher::Host { domains }],
        }
    }

    pub fn path_prefix_rule<S>(segments: S) -> Self
    where
        S: IntoIterator,
        S::Item: AsRef<str>,
    {
        Self {
            matches: vec![Matcher::PathPrefix {
                paths: vec![Self::path_prefix_from_segments(segments)],
            }],
        }
    }

    pub fn first_path_prefix(&self) -> Option<&String> {
        self.matches.iter().find_map(|m| match m {
            Matcher::PathPrefix { paths } => paths.first(),
            _ => None,
        })
    }

    pub fn merge_with(&mut self, other: TraefikRouterRule) {
        for other_match in other.matches {
            match other_match {
                Matcher::Headers { key, value } => {
                    self.matches.push(Matcher::Headers { key, value });
                }
                Matcher::Host {
                    domains: other_domains,
                } => {
                    let mut has_domains = false;
                    for own_matches in self.matches.iter_mut() {
                        if let Matcher::Host {
                            domains: own_domains,
                        } = own_matches
                        {
                            has_domains = true;
                            own_domains.extend(other_domains.iter().cloned());
                        }
                    }

                    if !has_domains {
                        self.matches.push(Matcher::Host {
                            domains: other_domains,
                        });
                    }
                }
                Matcher::PathPrefix { paths: other_paths } => {
                    let mut has_path_prefixes = false;
                    for own_matches in self.matches.iter_mut() {
                        if let Matcher::PathPrefix { paths: own_paths } = own_matches {
                            has_path_prefixes = true;

                            *own_paths = own_paths
                                .iter()
                                .flat_map(|own_path| {
                                    other_paths.iter().map(move |other_path| {
                                        Self::path_prefix_from_segments(&[
                                            own_path.as_str(),
                                            other_path.as_str(),
                                        ])
                                    })
                                })
                                .collect::<Vec<_>>();
                        }
                    }

                    if !has_path_prefixes {
                        self.matches
                            .push(Matcher::PathPrefix { paths: other_paths });
                    }
                }
            }
        }
    }
}

/// This provides a container possible values that are defined [in the Traefik Middleware
/// specification](https://doc.traefik.io/traefik/middlewares/http/overview/).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraefikMiddleware {
    pub name: String,
    pub spec: serde_value::Value,
}

impl TraefikMiddleware {
    pub fn name(&self) -> &String {
        &self.name
    }

    pub fn spec(&self) -> &serde_value::Value {
        &self.spec
    }

    pub fn is_strip_prefix(&self) -> bool {
        matches!(&self.spec,
            serde_value::Value::Map(m)
                if m.get(&serde_value::Value::String(String::from("stripPrefix")))
                    .is_some())
    }

    pub fn to_key_value_spec(&self) -> Vec<(String, String)> {
        let mut elements = Vec::new();
        let mut path = VecDeque::new();

        traverse_and_append(&mut elements, &self.spec, &mut path);

        elements
    }
}

fn traverse_and_append(
    elements: &mut Vec<(String, String)>,
    spec: &serde_value::Value,
    path: &mut VecDeque<String>,
) {
    match spec {
        Value::Unit => {}
        Value::Option(Some(value)) => {
            elements.push((path_to_dot_separated_string(path), value_to_string(value)));
        }
        Value::Map(btree_map) => {
            for (k, v) in btree_map {
                path.push_back(value_to_string(k));
                traverse_and_append(elements, v, path);
                path.pop_back();
            }
        }
        value => {
            elements.push((path_to_dot_separated_string(path), value_to_string(value)));
        }
    }
}

fn path_to_dot_separated_string(path: &VecDeque<String>) -> String {
    path.iter()
        .cloned()
        .reduce(|acc, s| format!("{acc}.{s}"))
        .unwrap_or_default()
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::Bool(v) => format!("{v}"),
        Value::U8(v) => format!("{v}"),
        Value::U16(v) => format!("{v}"),
        Value::U32(v) => format!("{v}"),
        Value::U64(v) => format!("{v}"),
        Value::I8(v) => format!("{v}"),
        Value::I16(v) => format!("{v}"),
        Value::I32(v) => format!("{v}"),
        Value::I64(v) => format!("{v}"),
        Value::F32(v) => format!("{v}"),
        Value::F64(v) => format!("{v}"),
        Value::Char(v) => format!("{v}"),
        Value::String(v) => format!("{v}"),
        Value::Option(value) if value.is_some() => {
            // SAFETY: match case is protected by is_some()
            value_to_string(unsafe { value.as_ref().unwrap_unchecked() })
        }
        Value::Newtype(value) => value_to_string(value),
        Value::Seq(values) => values
            .iter()
            .map(|v| value_to_string(v))
            .reduce(|acc, s| format!("{acc},{s}"))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

#[derive(pest_derive::Parser)]
#[grammar_inline = r#"
ident = { (ASCII_ALPHANUMERIC | PUNCTUATION)+ }

Root = _{ (Headers | Host | PathPrefix) ~ ( " "* ~ "&&" ~ " "* ~ (Headers | Host | PathPrefix))* }

Headers = { "Headers" ~ "(`" ~ ident ~ "`, `" ~ ident ~ "`)"  }
Host = { "Host" ~ "(`" ~ ident ~ ("`, `" ~ ident)* ~ "`)" }
PathPrefix = { "PathPrefix" ~ "(`" ~ ident ~ ("`, `" ~ ident)* ~ "`)" }
"#]
struct RuleParser;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Matcher {
    Headers { key: String, value: String },
    Host { domains: Vec<String> },
    PathPrefix { paths: Vec<String> },
}

impl FromStr for TraefikRouterRule {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut rule = TraefikRouterRule {
            matches: Vec::new(),
        };

        let pairs = RuleParser::parse(Rule::Root, s).map_err(|e| e.to_string())?;

        for pair in pairs {
            match pair.as_rule() {
                Rule::Headers => {
                    let key_and_value = pair
                        .into_inner()
                        .map(|pair| pair.as_str())
                        .collect::<Vec<_>>();

                    rule.matches.push(Matcher::Headers {
                        key: key_and_value[0].to_string(),
                        value: key_and_value[1].to_string(),
                    });
                }
                Rule::Host => {
                    let domains = pair
                        .into_inner()
                        .map(|pair| pair.as_str().to_string())
                        .collect::<Vec<_>>();

                    rule.matches.push(Matcher::Host { domains });
                }
                Rule::PathPrefix => {
                    let paths = pair
                        .into_inner()
                        .map(|pair| pair.as_str().to_string())
                        .collect::<Vec<_>>();

                    rule.matches.push(Matcher::PathPrefix { paths });
                }
                Rule::ident | Rule::Root => {}
            }
        }

        Ok(rule)
    }
}

impl Display for TraefikRouterRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, m) in self.matches.iter().enumerate() {
            if i > 0 {
                write!(f, " && ")?;
            }
            match m {
                Matcher::Headers { key, value } => {
                    write!(f, "Headers(`{key}`, `{value}`)")?;
                }
                Matcher::Host { domains } => {
                    write!(f, "Host(")?;

                    for (i, domain) in domains.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", `{domain}`")?;
                        } else {
                            write!(f, "`{domain}`")?;
                        }
                    }

                    write!(f, ")")?;
                }
                Matcher::PathPrefix { paths } => {
                    write!(f, "PathPrefix(")?;

                    for (i, path) in paths.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", `{path}`")?;
                        } else {
                            write!(f, "`{path}`")?;
                        }
                    }

                    write!(f, ")")?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraefikTLS {
    pub cert_resolver: String,
}

impl TryFrom<Image> for TraefikVersion {
    type Error = String;

    fn try_from(image: Image) -> std::result::Result<Self, Self::Error> {
        let Some(tag) = image.tag() else {
            return Err(format!("The image {image} must provide a tag"));
        };

        match tag.as_str() {
            "v2" | "2" => Ok(Self::V2),
            "v3" | "3" => Ok(Self::V3),
            _ if tag.len() >= 2 => {
                let tag = if tag.starts_with("v") {
                    &tag[1..=2]
                } else {
                    &tag[0..=1]
                };

                match tag {
                    "1." => Ok(Self::V1),
                    "2." | "2-" => Ok(Self::V2),
                    "3." | "3-" => Ok(Self::V3),
                    _ => Err(format!("Unknown version tag in {image}")),
                }
            }
            _ => Err(format!("Unknown version tag in {image}")),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn sound_failing() {
        let result = "Random String".parse::<TraefikRouterRule>();

        assert!(std::matches!(result, Err(_)));
    }

    #[test]
    fn parse_header_rule() {
        let rule = "Headers(`Host`, `example.com`)"
            .parse::<TraefikRouterRule>()
            .unwrap();

        assert_eq!(
            rule,
            TraefikRouterRule {
                matches: vec![Matcher::Headers {
                    key: String::from("Host"),
                    value: String::from("example.com"),
                }]
            }
        );
    }

    #[test]
    fn parse_host_rule() {
        let rule = "Host(`example.com`)".parse::<TraefikRouterRule>().unwrap();

        assert_eq!(
            rule,
            TraefikRouterRule {
                matches: vec![Matcher::Host {
                    domains: vec![String::from("example.com")]
                }]
            }
        )
    }

    #[test]
    fn parse_hosts_rule() {
        let rule = "Host(`example.com`, `api.example.com`)"
            .parse::<TraefikRouterRule>()
            .unwrap();

        assert_eq!(
            rule,
            TraefikRouterRule {
                matches: vec![Matcher::Host {
                    domains: vec![String::from("example.com"), String::from("api.example.com")]
                }]
            }
        )
    }

    #[test]
    fn parse_path_prefix_rule() {
        let rule = "PathPrefix(`/test`)".parse::<TraefikRouterRule>().unwrap();

        assert_eq!(
            rule,
            TraefikRouterRule {
                matches: vec![Matcher::PathPrefix {
                    paths: vec![String::from("/test")]
                }]
            }
        )
    }

    #[test]
    fn parse_path_prefixes_rules() {
        let rule = "PathPrefix(`/articles`, `/products`)"
            .parse::<TraefikRouterRule>()
            .unwrap();

        assert_eq!(
            rule,
            TraefikRouterRule {
                matches: vec![Matcher::PathPrefix {
                    paths: vec![String::from("/articles"), String::from("/products")]
                }]
            }
        )
    }

    #[test]
    fn parse_path_prefix_and_host_rule() {
        let rule = "PathPrefix(`/articles`) && Host(`example.com`)"
            .parse::<TraefikRouterRule>()
            .unwrap();

        assert_eq!(
            rule,
            TraefikRouterRule {
                matches: vec![
                    Matcher::PathPrefix {
                        paths: vec![String::from("/articles")]
                    },
                    Matcher::Host {
                        domains: vec![String::from("example.com")]
                    }
                ]
            }
        )
    }

    #[test]
    fn display_path_prefixes() {
        let rule = "PathPrefix(`/articles`, `/products`)"
            .parse::<TraefikRouterRule>()
            .unwrap();

        assert_eq!(&rule.to_string(), "PathPrefix(`/articles`, `/products`)");
    }

    #[test]
    fn display_host() {
        let rule = TraefikRouterRule::host_rule(vec![
            String::from("example.com"),
            String::from("api.example.com"),
        ]);

        assert_eq!(&rule.to_string(), "Host(`example.com`, `api.example.com`)");
    }

    #[test]
    fn display_headers() {
        let rule = "Headers(`Host`, `example.com`)"
            .parse::<TraefikRouterRule>()
            .unwrap();

        assert_eq!(&rule.to_string(), "Headers(`Host`, `example.com`)");
    }

    #[test]
    fn display_host_and_path_prefix() {
        let rule = TraefikRouterRule {
            matches: vec![
                Matcher::Host {
                    domains: vec![String::from("example.com")],
                },
                Matcher::PathPrefix {
                    paths: vec![String::from("/test")],
                },
            ],
        };

        assert_eq!(
            &rule.to_string(),
            "Host(`example.com`) && PathPrefix(`/test`)"
        );
    }

    #[test]
    fn merge_path_prefix_rules() {
        let mut base_prefix_rule = "PathPrefix(`/base`)".parse::<TraefikRouterRule>().unwrap();
        let path_prefix_rule = "PathPrefix(`/test`)".parse::<TraefikRouterRule>().unwrap();

        base_prefix_rule.merge_with(path_prefix_rule);

        assert_eq!(
            Ok(base_prefix_rule),
            "PathPrefix(`/base/test/`)".parse::<TraefikRouterRule>(),
        );
    }

    #[test]
    fn merge_host_path_prefix_rules() {
        let mut host_rule = "Host(`example.com`)".parse::<TraefikRouterRule>().unwrap();
        let path_prefix_rule = "PathPrefix(`/test`)".parse::<TraefikRouterRule>().unwrap();

        host_rule.merge_with(path_prefix_rule);

        assert_eq!(
            host_rule,
            TraefikRouterRule {
                matches: vec![
                    Matcher::Host {
                        domains: vec![String::from("example.com")],
                    },
                    Matcher::PathPrefix {
                        paths: vec![String::from("/test")]
                    }
                ]
            }
        );
    }

    #[test]
    fn merge_host_rules() {
        let mut host_rule1 = "Host(`example.com`)".parse::<TraefikRouterRule>().unwrap();
        let host_rule2 = "Host(`sub.example.com`)"
            .parse::<TraefikRouterRule>()
            .unwrap();

        host_rule1.merge_with(host_rule2);

        assert_eq!(
            host_rule1,
            TraefikRouterRule {
                matches: vec![Matcher::Host {
                    domains: vec![String::from("example.com"), String::from("sub.example.com")],
                },]
            }
        );
    }

    #[test]
    fn merge_swapped_host_path_prefix_rules() {
        let host_rule = "Host(`example.com`)".parse::<TraefikRouterRule>().unwrap();
        let mut path_prefix_rule = "PathPrefix(`/test`)".parse::<TraefikRouterRule>().unwrap();

        path_prefix_rule.merge_with(host_rule);

        assert_eq!(
            path_prefix_rule,
            TraefikRouterRule {
                matches: vec![
                    Matcher::PathPrefix {
                        paths: vec![String::from("/test")]
                    },
                    Matcher::Host {
                        domains: vec![String::from("example.com")],
                    },
                ]
            }
        );
    }

    #[test]
    fn merge_headers_rules() {
        let mut headers_rule_1 = "Headers(`Host`, `example.com`)"
            .parse::<TraefikRouterRule>()
            .unwrap();
        let headers_rule_2 = "Headers(`Content-Type`, `application/json`)"
            .parse::<TraefikRouterRule>()
            .unwrap();

        headers_rule_1.merge_with(headers_rule_2);

        assert_eq!(
            headers_rule_1,
            "Headers(`Host`, `example.com`) && Headers(`Content-Type`, `application/json`)"
                .parse::<TraefikRouterRule>()
                .unwrap()
        );
    }

    #[test]
    fn merge_headers_and_path_prefix_rules() {
        let mut path_prefix_rule = "PathPrefix(`/test`)".parse::<TraefikRouterRule>().unwrap();
        let headers_rule = "Headers(`Host`, `example.com`)"
            .parse::<TraefikRouterRule>()
            .unwrap();

        path_prefix_rule.merge_with(headers_rule);

        assert_eq!(
            path_prefix_rule,
            TraefikRouterRule {
                matches: vec![
                    Matcher::PathPrefix {
                        paths: vec![String::from("/test")]
                    },
                    Matcher::Headers {
                        key: String::from("Host"),
                        value: String::from("example.com"),
                    },
                ]
            }
        );
    }

    #[test]
    fn merge_ingress_routes() {
        let mut route1 = TraefikIngressRoute {
            entry_points: vec![String::from("web")],
            routes: vec![TraefikRoute {
                rule: TraefikRouterRule::host_rule(vec![String::from("prevant.example.com")]),
                middlewares: vec![TraefikMiddleware {
                    name: String::from("traefik-forward-auth"),
                    spec: serde_value::to_value(serde_json::json!({
                        "forwardAuth": {
                            "address": "http://traefik-forward-auth.my-namespace.svc.cluster.local:4181"
                        }
                    })).unwrap()
                }],
            }],
            tls: Some(TraefikTLS {
                cert_resolver: String::from("letsencrypt"),
            }),
        };
        let route2 = TraefikIngressRoute::with_defaults(&AppName::master(), "whoami");

        route1.merge_with(route2);

        assert_eq!(
            route1,
            TraefikIngressRoute {
                entry_points: vec![String::from("web")],
                routes: vec![TraefikRoute {
                    rule: TraefikRouterRule::from_str(
                        "Host(`prevant.example.com`) && PathPrefix(`/master/whoami/`)"
                    )
                    .unwrap(),
                    middlewares: vec![
                        TraefikMiddleware {
                            name: String::from("traefik-forward-auth"),
                            spec: serde_value::to_value(serde_json::json!({
                                "forwardAuth": {
                                    "address": "http://traefik-forward-auth.my-namespace.svc.cluster.local:4181"
                                }
                            })).unwrap()
                        },
                        TraefikMiddleware {
                            name: String::from("master-whoami-middleware"),
                            spec: Value::Map(BTreeMap::from([(
                                Value::String(String::from("stripPrefix")),
                                Value::Map(BTreeMap::from([(
                                    Value::String(String::from("prefixes")),
                                    Value::Seq(vec![Value::String(String::from(
                                        "/master/whoami/"
                                    ))])
                                )]))
                            )]))
                        }
                    ],
                }],
                tls: Some(TraefikTLS {
                    cert_resolver: String::from("letsencrypt"),
                }),
            }
        );
    }

    #[test]
    fn merge_ingress_routes_reverse() {
        let route1 = TraefikIngressRoute {
            entry_points: vec![String::from("web")],
            routes: vec![TraefikRoute {
                rule: TraefikRouterRule::host_rule(vec![String::from("prevant.example.com")]),
                middlewares: vec![TraefikMiddleware {
                    name: String::from("traefik-forward-auth"),
                    spec: serde_value::to_value(serde_json::json!({
                        "forwardAuth": {
                            "address": "http://traefik-forward-auth.my-namespace.svc.cluster.local:4181"
                        }
                    })).unwrap()
                }],
            }],
            tls: Some(TraefikTLS {
                cert_resolver: String::from("letsencrypt"),
            }),
        };
        let mut route2 = TraefikIngressRoute::with_defaults(&AppName::master(), "whoami");

        route2.merge_with(route1);

        assert_eq!(
            route2,
            TraefikIngressRoute {
                entry_points: vec![String::from("web")],
                routes: vec![TraefikRoute {
                    rule: TraefikRouterRule::from_str(
                        "PathPrefix(`/master/whoami/`) && Host(`prevant.example.com`)"
                    )
                    .unwrap(),
                    middlewares: vec![
                        TraefikMiddleware {
                            name: String::from("master-whoami-middleware"),
                            spec: serde_value::to_value(serde_json::json!({
                                "stripPrefix": {
                                    "prefixes": [
                                        "/master/whoami/"
                                    ]
                                }
                            })).unwrap()
                        },
                        TraefikMiddleware {
                            name: String::from("traefik-forward-auth"),
                            spec: serde_value::to_value(serde_json::json!({
                                "forwardAuth": {
                                    "address": "http://traefik-forward-auth.my-namespace.svc.cluster.local:4181"
                                }
                            })).unwrap()
                        },
                    ],
                }],
                tls: Some(TraefikTLS {
                    cert_resolver: String::from("letsencrypt"),
                }),
            }
        );
    }

    #[test]
    fn merge_empty_ingress_routes() {
        let mut route1 = TraefikIngressRoute::empty();
        let route2 = TraefikIngressRoute::empty();

        route1.merge_with(route2);

        assert_eq!(route1, TraefikIngressRoute::empty());
    }

    #[test]
    fn merge_empty_with_none_empty_ingress_routes() {
        let mut route1 = TraefikIngressRoute::empty();

        route1.merge_with(TraefikIngressRoute::with_defaults(
            &AppName::master(),
            "test",
        ));

        assert_eq!(
            route1,
            TraefikIngressRoute::with_defaults(&AppName::master(), "test",)
        );
    }

    #[test]
    fn merge_two_existing_tls_configs() {
        let mut route1 = TraefikIngressRoute::empty();
        route1.tls = Some(TraefikTLS {
            cert_resolver: String::from("first"),
        });
        let mut route2 = TraefikIngressRoute::empty();
        route2.tls = Some(TraefikTLS {
            cert_resolver: String::from("second"),
        });

        route1.merge_with(route2);

        assert_eq!(
            route1,
            TraefikIngressRoute {
                entry_points: Vec::new(),
                routes: Vec::new(),
                tls: Some(TraefikTLS {
                    cert_resolver: String::from("second")
                })
            }
        );
    }

    mod from_url {
        use super::*;

        #[test]
        fn with_host_and_path() {
            let url = Url::parse("http://prevant.example.com/master/whoami/").unwrap();

            assert_eq!(
                TraefikIngressRoute::from(url),
                TraefikIngressRoute::with_rule(
                    TraefikRouterRule::from_str(
                        "Host(`prevant.example.com`) && PathPrefix(`/master/whoami/`)",
                    )
                    .unwrap(),
                )
            );
        }

        #[test]
        fn with_https() {
            let url = Url::parse("https://prevant.example.com/").unwrap();

            let mut route =
                TraefikIngressRoute::with_rule(TraefikRouterRule::host_rule(vec![String::from(
                    "prevant.example.com",
                )]));
            route.entry_points.push(String::from("websecure"));

            assert_eq!(TraefikIngressRoute::from(url), route);
        }
    }

    mod to_url {
        use super::*;

        #[test]
        fn empty_route() {
            assert_eq!(TraefikIngressRoute::empty().to_url(), None);
        }

        #[test]
        fn with_host_rule() {
            let url =
                TraefikIngressRoute::with_rule(TraefikRouterRule::host_rule(vec![String::from(
                    "example.com",
                )]))
                .to_url();

            assert_eq!(url, Url::parse("http://example.com").ok());
        }

        #[test]
        fn with_host_and_path_rule() {
            let url = TraefikIngressRoute::with_rule(
                TraefikRouterRule::from_str(
                    "PathPrefix(`/master/whoami/`) && Host(`prevant.example.com`)",
                )
                .unwrap(),
            )
            .to_url();

            assert_eq!(
                url,
                Url::parse("http://prevant.example.com/master/whoami/").ok()
            );
        }

        #[test]
        fn with_host_rule_and_tls() {
            let mut route =
                TraefikIngressRoute::with_rule(TraefikRouterRule::host_rule(vec![String::from(
                    "example.com",
                )]));
            route.tls = Some(TraefikTLS {
                cert_resolver: String::from("first"),
            });
            let url = route.to_url();

            assert_eq!(url, Url::parse("https://example.com").ok());
        }

        #[test]
        fn with_host_rule_and_websecure_entrypoint() {
            let mut route =
                TraefikIngressRoute::with_rule(TraefikRouterRule::host_rule(vec![String::from(
                    "example.com",
                )]));
            route.entry_points.push(String::from("websecure"));
            let url = route.to_url();

            assert_eq!(url, Url::parse("https://example.com").ok());
        }

        #[test]
        fn with_host_and_port() {
            let url = Url::parse("https://prevant.example.com:8443/").unwrap();

            let route = TraefikIngressRoute::from(url);

            assert_eq!(
                route.to_url(),
                Url::parse("https://prevant.example.com:8443").ok()
            );
        }
    }

    #[test]
    fn use_rfc1123_middleware_name_when_creating_rule_with_app_only_defaults() {
        let route = TraefikIngressRoute::with_app_only_defaults(
            &AppName::from_str("ALL-CAPS-APP-NAME").unwrap(),
        );

        assert_eq!(
            route,
            TraefikIngressRoute {
                entry_points: Vec::new(),
                routes: vec![TraefikRoute {
                    rule: TraefikRouterRule::from_str("PathPrefix(`/ALL-CAPS-APP-NAME/`)").unwrap(),
                    middlewares: vec![TraefikMiddleware {
                        name: String::from("all-caps-app-name-middleware"),
                        spec: serde_value::to_value(serde_json::json!({
                            "stripPrefix": {
                                "prefixes": [
                                    "/ALL-CAPS-APP-NAME/"
                                ]
                            }
                        }))
                        .unwrap()
                    }]
                }],
                tls: None
            }
        )
    }

    #[test]
    fn is_strip_prefix_middleware() {
        let middleware = TraefikMiddleware {
            name: String::from("all-caps-app-name-middleware"),
            spec: serde_value::to_value(serde_json::json!({
                "stripPrefix": {
                    "prefixes": [
                        "/ALL-CAPS-APP-NAME/"
                    ]
                }
            }))
            .unwrap(),
        };

        assert!(middleware.is_strip_prefix());
    }

    #[test]
    fn is_not_strip_prefix_middleware() {
        let middleware = TraefikMiddleware {
            name: String::from("traefik-forward-auth"),
            spec: serde_value::to_value(serde_json::json!({
                "forwardAuth": {
                    "address": "http://traefik-forward-auth.my-namespace.svc.cluster.local:4181"
                }
            }))
            .unwrap(),
        };

        assert!(!middleware.is_strip_prefix());
    }

    mod parse_traefik_version {
        use crate::{config::TraefikVersion, models::Image};
        use rstest::rstest;
        use std::str::FromStr;

        #[rstest]
        #[case("2")]
        #[case("v2")]
        #[case("v2.11.31")]
        #[case("v2-nano-server-ltsc2022")]
        fn parse_v2(#[case] tag: &str) {
            let img = Image::from_str(&format!("traefik:{tag}")).unwrap();
            assert_eq!(TraefikVersion::try_from(img), Ok(TraefikVersion::V2));
        }

        #[rstest]
        #[case("3")]
        #[case("v3")]
        #[case("v3.6.2")]
        #[case("v3.6.2-nanoserver-ltsc2022")]
        #[case("v3-nanoserver-ltsc2022")]
        fn parse_v3(#[case] tag: &str) {
            let img = Image::from_str(&format!("traefik:{tag}")).unwrap();
            assert_eq!(TraefikVersion::try_from(img), Ok(TraefikVersion::V3));
        }
    }

    mod middleware {
        use super::*;

        #[test]
        fn to_key_value_spec() {
            let middleware = TraefikMiddleware {
                name: String::from("traefik-forward-auth"),
                spec: serde_value::to_value(serde_json::json!({
                    "forwardAuth": {
                        "address": "http://traefik-forward-auth.my-namespace.svc.cluster.local:4181"
                    }
                }))
                .unwrap(),
            };

            let kv_spec = middleware.to_key_value_spec();
            assert_eq!(
                kv_spec,
                vec![(
                    String::from("forwardAuth.address"),
                    String::from("http://traefik-forward-auth.my-namespace.svc.cluster.local:4181")
                )]
            )
        }
    }
}
