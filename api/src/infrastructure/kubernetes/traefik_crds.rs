use kube::CustomResource;
use schemars::JsonSchema;
use serde_json::Value;

#[derive(CustomResource, Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
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

