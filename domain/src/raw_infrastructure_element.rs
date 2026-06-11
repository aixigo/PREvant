/// Represents an infrastructure items, e.g. a Kubernetes stateful set or a Docker compose type,
/// that needs to be treated by the domain as an opaque value.
#[derive(Debug, PartialEq, Clone)]
pub struct RawInfrastructureElement(serde_json::Value);

impl RawInfrastructureElement {
    pub fn as_json(&self) -> &serde_json::Value {
        &self.0
    }
}

impl From<serde_json::Value> for RawInfrastructureElement {
    fn from(value: serde_json::Value) -> Self {
        Self(value)
    }
}

impl From<RawInfrastructureElement> for serde_json::Value {
    fn from(value: RawInfrastructureElement) -> Self {
        value.0
    }
}
