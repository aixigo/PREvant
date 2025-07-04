use jsonschema::Validator;
use serde::Serializer;
use serde_json::Value;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct UserDefinedParameters {
    data: Value,
}

impl serde::Serialize for UserDefinedParameters {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.data.serialize(serializer)
    }
}

impl UserDefinedParameters {
    pub fn new(data: Value, validator: &Validator) -> Result<Self, UserDefinedParametersError> {
        if let Err(err) = validator.validate(&data) {
            let err = err.to_string();
            return Err(UserDefinedParametersError::ValidationError { data, err });
        };

        Ok(Self { data })
    }

    pub fn merge(mut self, other: Self) -> Self {
        Self::merge_json(&mut self.data, other.data);
        self
    }

    fn merge_json(a: &mut Value, b: Value) {
        match (a, b) {
            (a @ &mut Value::Object(_), Value::Object(b)) => {
                let a = a.as_object_mut().unwrap();
                for (k, v) in b {
                    Self::merge_json(a.entry(k).or_insert(Value::Null), v);
                }
            }
            (a, b) => *a = b,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UserDefinedParametersError {
    #[error("Provided data ({data}) does not match schema: {err}")]
    ValidationError { data: Value, err: String },
}
