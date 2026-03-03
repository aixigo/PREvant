use serde::{Deserialize, Serialize};
use std::{fmt::Display, str::FromStr};

#[derive(Clone, Debug, Deserialize, Eq, Serialize, PartialEq)]
pub enum AppStatus {
    #[serde(rename = "deployed")]
    Deployed,
    #[serde(rename = "backed-up")]
    BackedUp,
}

#[derive(Debug, Default, Deserialize, Clone, Copy, Eq, Hash, PartialEq, Serialize)]
pub enum ContainerType {
    #[serde(rename = "instance")]
    #[default]
    Instance,
    #[serde(rename = "replica")]
    Replica,
    #[serde(rename = "app-companion")]
    ApplicationCompanion,
    #[serde(rename = "service-companion")]
    ServiceCompanion,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ContainerTypeParseError {
    #[error(
        "The {label} is unknown type. Use instance, replica, app-companion, or service-companion instead"
    )]
    Unknow { label: String },
}

impl FromStr for ContainerType {
    type Err = ContainerTypeParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "replica" => Ok(ContainerType::Replica),
            "instance" => Ok(ContainerType::Instance),
            "app-companion" => Ok(ContainerType::ApplicationCompanion),
            "service-companion" => Ok(ContainerType::ServiceCompanion),
            label => Err(ContainerTypeParseError::Unknow {
                label: String::from(label),
            }),
        }
    }
}

impl Display for ContainerType {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ContainerType::Instance => write!(f, "instance"),
            ContainerType::Replica => write!(f, "replica"),
            ContainerType::ApplicationCompanion => write!(f, "app-companion"),
            ContainerType::ServiceCompanion => write!(f, "service-companion"),
        }
    }
}
