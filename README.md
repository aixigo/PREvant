[![](https://images.microbadger.com/badges/image/aixigo/prevant.svg)](https://microbadger.com/images/aixigo/prevant)

# PREvant In a Nutshell

PREvant is a small set of docker containers that serves as an abstraction layer between continuous integration pipelines and some container orchestration platform. This abstraction serves as a reviewing platform to ensure that developers have built the features that domain expert requested. 

PREvant's name originates from this requirement: _Preview servant (PREvant, `prɪˈvɛnt`, it's pronounced like prevent)_ __serves__ developers to deploy previews of their application as simple as possible when their application consists of multiple microservices distributed across multiple source code repositories. These previews should __PREvant__ to do mistakes in feature development because domain experts can review changes as soon as possible.

![In a nutshell](assets/in-a-nutshell.svg "In a nutshell")

Through PREvant's web interface domain experts, managers, developers, and sales experts can review and demonstrate the application development.

![Access the application](assets/screenshot.png "Access the application")

# Usage

Have a look at [docker-compose.yml](docker-compose.yml) and use following command to start PREvant.

```
docker-compose up -d
```

Now, PREvant is running at [`http://localhost`](http://localhost).

If you want to customize PREvant's behaviour, you can mount a TOML file into the container at the path `/app/config.toml`. You will find more information about the configuration [here](api/README.md).

# Requirements for Your Services

PREvant is able to show the version of your service (build time, version string, and git commit hash) and also to integrate your API specification into the frontend through [Swagger UI](https://swagger.io/tools/swagger-ui/). In order to show the information, PREvant tries to resolve it by using the web-based protocol proposed by [RFC 6415](https://tools.ietf.org/html/rfc6415).

When you request the list of apps and services running through the frontend, PREvant makes a request for each service to the URL `.well-known/host-meta.json` and expects that the resource provides a [host-meta document](http://docs.oasis-open.org/xri/xrd/v1.0/xrd-1.0.html) serialized as JSON:

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
    }
  ]
}
```

This sample document contains the relevant information displayed in the frontend (each information is optional):

- The software version of the service (see `https://schema.org/softwareVersion`)
- The build time of the service (see `https://schema.org/dateModified`)
- The git commit id of the service (see `https://git-scm.com/docs/git-commit`)
- The link to the API specification (see `https://github.com/OAI/OpenAPI-Specification`)

In order to generate the correct link the API specification PREvant adds following headers to each of these requests:

- [`Forwarded` header](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Forwarded) with `host` and `proto`.
- `X-Forwarded-Prefix` (used by some reverse proxies, cf. [Traefik](https://docs.traefik.io/basics/) and [Zuul](https://cloud.spring.io/spring-cloud-static/Finchley.SR1/multi/multi__router_and_filter_zuul.html)).

# Further Readings

PREvant's concept has been published at the conference [_Microservices 2019_ in Dortmund](https://www.conf-micro.services/2019/). You can read [the abstract here](https://www.conf-micro.services/2019/papers/Microservices_2019_paper_14.pdf).

The talk is available on [YouTube](http://www.youtube.com/watch?v=O9GxapQR5bk). Click on the image to start the playback:

[![Video “PREvant: Composing Microservices into Reviewable and Testable Applications” at Microservices 2019](http://img.youtube.com/vi/O9GxapQR5bk/0.jpg)](http://www.youtube.com/watch?v=O9GxapQR5bk)
