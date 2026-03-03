use crate::{
    Image,
    templating::{TemplateData, Templated, TemplatedClone, TemplatedCloneError},
};
use handlebars::RenderError;
use secstr::SecUtf8;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::OnceLock,
};

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceConfig {
    pub service_name: String,
    pub image: Image,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<Environment>,
    #[serde(alias = "volumes", alias = "files", default)]
    pub files: Option<BTreeMap<PathBuf, SecUtf8>>,
}

impl ServiceConfig {
    pub fn new(service_name: String, image: Image) -> ServiceConfig {
        ServiceConfig {
            service_name,
            image,
            env: None,
            files: None,
        }
    }

    pub fn add_env(&mut self, variable: EnvironmentVariable) {
        if let Some(env) = &mut self.env {
            env.push(variable);
        } else {
            self.env = Some(Environment::new(vec![variable]));
        }
    }

    pub fn add_file(&mut self, path: PathBuf, data: SecUtf8) {
        if let Some(ref mut files) = self.files {
            files.insert(path, data);
        } else {
            let mut files = BTreeMap::new();
            files.insert(path, data);
            self.files = Some(files);
        }
    }

    /// Copy labels, envs and files from other into self.
    /// If something is defined in self and other, self has precedence.
    pub fn merge_with(mut self, mut other: Self) -> Self {
        if let Some(env) = other.env {
            self.env = match self.env.take() {
                Some(mut self_env) => {
                    for env in env.iter() {
                        if self_env.variable(env.key()).is_some() {
                            continue;
                        }
                        self_env.push(env.clone());
                    }
                    Some(self_env)
                }
                None => Some(env),
            }
        }

        if let Some(mut files) = other.files.take() {
            self.files = match self.files.take() {
                Some(self_files) => {
                    files.extend(self_files);
                    Some(files)
                }
                None => Some(files),
            }
        }

        self
    }
}

impl TemplatedClone<()> for ServiceConfig {
    fn templated_clone(
        &self,
        template_data: &TemplateData,
    ) -> Result<Self, TemplatedCloneError<()>> {
        Ok(self.clone().apply_template(template_data)?)
    }
}

impl Templated<()> for ServiceConfig {
    fn apply_template(
        mut self,
        template_data: &TemplateData,
    ) -> Result<Self, crate::templating::TemplatedError<()>> {
        let handlebars = template_data.as_handlerbars();

        let mut templated_config = self.clone();
        templated_config.service_name = handlebars.render(&self.service_name)?;

        if let Some(env) = self.env.take() {
            templated_config.env = Some(env.apply_template(template_data)?);
        }

        if let Some(files) = self.files.take() {
            templated_config.files = Some(
                files
                    .into_iter()
                    .map(|(path, file_content)| {
                        let file_template = file_content.unsecure();

                        Ok((path, SecUtf8::from(handlebars.render(file_template)?)))
                    })
                    .collect::<Result<BTreeMap<PathBuf, SecUtf8>, RenderError>>()?,
            );
        }

        Ok(templated_config)
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Environment {
    values: Vec<EnvironmentVariable>,
}

impl TemplatedClone<()> for Environment {
    fn templated_clone(
        &self,
        template_data: &TemplateData,
    ) -> Result<Self, TemplatedCloneError<()>> {
        Ok(self.clone().apply_template(template_data)?)
    }
}

impl Templated<()> for Environment {
    fn apply_template(
        self,
        template_data: &TemplateData,
    ) -> Result<Self, crate::templating::TemplatedError<()>> {
        let mut templated_env = Vec::new();

        let handlebars = template_data.as_handlerbars();
        for e in self.values.into_iter() {
            let v = if e.templated() {
                EnvironmentVariable::with_original(
                    SecUtf8::from(handlebars.render(e.value().unsecure())?),
                    e,
                )
            } else {
                e
            };
            templated_env.push(v);
        }

        Ok(Environment::new(templated_env))
    }
}

impl Environment {
    pub fn new(mut values: Vec<EnvironmentVariable>) -> Self {
        values.sort_by(|a, b| a.key.cmp(&b.key));
        Environment { values }
    }

    #[cfg(test)]
    pub fn get<'a, 'b: 'a>(&'b self, index: usize) -> Option<&'a EnvironmentVariable> {
        self.values.get(index)
    }

    pub fn iter<'a, 'b: 'a>(&'b self) -> std::slice::Iter<'a, EnvironmentVariable> {
        self.values.iter()
    }

    pub fn into_values_iter(self) -> std::vec::IntoIter<EnvironmentVariable> {
        self.values.into_iter()
    }

    pub fn variable<'a, 'b: 'a>(&'b self, env_name: &str) -> Option<&'a EnvironmentVariable> {
        self.values.iter().find(|&env| env.key == env_name)
    }

    pub(super) fn push(&mut self, variable: EnvironmentVariable) {
        self.values.push(variable);
        self.values.sort_by(|a, b| a.key.cmp(&b.key));
    }
}

static ENVIRONMENT_KEY_VALUE: OnceLock<regex::Regex> = OnceLock::new();

impl<'de> Deserialize<'de> for Environment {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use regex::Regex;
        use serde::de::Error;

        match Value::deserialize(deserializer)? {
            Value::Object(map) => {
                let mut values = Vec::with_capacity(map.len());
                for key_and_value in map.into_iter() {
                    values
                        .push(EnvironmentVariable::try_from(key_and_value).map_err(Error::custom)?);
                }

                Ok(Environment::new(values))
            }
            Value::Array(raw_values) => {
                let regex = ENVIRONMENT_KEY_VALUE.get_or_init(|| Regex::new("(.*)=(.*)").unwrap());

                let mut values = Vec::with_capacity(raw_values.len());
                for value in raw_values {
                    values.push(match value {
                        Value::String(value) => match regex.captures(&value) {
                            Some(captures) => EnvironmentVariable::new(
                                captures.get(1).map_or("", |m| m.as_str()).to_string(),
                                SecUtf8::from(captures.get(2).map_or("", |m| m.as_str())),
                            ),
                            None => return Err(Error::custom(
                                "Invalid env value payload: Key and value must be separated by equal sign."
                            )),
                        },
                        _ => {
                            return Err(Error::custom(
                                "Invalid environment payload: Payload must be an array of string.",
                            ));
                        }
                    })
                }
                Ok(Environment::new(values))
            }
            _ => Err(Error::custom("Invalid environment payload.")),
        }
    }
}

impl Serialize for Environment {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_map(Some(self.values.len()))?;

        #[derive(Serialize)]
        struct Inner<'a> {
            value: &'a str,
            templated: bool,
            replicate: bool,
        }

        for value in &self.values {
            serde::ser::SerializeMap::serialize_entry(
                &mut state,
                value.key.as_str(),
                &Inner {
                    value: value.value.unsecure(),
                    templated: value.templated,
                    replicate: value.replicate,
                },
            )?;
        }

        serde::ser::SerializeMap::end(state)
    }
}

#[derive(Clone, Debug)]
pub struct EnvironmentVariable {
    key: String,
    value: SecUtf8,
    original_value: Option<SecUtf8>,
    templated: bool,
    replicate: bool,
}

impl EnvironmentVariable {
    pub fn new(key: String, value: SecUtf8) -> Self {
        EnvironmentVariable {
            key,
            value,
            original_value: None,
            templated: false,
            replicate: false,
        }
    }

    pub(crate) fn with_original(value: SecUtf8, original: EnvironmentVariable) -> Self {
        EnvironmentVariable {
            key: original.key,
            value,
            original_value: Some(original.value),
            templated: original.templated,
            replicate: original.replicate,
        }
    }

    #[cfg(test)]
    pub fn with_templating(key: String, value: SecUtf8) -> Self {
        EnvironmentVariable {
            key,
            value,
            original_value: None,
            templated: true,
            replicate: false,
        }
    }

    pub fn with_replicated(key: String, value: SecUtf8) -> Self {
        EnvironmentVariable {
            key,
            value,
            original_value: None,
            templated: false,
            replicate: true,
        }
    }

    pub fn key(&self) -> &String {
        &self.key
    }

    pub fn with_value(mut self, value: SecUtf8) -> Self {
        self.value = value;
        self
    }

    pub fn value(&self) -> &SecUtf8 {
        &self.value
    }

    pub fn with_templated(mut self, templated: bool) -> Self {
        self.templated = templated;
        self
    }

    pub fn templated(&self) -> bool {
        self.templated
    }

    pub fn replicate(&self) -> bool {
        self.replicate
    }

    pub fn original(&self) -> Self {
        match &self.original_value {
            Some(original_value) => EnvironmentVariable {
                key: self.key.clone(),
                value: original_value.clone(),
                templated: self.templated,
                replicate: self.replicate,
                original_value: None,
            },
            None => self.clone(),
        }
    }
}

impl Hash for EnvironmentVariable {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key.hash(state);
        self.value.unsecure().hash(state);
    }
}

impl TryFrom<(String, Value)> for EnvironmentVariable {
    type Error = &'static str;

    fn try_from(value: (String, Value)) -> Result<Self, Self::Error> {
        let (key, value) = value;

        let (value, templated, replicate) = match value {
            Value::String(v) => (SecUtf8::from(v), false, false),
            Value::Object(values) => {
                let value = values
                    .get("value")
                    .ok_or("Invalid env value payload: value is a required field.")?;

                let value = match value {
                    Value::String(v) => v,
                    _ => return Err("Invalid env value payload: value must be a string."),
                };

                (
                    SecUtf8::from(value),
                    values
                        .get("templated")
                        .is_some_and(|templated| templated.as_bool().unwrap_or(false)),
                    values
                        .get("replicate")
                        .is_some_and(|replicate| replicate.as_bool().unwrap_or(false)),
                )
            }
            _ => {
                return Err("Invalid env value payload: The value must be a string or an object.");
            }
        };

        Ok(EnvironmentVariable {
            key,
            value,
            original_value: None,
            templated,
            replicate,
        })
    }
}

impl PartialEq for EnvironmentVariable {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.value == other.value
    }
}
impl Eq for EnvironmentVariable {}

#[macro_export]
macro_rules! blueprint_service {

    ($config:expr_2021, env = ($($env_key:expr_2021 => $env_value:expr_2021),*)) => {{
        let config = $config;

        let mut merge_into =
            $crate::app_blueprints::ServiceConfig::new(config.service_name.clone(), config.image.clone());

        let env = vec![
            $( $crate::app_blueprints::EnvironmentVariable::new(String::from($env_key), secstr::SecUtf8::from($env_value)), )*
        ];
        merge_into.env = Some($crate::app_blueprints::Environment::new(env));

        config.merge_with(merge_into)
    }};

    ($config:expr_2021, env = ($($env_key:expr_2021 => $env_value:expr_2021),*), files = ($($v_key:expr_2021 => $v_value:expr_2021),*) ) => {{
        let config = $config;

        let mut merge_into =
            $crate::app_blueprints::ServiceConfig::new(config.service_name.clone(), config.image.clone());

        let files = std::collections::BTreeMap::from([
            $( (std::path::PathBuf::from($v_key), secstr::SecUtf8::from($v_value)), )*
        ]);
        merge_into.files = Some(files);

        let env = vec![
            $( $crate::app_blueprints::EnvironmentVariable::new(String::from($env_key), secstr::SecUtf8::from($env_value)), )*
        ];
        merge_into.env = Some($crate::app_blueprints::Environment::new(env));

        config.merge_with(merge_into)
    }};

    ( $name:expr_2021 ) => {{
        use sha2::Digest;
        let mut hasher = ::sha2::Sha256::new();
        hasher.update($name);
        let img_hash = &format!("sha256:{:x}", hasher.finalize());

        $crate::blueprint_service!($name, img_hash)
    }};

    ( $name:expr_2021, $img:expr_2021 ) => {{
        use std::str::FromStr;
        use $crate::app_blueprints::ServiceConfig;
        ServiceConfig::new(String::from($name), $crate::Image::from_str($img).unwrap())
    }};

    ( $name:expr_2021, $img:expr_2021,
        env = ($($env_key:expr_2021 => $env_value:expr_2021),*)
        ) => {{
        use $crate::app_blueprints::ServiceConfig;
        use std::str::FromStr;
        let mut config =
            ServiceConfig::new(String::from($name), $crate::Image::from_str($img).unwrap());

        let env = vec![
            $( $crate::app_blueprints::EnvironmentVariable::new(String::from($env_key), secstr::SecUtf8::from($env_value)), )*
        ];
        config.env = Some($crate::app_blueprints::Environment::new(env));

        config
    }};

    ( $name:expr_2021, $img:expr_2021,
        templated_env = ($($env_key:expr_2021 => $env_value:expr_2021),*)
        ) => {{
        use $crate::app_blueprints::ServiceConfig;
        use std::str::FromStr;
        let mut config =
            ServiceConfig::new(String::from($name), $crate::Image::from_str($img).unwrap());

        let env = vec![
            $( $crate::app_blueprints::EnvironmentVariable::new(String::from($env_key), secstr::SecUtf8::from($env_value)).with_templated(true), )*
        ];
        config.env = Some($crate::app_blueprints::Environment::new(env));

        config
    }};

    ( $name:expr_2021, $img:expr_2021,
        env = ($($env_key:expr_2021 => $env_value:expr_2021),*),
        files = ($($v_key:expr_2021 => $v_value:expr_2021),*) ) => {{
        use $crate::app_blueprints::ServiceConfig;
        use std::str::FromStr;
        let mut config =
            ServiceConfig::new(String::from($name), $crate::Image::from_str($img).unwrap());

        let files = std::collections::BTreeMap::from([
            $( (std::path::PathBuf::from($v_key), secstr::SecUtf8::from($v_value)), )*
        ]);
        config.files = Some(files);

        let env = vec![
            $( $crate::app_blueprints::EnvironmentVariable::new(String::from($env_key), secstr::SecUtf8::from($env_value)), )*
        ];
        config.env = Some($crate::app_blueprints::Environment::new(env));

        config
    }};

    ( $name:expr_2021, $img:expr_2021,
        files = ($($v_key:expr_2021 => $v_value:expr_2021),*) ) => {{
        use $crate::app_blueprints::ServiceConfig;
        use std::str::FromStr;
        let mut config =
            ServiceConfig::new(String::from($name), $crate::Image::from_str($img).unwrap());

        let files = std::collections::BTreeMap::from([
            $( (std::path::PathBuf::from($v_key), secstr::SecUtf8::from($v_value)), )*
        ]);
        config.files = Some(files);

        config
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AppName,
        app_instance::ContainerType,
        templating::{ApplicationTemplateData, ServiceOrServices, ServiceTemplateData},
    };
    use assert_json_diff::assert_json_eq;
    use serde_json::from_value;
    use std::str::FromStr;

    #[test]
    fn parse_service_config_json() {
        let config = from_value::<ServiceConfig>(serde_json::json!({
            "serviceName": "mariadb",
            "image": "mariadb:10.3",
            "env": [
              "MYSQL_USER=admin",
              "MYSQL_DATABASE=dbname"
            ]
        }))
        .unwrap();

        assert_eq!(config.service_name, "mariadb");
        assert_eq!(config.image.to_string(), "docker.io/library/mariadb:10.3");
        assert_eq!(
            config.env,
            Some(Environment::new(vec![
                EnvironmentVariable::new("MYSQL_USER".to_string(), SecUtf8::from("admin")),
                EnvironmentVariable::new("MYSQL_DATABASE".to_string(), SecUtf8::from("dbname"))
            ]))
        );
    }

    #[test]
    fn merge_service_configs_envs() {
        let mut config = blueprint_service!(
            "proxy",
            "nginx",
            env = ("VAR_1" => "abcd", "VAR_2" => "1234")
        );

        let config2 = blueprint_service!(
            "proxy",
            "nginx",
            env = ("VAR_1" => "efgh", "VAR_3" => "1234")
        );

        config = config.merge_with(config2);

        let env = config.env.unwrap();
        assert_eq!(env.iter().count(), 3);
        assert_eq!(
            env.variable("VAR_1"),
            Some(&EnvironmentVariable::new(
                String::from("VAR_1"),
                SecUtf8::from("abcd")
            ))
        );
        assert_eq!(
            env.variable("VAR_2"),
            Some(&EnvironmentVariable::new(
                String::from("VAR_2"),
                SecUtf8::from("1234")
            ))
        );
        assert_eq!(
            env.variable("VAR_3"),
            Some(&EnvironmentVariable::new(
                String::from("VAR_3"),
                SecUtf8::from("1234")
            ))
        );
    }

    #[test]
    fn merge_service_configs_volumes() {
        let mut config = blueprint_service!(
            "proxy",
            "nginx",
            env = (),
            files = ("/etc/mysql/my.cnf" => "ABCD", "/etc/folder/abcd.conf" => "1234")
        );
        let config2 = blueprint_service!(
            "proxy",
            "nginx",
            env = (),
            files = ("/etc/mysql/my.cnf" => "EFGH", "/etc/test.conf" => "5678")
        );

        config = config.merge_with(config2);

        let files = config.files.as_ref().expect("No value found");
        assert_eq!(files.len(), 3);
        assert_eq!(
            files.get(&PathBuf::from("/etc/mysql/my.cnf")),
            Some(&SecUtf8::from("ABCD"))
        );
        assert_eq!(
            files.get(&PathBuf::from("/etc/folder/abcd.conf")),
            Some(&SecUtf8::from("1234"))
        );
        assert_eq!(
            files.get(&PathBuf::from("/etc/test.conf")),
            Some(&SecUtf8::from("5678"))
        );
    }

    #[test]
    fn parse_volume_service_config_json() {
        let config_string = r#"{
            "serviceName": "mariadb",
            "image": "mariadb:10.3",
            "env": [
              "MYSQL_USER=admin",
              "MYSQL_DATABASE=dbname"
            ],
            "volumes": {
                "/etc/mysql/my.cnf": "ABCD"
            }
        }"#;
        let config_volume =
            from_value::<ServiceConfig>(serde_json::from_slice(config_string.as_bytes()).unwrap())
                .unwrap();

        let config_file = from_value::<ServiceConfig>(serde_json::json!({
            "serviceName": "mariadb",
            "image": "mariadb:10.3",
            "env": [
              "MYSQL_USER=admin",
              "MYSQL_DATABASE=dbname"
            ],
            "files": {
                "/etc/mysql/my.cnf" : "EFGH"
            }
        }))
        .unwrap();

        assert_eq!(
            config_volume
                .files
                .unwrap()
                .get(&PathBuf::from("/etc/mysql/my.cnf")),
            Some(&SecUtf8::from("ABCD"))
        );

        assert_eq!(
            config_file
                .files
                .unwrap()
                .get(&PathBuf::from("/etc/mysql/my.cnf")),
            Some(&SecUtf8::from("EFGH"))
        );
    }

    #[test]
    fn parse_env_from_kv_string() {
        let e = from_value::<Environment>(serde_json::json!(["MYSQL_USER=admin"]))
            .unwrap()
            .values
            .into_iter()
            .next()
            .unwrap();

        assert_eq!(e.key, "MYSQL_USER".to_string());
        assert_eq!(e.value.unsecure(), "admin".to_string());
    }

    #[test]
    fn parse_env_from_kv_object() {
        let e = from_value::<Environment>(serde_json::json!({"MYSQL_USER": "admin"}))
            .unwrap()
            .values
            .into_iter()
            .next()
            .unwrap();

        assert_eq!(e.key, "MYSQL_USER".to_string());
        assert_eq!(e.value.unsecure(), "admin".to_string());
    }

    #[test]
    fn parse_env_from_object_without_templating() {
        let e = from_value::<Environment>(serde_json::json!({
            "MYSQL_USER": {"value": "admin"}
        }))
        .unwrap()
        .values
        .into_iter()
        .next()
        .unwrap();

        assert_eq!(e.key, "MYSQL_USER".to_string());
        assert_eq!(e.value.unsecure(), "admin".to_string());
        assert!(!e.templated);
        assert!(!e.replicate);
    }

    #[test]
    fn serialize_environment() {
        let e = Environment {
            values: vec![EnvironmentVariable {
                key: String::from("MYSQL_USER"),
                value: SecUtf8::from(String::from("admin-{{application.name}}")),
                original_value: None,
                templated: true,
                replicate: true,
            }],
        };

        assert_json_eq!(
            serde_json::to_value(e).unwrap(),
            serde_json::json!({
                "MYSQL_USER": {"value": "admin-{{application.name}}", "templated": true, "replicate": true}
            })
        );
    }

    #[test]
    fn parse_env_from_object_with_templating() {
        let e = from_value::<Environment>(serde_json::json!({
            "MYSQL_USER": {"value": "admin-{{application.name}}", "templated": true, "replicate": true}
        }))
        .unwrap()
        .values
        .into_iter()
        .next()
        .unwrap();

        assert_eq!(e.key, "MYSQL_USER".to_string());
        assert_eq!(e.value.unsecure(), "admin-{{application.name}}".to_string());
        assert!(e.templated);
        assert!(e.replicate);
    }

    #[test]
    fn not_parse_env_from_kv_object_due_to_invalid_env_value_type() {
        let e = from_value::<Environment>(serde_json::json!({
            "MYSQL_USER": {"value": {}}
        }));

        assert_eq!(
            &e.unwrap_err().to_string(),
            "Invalid env value payload: value must be a string."
        )
    }

    #[test]
    fn not_parse_env_from_kv_object_due_to_invalid_env_value() {
        let e = from_value::<Environment>(serde_json::json!({"MYSQL_USER": {}}));

        assert_eq!(
            &e.unwrap_err().to_string(),
            "Invalid env value payload: value is a required field."
        );
    }

    #[test]
    fn not_parse_env_unexpected_json() {
        let e = from_value::<Environment>(serde_json::json!("Some random string"));

        assert_eq!(&e.unwrap_err().to_string(), "Invalid environment payload.");
    }

    #[test]
    fn not_parse_env_unexpected_array_form() {
        let e = from_value::<Environment>(serde_json::json!([{}]));

        assert_eq!(
            &e.unwrap_err().to_string(),
            "Invalid environment payload: Payload must be an array of string."
        );
    }

    #[test]
    fn not_parse_env_unexpected_kv_definitions() {
        let e = from_value::<Environment>(serde_json::json!(["MYSQL_USER"]));

        assert_eq!(
            &e.unwrap_err().to_string(),
            "Invalid env value payload: Key and value must be separated by equal sign."
        );
    }

    #[test]
    fn not_apply_templating_for_environment() {
        let mut config = blueprint_service!("db", "maria-db");
        config.add_env(EnvironmentVariable::new(
            String::from("DB_USER"),
            SecUtf8::from("admin-{{service.name}}"),
        ));

        let config = config
            .templated_clone(&TemplateData {
                application: ApplicationTemplateData {
                    name: &AppName::master(),
                    ..Default::default()
                },
                service_or_services: ServiceOrServices::Service {
                    service: ServiceTemplateData {
                        name: "wordpress",
                        image: &Image::from_str("wordpress:alpine").unwrap(),
                        port: 80,
                        container_type: &ContainerType::Instance,
                    },
                },
                ..Default::default()
            })
            .unwrap();

        let env = config.env.as_ref().unwrap().iter().next().unwrap();

        assert_eq!(env.value().unsecure(), "admin-{{service.name}}");
    }

    #[test]
    fn apply_templating_for_environment() {
        let mut config = blueprint_service!("db", "maria-db");
        config.add_env(EnvironmentVariable::with_templating(
            String::from("DB_USER"),
            SecUtf8::from("admin-{{service.name}}"),
        ));

        let config = config
            .templated_clone(&TemplateData {
                application: ApplicationTemplateData {
                    name: &AppName::master(),
                    ..Default::default()
                },
                service_or_services: ServiceOrServices::Service {
                    service: ServiceTemplateData {
                        name: "wordpress",
                        image: &Image::from_str("wordpress:alpine").unwrap(),
                        port: 80,
                        container_type: &ContainerType::Instance,
                    },
                },
                ..Default::default()
            })
            .unwrap();

        let env = config.env.as_ref().unwrap().get(0).unwrap();

        assert_eq!(env.value().unsecure(), "admin-wordpress");
    }

    #[test]
    fn keep_original_environment_variable_when_templating() {
        let mut config = blueprint_service!("db", "maria-db");
        config.add_env(EnvironmentVariable::with_templating(
            String::from("DB_USER"),
            SecUtf8::from("admin-{{service.name}}"),
        ));

        let config = config
            .templated_clone(&TemplateData {
                application: ApplicationTemplateData {
                    name: &AppName::master(),
                    ..Default::default()
                },
                service_or_services: ServiceOrServices::Service {
                    service: ServiceTemplateData {
                        name: "wordpress",
                        image: &Image::from_str("wordpress:alpine").unwrap(),
                        port: 80,
                        container_type: &ContainerType::Instance,
                    },
                },
                ..Default::default()
            })
            .unwrap();

        let env = config.env.as_ref().unwrap().get(0).unwrap();

        assert!(
            env.original().templated(),
            "After applying a template, the environment keep that information"
        );
        assert_eq!(env.original().value().unsecure(), "admin-{{service.name}}");
    }

    #[test]
    fn apply_templating_for_environment_with_user_defined_variable() {
        let mut config = blueprint_service!("db", "maria-db");
        config.add_env(EnvironmentVariable::with_templating(
            String::from("DB_USER"),
            SecUtf8::from("admin-{{userDefined.test}}"),
        ));

        let config = config
            .templated_clone(&TemplateData {
                application: ApplicationTemplateData {
                    name: &AppName::master(),
                    ..Default::default()
                },
                service_or_services: ServiceOrServices::Service {
                    service: ServiceTemplateData {
                        name: "wordpress",
                        image: &Image::from_str("wordpress:alpine").unwrap(),
                        port: 80,
                        container_type: &ContainerType::Instance,
                    },
                },
                user_defined_parameters: Some(&serde_json::json!({
                    "test": "wordpress"
                })),
                ..Default::default()
            })
            .unwrap();

        let env = config.env.as_ref().unwrap().get(0).unwrap();

        assert_eq!(env.value().unsecure(), "admin-wordpress");
    }
}
