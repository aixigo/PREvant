use crate::{
    config::{ApiAccessMode, Config},
    infrastructure::TraefikIngressRoute,
};
use anyhow::Context;
use futures::{stream::FuturesUnordered, StreamExt as _};
use http::StatusCode;
use http_api_problem::HttpApiProblem;
use openidconnect::{
    core::{CoreGenderClaim, CoreProviderMetadata},
    Client, ClientId, ClientSecret, EndpointMaybeSet, EndpointNotSet, EndpointSet, IssuerUrl,
    Nonce, RedirectUrl, RefreshToken,
};
use rocket::{
    fairing::{self, Fairing, Info, Kind},
    http::{Cookie, CookieJar, SameSite, Status},
    request::{FromRequest, Outcome},
    Build, Request, Rocket,
};
pub use routes::auth_routes;
use std::{convert::TryFrom, str::FromStr};
use url::Url;
pub use user::{AdditionalClaims, User};

mod routes;
mod user;

pub struct Auth {}
struct BaseUrl(Url);

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

        let base_url = base_url(&rocket);
        let redirect_url = base_url.0.join("auth/oidc-response").unwrap();

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
                            oidc_provider.client_secret.0.into_unsecure(),
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

        if !oidc_providers.is_empty() {
            log::debug!("Using OIDC authentication with redirect url {redirect_url}");
        }

        let issuers = config
            .api_access
            .openid_providers
            .iter()
            .map(|oidc| {
                let mut login_url = base_url.0.join("/auth/login").unwrap();
                login_url
                    .query_pairs_mut()
                    .append_pair("issuer", oidc.issuer_url.as_str());

                serde_json::json!({
                    "issuer": oidc.issuer_url,
                    "loginUrl": login_url,
                })
            })
            .collect::<Issuers>();

        fairing::Result::Ok(
            rocket
                .manage(oidc_providers)
                .manage(issuers)
                .manage(base_url),
        )
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
    client: openidconnect::Client<
        AdditionalClaims,
        openidconnect::core::CoreAuthDisplay,
        openidconnect::core::CoreGenderClaim,
        openidconnect::core::CoreJweContentEncryptionAlgorithm,
        openidconnect::core::CoreJsonWebKey,
        openidconnect::core::CoreAuthPrompt,
        openidconnect::StandardErrorResponse<openidconnect::core::CoreErrorResponseType>,
        openidconnect::StandardTokenResponse<
            openidconnect::IdTokenFields<
                AdditionalClaims,
                AdditionalClaims,
                CoreGenderClaim,
                openidconnect::core::CoreJweContentEncryptionAlgorithm,
                openidconnect::core::CoreJwsSigningAlgorithm,
            >,
            openidconnect::core::CoreTokenType,
        >,
        openidconnect::core::CoreTokenIntrospectionResponse,
        openidconnect::core::CoreRevocableToken,
        openidconnect::core::CoreRevocationErrorResponse,
        EndpointSet,
        EndpointNotSet,
        EndpointNotSet,
        EndpointNotSet,
        EndpointMaybeSet,
        EndpointMaybeSet,
    >,
}

pub type Issuers = serde_json::Value;
type IdToken = openidconnect::IdToken<
    AdditionalClaims,
    CoreGenderClaim,
    openidconnect::core::CoreJweContentEncryptionAlgorithm,
    openidconnect::core::CoreJwsSigningAlgorithm,
>;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    id_token: IdToken,
    refresh_token: Option<RefreshToken>,
}

static SESSION_COOKIE_NAME: &str = "prevant_oidc_user_session";
impl Session {
    fn to_cookie<'r>(self, base_url: &BaseUrl) -> Cookie<'r> {
        Cookie::build((SESSION_COOKIE_NAME, serde_json::to_string(&self).unwrap()))
            .domain(base_url.0.domain().unwrap().to_string())
            .path(base_url.0.path().to_string())
            .secure(true)
            .same_site(SameSite::Lax)
            .http_only(true)
            .build()
    }

    fn parse_issuer_url(&self) -> Result<IssuerUrl, String> {
        use base64::prelude::*;

        let jwt = self.id_token.to_string();
        let mut base64_payload_it = jwt.as_str().split('.');

        base64_payload_it.next();
        let base64_payload = base64_payload_it
            .next()
            .ok_or_else(|| String::from("expecting dot separated JWT data"))?
            .to_string();

        let payload = serde_json::from_slice::<serde_json::Value>(
            &BASE64_URL_SAFE_NO_PAD
                .decode(base64_payload)
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        payload
            .get("iss")
            .and_then(|iss| iss.as_str())
            .map(|iss| IssuerUrl::new(iss.to_string()).map_err(|e| e.to_string()))
            .ok_or_else(|| String::from("No issuer field"))?
    }
}

impl TryFrom<&Request<'_>> for Session {
    type Error = String;

    fn try_from(request: &Request<'_>) -> Result<Self, Self::Error> {
        if let Some(session_cookie) = request.cookies().get_private(SESSION_COOKIE_NAME) {
            return serde_json::from_str::<Session>(session_cookie.value())
                .map_err(|err| format!("Cannot deserialize session cookie: {err}"));
        }

        if let Some(authorization_header) = request
            .headers()
            .get(http::header::AUTHORIZATION.as_str())
            .next()
        {
            if authorization_header
                .to_ascii_uppercase()
                .starts_with("BEARER ")
            {
                let id_token =
                    IdToken::from_str(authorization_header[7..].trim()).map_err(|err| {
                        format!("Cannot deserialize id token from Authorization header: {err}")
                    })?;

                return Ok(Session {
                    id_token,
                    refresh_token: None,
                });
            }
        }

        Err(String::from("No authentication information"))
    }
}

fn base_url<P>(rocket: &Rocket<P>) -> BaseUrl
where
    P: rocket::Phase,
{
    BaseUrl(
        rocket
            .state::<Option<TraefikIngressRoute>>()
            .cloned()
            .flatten()
            .and_then(|base_route| base_route.to_url())
            .unwrap_or_else(|| Url::from_str("http://localhost:8000").unwrap()),
    )
}

async fn refresh_token_due_to_id_token_expiry<'r>(
    oidc: &OidcClient,
    cookies: &CookieJar<'r>,
    refresh_token: &RefreshToken,
    id_token_verifier: &openidconnect::core::CoreIdTokenVerifier<'_>,
    base_url: &BaseUrl,
) -> Outcome<User, HttpApiProblem> {
    let http_client = reqwest::ClientBuilder::new()
        // Following redirects opens the client up to SSRF vulnerabilities.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("Client should build");

    let token_response = match oidc
        .client
        .exchange_refresh_token(refresh_token)
        .expect("Request object should be buildable")
        .request_async(&http_client)
        .await
    {
        Ok(token_response) => token_response,
        Err(err) => {
            log::debug!("Cannot refresh ID token: {err}");
            cookies.remove_private(SESSION_COOKIE_NAME);
            return Outcome::Success(User::Anonymous);
        }
    };

    let session_cookie = Session {
        id_token: token_response.extra_fields().id_token().unwrap().clone(),
        refresh_token: Some(refresh_token.clone()),
    };

    cookies.add_private(session_cookie.to_cookie(base_url));

    match token_response
        .extra_fields()
        .id_token()
        .unwrap()
        .claims(id_token_verifier, |_: Option<&Nonce>| Ok::<_, String>(()))
    {
        Ok(id_token_claims) => Outcome::Success(User::Oidc {
            id_token_claims: id_token_claims.clone(),
        }),
        Err(err) => {
            cookies.remove_private(SESSION_COOKIE_NAME);
            Outcome::Error((
                Status::UnprocessableEntity,
                HttpApiProblem::with_title_and_type(StatusCode::UNAUTHORIZED)
                    .detail(format!("Cannot verify token: {err}")),
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

        let session = match Session::try_from(request) {
            Ok(session) => session,
            Err(err) => {
                log::debug!("Cannot authenticate client: {err}");
                return Outcome::Success(User::Anonymous);
            }
        };

        let issuer_url = match session.parse_issuer_url() {
            Ok(issuer_url) => issuer_url,
            Err(err) => {
                cookies.remove_private(SESSION_COOKIE_NAME);
                return Outcome::Error((
                    Status::Unauthorized,
                    HttpApiProblem::with_title_and_type(StatusCode::UNAUTHORIZED)
                        .detail(format!("Cannot parse issuer URL from token: {err}")),
                ));
            }
        };
        let Some(oidc) = auth.iter().find(|oidc| oidc.issuer_url == issuer_url) else {
            log::debug!("Cannot find issuere matching {issuer_url}");
            return Outcome::Success(User::Anonymous);
        };

        let id_token_verifier = oidc.client.id_token_verifier();

        match session
            .id_token
            .claims(&id_token_verifier, |_: Option<&Nonce>| Ok::<_, String>(()))
        {
            Ok(id_token_claims) => Outcome::Success(User::Oidc {
                id_token_claims: id_token_claims.clone(),
            }),
            Err(openidconnect::ClaimsVerificationError::Expired(err)) => {
                let base_url = request
                    .rocket()
                    .state::<BaseUrl>()
                    .expect("Must manage the base_url");

                let Some(refresh_token) = &session.refresh_token else {
                    cookies.remove_private(SESSION_COOKIE_NAME);
                    return Outcome::Error((
                        Status::Unauthorized,
                        HttpApiProblem::with_title_and_type(StatusCode::UNAUTHORIZED)
                            .detail(format!("The session expired: {err}")),
                    ));
                };

                refresh_token_due_to_id_token_expiry(
                    oidc,
                    cookies,
                    refresh_token,
                    &id_token_verifier,
                    base_url,
                )
                .await
            }
            Err(err) => {
                cookies.remove_private(SESSION_COOKIE_NAME);
                Outcome::Error((
                    Status::Unauthorized,
                    HttpApiProblem::with_title_and_type(StatusCode::UNAUTHORIZED)
                        .detail(format!("Cannot verify token: {err}")),
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
