# Build Frontend
FROM docker.io/library/node:10-alpine as frontend-builder
RUN apk add --no-cache python3 make g++
WORKDIR /usr/src/frontend/
COPY frontend/package.json frontend/package-lock.json frontend/*.config.js /usr/src/frontend/
COPY frontend/public /usr/src/frontend/public/
COPY frontend/src /usr/src/frontend/src/
RUN npm ci && npm run build


# Build Backend
FROM docker.io/ekidd/rust-musl-builder as backend-builder
USER root
RUN rustup default nightly && rustup target add x86_64-unknown-linux-musl
COPY api/Cargo.toml api/Cargo.lock /usr/src/api/
COPY api/src /usr/src/api/src
WORKDIR /usr/src/api
RUN cargo build --release --target x86_64-unknown-linux-musl


# Compose application directory
FROM scratch as directory-composer
COPY --from=backend-builder /usr/src/api/target/x86_64-unknown-linux-musl/release/prevant /app/prevant
COPY api/res/Rocket.toml api/res/config.toml api/res/openapi.yml /app/
COPY --from=frontend-builder /usr/src/frontend/dist/index.html /usr/src/frontend/dist/favicon.svg /app/frontend/
COPY --from=frontend-builder /usr/src/frontend/dist/js /app/frontend/js
COPY --from=frontend-builder /usr/src/frontend/dist/css /app/frontend/css


# Build whole application
FROM docker.io/library/alpine
LABEL maintainer="marc.schreiber@aixigo.de"
RUN adduser -D -u 1000 prevant

WORKDIR /app
EXPOSE 80
ENV ROCKET_ENV=staging
ENV RUST_LOG=info
CMD ["./prevant"]

COPY --chown=prevant --from=directory-composer /app /app
