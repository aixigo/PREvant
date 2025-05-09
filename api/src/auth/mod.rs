use crate::{
    apps::AppsService,
    config::{ApiAccessMode, Config},
};
use anyhow::Context;
use futures::{stream::FuturesUnordered, StreamExt as _};
use http::StatusCode;
use http_api_problem::HttpApiProblem;
use openidconnect::{
    core::{CoreClient, CoreProviderMetadata},
    ClientId, ClientSecret, EndpointMaybeSet, EndpointNotSet, EndpointSet, IssuerUrl, Nonce,
    RedirectUrl,
};
use rocket::{
    fairing::{self, Fairing, Info, Kind},
    http::Status,
    request::{FromRequest, Outcome},
    Build, Request, Rocket,
};
pub use routes::auth_routes;
use std::{str::FromStr, sync::Arc};
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
        let Some(apps) = rocket.state::<Arc<AppsService>>() else {
            log::error!("There is no apps in Rocket's state.");
            return fairing::Result::Err(rocket);
        };

        // TODO: where to resolve the base URL, at start up in a new fairing which might also
        // update if the PREvant ingress changed? If it updates, we should also need to recreate
        // the oidc_clients
        let base_url = apps.base_url().await;

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

                    let client = CoreClient::from_provider_metadata(
                        provider_metadata,
                        ClientId::new(oidc_provider.client_id),
                        Some(ClientSecret::new(
                            oidc_provider.client_secret.into_unsecure(),
                        )),
                    )
                    .enable_openid_scope()
                    .set_redirect_uri(redirect_url);

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

        fairing::Result::Ok(rocket.manage(oidc_providers))
    }
}

impl Auth {
    pub fn fairing() -> Self {
        Self {}
    }
}

type OidcInners = Vec<OidcClient>;

struct OidcClient {
    client: openidconnect::core::CoreClient<
        EndpointSet,
        EndpointNotSet,
        EndpointNotSet,
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
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SessionCookie {
    id_token: openidconnect::core::CoreIdToken,
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
                        .claims(&id_token_verifier, |_: Option<&Nonce>| Ok::<_, String>(()))
                    {
                        Ok(id_token_claims) => id_token_claims,
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

        let config = request.rocket().state::<Config>().unwrap();

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
