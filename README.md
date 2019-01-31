# PREvant In a Nutshell

PREvant is a small set of docker containers that serves as an abstraction layer between continuous integration pipelines and some container orchestration platform. This abstraction serves as a reviewing platform to ensure that developers have built the features that domain expert requested. 

PREvant's name originates from this requirement: _Preview servant (PREvant, it's pronounced like prevent)_ __serves__ developers to deploy previews of their application as simple as possible when their application consists of multiple microservices distributed across multiple source code repositories. These previews should __PREvant__ and help developers to do mistakes in their feature development because domain experts can review changes as soon as possible.

![In a nutshell](assets/in-a-nutshell.svg "In a nutshell")

Through PREvant's web interface domain experts, managers, developers, and sales experts can review and demonstrate the application development.

![Access the application](assets/screenshot.png "Access the application")

# Disclaimer :warning:

This project is currently being made available as open source. Not all features are available yet.

# Usage

In order to use the project you have to build the docker images (they will be released in the docker hub in the future).

```bash
mvn package -f api
mvn package -f frontend
```

When you have build the images, you can start the docker containers with the provided docker-compose file.

```
docker-compose up -d
```

Now, PREvant is running at [`http://localhost`](http://localhost).

# Further Readings

- [All about API image](api/README.md)
