use crate::models::AppName;
use pest::Parser;
use serde_value::Value;
use std::collections::BTreeMap;
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
                middlewares: vec![TraefikMiddleware::Spec {
                    name: format!("{app_name}-middleware"),
                    spec: Value::Map(middlewares),
                }],
            }],
            tls: None,
        }
    }

    pub fn with_defaults(app_name: &AppName, service_name: &str) -> Self {
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

        Self {
            entry_points: Vec::new(),
            routes: vec![TraefikRoute {
                rule: TraefikRouterRule::path_prefix_rule(&[app_name.as_str(), service_name]),
                middlewares: vec![TraefikMiddleware::Spec {
                    name: format!("{app_name}-{service_name}-middleware"),
                    spec: Value::Map(middlewares),
                }],
            }],
            tls: None,
        }
    }

    pub fn with_rule(rule: TraefikRouterRule) -> Self {
        Self::with_existing_routing_rules(Vec::new(), rule, Vec::new(), None)
    }

    /// Constructs a new [`TraefikIngressRoute`] that is based on existing list of
    /// [entrypoints](https://doc.traefik.io/traefik/routing/entrypoints/),
    /// [rules and middlewares](https://doc.traefik.io/traefik/routing/routers/), and
    /// existng [TLS cert resolver](https://doc.traefik.io/traefik/routing/routers/#certresolver).
    pub fn with_existing_routing_rules(
        entry_points: Vec<String>,
        rule: TraefikRouterRule,
        middlewares: Vec<String>,
        cert_resolver: Option<String>,
    ) -> Self {
        let middlewares = middlewares
            .into_iter()
            .map(TraefikMiddleware::Ref)
            .collect::<Vec<_>>();

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
    fn path_prefix_from_segments<S>(segments: S) -> String
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
                            domains: ref mut own_domains,
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
                        if let Matcher::PathPrefix {
                            paths: ref mut own_paths,
                        } = own_matches
                        {
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TraefikMiddleware {
    /// This refers to an existing middleware within the cluster
    Ref(String),
    /// This provides a container possible values that are defined [in the Traefik Middleware
    /// specification](https://doc.traefik.io/traefik/middlewares/http/overview/).
    Spec {
        name: String,
        spec: serde_value::Value,
    },
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

                    for (i, domain) in paths.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", `{domain}`")?;
                        } else {
                            write!(f, "`{domain}`")?;
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
                middlewares: vec![TraefikMiddleware::Ref(String::from("traefik-forward-auth"))],
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
                        TraefikMiddleware::Ref(String::from("traefik-forward-auth")),
                        TraefikMiddleware::Spec {
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
                middlewares: vec![TraefikMiddleware::Ref(String::from("traefik-forward-auth"))],
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
                        TraefikMiddleware::Spec {
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
                        },
                        TraefikMiddleware::Ref(String::from("traefik-forward-auth")),
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
    }
}
