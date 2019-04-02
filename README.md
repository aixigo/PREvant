# PREvant In a Nutshell

PREvant is a small set of docker containers that serves as an abstraction layer between continuous integration pipelines and some container orchestration platform. This abstraction serves as a reviewing platform to ensure that developers have built the features that domain expert requested. 

PREvant's name originates from this requirement: _Preview servant (PREvant, it's pronounced like prevent)_ __serves__ developers to deploy previews of their application as simple as possible when their application consists of multiple microservices distributed across multiple source code repositories. These previews should __PREvant__ and help developers to do mistakes in their feature development because domain experts can review changes as soon as possible.

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

# Further Readings

PREvant's concept has been published at the conference [_Microservices 2019_ in Dortmund](https://www.conf-micro.services/2019/). You can read [the abstract here](https://www.conf-micro.services/2019/papers/Microservices_2019_paper_14.pdf).

