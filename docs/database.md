# Database

It is recommended to run PREvant with a persistent PostgreSQL database
connection which gives PREvant the ability to preserve, for example, deployment
tasks of application even though PREvant get's shutdown in the middle of
creating an application. Also, this features provides the advantage to run
multiple instances of PREvant at the same time.

Use following configuration block to connect to a PostgreSQL database.

```toml
[database]
host = "db"
port = 5432
username = "postgres"
password =  "${env:POSTGRES_PASSWORD}"
database = "postgres"
```
