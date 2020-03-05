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

use secstr::SecUtf8;
use serde::de::Error as SerdeError;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::convert::TryFrom;
use std::hash::{Hash, Hasher};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Environment {
    values: Vec<EnvironmentVariable>,
}

impl Environment {
    pub fn new(values: Vec<EnvironmentVariable>) -> Self {
        Environment { values }
    }

    pub fn get<'a, 'b: 'a>(&'b self, index: usize) -> Option<&'a EnvironmentVariable> {
        self.values.get(index)
    }

    pub fn iter<'a, 'b: 'a>(&'b self) -> std::slice::Iter<'a, EnvironmentVariable> {
        self.values.iter()
    }

    pub fn variable<'a, 'b: 'a>(&'b self, env_name: &str) -> Option<&'a EnvironmentVariable> {
        for env in &self.values {
            if &env.key == env_name {
                return Some(env);
            }
        }
        None
    }

    pub(super) fn push(&mut self, variable: EnvironmentVariable) {
        self.values.push(variable);
    }
}

impl<'de> Deserialize<'de> for Environment {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use regex::Regex;

        match Value::deserialize(deserializer)? {
            Value::Object(map) => {
                let mut values = Vec::with_capacity(map.len());
                for key_and_value in map.into_iter() {
                    values.push(
                        EnvironmentVariable::try_from(key_and_value).map_err(SerdeError::custom)?,
                    );
                }

                Ok(Environment { values })
            }
            Value::Array(raw_values) => {
                lazy_static! {
                    static ref RE: Regex = Regex::new("(.*)=(.*)").unwrap();
                }

                let mut values = Vec::with_capacity(raw_values.len());
                for value in raw_values {
                    values.push(match value {
                        Value::String(value) => match RE.captures(&value) {
                            Some(captures) => EnvironmentVariable::new(
                                captures.get(1).map_or("", |m| m.as_str()).to_string(),
                                SecUtf8::from(captures.get(2).map_or("", |m| m.as_str())),
                            ),
                            None => return Err(SerdeError::custom(
                                "Invalid env value payload: Key and value must be separated by equal sign."
                            )),
                        },
                        _ => {
                            return Err(SerdeError::custom(
                                "Invalid environment payload: Payload must be an array of string.",
                            ));
                        }
                    })
                }
                Ok(Environment { values })
            }
            _ => Err(SerdeError::custom("Invalid environment payload.")),
        }
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

    pub(super) fn with_original(value: SecUtf8, original: EnvironmentVariable) -> Self {
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

    #[cfg(test)]
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
                        .map_or(false, |templated| templated.as_bool().unwrap_or(false)),
                    values
                        .get("replicate")
                        .map_or(false, |replicate| replicate.as_bool().unwrap_or(false)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::from_value;

    #[test]
    fn should_parse_env_from_kv_string() {
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
    fn should_parse_env_from_kv_object() {
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
    fn should_parse_env_from_object_without_templating() {
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
        assert_eq!(e.templated, false);
        assert_eq!(e.replicate, false);
    }

    #[test]
    fn should_parse_env_from_object_with_templating() {
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
        assert_eq!(e.templated, true);
        assert_eq!(e.replicate, true);
    }

    #[test]
    fn should_not_parse_env_from_kv_object_due_to_invalid_env_value_type() {
        let e = from_value::<Environment>(serde_json::json!({
            "MYSQL_USER": {"value": {}}
        }));

        assert_eq!(
            &e.unwrap_err().to_string(),
            "Invalid env value payload: value must be a string."
        )
    }

    #[test]
    fn should_not_parse_env_from_kv_object_due_to_invalid_env_value() {
        let e = from_value::<Environment>(serde_json::json!({"MYSQL_USER": {}}));

        assert_eq!(
            &e.unwrap_err().to_string(),
            "Invalid env value payload: value is a required field."
        );
    }

    #[test]
    fn should_not_parse_env_unexpected_json() {
        let e = from_value::<Environment>(serde_json::json!("Some random string"));

        assert_eq!(&e.unwrap_err().to_string(), "Invalid environment payload.");
    }

    #[test]
    fn should_not_parse_env_unexpected_array_form() {
        let e = from_value::<Environment>(serde_json::json!([{}]));

        assert_eq!(
            &e.unwrap_err().to_string(),
            "Invalid environment payload: Payload must be an array of string."
        );
    }

    #[test]
    fn should_not_parse_env_unexpected_kv_definitions() {
        let e = from_value::<Environment>(serde_json::json!(["MYSQL_USER"]));

        assert_eq!(
            &e.unwrap_err().to_string(),
            "Invalid env value payload: Key and value must be separated by equal sign."
        );
    }
}
