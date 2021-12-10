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
use crate::models::service::ServiceError;
use regex::Regex;
use serde::ser::{Serialize, Serializer};
use serde::{Deserialize, Deserializer};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

#[derive(Clone, Debug, Eq, Hash)]
pub enum Image {
    Named {
        image_repository: String,
        registry: Option<String>,
        image_user: Option<String>,
        image_tag: Option<String>,
    },
    Digest {
        hash: String,
    },
}

impl PartialEq for Image {
    fn eq(&self, other: &Self) -> bool {
        use Image::*;
        match (self, other) {
            (
                Named {
                    image_repository,
                    registry,
                    image_user,
                    image_tag,
                },
                Named {
                    image_repository: image_repository_other,
                    registry: registry_other,
                    image_user: image_user_other,
                    image_tag: image_tag_other,
                },
            ) => {
                if image_repository != image_repository_other {
                    return false;
                }

                (match (registry, registry_other) {
                    (Some(registry), Some(registry_other)) => registry == registry_other,
                    (None, None) => true,
                    (Some(registry), None) => registry == "docker.io",
                    (None, Some(registry_other)) => registry_other == "docker.io",
                }) && (match (image_user, image_user_other) {
                    (Some(image_user), Some(image_user_other)) => image_user == image_user_other,
                    (None, None) => true,
                    (Some(image_user), None) => image_user == "library",
                    (None, Some(image_user_other)) => image_user_other == "library",
                }) && (match (image_tag, image_tag_other) {
                    (Some(image_tag), Some(image_tag_other)) => image_tag == image_tag_other,
                    (None, None) => true,
                    (Some(image_tag), None) => image_tag == "latest",
                    (None, Some(image_tag_other)) => image_tag_other == "latest",
                })
            }
            (Digest { hash }, Digest { hash: hash_other }) => hash == hash_other,
            _ => false,
        }
    }
}

impl Image {
    pub fn tag(&self) -> Option<String> {
        match &self {
            Image::Digest { .. } => None,
            Image::Named {
                image_repository: _,
                registry: _,
                image_user: _,
                image_tag,
            } => match &image_tag {
                None => Some(String::from("latest")),
                Some(tag) => Some(tag.clone()),
            },
        }
    }

    pub fn name(&self) -> Option<String> {
        match &self {
            Image::Digest { .. } => None,
            Image::Named {
                image_repository,
                registry: _,
                image_user,
                image_tag: _,
            } => {
                let user = match &image_user {
                    None => String::from("library"),
                    Some(user) => user.clone(),
                };

                Some(format!("{}/{}", user, image_repository))
            }
        }
    }

    pub fn registry(&self) -> Option<String> {
        match &self {
            Image::Digest { .. } => None,
            Image::Named {
                image_repository: _,
                registry,
                image_user: _,
                image_tag: _,
            } => Some(
                registry
                    .clone()
                    .unwrap_or_else(|| String::from("docker.io")),
            ),
        }
    }
}

/// Parse a docker image string and returns an image
impl FromStr for Image {
    type Err = ServiceError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut regex = Regex::new(r"^(sha256:)?(?P<id>[a-fA-F0-9]+)$").unwrap();
        if let Some(_captures) = regex.captures(s) {
            return Ok(Image::Digest {
                hash: s.to_string(),
            });
        }

        regex = Regex::new(
            r"^(((?P<registry>.+)/)?(?P<user>[\w-]+)/)?(?P<repo>[\w-]+)(:(?P<tag>[\w\.-]+))?$",
        )
        .unwrap();
        let captures = match regex.captures(s) {
            Some(captures) => captures,
            None => {
                return Err(ServiceError::InvalidImageString {
                    invalid_string: s.to_string(),
                });
            }
        };

        let repo = captures
            .name("repo")
            .map(|m| String::from(m.as_str()))
            .unwrap();
        let registry = captures.name("registry").map(|m| String::from(m.as_str()));
        let user = captures.name("user").map(|m| String::from(m.as_str()));
        let tag = captures.name("tag").map(|m| String::from(m.as_str()));

        Ok(Image::Named {
            image_repository: repo,
            registry,
            image_user: user,
            image_tag: tag,
        })
    }
}

impl Display for Image {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self {
            Image::Digest { hash } => write!(f, "{}", hash),
            Image::Named {
                image_repository,
                registry,
                image_user,
                image_tag,
            } => {
                let registry = match &registry {
                    None => String::from("docker.io"),
                    Some(registry) => registry.clone(),
                };

                let user = match &image_user {
                    None => String::from("library"),
                    Some(user) => user.clone(),
                };

                let tag = match &image_tag {
                    None => "latest".to_owned(),
                    Some(tag) => tag.clone(),
                };

                write!(f, "{}/{}/{}:{}", registry, user, image_repository, tag)
            }
        }
    }
}

impl<'de> Deserialize<'de> for Image {
    fn deserialize<D>(deserializer: D) -> Result<Image, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ImageVisitor;
        impl<'de> serde::de::Visitor<'de> for ImageVisitor {
            type Value = Image;
            fn expecting(&self, formatter: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                write!(formatter, "a string containing a docker image reference")
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Image::from_str(v).map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_string(ImageVisitor)
    }
}

impl Serialize for Image {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn should_parse_image_id_with_sha_prefix() {
        let image = Image::from_str(
            "sha256:9895c9b90b58c9490471b877f6bb6a90e6bdc154da7fbb526a0322ea242fc913",
        )
        .unwrap();

        assert_eq!(
            &image.to_string(),
            "sha256:9895c9b90b58c9490471b877f6bb6a90e6bdc154da7fbb526a0322ea242fc913"
        );
        assert_eq!(image.name(), None);
        assert_eq!(image.tag(), None);
    }

    #[test]
    fn should_convert_to_string_for_named() {
        let image = Image::from_str("zammad/zammad-docker-compose").unwrap();

        assert_eq!(
            &image.to_string(),
            "docker.io/zammad/zammad-docker-compose:latest"
        );
    }

    #[test]
    fn should_convert_to_string_for_digest() {
        let image = Image::from_str(
            "sha256:9895c9b90b58c9490471b877f6bb6a90e6bdc154da7fbb526a0322ea242fc913",
        )
        .unwrap();

        assert_eq!(
            &image.to_string(),
            "sha256:9895c9b90b58c9490471b877f6bb6a90e6bdc154da7fbb526a0322ea242fc913"
        );
    }

    #[test]
    fn should_parse_image_id() {
        let image = Image::from_str("9895c9b90b58").unwrap();

        assert_eq!(&image.to_string(), "9895c9b90b58");
        assert_eq!(image.name(), None);
        assert_eq!(image.tag(), None);
    }

    #[test]
    fn should_parse_image_with_repo_and_user() {
        let image = Image::from_str("zammad/zammad-docker-compose").unwrap();

        assert_eq!(&image.name().unwrap(), "zammad/zammad-docker-compose");
        assert_eq!(&image.tag().unwrap(), "latest");
    }

    #[test]
    fn should_parse_image_with_version() {
        let image = Image::from_str("mariadb:10.3").unwrap();

        assert_eq!(&image.name().unwrap(), "library/mariadb");
        assert_eq!(&image.tag().unwrap(), "10.3");
        assert_eq!(&image.to_string(), "docker.io/library/mariadb:10.3");
    }

    #[test]
    fn should_parse_image_with_latest_version() {
        let image = Image::from_str("nginx:latest").unwrap();

        assert_eq!(&image.name().unwrap(), "library/nginx");
        assert_eq!(&image.tag().unwrap(), "latest");
        assert_eq!(&image.to_string(), "docker.io/library/nginx:latest");
    }

    #[test]
    fn should_parse_image_with_all_information() {
        let image = Image::from_str("docker.io/library/nginx:latest").unwrap();

        assert_eq!(&image.to_string(), "docker.io/library/nginx:latest");
    }

    #[test]
    fn should_parse_image_from_localhost() {
        let image = Image::from_str("localhost:5000/library/nginx:latest").unwrap();

        assert_eq!(&image.to_string(), "localhost:5000/library/nginx:latest");
        assert_eq!(&image.registry().unwrap(), "localhost:5000");
    }

    #[test]
    fn should_compare_images() {
        let image_full = Image::from_str("docker.io/library/nginx:latest").unwrap();
        let image_partially = Image::from_str("docker.io/library/nginx").unwrap();
        let image_short = Image::from_str("nginx").unwrap();
        assert_eq!(image_full, image_partially);
        assert_eq!(image_partially, image_full);
        assert_eq!(image_partially, image_short);
        assert_eq!(image_short, image_partially);
        assert_eq!(image_full, image_short);
        assert_eq!(image_short, image_full);

        let image_local_registry = Image::from_str("localhost:5000/library/nginx:latest").unwrap();
        assert_ne!(image_local_registry, image_short);
    }
}
