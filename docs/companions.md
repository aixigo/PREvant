# Companions

A companion is a service that is running inside an application

It is possible to start containers that will be started when the client requests to create a new service. For example, if the application requires an [OpenID](https://en.wikipedia.org/wiki/OpenID_Connect) provider, it is possible to create a configuration that starts the provider for each application. Another use case might be a Kafka services that is required by the application.

Furthermore, it is also possible to create containers for each service. For example, for each service a database container could be started.

For these use cases following sections provide example configurations.

## Application Wide

If you want to include an OpenID provider for every application, you could use following configuration.

```toml
[companions.openid]
type = 'application'
image = 'private.example.com/library/openid:latest'
env = [ 'KEY=VALUE' ]
```

The provided values of `serviceName` and `env` can include the [handlebars syntax](https://handlebarsjs.com/) in order to access dynamic values.

Additionally, you could mount files that are generated from handlebars templates (example contains a properties generation):

```toml
[companions.openid.volumes]
"/path/to/volume.properties" = """
remote.services={{#each services~}}
  {{~#if (eq type 'instance')~}}
    {{name}}:{{port}},
  {{~/if~}}
{{~/each~}}
"""
```

Furthermore, you can provide labels through handlebars templating:

```toml
[companions.openid.labels]
"com.github.prevant" = "bar-{{application.name}}"
```

### Template Variables

The list of available handlebars variables:

- `application`: The companion's application information
  - `name`: The application name
- `services`: An array of the services of the application. Each element has following structure:
  - `name`: The service name which is equivalent to the network alias
  - `port`: The exposed port of the service
  - `type`: The type of service. For example, `instance`, `replica`, `app-companion`, or `service-companion`.

### Handlebar Helpers

PREvant provides some handlebars helpers which can be used to generate more complex configuration files. See handlerbar's [block helper documentation](https://handlebarsjs.com/block_helpers.html) for more details.

- `{{#isCompanion <type>}}` A conditional handlerbars block helper that checks if the given service type matches any companion type.
- `isNotCompanion <type>` A conditional handlerbars block helper that checks if the given service type does not match any companion type.

## Service Based

The service-based companions works the in the same way as the application-based services. Make sure, that the `serviceName` is unique by using the handlebars templating.

```toml
[companions.service-name]
serviceName = '{{service.name}}-db'
image = 'postgres:11'
env = [ 'KEY=VALUE' ]

[companions.service-name.postgres.volumes]
"/path/to/volume.properties" == "…"
[companions.openid.labels]
"com.github.prevant" = "bar-{{application.name}}"
```


### Template Variables

The list of available handlebars variables:

- `application`: The companion's application information
  - `name`: The application name
- `service`: The companion's service containing following fields:
  - `name`: The service name which is equivalent to the network alias
  - `port`: The exposed port of the service
  - `type`: The type of service. For example, `instance`, `replica`, `app-companion`, or `service-companion`.

## Deployment Strategy

Companions offer different deployment strategies so that a companion could be restarted or not under certain conditions. Therefore, PREvant offers following configuration flags:

```toml
[companions.openid]
type = 'application'
image = 'private.example.com/library/openid:latest'
deploymentStrategy = 'redeploy-on-image-update'
```

`deploymentStrategy` offers following values and if a companion exists for an app following strategy will be applied:

- `redeploy-always` (_default_): Re-deploys the companion every time there is a new deployment request.
- `redeploy-on-image-update`: Re-deploys the companion if there is a more rescent image available.
- `redeploy-never`: Even if there is a new deployment request the companion won't be redeployed and stays running.

a
