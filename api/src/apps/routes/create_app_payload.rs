use crate::models::ServiceConfig;
use http::StatusCode;
use http_api_problem::HttpApiProblem;
use rocket::{
    data::{FromData, Outcome},
    http::Status,
    serde::json::Json,
    Data, Request,
};

pub struct CreateAppPayload {
    pub services: Vec<ServiceConfig>,
    pub user_defined_parameters: Option<serde_json::Value>,
}

#[rocket::async_trait]
impl<'r> FromData<'r> for CreateAppPayload {
    type Error = HttpApiProblem;

    async fn from_data(req: &'r Request<'_>, data: Data<'r>) -> Outcome<'r, Self> {
        let rocket::outcome::Outcome::Success(data) =
            Json::<serde_json::Value>::from_data(req, data).await
        else {
            return Outcome::Error((
                Status::NotAcceptable,
                HttpApiProblem::with_title_and_type(StatusCode::NOT_ACCEPTABLE)
                    .detail(String::from("Accepting only JSON payload")),
            ));
        };

        let (services, user_defined_parameters) = match data.0 {
            serde_json::Value::Array(services) => (
                match serde_json::from_value(serde_json::Value::Array(services)) {
                    Ok(services) => services,
                    Err(err) => {
                        return Outcome::Error((
                            Status::BadRequest,
                            HttpApiProblem::with_title_and_type(StatusCode::BAD_REQUEST)
                                .detail(err.to_string()),
                        ));
                    }
                },
                None,
            ),
            serde_json::Value::Object(mut object) => (
                match object.remove("services") {
                    Some(services) => match services {
                        serde_json::Value::Array(services) => {
                            match serde_json::from_value::<Vec<ServiceConfig>>(
                                serde_json::Value::Array(services),
                            ) {
                                Ok(services) => services,
                                Err(err) => {
                                    return Outcome::Error((
                                        Status::BadRequest,
                                        HttpApiProblem::with_title_and_type(
                                            StatusCode::BAD_REQUEST,
                                        )
                                        .detail(err.to_string()),
                                    ));
                                }
                            }
                        }
                        _ => {
                            return Outcome::Error((
                                Status::BadRequest,
                                HttpApiProblem::with_title_and_type(StatusCode::BAD_REQUEST)
                                    .detail(String::from("expected an JSON array for services")),
                            ))
                        }
                    },
                    None => Vec::new(),
                },
                object.remove("userDefined"),
            ),
            _ => {
                return Outcome::Error((
                    Status::BadRequest,
                    HttpApiProblem::with_title_and_type(StatusCode::BAD_REQUEST)
                        .detail(String::from("expected an JSON object or an JSON array")),
                ))
            }
        };

        Outcome::Success(Self {
            services,
            user_defined_parameters,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::http_result::HttpApiError;

    use super::*;
    use assert_json_diff::assert_json_include;
    use rocket::{http::ContentType, local::asynchronous::Client};
    use serde_json::json;

    async fn create_client() -> Client {
        #[post("/", data = "<data>")]
        fn test_route(
            data: Result<CreateAppPayload, HttpApiProblem>,
        ) -> Result<&'static str, HttpApiError> {
            data.map(|_| "dummy").map_err(HttpApiError::from)
        }
        let rocket = rocket::build().mount("/", routes![test_route]);

        Client::tracked(rocket).await.expect("valid rocket")
    }

    #[tokio::test]
    async fn unexpected_text_payload() {
        let client = create_client().await;

        let response = client
            .post("/")
            .body(String::from("some text"))
            .header(ContentType::Text)
            .dispatch()
            .await;

        let body = response.into_string().await.unwrap();
        assert_json_include!(
            actual: serde_json::from_str::<serde_json::Value>(&body).unwrap(),
            expected: json!({
                "status": 406,
                "detail": "Accepting only JSON payload"
            })
        );
    }

    #[tokio::test]
    async fn invalid_payload_at_root() {
        let client = create_client().await;

        let response = client
            .post("/")
            .body(json!("test").to_string())
            .header(ContentType::JSON)
            .dispatch()
            .await;

        let body = response.into_string().await.unwrap();
        assert_json_include!(
            actual: serde_json::from_str::<serde_json::Value>(&body).unwrap(),
            expected: json!({
                "status": 400,
                "detail": "expected an JSON object or an JSON array"
            })
        );
    }

    #[tokio::test]
    async fn invalid_payload_for_services_attribute() {
        let client = create_client().await;

        let response = client
            .post("/")
            .body(json!({ "services": "test" }).to_string())
            .header(ContentType::JSON)
            .dispatch()
            .await;

        let body = response.into_string().await.unwrap();
        assert_json_include!(
            actual: serde_json::from_str::<serde_json::Value>(&body).unwrap(),
            expected: json!({
                "status": 400,
                "detail": "expected an JSON array for services"
            })
        );
    }

    #[tokio::test]
    async fn invalid_image_payload_variant_within_service_list() {
        let client = create_client().await;

        let response = client
            .post("/")
            .body(
                json!([{
                    "serviceName": "db",
                    "image": "private-registry.example.com/_/postgres"
                }])
                .to_string(),
            )
            .header(ContentType::JSON)
            .dispatch()
            .await;

        let body = response.into_string().await.unwrap();
        assert_json_include!(
            actual: serde_json::from_str::<serde_json::Value>(&body).unwrap(),
            expected: json!({
                "status": 400,
                "detail": "Invalid image: private-registry.example.com/_/postgres"
            })
        );
    }

    #[tokio::test]
    async fn invalid_image_payload_variant_within_service_attribute() {
        let client = create_client().await;

        let response = client
            .post("/")
            .body(
                json!({
                    "services": [{
                        "serviceName": "db",
                        "image": "private-registry.example.com/_/postgres"
                    }]
                })
                .to_string(),
            )
            .header(ContentType::JSON)
            .dispatch()
            .await;

        let body = response.into_string().await.unwrap();
        assert_json_include!(
            actual: serde_json::from_str::<serde_json::Value>(&body).unwrap(),
            expected: json!({
                "status": 400,
                "detail": "Invalid image: private-registry.example.com/_/postgres"
            })
        );
    }
}
