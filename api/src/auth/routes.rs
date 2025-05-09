use super::SessionCookie;
use crate::http_result::HttpApiError;
use http::StatusCode;
use http_api_problem::HttpApiProblem;
use openidconnect::{
    core::CoreResponseType, AuthenticationFlow, AuthorizationCode, CsrfToken, Nonce,
};
use rocket::{
    get,
    http::{Cookie, CookieJar, SameSite},
    response::Redirect,
    routes, State,
};

pub fn auth_routes() -> Vec<rocket::Route> {
    routes![openid_login, openid_response]
}

#[get("/login")]
fn openid_login(
    openid_providers: &State<super::OidcInners>,
    cookie_jar: &CookieJar<'_>,
) -> Redirect {
    // TODO: how to select the OpenID provider?
    let Some(oidc_provider) = openid_providers.first() else {
        // TODO: the PREvant base_path should be used.
        return Redirect::to("/");
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

    cookie_jar.add(
        Cookie::build(("oidc_nonce", nonce.secret().to_string()))
            // TODO: the PREvant base_path should be used.
            .path("/auth/")
            .secure(true)
            .same_site(SameSite::Lax)
            .http_only(true)
            .build(),
    );

    Redirect::to(authorize_url.to_string())
}

#[get("/oidc-response?<code>")]
async fn openid_response(
    openid_providers: &State<super::OidcInners>,
    code: &str,
    cookie_jar: &CookieJar<'_>,
) -> Result<Redirect, HttpApiError> {
    let Some(nonce) = cookie_jar
        .get("oidc_nonce")
        .map(|nonce| Nonce::new(nonce.value().to_string()))
    else {
        // TODO: the PREvant base_path should be used.
        return Ok(Redirect::to("/auth/login"));
    };

    // TODO: how to select the OpenID provider?
    let Some(oidc_provider) = openid_providers.first() else {
        // TODO: the PREvant base_path should be used.
        return Ok(Redirect::to("/"));
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

    if let Err(err) = id_token.claims(&oidc_provider.client.id_token_verifier(), &nonce) {
        return Err(
            HttpApiProblem::with_title_and_type(StatusCode::UNPROCESSABLE_ENTITY)
                .detail(err.to_string())
                .into(),
        );
    }

    cookie_jar.remove("oidc_nonce");

    let session_cookie = SessionCookie {
        id_token: id_token.clone(),
    };

    cookie_jar.add(
        Cookie::build((
            "oidc_user_session",
            serde_json::to_string(&session_cookie).unwrap(),
        ))
        // TODO: the PREvant base_path should be used.
        .path("/")
        .secure(true)
        .same_site(SameSite::Lax)
        .http_only(true)
        .build(),
    );

    // TODO: the PREvant base_path should be used.
    Ok(Redirect::to("/"))
}
