use openidconnect::{core::CoreGenderClaim, IdTokenClaims};
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub enum User {
    Anonymous,
    Oidc {
        id_token_claims: IdTokenClaims<AdditionalClaims, CoreGenderClaim>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdditionalClaims(serde_json::Value);

impl AdditionalClaims {
    #[cfg(test)]
    pub fn empty() -> Self {
        Self(serde_json::Value::Object(serde_json::Map::new()))
    }
    #[cfg(test)]
    pub fn with_claims(claims: serde_json::Value) -> Self {
        assert!(claims.is_object());
        Self(claims)
    }
}

impl openidconnect::AdditionalClaims for AdditionalClaims {}
impl openidconnect::ExtraTokenFields for AdditionalClaims {}

impl Serialize for AdditionalClaims {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for AdditionalClaims {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Self(serde_json::Value::deserialize(deserializer)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_json_diff::assert_json_eq;
    use chrono::TimeZone;
    use openidconnect::{IssuerUrl, StandardClaims, SubjectIdentifier};

    #[test]
    fn serialize_additional_claims() {
        let claims = IdTokenClaims::<AdditionalClaims, CoreGenderClaim>::new(
            IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
            Vec::new(),
            chrono::Utc.with_ymd_and_hms(2025, 5, 1, 0, 0, 0).unwrap(),
            chrono::Utc.with_ymd_and_hms(2025, 5, 1, 0, 30, 0).unwrap(),
            StandardClaims::new(SubjectIdentifier::new(String::from("gitlab-user"))),
            AdditionalClaims::with_claims(serde_json::json!({ "user_id": "1234566" })),
        );

        assert_json_eq!(
            claims,
            serde_json::json!({
                "sub": "gitlab-user",
                "aud": [],
                "exp": 1746057600,
                "iat": 1746059400,
                "iss": "https://gitlab.com",
                "user_id": "1234566"
            })
        )
    }

    #[test]
    fn deserialize_additional_claims() {
        let token = serde_json::from_value::<IdTokenClaims<AdditionalClaims, CoreGenderClaim>>(
            serde_json::json!({
                "sub": "gitlab-user",
                "aud": [],
                "exp": 1746057600,
                "iat": 1746059400,
                "iss": "https://gitlab.com",
                "user_id": "1234566"
            }),
        )
        .unwrap();

        assert_eq!(
            token.additional_claims(),
            &AdditionalClaims::with_claims(serde_json::json!({
                "user_id": "1234566"
            }))
        )
    }

    #[test]
    fn serialize_empty_additional_claims() {
        let claims = IdTokenClaims::<AdditionalClaims, CoreGenderClaim>::new(
            IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
            Vec::new(),
            chrono::Utc.with_ymd_and_hms(2025, 5, 1, 0, 0, 0).unwrap(),
            chrono::Utc.with_ymd_and_hms(2025, 5, 1, 0, 30, 0).unwrap(),
            StandardClaims::new(SubjectIdentifier::new(String::from("gitlab-user"))),
            AdditionalClaims::empty(),
        );

        assert_json_eq!(
            claims,
            serde_json::json!({
                "sub": "gitlab-user",
                "aud": [],
                "exp": 1746057600,
                "iat": 1746059400,
                "iss": "https://gitlab.com",
            })
        )
    }

    #[test]
    fn deserialize_empty_additional_claims() {
        let token = serde_json::from_value::<IdTokenClaims<AdditionalClaims, CoreGenderClaim>>(
            serde_json::json!({
                "sub": "gitlab-user",
                "aud": [],
                "exp": 1746057600,
                "iat": 1746059400,
                "iss": "https://gitlab.com",
            }),
        )
        .unwrap();

        assert_eq!(token.additional_claims(), &AdditionalClaims::empty())
    }
}
