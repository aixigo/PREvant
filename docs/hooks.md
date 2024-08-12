## Hooks

Hooks can be used to manipulate the deployment before handing it over to actual infrastructure and they are able to manipulate all service configurations once for any deployment REST API call. For example, based on the deployment's app name you can decide to reconfigure your services to use a different DBMS so that you are able to verify that your services work with different DBMSs.

Technically, hooks are Javascript files that provide functions to modify all service configurations of a deployment. For example, add following section to your PREvant configuration. This configuration snippet enables the _deployemnt hook_ that will be used to modify the services' configurations.

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

Note: the templating has been already applied and the hook has only access to the final value and it cannot modify the original values.

Note: the hook's Javascript engine is based on [Boa](https://github.com/boa-dev/boa) and this engine does not implement all ECMAScript features yet.

PREvant calls this function with the app name (see `appName`) and an array of service configurations (see `serviceConfigs`). This array can be modified and must be returned by the function. The elements in the array are object with following fields:

| Key           | Description                                                                                                |
|---------------|------------------------------------------------------------------------------------------------------------|
| `name`        | The service name (readonly).                                                                               |
| `image`       | The OCI image of this service (readonly).                                                                  |
| `type`        | The type of the service, e.g. `instance`, `replica`, etc. (readonly).                                      |
| `env`         | A map of key and value containing the environment variables that will be used when creating the container. |
| `files`       | A map of key and value containing the files that will be mounted into the container.                       |
