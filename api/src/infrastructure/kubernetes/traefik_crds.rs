use domain::traefik::TraefikVersion;
use kube::{CustomResource, api::ObjectMeta};
use schemars::JsonSchema;
use serde_json::Value;

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
pub struct TraefikRuleSpec {
    pub kind: String,
    pub r#match: String,
    pub services: Vec<TraefikRuleService>,
    pub middlewares: Option<Vec<TraefikRuleMiddlewareRef>>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
pub struct TraefikRuleService {
    pub kind: Option<String>,
    pub name: String,
    pub port: Option<u16>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
pub struct TraefikRuleMiddlewareRef {
    pub name: String,
    pub namespace: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraefikTls {
    pub cert_resolver: Option<String>,
}

#[derive(Clone, Serialize, Debug, PartialEq)]
#[serde(untagged)]
pub enum TraefikMiddleware {
    V2(v2::Middleware),
    V3(v3::Middleware),
}

impl TraefikMiddleware {
    pub fn metadata_mut(&mut self) -> &mut ObjectMeta {
        match self {
            Self::V2(middleware) => &mut middleware.metadata,
            Self::V3(middleware) => &mut middleware.metadata,
        }
    }

    pub fn metadata(&self) -> &ObjectMeta {
        match self {
            Self::V2(middleware) => &middleware.metadata,
            Self::V3(middleware) => &middleware.metadata,
        }
    }

    pub fn new(
        traefik_version: domain::traefik::TraefikVersion,
        metadata: ObjectMeta,
        spec: Value,
    ) -> TraefikMiddleware {
        match traefik_version {
            TraefikVersion::V1 => {
                unreachable!("Traefik V1 has never been implemented")
            }
            TraefikVersion::V2 => Self::V2(v2::Middleware {
                metadata,
                spec: v2::MiddlewareSpec(spec),
            }),
            TraefikVersion::V3 => Self::V3(v3::Middleware {
                metadata,
                spec: v3::MiddlewareSpec(spec),
            }),
        }
    }

    pub fn spec(&self) -> &Value {
        match self {
            Self::V2(middleware) => &middleware.spec.0,
            Self::V3(middleware) => &middleware.spec.0,
        }
    }

    pub fn traefik_version(&self) -> TraefikVersion {
        match self {
            Self::V2(_) => TraefikVersion::V2,
            Self::V3(_) => TraefikVersion::V3,
        }
    }
}

#[derive(Clone, Serialize, Debug, PartialEq)]
#[serde(untagged)]
pub enum TraefikIngressRoute {
    V2(v2::IngressRoute),
    V3(v3::IngressRoute),
}

impl TraefikIngressRoute {
    pub fn metadata_mut(&mut self) -> &mut ObjectMeta {
        match self {
            Self::V2(ingress) => &mut ingress.metadata,
            Self::V3(ingress) => &mut ingress.metadata,
        }
    }

    pub fn metadata(&self) -> &ObjectMeta {
        match self {
            Self::V2(ingress) => &ingress.metadata,
            Self::V3(ingress) => &ingress.metadata,
        }
    }


    pub fn rule_spec_mut(&mut self) -> Option<&mut Vec<TraefikRuleSpec>> {
        match self {
            Self::V2(ingress_route) => ingress_route.spec.routes.as_mut(),
            Self::V3(ingress_route) => ingress_route.spec.routes.as_mut(),
        }
    }

    pub fn rule_spec(&self) -> Option<&Vec<TraefikRuleSpec>> {
        match self {
            Self::V2(ingress_route) => ingress_route.spec.routes.as_ref(),
            Self::V3(ingress_route) => ingress_route.spec.routes.as_ref(),
        }
    }

    pub fn new(
        traefik_version: TraefikVersion,
        metadata: ObjectMeta,
        entry_points: Option<Vec<String>>,
        routes: Option<Vec<TraefikRuleSpec>>,
        tls: Option<TraefikTls>,
    ) -> Self {
        match traefik_version {
            TraefikVersion::V1 => {
                unreachable!("Traefik V1 has never been implemented")
            }
            TraefikVersion::V2 => Self::V2(v2::IngressRoute {
                metadata,
                spec: v2::IngressRouteSpec {
                    entry_points,
                    routes,
                    tls,
                },
            }),
            TraefikVersion::V3 => Self::V3(v3::IngressRoute {
                metadata,
                spec: v3::IngressRouteSpec {
                    entry_points,
                    routes,
                    tls,
                },
            }),
        }
    }

    pub fn routes(&self) -> Option<&Vec<TraefikRuleSpec>> {
        match &self {
            Self::V2(ingress_route) => ingress_route.spec.routes.as_ref(),
            Self::V3(ingress_route) => ingress_route.spec.routes.as_ref(),
        }
    }

    pub fn tls(&self) -> Option<&TraefikTls> {
        match self {
            Self::V2(ingress_route) => ingress_route.spec.tls.as_ref(),
            Self::V3(ingress_route) => ingress_route.spec.tls.as_ref(),
        }
    }

    pub fn entry_points(&self) -> Option<&Vec<String>> {
        match self {
            Self::V2(ingress_route) => ingress_route.spec.entry_points.as_ref(),
            Self::V3(ingress_route) => ingress_route.spec.entry_points.as_ref(),
        }
    }

    pub fn traefik_version(&self) -> TraefikVersion {
        match self {
            Self::V2(_) => TraefikVersion::V2,
            Self::V3(_) => TraefikVersion::V3,
        }
    }
}

pub mod v2 {
    use super::*;

    #[derive(
        CustomResource, Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq,
    )]
    #[kube(
        derive = "PartialEq",
        group = "traefik.containo.us",
        version = "v1alpha1",
        kind = "IngressRoute",
        namespaced
    )]
    #[serde(rename_all = "camelCase")]
    pub struct IngressRouteSpec {
        pub entry_points: Option<Vec<String>>,
        pub routes: Option<Vec<TraefikRuleSpec>>,
        pub tls: Option<TraefikTls>,
    }

    #[derive(CustomResource, Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
    #[kube(
        derive = "PartialEq",
        group = "traefik.containo.us",
        version = "v1alpha1",
        kind = "Middleware",
        namespaced
    )]
    #[serde(rename_all = "camelCase")]
    pub struct MiddlewareSpec(pub Value);
}

pub mod v3 {
    use super::*;

    #[derive(
        CustomResource, Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq,
    )]
    #[kube(
        derive = "PartialEq",
        group = "traefik.io",
        version = "v1alpha1",
        kind = "IngressRoute",
        namespaced
    )]
    #[serde(rename_all = "camelCase")]
    pub struct IngressRouteSpec {
        pub entry_points: Option<Vec<String>>,
        pub routes: Option<Vec<TraefikRuleSpec>>,
        pub tls: Option<TraefikTls>,
    }

    #[derive(CustomResource, Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
    #[kube(
        derive = "PartialEq",
        group = "traefik.io",
        version = "v1alpha1",
        kind = "Middleware",
        namespaced
    )]
    #[serde(rename_all = "camelCase")]
    pub struct MiddlewareSpec(pub Value);
}
