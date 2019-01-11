FROM docker.io/library/rust:1.31.0-slim as builder
RUN apt update ; \
    apt install -y libssl-dev pkg-config ; \
    rustup default nightly
WORKDIR /usr/src/api
COPY Cargo.toml Cargo.lock /usr/src/api/
COPY src /usr/src/api/src
RUN cargo install --path . ; \
    chown root:root /usr/local/cargo/bin/api

FROM docker.io/library/debian:stretch-slim
LABEL maintainer="marc.schreiber@aixigo.de"

RUN set -eux ; \
    apt-get update ; \
    apt-get install -y openssl ca-certificates

WORKDIR /app
COPY --from=builder /usr/local/cargo/bin/api .
COPY res/Rocket.toml res/config.toml /app/
EXPOSE 80
ENV ROCKET_ENV=staging
CMD ["./api"]