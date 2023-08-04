/*-
 * ========================LICENSE_START=================================
 * PREvant REST API
 * %%
 * Copyright (C) 2018 - 2020 aixigo AG
 * %%
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
 * THE SOFTWARE.
 * =========================LICENSE_END==================================
 */
use crate::config::AppSelector;
use base64::{Engine, engine::general_purpose};
use secstr::SecUtf8;
use serde::{de, Deserialize, Deserializer};
use std::path::PathBuf;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Secret {
    name: String,
    #[serde(deserialize_with = "Secret::parse_secstr", rename = "data")]
    secret: SecUtf8,
    #[serde(default = "AppSelector::default")]
    app_selector: AppSelector,
    path: Option<PathBuf>,
}

impl Secret {
    fn parse_secstr<'de, D>(deserializer: D) -> Result<SecUtf8, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secret = String::deserialize(deserializer)?;
        let decoded = general_purpose::STANDARD.decode(&secret).map_err(de::Error::custom)?;
        let sec_value = String::from_utf8(decoded).map_err(de::Error::custom)?;
        Ok(SecUtf8::from(sec_value))
    }

    pub fn matches_app_name(&self, app_name: &str) -> bool {
        self.app_selector.matches(app_name)
    }
}

impl Into<(PathBuf, SecUtf8)> for Secret {
    fn into(self) -> (PathBuf, SecUtf8) {
        let name = self.name;
        (
            self.path
                .map(|path| path.join(&name))
                .unwrap_or(PathBuf::from(format!("/run/secrets/{}", &name))),
            self.secret,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! secret_from_str {
        ( $config_str:expr ) => {
            toml::de::from_str::<Secret>($config_str).unwrap()
        };
    }

    #[test]
    fn should_parse_secret_with_required_fields() {
        let secret = secret_from_str!(
            r#"
            name = "user"
            data = "SGVsbG8="
        "#
        );

        assert_eq!(&secret.name, "user");
        assert_eq!(&secret.secret.into_unsecure(), "Hello");
    }

    #[test]
    fn should_not_parse_secret_because_of_invalid_secret_data() {
        let config_str = r#"
            name = "user"
            data = "+++"
        "#;

        let parse_result = toml::de::from_str::<Secret>(config_str);
        assert!(parse_result.is_err(), "should not parse secret");
    }

    #[test]
    fn should_convert_into_pathbuf_and_value() {
        let secret = secret_from_str!(
            r#"
            name = "user"
            data = "SGVsbG8="
        "#
        );

        let (path, v) = secret.into();

        assert_eq!(path.as_os_str().to_str().unwrap(), "/run/secrets/user");
        assert_eq!(&v.into_unsecure(), "Hello");
    }

    #[test]
    fn should_convert_into_pathbuf_and_value_with_path() {
        let secret = secret_from_str!(
            r#"
            name = "user"
            data = "SGVsbG8="
            path = "/opt"
        "#
        );

        let (path, v) = secret.into();

        assert_eq!(path.as_os_str().to_str().unwrap(), "/opt/user");
        assert_eq!(&v.into_unsecure(), "Hello");
    }
}
