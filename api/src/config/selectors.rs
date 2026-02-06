use crate::models::{AppName, Image};
use regex::Regex;

#[derive(Clone, Debug)]
pub struct AppSelector(Regex);

impl AppSelector {
    pub fn new(app_name: &AppName) -> Self {
        Self(Regex::new(&format!("^{app_name}$")).unwrap())
    }

    pub fn matches(&self, app_name: &AppName) -> bool {
        match self.0.captures(app_name) {
            None => false,
            Some(captures) => captures.get(0).map_or("", |m| m.as_str()) == app_name.as_str(),
        }
    }
}

impl Default for AppSelector {
    fn default() -> Self {
        Self(Regex::new(".+").unwrap())
    }
}

impl<'de> serde::Deserialize<'de> for AppSelector {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        serde_regex::deserialize(deserializer).map(Self)
    }
}

impl PartialEq for AppSelector {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_str() == other.0.as_str()
    }
}
impl Eq for AppSelector {}

#[derive(Clone, Debug)]
pub(super) struct ImageSelector(Regex);

impl ImageSelector {
    pub fn matches(&self, image: &Image) -> bool {
        let image = image.to_string();
        match self.0.captures(&image) {
            None => false,
            Some(captures) => captures.get(0).map_or("", |m| m.as_str()) == image,
        }
    }
}

impl Default for ImageSelector {
    fn default() -> Self {
        Self(Regex::new(".+").unwrap())
    }
}

impl<'de> serde::Deserialize<'de> for ImageSelector {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        serde_regex::deserialize(deserializer).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_app_name() {
        let selector = AppSelector::new(&AppName::master());

        assert!(selector.matches(&AppName::master()));
    }
}
