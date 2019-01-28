FROM docker.io/ekidd/rust-musl-builder as builder
USER root
RUN rustup default nightly && rustup target add x86_64-unknown-linux-musl
COPY Cargo.toml Cargo.lock /usr/src/api/
COPY src /usr/src/api/src
WORKDIR /usr/src/api
RUN cargo build --release --target x86_64-unknown-linux-musl

FROM docker.io/library/alpine
LABEL maintainer="marc.schreiber@aixigo.de"

RUN adduser -D -u 1000 api
COPY --chown=api --from=builder /usr/src/api/target/x86_64-unknown-linux-musl/release/api /app/api
COPY --chown=api res/Rocket.toml res/config.toml /app/

WORKDIR /app
EXPOSE 80
ENV ROCKET_ENV=staging
ENV RUST_LOG=info
CMD ["./api"]