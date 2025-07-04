use crate::{
    config::{ApiAccessMode, Config},
    infrastructure::TraefikIngressRoute,
};
use anyhow::Context;
use futures::{stream::FuturesUnordered, StreamExt as _};
use http::StatusCode;
use http_api_problem::HttpApiProblem;
use openidconnect::{
    core::CoreProviderMetadata, Client, ClientId, ClientSecret, EndpointMaybeSet, EndpointNotSet,
    EndpointSet, IssuerUrl, Nonce, RedirectUrl, RefreshToken,
};
use rocket::{
    fairing::{self, Fairing, Info, Kind},
    http::{Cookie, CookieJar, SameSite, Status},
    request::{FromRequest, Outcome},
    Build, Request, Rocket,
};
pub use routes::auth_routes;
use std::str::FromStr;
use url::Url;

mod routes;

pub struct Auth {}

#[rocket::async_trait]
impl Fairing for Auth {
    fn info(&self) -> Info {
        Info {
            name: "auth",
            kind: Kind::Ignite,
        }
    }

    async fn on_ignite(&self, rocket: Rocket<Build>) -> fairing::Result {
        let Some(config) = rocket.state::<Config>() else {
            log::error!("There is no config in Rocket's state.");
            return fairing::Result::Err(rocket);
        };

        if matches!(config.api_access.mode, ApiAccessMode::RequireAuth if config.api_access.openid_providers.is_empty())
        {
            log::error!(
                "The API requires authentication but there are no login providers configured."
            );
            return fairing::Result::Err(rocket);
        }

        if matches!(config.api_access.mode, ApiAccessMode::Any) {
            log::warn!("There is no API protection configured which let any API client deploy any OCI image on your infrastructure.");
        }

        let base_url = rocket
            .state::<Option<TraefikIngressRoute>>()
            .cloned()
            .flatten()
            .and_then(|base_route| base_route.to_url())
            .unwrap_or_else(|| Url::from_str("http://localhost:8000").unwrap());

        let redirect_url = base_url.join("auth/oidc-response").unwrap();

        log::debug!("Using OIDC authentication with redirect url {redirect_url}");

        let mut oidc_clients = config
            .api_access
            .openid_providers
            .iter()
            .cloned()
            .map(|oidc_provider| {
                let redirect_url = RedirectUrl::new(redirect_url.to_string()).unwrap();
                async move {
                    let http_client = reqwest::ClientBuilder::new()
                        // Following redirects opens the client up to SSRF vulnerabilities.
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .expect("Client should build");

                    let issuer_url = IssuerUrl::new(oidc_provider.issuer_url.clone())
                        .with_context(|| {
                            format!("Invalid issuer URL: {}", oidc_provider.issuer_url)
                        })?;
                    let provider_metadata =
                        CoreProviderMetadata::discover_async(issuer_url.clone(), &http_client)
                            .await
                            .with_context(|| {
                                format!(
                                    "Cannot perform OpenID issuer discovery for {}",
                                    oidc_provider.issuer_url
                                )
                            })?;

                    let client = Client::from_provider_metadata(
                        provider_metadata,
                        ClientId::new(oidc_provider.client_id),
                        Some(ClientSecret::new(
                            oidc_provider.client_secret.into_unsecure(),
                        )),
                    )
                    .enable_openid_scope()
                    .set_redirect_uri(redirect_url);

                    Ok::<_, anyhow::Error>((issuer_url, client))
                }
            })
            .collect::<FuturesUnordered<_>>();

        let mut oidc_providers = Vec::with_capacity(config.api_access.openid_providers.len());
        while let Some(oidc_client) = oidc_clients.next().await {
            let oidc_client = match oidc_client {
                Ok((issuer_url, client)) => OidcClient { issuer_url, client },
                Err(err) => {
                    log::error!("Cannot initialize OpenID client: {err}");
                    return fairing::Result::Err(rocket);
                }
            };

            oidc_providers.push(oidc_client);
        }

        let issuers = config
            .api_access
            .openid_providers
            .iter()
            .map(|oidc| {
                let mut login_url = base_url.join("/auth/login").unwrap();
                login_url
                    .query_pairs_mut()
                    .append_pair("issuer", oidc.issuer_url.as_str());

                serde_json::json!({
                    "issuer": oidc.issuer_url,
                    "loginUrl": login_url,
                })
            })
            .collect::<Issuers>();

        fairing::Result::Ok(rocket.manage(oidc_providers).manage(issuers))
    }
}

impl Auth {
    pub fn fairing() -> Self {
        Self {}
    }
}

type OidcInners = Vec<OidcClient>;

struct OidcClient {
    issuer_url: IssuerUrl,
    client: openidconnect::core::CoreClient<
        EndpointSet,
        EndpointNotSet,
        EndpointNotSet,
        EndpointNotSet,
        EndpointMaybeSet,
        EndpointMaybeSet,
    >,
}

pub type Issuers = serde_json::Value;

pub enum User {
    Anonymous,
    Oidc {
        sub: openidconnect::SubjectIdentifier,
        iss: openidconnect::IssuerUrl,
        name: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SessionCookie {
    id_token: openidconnect::core::CoreIdToken,
    refresh_token: Option<RefreshToken>,
}

async fn refresh_token_due_to_id_token_expiry<'r>(
    oidc: &OidcClient,
    cookies: &CookieJar<'r>,
    oidc_session: &SessionCookie,
    id_token_verifier: &openidconnect::core::CoreIdTokenVerifier<'_>,
    expiry_err: String,
) -> Outcome<User, HttpApiProblem> {
    let Some(refresh_token) = &oidc_session.refresh_token else {
        cookies.remove("oidc_user_session");
        return Outcome::Error((
            Status::UnprocessableEntity,
            HttpApiProblem::with_title_and_type(StatusCode::UNAUTHORIZED).detail(expiry_err),
        ));
    };

    let http_client = reqwest::ClientBuilder::new()
        // Following redirects opens the client up to SSRF vulnerabilities.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("Client should build");

    let token_response = match oidc
        .client
        .exchange_refresh_token(&refresh_token)
        .expect("Request object should be buildable")
        .request_async(&http_client)
        .await
    {
        Ok(token_response) => token_response,
        Err(err) => {
            cookies.remove("oidc_user_session");
            return Outcome::Success(User::Anonymous);
        }
    };

    let session_cookie = SessionCookie {
        id_token: token_response.extra_fields().id_token().unwrap().clone(),
        refresh_token: Some(refresh_token.clone()),
    };

    cookies.add(
        Cookie::build((
            "oidc_user_session",
            serde_json::to_string(&session_cookie).unwrap(),
        ))
        // TODO .path(base_path.clone())
        .secure(true)
        .same_site(SameSite::Lax)
        .http_only(true)
        .build(),
    );

    match token_response
        .extra_fields()
        .id_token()
        .unwrap()
        .claims(&id_token_verifier, |_: Option<&Nonce>| Ok::<_, String>(()))
    {
        Ok(id_token_claims) => Outcome::Success(User::Oidc {
            sub: id_token_claims.subject().clone(),
            iss: id_token_claims.issuer().clone(),
            name: id_token_claims
                .name()
                .and_then(|ln| ln.get(None))
                .map(|name| name.to_string()),
        }),
        Err(err) => {
            cookies.remove("oidc_user_session");
            Outcome::Error((
                Status::UnprocessableEntity,
                HttpApiProblem::with_title_and_type(StatusCode::UNAUTHORIZED)
                    .detail(err.to_string()),
            ))
        }
    }
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for User {
    type Error = HttpApiProblem;

    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        let Some(auth) = request.rocket().state::<OidcInners>() else {
            return Outcome::Success(User::Anonymous);
        };

        let cookies = request.cookies();

        // TODO: do we want to have private cookies?
        let (Some(oidc), Some(serialized_session)) =
            (&auth.first(), cookies.get("oidc_user_session"))
        else {
            return Outcome::Success(User::Anonymous);
        };

        let Ok(oidc_session) = serde_json::from_str::<SessionCookie>(serialized_session.value())
        else {
            log::debug!("Cannot deserialize session cookie");
            return Outcome::Success(User::Anonymous);
        };

        let id_token_verifier: openidconnect::core::CoreIdTokenVerifier =
            oidc.client.id_token_verifier();

        match oidc_session
            .id_token
            .claims(&id_token_verifier, |_: Option<&Nonce>| Ok::<_, String>(()))
        {
            Ok(id_token_claims) => Outcome::Success(User::Oidc {
                sub: id_token_claims.subject().clone(),
                iss: id_token_claims.issuer().clone(),
                name: id_token_claims
                    .name()
                    .and_then(|ln| ln.get(None))
                    .map(|name| name.to_string()),
            }),
            Err(openidconnect::ClaimsVerificationError::Expired(err)) => {
                refresh_token_due_to_id_token_expiry(
                    oidc,
                    &cookies,
                    &oidc_session,
                    &id_token_verifier,
                    err,
                )
                .await
            }
            Err(err) => {
                cookies.remove("oidc_user_session");
                Outcome::Error((
                    Status::UnprocessableEntity,
                    HttpApiProblem::with_title_and_type(StatusCode::UNAUTHORIZED)
                        .detail(err.to_string()),
                ))
            }
        }
    }
}

/// This struct can be used to protect APIs so that only authenticated users are allowed to call
/// them if it has been configured.
pub struct UserValidatedByAccessMode {
    pub user: User,
    // pub access_mode: ApiAccessMode,
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for UserValidatedByAccessMode {
    type Error = HttpApiProblem;

    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        let user = match User::from_request(request).await {
            rocket::outcome::Outcome::Success(user) => user,
            rocket::outcome::Outcome::Error(outcome) => {
                return Outcome::Error(outcome);
            }
            rocket::outcome::Outcome::Forward(outcome) => {
                return Outcome::Forward(outcome);
            }
        };

        let Some(config) = request.rocket().state::<Config>() else {
            log::warn!("No configuration in rocket's state. Assuming no authentication.");
            return Outcome::Success(Self { user });
        };

        match (user, &config.api_access.mode) {
            (User::Anonymous, ApiAccessMode::RequireAuth) => Outcome::Error((
                Status::Forbidden,
                HttpApiProblem::with_title_and_type(StatusCode::FORBIDDEN),
            )),
            (user, _access_mode) => Outcome::Success(Self {
                user,
                // access_mode: access_mode.clone(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rocket::local::asynchronous::Client;

    #[rocket::get("/")]
    pub(super) async fn test_route(_user: User) -> &'static str {
        ""
    }

    #[tokio::test]
    async fn fail_to_create_fairing_because_access_mode_requires_auth_but_no_providers() {
        let config = crate::config_from_str!(
            r#"
            [apiAccess]
            mode = "requireAuth"
            openidProviders = []
            "#
        );
        let rocket = rocket::build()
            .manage(config)
            .mount("/", rocket::routes![test_route])
            .attach(Auth::fairing());

        let client = Client::tracked(rocket).await.expect_err("invalid rocket");

        assert!(matches!(
            client.kind(),
            rocket::error::ErrorKind::FailedFairings(_)
        ))
    }
}
