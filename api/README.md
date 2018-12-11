The image `aixigo/prevant-api` provides the REST-API in order to deploy Docker containers and to compose them into reviewable application.

# Configuration

In order to configure the REST-API container create a [TOML](https://github.com/toml-lang/toml) file that is mounted to the docker container's path `/app/config.toml`.

## Container Options

Create a table `containers` with following options:

```toml
[containers]

# Restrict memory usage of containers
memory_limit = '1g'
```

## Issue Tracking options

Application names are compared to issues which will be linked to cards on the frontend. Therefore, the REST backend needs to be able to compare the application names with issue tracking information.

Currently, Jira as a tracking system is supported.

```toml
[jira]
host = 'https://jira.example.com'
user = ''
password = ''
```

## Companions

It is possible to start containers that will be started when the client requests to create a new service. For example, if the application requires an [OpenID](https://en.wikipedia.org/wiki/OpenID_Connect) provider, it is possible to create a configuration that starts the provider for each application. Another use case might be a Kafka services that is required by the application.

Furthermore, it is also possible to create containers for each service. For example, for each service a database container could be started.

For these use cases following sections provide example configurations.

### Application Wide

If you want to include an OpenID provider for every application, you could use following configuration.

```toml
[[companions]]
[[companions.application]]
serviceName = 'openid'
image = 'private.example.com/library/opendid:latest'
env = [ 'KEY=VALUE' ]
```

The provided values of `serviceName` and `env` can include the [handlebars syntax](https://handlebarsjs.com/) in order to access dynamic values.

Additionally, you could mount files that are generated from handlebars templates (example contains a properties generation):

```toml
[companions.application.openid.volumes]
"/path/to/volume.properties" = """
remote.services={{#each services~}}
  {{~#if (eq type 'instance')~}}
    {{name}}:{{port}},
  {{~/if~}}
{{~/each~}}
"""
```

#### Template Variables

The list of available handlebars variables:

- `application`: The companion's application information
  - `name`: The application name
- `applicationPath`: The root path to the application.
- `services`: An array of the services of the application. Each element has following structure:
  - `name`: The service name which is equivalent to the network alias
  - `port`: The exposed port of the service
  - `type`: The type of service. For example, `instance`, `replica`, `app-companion`, or `service-companion`.

### Service Based

The service-based companions works the in the same way as the application-based services. Make sure, that the `serviceName` is unique by using the handlebars templating.

```toml
[[companions]]
[[companions.services]]
serviceName = '{{service.name}}-db'
image = 'postgres:11'
env = [ 'KEY=VALUE' ]
[companions.services.postgres.volumes]
"/path/to/volume.properties" == "…"
```


#### Template Variables

The list of available handlebars variables:

- `application`: The companion's application information
  - `name`: The application name
- `service`: The companion's service containing following fields:
  - `name`: The service name which is equivalent to the network alias
  - `port`: The exposed port of the service
  - `type`: The type of service. For example, `instance`, `replica`, `app-companion`, or `service-companion`.
