use crate::{
    config::{ApiAccessMode, Config},
    infrastructure::TraefikIngressRoute,
};
use anyhow::Context;
use futures::{stream::FuturesUnordered, StreamExt as _};
use http::StatusCode;
use http_api_problem::HttpApiProblem;
use openidconnect::{
    core::{
        CoreAuthDisplay, CoreAuthPrompt, CoreGenderClaim, CoreJsonWebKey,
        CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm, CoreProviderMetadata,
    },
    Client, ClientId, ClientSecret, EmptyAdditionalClaims, EmptyExtraTokenFields, EndpointMaybeSet,
    EndpointNotSet, EndpointSet, IdTokenFields, IntrospectionUrl, IssuerUrl, Nonce, RedirectUrl,
    RefreshToken, RevocationErrorResponseType, StandardErrorResponse, StandardTokenResponse,
};
use rocket::{
    fairing::{self, Fairing, Info, Kind},
    http::{Cookie, SameSite, Status},
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

        if matches!(config.api_access.mode, ApiAccessMode::Any) {
            log::warn!("There is no API protection configured which let any API client deploy any OCI image on your infrastructure.");
            return fairing::Result::Ok(rocket.manage(Vec::<OidcClient>::new()));
        }

        let base_url = rocket
            .state::<Option<TraefikIngressRoute>>()
            .cloned()
            .flatten();

        let redirect_url = base_url
            .and_then(|base_route| base_route.to_url())
            .unwrap_or_else(|| Url::from_str("http://localhost:8000").unwrap())
            .join("auth/oidc-response")
            .unwrap();

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

                    let provider_metadata = CoreProviderMetadata::discover_async(
                        IssuerUrl::new(oidc_provider.issuer_url.clone()).with_context(|| {
                            format!("Invalid issuer URL: {}", oidc_provider.issuer_url)
                        })?,
                        &http_client,
                    )
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

                    // TODO: discover and then mabyeset????
                    let client = client.set_introspection_url(
                        IntrospectionUrl::new(String::from("https://gitlab.com/oauth/introspect"))
                            .unwrap(),
                    );
                    Ok::<_, anyhow::Error>(client)
                }
            })
            .collect::<FuturesUnordered<_>>();

        let mut oidc_providers = Vec::with_capacity(config.api_access.openid_providers.len());
        while let Some(oidc_client) = oidc_clients.next().await {
            let oidc_client = match oidc_client {
                Ok(oidc_client) => oidc_client,
                Err(err) => {
                    log::error!("Cannot initialize OpenID client: {err}");
                    return fairing::Result::Err(rocket);
                }
            };

            oidc_providers.push(OidcClient {
                client: oidc_client,
            });
        }

        if matches!(config.api_access.mode, ApiAccessMode::RequireAuth if oidc_providers.is_empty())
        {
            // TODO: have a good error message here
            log::error!("");
            fairing::Result::Err(rocket)
        } else {
            fairing::Result::Ok(rocket.manage(oidc_providers))
        }
    }
}

impl Auth {
    pub fn fairing() -> Self {
        Self {}
    }
}

type OidcInners = Vec<OidcClient>;

struct OidcClient {
    client: Client<
        EmptyAdditionalClaims,
        CoreAuthDisplay,
        CoreGenderClaim,
        CoreJweContentEncryptionAlgorithm,
        CoreJsonWebKey,
        CoreAuthPrompt,
        StandardErrorResponse<openidconnect::core::CoreErrorResponseType>,
        StandardTokenResponse<
            IdTokenFields<
                EmptyAdditionalClaims,
                EmptyExtraTokenFields,
                CoreGenderClaim,
                CoreJweContentEncryptionAlgorithm,
                CoreJwsSigningAlgorithm,
            >,
            openidconnect::core::CoreTokenType,
        >,
        openidconnect::StandardTokenIntrospectionResponse<
            EmptyExtraTokenFields,
            openidconnect::core::CoreTokenType,
        >,
        openidconnect::core::CoreRevocableToken,
        StandardErrorResponse<RevocationErrorResponseType>,
        EndpointSet,
        EndpointNotSet,
        EndpointSet,
        EndpointNotSet,
        EndpointMaybeSet,
        EndpointMaybeSet,
    >,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum User {
    Anonymous,
    Oidc {
        sub: openidconnect::SubjectIdentifier,
        iss: openidconnect::IssuerUrl,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SessionCookie {
    id_token: openidconnect::core::CoreIdToken,
    refresh_token: Option<RefreshToken>,
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for User {
    type Error = HttpApiProblem;

    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        let Some(auth) = request.rocket().state::<OidcInners>() else {
            return Outcome::Success(User::Anonymous);
        };

        let cookies = request.cookies();

        if let Some(oidc) = &auth.first() {
            // TODO: do we want to have private cookies?
            if let Some(serialized_session) = cookies.get("oidc_user_session") {
                if let Ok(oidc_session) =
                    serde_json::from_str::<SessionCookie>(serialized_session.value())
                {
                    let id_token_verifier: openidconnect::core::CoreIdTokenVerifier =
                        oidc.client.id_token_verifier();

                    let id_token_claims = match oidc_session
                        .id_token
                        .into_claims(&id_token_verifier, |_: Option<&Nonce>| Ok::<_, String>(()))
                    {
                        Ok(id_token_claims) => id_token_claims,
                        Err(openidconnect::ClaimsVerificationError::Expired(err)) => {
                            let Some(refresh_token) = oidc_session.refresh_token else {
                                cookies.remove("oidc_user_session");
                                return Outcome::Error((
                                    Status::UnprocessableEntity,
                                    HttpApiProblem::with_title_and_type(StatusCode::UNAUTHORIZED)
                                        .detail(err.to_string()),
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
                                // TODO: Error handling
                                .unwrap()
                                .request_async(&http_client)
                                .await
                            {
                                Ok(token_response) => token_response,
                                Err(err) => {
                                    todo!("{err}")
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

                            let id_token_claims = token_response
                                .extra_fields()
                                .id_token()
                                .unwrap()
                                .claims(&id_token_verifier, |_: Option<&Nonce>| Ok::<_, String>(()))
                                .unwrap();

                            return Outcome::Success(User::Oidc {
                                sub: id_token_claims.subject().clone(),
                                iss: id_token_claims.issuer().clone(),
                            });
                        }
                        Err(err) => {
                            cookies.remove("oidc_user_session");
                            return Outcome::Error((
                                Status::UnprocessableEntity,
                                HttpApiProblem::with_title_and_type(StatusCode::UNAUTHORIZED)
                                    .detail(err.to_string()),
                            ));
                        }
                    };

                    return Outcome::Success(User::Oidc {
                        sub: id_token_claims.subject().clone(),
                        iss: id_token_claims.issuer().clone(),
                    });
                }
            }
        }

        Outcome::Success(User::Anonymous)
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
