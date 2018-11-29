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

use regex::Regex;
use serde::ser::{Serialize, Serializer};
use shiplift::rep::Container;
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
    registry: String,
    image_user: Option<String>,
    image_repository: Option<String>,
    image_tag: Option<String>,
    env: Option<Vec<String>>,
    volumes: Option<Vec<String>>,
}

impl ServiceConfig {
    pub fn new(
        service_name: &String,
        registry: &String,
        env: Option<Vec<String>>,
    ) -> ServiceConfig {
        ServiceConfig {
            service_name: service_name.clone(),
            registry: registry.clone(),
            image_user: None,
            image_repository: None,
            image_tag: None,
            env,
            volumes: None,
        }
    }

    fn get_docker_image_base(&self) -> Result<String, ServiceError> {
        let mut service_name = &self.service_name;

        if let Some(ref image_repository) = self.image_repository {
            if let Some(ref image_user) = self.image_user {
                return Ok(image_user.to_owned() + "/" + image_repository);
            }

            service_name = image_repository;
        }

        let re: Regex = Regex::new(r"^((\w+)-.+)$").unwrap();

        let caps = match re.captures(service_name) {
            Some(c) => c,
            None => return Err(ServiceError::InvalidServiceName(self.service_name.clone())),
        };

        Ok(String::from(caps.get(2).unwrap().as_str()) + "/" + caps.get(0).unwrap().as_str())
    }

    pub fn get_docker_image(&self, app_name: &String) -> Result<String, ServiceError> {
        let image_name = self.registry.to_owned() + "/" + &self.get_docker_image_base()?;

        let image_with_tag = match &self.image_tag {
            None => image_name + ":" + app_name,
            Some(tag) => image_name + ":" + &tag,
        };

        Ok(image_with_tag)
    }

    pub fn get_service_name(&self) -> &String {
        &self.service_name
    }

    pub fn get_image_tag(&self) -> Option<String> {
        match &self.image_tag {
            None => None,
            Some(tag) => Some(tag.clone()),
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
    pub fn from(c: &Container) -> Result<Service, ServiceError> {
        let service_name = match c.labels.get("service-name") {
            Some(name) => name,
            None => return Err(ServiceError::MissingServiceNameLabel),
        };

        let app_name = match c.labels.get("review-app-name") {
            Some(name) => name,
            None => return Err(ServiceError::MissingReviewAppNameLabel),
        };

        let container_type = match c.labels.get("container-type") {
            None => ContainerType::Instance,
            Some(lb) => lb.parse::<ContainerType>()?,
        };

        Ok(Service {
            app_name: app_name.clone(),
            service_name: service_name.clone(),
            container_id: c.id.clone(),
            container_type,
            base_url: None,
        })
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
