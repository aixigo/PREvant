use super::{BaseUrl, Session};
use crate::http_result::HttpApiError;
use http::StatusCode;
use http_api_problem::HttpApiProblem;
use openidconnect::{
    core::CoreResponseType, AuthenticationFlow, AuthorizationCode, CsrfToken, IssuerUrl, Nonce,
    OAuth2TokenResponse,
};
use rocket::{
    get,
    http::{Cookie, CookieJar, SameSite},
    response::Redirect,
    routes, State,
};

#[cfg(not(debug_assertions))]
pub fn auth_routes() -> Vec<rocket::Route> {
    routes![openid_login, openid_response]
}

#[cfg(debug_assertions)]
pub fn auth_routes() -> Vec<rocket::Route> {
    routes![openid_login, openid_response, me, issuers]
}

#[cfg(debug_assertions)]
#[get("/me", format = "application/json")]
fn me(user: super::User) -> serde_json::Value {
    match user {
        super::User::Anonymous => serde_json::Value::Null,
        super::User::Oidc { sub, iss, name } => serde_json::json!({
            "sub": sub,
            "iss": iss,
            "name": name
        }),
    }
}

#[cfg(debug_assertions)]
#[get("/issuers", format = "application/json")]
fn issuers<'res, 'req: 'res>(
    issuers: &'req State<super::Issuers>,
) -> rocket::serde::json::Json<&'res serde_json::Value> {
    rocket::serde::json::Json(&issuers)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthorizeSession {
    nonce: Nonce,
    issuer_url: IssuerUrl,
}

static AUTHORIZE_SESSION_COOKIE_NAME: &str = "prevant_authorize_session";
impl AuthorizeSession {
    fn to_cookie<'r>(self, base_url: &BaseUrl) -> Cookie<'r> {
        Cookie::build((
            AUTHORIZE_SESSION_COOKIE_NAME,
            serde_json::to_string(&self).unwrap(),
        ))
        .domain(base_url.0.domain().unwrap().to_string())
        .path(format!("{}auth/", base_url.0.path()))
        .secure(true)
        .same_site(SameSite::Lax)
        .http_only(true)
        .build()
    }
}

#[get("/login?<issuer>")]
fn openid_login(
    openid_providers: &State<super::OidcInners>,
    cookie_jar: &CookieJar<'_>,
    prevant_base_url: &State<BaseUrl>,
    issuer: &str,
) -> Redirect {
    let base_path = prevant_base_url.0.path();

    assert!(
        base_path.ends_with("/"),
        "The base path needs to end with a slash"
    );

    let Some(oidc_provider) = openid_providers
        .iter()
        .find(|provider| provider.issuer_url.as_str() == issuer)
    else {
        return Redirect::to(base_path.to_string());
    };

    // TODO crsf state??
    let (authorize_url, _csrf_state, nonce) = oidc_provider
        .client
        .authorize_url(
            AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .url();

    cookie_jar.add_private(
        AuthorizeSession {
            nonce,
            issuer_url: oidc_provider.issuer_url.clone(),
        }
        .to_cookie(prevant_base_url),
    );

    Redirect::to(authorize_url.to_string())
}

#[get("/oidc-response?<code>")]
async fn openid_response(
    openid_providers: &State<super::OidcInners>,
    code: &str,
    cookie_jar: &CookieJar<'_>,
    prevant_base_url: &State<BaseUrl>,
) -> Result<Redirect, HttpApiError> {
    let base_path = prevant_base_url.0.path();

    assert!(
        base_path.ends_with("/"),
        "The base path needs to end with a slash"
    );

    let Some(authorize_session) = cookie_jar
        .get_private(AUTHORIZE_SESSION_COOKIE_NAME)
        .and_then(|session| serde_json::from_str::<AuthorizeSession>(&session.value()).ok())
    else {
        return Ok(Redirect::to(format!("{base_path}auth/login")));
    };

    let Some(oidc_provider) = openid_providers
        .iter()
        .find(|provider| provider.issuer_url == authorize_session.issuer_url)
    else {
        return Ok(Redirect::to(base_path.to_string()));
    };

    let http_client = reqwest::ClientBuilder::new()
        // Following redirects opens the client up to SSRF vulnerabilities.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("Client should build");

    let token_response = oidc_provider
        .client
        .exchange_code(AuthorizationCode::new(code.to_string()))
        .unwrap()
        .request_async(&http_client)
        .await
        .map_err(|err| {
            HttpApiProblem::with_title_and_type(StatusCode::INTERNAL_SERVER_ERROR)
                .detail(err.to_string())
        })?;

    let Some(id_token) = token_response.extra_fields().id_token() else {
        return Err(
            HttpApiProblem::with_title_and_type(StatusCode::INTERNAL_SERVER_ERROR)
                .detail("Token response did not include ID token.")
                .into(),
        );
    };

    if let Err(err) = id_token.claims(
        &oidc_provider.client.id_token_verifier(),
        &authorize_session.nonce,
    ) {
        return Err(
            HttpApiProblem::with_title_and_type(StatusCode::UNPROCESSABLE_ENTITY)
                .detail(err.to_string())
                .into(),
        );
    }

    cookie_jar.remove_private(AUTHORIZE_SESSION_COOKIE_NAME);

    let session_cookie = Session {
        id_token: id_token.clone(),
        refresh_token: token_response.refresh_token().cloned(),
    };

    cookie_jar.add_private(session_cookie.to_cookie(prevant_base_url));

    Ok(Redirect::to(base_path.to_string()))
}
