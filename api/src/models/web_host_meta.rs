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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WebHostMeta {
    properties: Option<Properties>,
    links: Option<Vec<Link>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Properties {
    #[serde(rename = "https://schema.org/softwareVersion")]
    version: Option<String>,
    #[serde(rename = "https://git-scm.com/docs/git-commit")]
    commit: Option<String>,
    #[serde(rename = "https://schema.org/dateModified")]
    date_modified: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Link {
    rel: String,
    href: String,
}

impl WebHostMeta {
    pub fn empty() -> Self {
        WebHostMeta {
            properties: None,
            links: None,
        }
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

    pub fn openapi(&self) -> Option<String> {
        match &self.links {
            None => None,
            Some(links) => links
                .iter()
                .find(|link| link.rel == "https://github.com/OAI/OpenAPI-Specification")
                .and_then(|link| Some(link.href.clone())),
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
            Some(properties) => properties.date_modified.clone(),
        }
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
            Some(Utc.ymd(2019, 4, 17).and_hms(17, 21, 00))
        );
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
            Some(String::from("https://speca.io/speca/petstore-api"))
        );
    }
}
