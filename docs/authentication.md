# Authentication

It is recommended to run PREvant with authentication enabled because without
authentication any HTTP client can deploy any OCI image to your infrastructure
(Docker or Kubernetes). Therefore, PREvant offers the authentication via
[OpenID]. Example providers are [GitLab], [Google], [Azure] or [Keycloak]. With
authentication configured, PREvant also tracks the owners of each application.
Meaning, that you see in the dashboard who created or modified with application.

When at least one OpenID provider is configured (you can configure multiple
providers, see example below paragraph), requests will be authenticated by
PREvant via secure browser cookie. The PREvant dashboard will provide a login
button for each OpenID provider.

```toml
[[apiAccess.openidProviders]]
issuerUrl = "https://gitlab.com"
clientId = "…"
# Value will be read from the environment variable SOME_VAR_NAME.
clientSecret = "${env:SOME_VAR_NAME}"
```

Also make sure, to set a random encryption key to ensure that cookies survive
restarts (see [explanation of PREvant web framework][Rocket-Cookie-Encryption])
by setting the environment variable.

```bash
export ROCKET_SECRET_KEY=$(openssl rand -base64 32)
```

To interact with the PREvant REST API with authentication one can send an HTTP
request with an `Authorization` header (see example below). The header must
contain the ID token and can be retrieve usually from the OpenID provider. For
example, it can be injected into [GitLab
CI](https://docs.gitlab.com/ci/yaml/#id_tokens) or can be [retrieved form
Keycloak](https://stackoverflow.com/a/77880906/5088458).

```bash
export JWT_ID_TOKEN="…get the token from a secure place…"
# FYI: the combination of --config and echo make sure that you don't leak the
# token to another process on the same host
echo { "header=\"Authorization: Bearer ${JWT_ID_TOKEN}\""} \
   curl --config - -X DELETE https://prevant.local/apps/app_name
```

[OpenID]: https://openid.net/
[Rocket-Cookie-Encryption]: https://api.rocket.rs/v0.5/rocket/http/struct.CookieJar#encryption-key
[Azure]: https://learn.microsoft.com/en-us/entra/identity-platform/v2-protocols-oidc
[Google]: https://developers.google.com/identity/openid-connect/openid-connect
[GitLab]: https://docs.gitlab.com/integration/openid_connect_provider/
[Keycloak]: https://www.keycloak.org/
