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
example, it can be injected into [GitLab CI][GitLab CI ID Token] or can be
[retrieved form Keycloak](https://stackoverflow.com/a/77880906/5088458).

```bash
export JWT_ID_TOKEN="…get the token from a secure place…"
# FYI: the combination of --config and echo make sure that you don't leak the
# token to another process on the same host
echo { "header=\"Authorization: Bearer ${JWT_ID_TOKEN}\""} \
   curl --config - -X DELETE https://prevant.local/apps/app_name
```

## Mapping ID Token Claims to Owners

In some situations the ID token, used for authenticating the request, is
generated on behalf of someone and thus, the `sub` claim doesn't match to the
user's `sub` claim on the PREvant dashboard. For example, [GitLab CI ID
tokens][GitLab CI ID Token] can contain the `user_id` but not in the `sub`
field.

In order to be able to have this mapping aligned, there is a hook that allows
transforming the ID token claims to the corrected owner. First, create a
Javascript file that will be mounted into the PREvant container. For example,
like this:

```javascript
function idTokenClaimsToOwnerHook(claims) {
   return {
      sub: claims.user_id ? claims.user_id : claims.sub,
      iss: claims.iss,
      name: claims.user_name ? claims.user_name : claims.name,
   };
}
```
Then, make sure that this file is configured as [hook](hooks.md):

```toml
[hooks]
idTokenClaimsToOwner = "/path/to/file.js"
```


[OpenID]: https://openid.net/
[Rocket-Cookie-Encryption]: https://api.rocket.rs/v0.5/rocket/http/struct.CookieJar#encryption-key
[Azure]: https://learn.microsoft.com/en-us/entra/identity-platform/v2-protocols-oidc
[Google]: https://developers.google.com/identity/openid-connect/openid-connect
[GitLab]: https://docs.gitlab.com/integration/openid_connect_provider/
[GitLab CI ID Token]: https://docs.gitlab.com/ci/yaml/#id_tokens
[Keycloak]: https://www.keycloak.org/
