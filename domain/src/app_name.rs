use std::collections::HashSet;
use std::ops::Deref;
use std::str::{FromStr, Utf8Error};
use std::sync::OnceLock;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AppName(String);

impl AppName {
    pub fn master() -> Self {
        Self(String::from("master"))
    }

    pub fn into_string(self) -> String {
        self.0
    }

    /// See <https://kubernetes.io/docs/concepts/overview/working-with-objects/names/#dns-label-names>
    pub fn to_rfc1123_namespace_id(&self) -> String {
        self.to_string().to_lowercase()
    }
}

impl<'de> serde::Deserialize<'de> for AppName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let app_name = String::deserialize(deserializer)?;
        Self::from_str(&app_name).map_err(serde::de::Error::custom)
    }
}

impl serde::Serialize for AppName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl Deref for AppName {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::fmt::Display for AppName {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for AppName {
    fn as_ref(&self) -> &str {
        self.0.as_str()
    }
}

static INVALID_CHARS_REGEX: OnceLock<regex::Regex> = OnceLock::new();

impl FromStr for AppName {
    type Err = AppNameError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        let regex = INVALID_CHARS_REGEX.get_or_init(|| regex::Regex::new("(\\s|/|\\.)").unwrap());

        match regex.captures(name) {
            None => Ok(AppName(name.to_string())),
            Some(captures) => {
                let invalid_chars = captures
                    .iter()
                    .flatten()
                    .map(|c| c.as_str())
                    .collect::<HashSet<&str>>()
                    .into_iter()
                    .collect::<Vec<&str>>()
                    .join("");

                Err(AppNameError::InvalidChars { invalid_chars })
            }
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum AppNameError {
    #[error("Invalid characters in app name: “{invalid_chars}” are invalid.")]
    InvalidChars { invalid_chars: String },
    #[error("Invalid url encoded parameter: {err}")]
    InvalidUrlDecodedParam { err: String },
}

impl From<Utf8Error> for AppNameError {
    fn from(err: Utf8Error) -> Self {
        AppNameError::InvalidUrlDecodedParam {
            err: format!("{err}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_create_app_name_from_str() {
        let app_name = AppName::from_str("master").unwrap();

        assert_eq!(app_name.0, "master");
    }

    #[test]
    fn should_create_app_name_from_utf_str() {
        let app_name = AppName::from_str("Üß¥$Ω").unwrap();

        assert_eq!(app_name.0, "Üß¥$Ω");
    }

    #[test]
    fn should_not_create_app_name_app_name_contains_whitespaces() {
        let app_name = AppName::from_str(" master\n ");

        assert_eq!(
            app_name,
            Err(AppNameError::InvalidChars {
                invalid_chars: String::from(" "),
            })
        );
    }

    #[test]
    fn should_not_create_app_name_app_name_contains_slashes() {
        let app_name = AppName::from_str("feature/xxx");

        assert_eq!(
            app_name,
            Err(AppNameError::InvalidChars {
                invalid_chars: String::from("/"),
            })
        );
    }

    #[test]
    fn should_not_create_app_name_app_name_contains_dot() {
        let app_name = AppName::from_str("feature.xxx");

        assert_eq!(
            app_name,
            Err(AppNameError::InvalidChars {
                invalid_chars: String::from("."),
            })
        );
    }
}
