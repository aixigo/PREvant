use jsonschema::Validator;
use serde::Serializer;
use serde_json::Value;

#[derive(Debug)]
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
}

#[derive(Debug, thiserror::Error)]
pub enum UserDefinedParametersError {
    #[error("Provided data ({data}) does not match schema: {err}")]
    ValidationError { data: Value, err: String },
}
