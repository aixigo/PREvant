FROM docker.io/library/rust:1.30.0-slim as builder
RUN apt update ; \
    apt install -y libssl-dev pkg-config
WORKDIR /usr/src/api
COPY . .
RUN rustup default nightly ; \
    cargo install --path . ; \
    chown root:root /usr/local/cargo/bin/api

FROM docker.io/library/debian:stretch-slim
LABEL maintainer="marc.schreiber@aixigo.de"

RUN set -eux ; \
    apt-get update ; \
    apt-get install -y openssl ca-certificates

WORKDIR /app
COPY --from=builder /usr/local/cargo/bin/api .
COPY res/Rocket.toml /app/Rocket.toml
EXPOSE 80
ENV ROCKET_ENV=staging
CMD ["./api"]