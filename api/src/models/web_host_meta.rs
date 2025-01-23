/*-
 * ========================LICENSE_START=================================
 * PREvant REST API
 * %%
 * Copyright (C) 2018 - 2019 aixigo AG
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
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer};
use url::Url;

#[derive(Clone, Debug, Deserialize, Eq, Hash, Serialize, PartialEq)]
pub struct WebHostMeta {
    properties: Option<Properties>,
    links: Option<Vec<Link>>,
    #[serde(default = "valid_web_host")]
    valid: bool,
}

fn valid_web_host() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Serialize, PartialEq)]
struct Properties {
    #[serde(rename = "https://schema.org/softwareVersion")]
    version: Option<String>,
    #[serde(rename = "https://git-scm.com/docs/git-commit")]
    commit: Option<String>,
    #[serde(
        rename = "https://schema.org/dateModified",
        default,
        deserialize_with = "WebHostMeta::parse_date_modified"
    )]
    date_modified: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Serialize, PartialEq)]
struct Link {
    rel: String,
    href: Url,
}

impl WebHostMeta {
    pub fn empty() -> Self {
        WebHostMeta {
            properties: None,
            links: None,
            valid: true,
        }
    }

    pub fn invalid() -> Self {
        WebHostMeta {
            properties: None,
            links: None,
            valid: false,
        }
    }

    pub fn with_version_and_open_api_spec_link(
        version: Option<String>,
        open_api_spec_url: Option<Url>,
    ) -> Self {
        match (version, open_api_spec_url) {
            (None, None) => Self::empty(),
            (None, Some(open_api_spec_url)) => Self {
                properties: None,
                links: Some(vec![Link {
                    rel: String::from("https://github.com/OAI/OpenAPI-Specification"),
                    href: open_api_spec_url,
                }]),
                valid: true,
            },
            (Some(version), None) => Self::with_version(version),
            (Some(version), Some(open_api_spec_url)) => Self {
                properties: Some(Properties {
                    version: Some(version),
                    commit: None,
                    date_modified: None,
                }),
                links: Some(vec![Link {
                    rel: String::from("https://github.com/OAI/OpenAPI-Specification"),
                    href: open_api_spec_url,
                }]),
                valid: true,
            },
        }
    }

    pub fn with_version(version: String) -> Self {
        Self {
            properties: Some(Properties {
                version: Some(version),
                commit: None,
                date_modified: None,
            }),
            links: None,
            valid: true,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.valid
    }

    pub fn is_empty(&self) -> bool {
        self.properties.is_none() && self.links.is_none()
    }

    pub fn version(&self) -> Option<String> {
        match &self.properties {
            None => None,
            Some(properties) => properties.version.clone(),
        }
    }

    pub fn openapi(&self) -> Option<&Url> {
        match self.links.as_ref() {
            None => None,
            Some(links) => links
                .iter()
                .find(|link| link.rel == "https://github.com/OAI/OpenAPI-Specification")
                .map(|link| &link.href),
        }
    }

    pub fn asyncapi(&self) -> Option<&Url> {
        match self.links.as_ref() {
            None => None,
            Some(links) => links
                .iter()
                .find(|link| link.rel == "https://github.com/asyncapi/spec")
                .map(|link| &link.href),
        }
    }

    pub fn commit(&self) -> Option<String> {
        match &self.properties {
            None => None,
            Some(properties) => properties.commit.clone(),
        }
    }

    pub fn date_modified(&self) -> Option<DateTime<Utc>> {
        match &self.properties {
            None => None,
            Some(properties) => properties.date_modified,
        }
    }

    fn parse_date_modified<'de, D>(deserializer: D) -> Result<Option<DateTime<Utc>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(DateTime::<Utc>::deserialize(deserializer).ok())
    }

    pub fn with_base_url(&self, url: &Url) -> WebHostMeta {
        let mut web_host_meta = self.clone();
        if let Some(ref mut links) = web_host_meta.links {
            for link in links {
                link.href = url
                    .join(link.href.path())
                    .expect("invalid urls in web host meta data");
            }
        }
        web_host_meta
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use chrono::TimeZone;

    #[test]
    fn should_parse_meta_without_version_property() {
        let json = r#"{
          "properties":{
            "http://blgx.example.net/ns/ext":null
          }
        }"#;

        let meta = serde_json::from_str::<WebHostMeta>(json).unwrap();

        assert_eq!(meta.version(), None);
    }

    #[test]
    fn should_parse_meta_with_version_property() {
        let json = r#"{
          "properties":{
            "https://schema.org/softwareVersion":"1.3",
            "http://blgx.example.net/ns/ext":null
          }
        }"#;

        let meta = serde_json::from_str::<WebHostMeta>(json).unwrap();

        assert_eq!(meta.version(), Some(String::from("1.3")));
    }

    #[test]
    fn should_parse_meta_without_openapi_property() {
        let json = r#"{
          "properties":{
            "http://blgx.example.net/ns/ext":null
          }
        }"#;

        let meta = serde_json::from_str::<WebHostMeta>(json).unwrap();

        assert_eq!(meta.openapi(), None);
    }

    #[test]
    fn should_parse_meta_without_date_modified_property() {
        let json = r#"{
          "properties":{
            "http://blgx.example.net/ns/ext":null
          }
        }"#;

        let meta = serde_json::from_str::<WebHostMeta>(json).unwrap();

        assert_eq!(meta.date_modified(), None);
    }

    #[test]
    fn should_parse_meta_with_date_modified_property() {
        let json = r#"{
          "properties":{
            "http://blgx.example.net/ns/ext": null,
            "https://schema.org/dateModified": "2019-04-17T19:21:00.000+02:00"
          }
        }"#;

        let meta = serde_json::from_str::<WebHostMeta>(json).unwrap();

        assert_eq!(
            meta.date_modified(),
            Utc.with_ymd_and_hms(2019, 4, 17, 17, 21, 00).single()
        );
    }

    #[test]
    fn should_parse_meta_with_invalid_date_modified_property() {
        let json = r#"{
          "properties":{
            "http://blgx.example.net/ns/ext": null,
            "https://schema.org/dateModified": "random string"
          }
        }"#;

        let meta = serde_json::from_str::<WebHostMeta>(json).unwrap();

        assert_eq!(meta.date_modified(), None,);
    }

    #[test]
    fn should_parse_meta_without_commit_property() {
        let json = r#"{
          "properties":{
            "http://blgx.example.net/ns/ext":null
          }
        }"#;

        let meta = serde_json::from_str::<WebHostMeta>(json).unwrap();

        assert_eq!(meta.commit(), None);
    }

    #[test]
    fn should_parse_meta_with_commit_property() {
        let json = r#"{
          "properties":{
            "https://git-scm.com/docs/git-commit": "43de4c6edf3c7ed93cdf8983f1ea7d73115176cc"
          }
        }"#;

        let meta = serde_json::from_str::<WebHostMeta>(json).unwrap();

        assert_eq!(
            meta.commit(),
            Some(String::from("43de4c6edf3c7ed93cdf8983f1ea7d73115176cc"))
        );
    }

    #[test]
    fn should_parse_meta_with_openapi_property() {
        let json = r#"{
          "links":[{
            "rel": "https://github.com/OAI/OpenAPI-Specification",
            "href":"https://speca.io/speca/petstore-api"
          }]
        }"#;

        let meta = serde_json::from_str::<WebHostMeta>(json).unwrap();

        assert_eq!(
            meta.openapi(),
            Some(&Url::parse("https://speca.io/speca/petstore-api").unwrap())
        );
    }

    #[test]
    fn should_replace_base_url_in_links() {
        let json = r#"{
          "links":[{
            "rel": "https://github.com/OAI/OpenAPI-Specification",
            "href":"https://speca.io/speca/petstore-api"
          }]
        }"#;

        let meta = serde_json::from_str::<WebHostMeta>(json)
            .unwrap()
            .with_base_url(&Url::parse("http://example.com").unwrap());

        assert_eq!(
            meta.openapi(),
            Some(&Url::parse("http://example.com/speca/petstore-api").unwrap())
        );
    }

    #[test]
    fn should_parse_meta_with_asyncapi_property() {
        let json = r#"{
          "links":[{
            "rel": "https://github.com/asyncapi/spec",
            "href":"https://raw.githubusercontent.com/asyncapi/spec/refs/heads/master/examples/streetlights-kafka-asyncapi.yml"
          }]
        }"#;

        let meta = serde_json::from_str::<WebHostMeta>(json).unwrap();

        assert_eq!(
            meta.asyncapi(),
            Some(&Url::parse("https://raw.githubusercontent.com/asyncapi/spec/refs/heads/master/examples/streetlights-kafka-asyncapi.yml").unwrap())
        );
    }

    #[test]
    fn should_replace_base_url_in_async_links() {
        let json = r#"{
          "links":[{
            "rel": "https://github.com/asyncapi/spec",
            "href":"https://raw.githubusercontent.com/asyncapi/spec/refs/heads/master/examples/streetlights-kafka-asyncapi.yml"
          }]
        }"#;

        let meta = serde_json::from_str::<WebHostMeta>(json)
            .unwrap()
            .with_base_url(&Url::parse("http://example.com").unwrap());

        assert_eq!(
            meta.asyncapi(),
            Some(&Url::parse("http://example.com/asyncapi/spec/refs/heads/master/examples/streetlights-kafka-asyncapi.yml").unwrap())
        );
    }
}
