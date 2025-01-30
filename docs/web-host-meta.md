# Service Integration via Host Meta

PREvant is able to show the version of your services (build time, version
string, and git commit hash). It also integrate your [OpenAPI
specification][OpenAPI] into the frontend through [Swagger UI] and the
[AsyncAPI specification][AsyncAPI] through the [asyncapi-react
component][AsyncAPI UI].

## Dynamic Web Host Meta

In order to show the information, PREvant tries to resolve it by using the
web-based protocol proposed by [RFC 6415](https://tools.ietf.org/html/rfc6415).

When you request the list of apps and services running through the frontend,
PREvant makes a request for each service to the URL
`.well-known/host-meta.json` and expects that the resource provides a
[host-meta document](http://docs.oasis-open.org/xri/xrd/v1.0/xrd-1.0.html)
serialized as JSON:

```json
{
  "properties": {
    "https://schema.org/softwareVersion": "0.9",
    "https://schema.org/dateModified": "2019-04-09T15:31:01.363+0200",
    "https://git-scm.com/docs/git-commit": "43de4c6edf3c7ed93cdf8983f1ea7d73115176cc"
  },
  "links": [
    {
      "rel": "https://github.com/OAI/OpenAPI-Specification",
      "href": "https://example.com/master/service-name/swagger.json"
    },
    {
      "rel": "https://github.com/asyncapi/spec",
      "href": "https://github.com/asyncapi/spec/blob/master/examples/streetlights-kafka-asyncapi.yml"
    }
  ]
}
```

This sample document contains the relevant information displayed in the frontend (each information is optional):

- The software version of the service (see `https://schema.org/softwareVersion`)
- The build time of the service (see `https://schema.org/dateModified`)
- The git commit id of the service (see `https://git-scm.com/docs/git-commit`)
- The link to the OpenAPI specification (see `https://github.com/OAI/OpenAPI-Specification`)
- The link to the AsyncAPI specification (see `https://github.com/asyncapi/spec`)

In order to generate the correct link to the API specification, PREvant adds
following headers to each of these requests:

- [`Forwarded` header](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Forwarded)
  with `host` and `proto`.
- `X-Forwarded-Prefix` (used by some reverse proxies, cf. [Traefik](https://docs.traefik.io/basics/) and
  [Zuul](https://cloud.spring.io/spring-cloud-static/Finchley.SR1/multi/multi__router_and_filter_zuul.html)).

## Static Web Host Meta via Configuration

If you are not in control of the service that PREvant hosts, you could expand
the configuration so that PREvant provides the dashboard integration even if
the service does not provide `.well-known/host-meta.json`. This configuration
is helpful, if you want to deploy some companions for debugging purposes. For
example, [Kafka REST Proxy](https://github.com/confluentinc/kafka-rest) that
let's you interact with your Kafka cluster in each application.

```toml
[[staticHostMeta]]
# A regex that filters all services based on the image
imageSelector = "docker.io/conuentinc/cp-kafka-rest:.+"
# Optional: PREvant displays the tag as version in the dashboard.
imageTagAsVersion = true
# Optional: The OpenAPI specification that should be accessible in the
# dashboard. The servers section will be updated so that it points to the running
# service.
openApiSpec = { sourceUrl = "https://raw.githubusercontent.com/confluentinc/kafka-rest/refs/tags/v{{image.tag}}/api/v3/openapi.yaml", subPath = "v3" }
# Could be also just a string if you don't have to set the path.
# openApiSpec = "https://raw.githubusercontent.com/confluentinc/kafka-rest/refs/tags/v{{image.tag}}/api/v3/openapi.yaml"
```

The `openApiSpecUrl` can be templated with following [handlebars] template variables:

- `image.tag`: the OCI image tag.

[handlebars]: https://handlebarsjs.com/
[AsyncAPI]: https://github.com/asyncapi/spec
[AsyncAPI UI]: https://github.com/asyncapi/asyncapi-react
[OpenAPI]: https://github.com/OAI/OpenAPI-Specification
[Swagger UI]: https://swagger.io/tools/swagger-ui/
