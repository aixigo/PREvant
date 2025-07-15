# Hooks

Note: the hook's Javascript engine is based on
[Boa](https://github.com/boa-dev/boa) and this engine does not implement all
ECMAScript features yet.

## Deployment

Hooks can be used to manipulate the deployment before handing it over to actual
infrastructure and they are able to manipulate all service configurations once
for any deployment REST API call. For example, based on the deployment's app
name you can decide to reconfigure your services to use a different DBMS so
that you are able to verify that your services work with different DBMSs.

Technically, hooks are Javascript files that provide functions to modify all
service configurations of a deployment. For example, add following section to
your PREvant configuration. This configuration snippet enables the _deployemnt
hook_ that will be used to modify the services' configurations.

```toml
[hooks]
deployment = 'path/to/hook.js'
```

The hook at `path/to/hook.js` must provide following Javascript function:

```javascript
function deploymentHook(appName, serviceConfigs) {
  return serviceConfigs.filter( _ => true );
}
```

Note: the templating has been already applied and the hook has only access to
the final value and it cannot modify the original values.

PREvant calls this function with the app name (see `appName`) and an array of
service configurations (see `serviceConfigs`). This array can be modified and
must be returned by the function. The elements in the array are object with
following fields:

| Key           | Description                                                                                                |
|---------------|------------------------------------------------------------------------------------------------------------|
| `name`        | The service name (readonly).                                                                               |
| `image`       | The OCI image of this service (readonly).                                                                  |
| `type`        | The type of the service, e.g. `instance`, `replica`, etc. (readonly).                                      |
| `env`         | A map of key and value containing the environment variables that will be used when creating the container. |
| `files`       | A map of key and value containing the files that will be mounted into the container.                       |

## ID Token Claims to Owner Mapping

```toml
[hooks]
idTokenClaimsToOwner = 'path/to/hook.js'
```

The hook at `path/to/hook.js` must provide following Javascript function:

```javascript
function idTokenClaimsToOwnerHook(claims) {
   return {
      sub: claims.user_id ? claims.user_id : claims.sub,
      iss: claims.iss,
      name: claims.user_name ? claims.user_name : claims.name,
   };
}
```

PREvant calls this function with the [ID token claims] (see parameter `claims`)
as first argument, serialized as the JWT payload, and it expects a return value
of the following form:

```typescript
interface Owner {
   // sub & iss will be compared against the ID token claims of the user logged
   // in at the dashboard.
   sub: string,
   iss: string,
   name?: string,
}
```

[ID token claims]: https://auth0.com/docs/secure/tokens/id-tokens/id-token-structure
