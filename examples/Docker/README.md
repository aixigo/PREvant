# PREvant in a Docker Setup

Have a look at [docker-compose.yml](docker-compose.yml) and use following command to start PREvant.

```bash
export POSTGRES_PASSWORD=$(< /dev/urandom tr -dc 'A-Za-z0-9_@#$%!' | head -c 16; echo)
docker compose up -d
```

Now, PREvant is running at [`http://localhost`](http://localhost).
