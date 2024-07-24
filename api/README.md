The image `aixigo/prevant` provides the REST-API in order to deploy containers and to compose them into reviewable application.

# Configuration

In order to configure PREvant create a [TOML](https://github.com/toml-lang/toml) file that is mounted to the container's path `/app/config.toml` (path can be changed by the CLI option `--config`). Additionally, PREvant utilizes [figment][1] to read configuration options from file, environment variable, and from some CLI options.

## Runtime Configuration

### Kubernetes

```toml
[runtime]
type = 'Kubernetes'

# This map of annotations allow to add additionall annotations to Kubernetes namespaces that will be created
# through PREVant. In this example, the annotations will be used to connect the namespaces to a Rancher project.
# Futher information is provided here: https://stackoverflow.com/a/74405246/5088458
[runtime.annotations.namespace]
'field.cattle.io/projectId' = 'rancher-project-id'

[runtime.downwardApi]
# Path to the file that contains the labels that have been assigned to the PREvant deployemnt itself.
# This information is crucial if you run PREvant behind a Traefik instance that enforces the user ot be
# logged in.
labelsPath = '/run/podinfo/labels'

[runtime.storageConfig]
# Size of the storage space that is reserved and mounted to the deployed companion with storage.
# If unspecified storage is defaulted to 2g.
storageSize = '10g'
# Storage class denotes the type of storage to be used for companions deployed with storage.
# Manually managed storage classes can be specified here. If unspecified default storage class will be used.
storageClass = 'local-path'
```

## Container Options

Create a table `containers` with following options:

```toml
[containers]

# Restrict memory usage of containers
memory_limit = '1g'
```

## Issue Tracking options

Application names are compared to issues which will be linked to cards on the frontend. Therefore, the REST backend needs to be able to compare the application names with issue tracking information.

Currently, Jira is supported as the tracking system.

```toml
[jira]
host = 'https://jira.example.com'
apiKey = ''
```

```toml
[jira]
host = 'https://jira.example.com'
user = ''
password = ''
```

## Services

PREvant offers centralized configuration options for services deployed via its REST-API. For example, you can specify that PREvant mounts a secret for a specific service of an application.

### Secrets

Similar to secrets in [Docker Swarm](https://docs.docker.com/engine/swarm/secrets/) and [Docker-Compose](https://docs.docker.com/compose/compose-file/#secrets), PREvant mounts secrets under `/run/secrets/<secret_name>`. Therefore, you can use the following configuration section to define secrets for each service.

Following example provides two secrets for the service `nginx`, mounted at `/run/secrets/cert.pem` and `/run/secrets/key.pem`, available for the application `master`.

```toml
[services.nginx]
[[services.nginx.secrets]]
# The name of the secret mounted in the container.
name = "cert.pem"
# base64 encoded value of the secret
data = "LS0tLS1CRUdJTiBDRVJUSUZJQ0FURS…FUlRJRklDQVRFLS0tLS0K"
# An optional regular expression that checks if the secret has to be
# mounted for this service. Default is ".+" (any app)
appSelector = "master"
# An optional path that points to the secret's parent directory.
# Default is "/run/secrets"
path = "/run/secrets"

[[services.nginx.secrets]]
name = "key.pem"
data = "LS0tLS1CRUdJTiBFTkNSWVBURUQgUF…JVkFURSBLRVktLS0tLQo="
```

## Companions

See [here](../docs/companions.md) how to configure companions.

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

## Registries

Private registries require login information, therefore, PREvant offers authentication for secured registries. Add following block to your configuration file:

```toml
[registries.'docker.io']
username = "user"
password = "pass"

[registries.'registry.gitlab.com']
username = "oauth2"
password = "your-private-token"
```

## Configure With Environment Variables

As stated above, PREvant utilizes [figment][1] to resolve configuration values from file, environment variables, and CLI options. The following examples provide a reference how to use environment variables to configure PREvant:

```bash
# Configure downwardApi label path
export PREVANT_RUNTIME='{downwardApi={labelsPath="/random"}}'

# Use GitLab token to pull images from a private registry
export PREVANT_REGISTRIES='{"registry.gitlab.com"={username="oauth2",password="your-private-token"}}'
```

[1]: https://docs.rs/figment/latest/figment/#overview
