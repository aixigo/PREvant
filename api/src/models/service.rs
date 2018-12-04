/*-
 * ========================LICENSE_START=================================
 * PREvant
 * %%
 * Copyright (C) 2018 aixigo AG
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
use std::fmt::{self, Debug, Formatter};
use std::str::FromStr;

use serde::ser::{Serialize, Serializer};
use url::Url;

#[derive(Clone)]
pub struct Service {
    app_name: String,
    service_name: String,
    container_type: ContainerType,
    container_id: String,
    base_url: Option<Url>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceConfig {
    service_name: String,
    image_repository: String,
    registry: Option<String>,
    image_user: Option<String>,
    image_tag: Option<String>,
    env: Option<Vec<String>>,
    volumes: Option<Vec<String>>,
}

impl ServiceConfig {
    pub fn new(
        service_name: &String,
        image_repository: &String,
        env: Option<Vec<String>>,
    ) -> ServiceConfig {
        ServiceConfig {
            service_name: service_name.clone(),
            image_repository: image_repository.clone(),
            registry: None,
            image_user: None,
            image_tag: None,
            env,
            volumes: None,
        }
    }

    pub fn set_registry(&mut self, registry: &Option<String>) {
        self.registry = registry.clone()
    }

    pub fn set_image_user(&mut self, image_user: &Option<String>) {
        self.image_user = image_user.clone()
    }

    pub fn set_image_tag(&mut self, image_tag: &Option<String>) {
        self.image_tag = image_tag.clone()
    }

    fn get_docker_image_base(&self) -> String {
        let image_user = match &self.image_user {
            Some(user) => user.clone(),
            None => String::from("library"),
        };

        format!("{}/{}", image_user, self.image_repository)
    }

    pub fn get_docker_image(&self) -> String {
        let registry = match &self.registry {
            None => String::from("docker.io"),
            Some(registry) => registry.clone(),
        };

        format!(
            "{}/{}:{}",
            registry,
            self.get_docker_image_base(),
            self.get_image_tag()
        )
    }

    pub fn get_service_name(&self) -> &String {
        &self.service_name
    }

    pub fn get_image_tag(&self) -> String {
        match &self.image_tag {
            None => "latest".to_owned(),
            Some(tag) => tag.clone(),
        }
    }

    pub fn get_env(&self) -> Option<Vec<String>> {
        match &self.env {
            None => None,
            Some(env) => Some(env.clone()),
        }
    }

    pub fn get_volumes(&self) -> Option<Vec<String>> {
        match &self.volumes {
            None => None,
            Some(volumes) => Some(volumes.clone()),
        }
    }
}

impl Service {
    pub fn new(
        app_name: String,
        service_name: String,
        container_id: String,
        container_type: ContainerType,
    ) -> Service {
        Service {
            app_name,
            service_name,
            container_id,
            container_type,
            base_url: None,
        }
    }

    pub fn set_app_name(&mut self, app_name: &String) {
        self.app_name = app_name.clone();
    }

    pub fn set_base_url(&mut self, base_url: &Url) {
        self.base_url = Some(base_url.clone());
    }

    pub fn set_container_type(&mut self, container_type: ContainerType) {
        self.container_type = container_type;
    }

    fn get_base_url(&self) -> Url {
        match &self.base_url {
            None => Url::parse("http://example.org").unwrap(),
            Some(base_url) => base_url.clone(),
        }
    }

    fn get_root_url(&self) -> String {
        let mut base = self.get_base_url();
        base.set_path(&format!("/{}/{}/", &self.app_name, &self.service_name));
        base.into_string()
    }

    pub fn get_service_name(&self) -> &String {
        &self.service_name
    }

    pub fn get_container_id(&self) -> &String {
        &self.container_id
    }

    pub fn get_container_type(&self) -> &ContainerType {
        &self.container_type
    }
}

impl Serialize for Service {
    fn serialize<S>(&self, serializer: S) -> Result<<S as Serializer>::Ok, <S as Serializer>::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Service<'a> {
            vhost: &'a String,
            url: String,
            container_type: String,
        }

        let s = Service {
            vhost: &self.service_name,
            url: self.get_root_url(),
            container_type: self.container_type.to_string(),
        };

        Ok(s.serialize(serializer)?)
    }
}

#[derive(Serialize, Clone, PartialEq)]
pub enum ContainerType {
    Instance,
    Replica,
    Linked,
}

impl FromStr for ContainerType {
    type Err = ServiceError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "linked" => Ok(ContainerType::Linked),
            "replica" => Ok(ContainerType::Replica),
            "instance" => Ok(ContainerType::Instance),
            lb => Err(ServiceError::InvalidContainerTypeLabel(String::from(lb))),
        }
    }
}

impl std::fmt::Display for ContainerType {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            &ContainerType::Instance => write!(f, "instance"),
            &ContainerType::Replica => write!(f, "replica"),
            &ContainerType::Linked => write!(f, "linked"),
        }
    }
}

pub enum ServiceError {
    MissingServiceNameLabel,
    MissingReviewAppNameLabel,
    InvalidContainerTypeLabel(String),
    InvalidServiceName(String),
}

impl Debug for ServiceError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            &ServiceError::MissingReviewAppNameLabel => {
                f.write_str("Container does not provide review-app-name label")?
            }
            &ServiceError::MissingServiceNameLabel => {
                f.write_str("Container does not provide service-name label")?
            }
            &ServiceError::InvalidContainerTypeLabel(ref lb) => {
                f.write_str(&format!("Invalid label for 'container-type': {:?}", lb))?
            }
            &ServiceError::InvalidServiceName(ref service_name) => f.write_str(&format!(
                "service name {:?} does not match pattern ((.+)-.+).",
                service_name
            ))?,
        }

        Ok(())
    }
}
