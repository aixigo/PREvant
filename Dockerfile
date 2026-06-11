# Build Frontend
FROM node:22-alpine AS frontend-builder
WORKDIR /usr/src/frontend/
COPY frontend/package.json frontend/index.html frontend/package-lock.json frontend/*.config.mjs /usr/src/frontend/
COPY frontend/public /usr/src/frontend/public/
COPY frontend/src /usr/src/frontend/src/
RUN npm ci && npm run build


# Build Backend
FROM rust:1-bookworm AS backend-builder
COPY domain/README.md domain/Cargo.toml /usr/src/domain/
COPY api/Cargo.toml /usr/src/api/
COPY api-tests/Cargo.toml /usr/src/api-tests/
COPY Cargo.toml Cargo.lock /usr/src/
WORKDIR /usr/src/

# Improves build caching, see https://stackoverflow.com/a/58474618/5088458
RUN sed -i 's#src/main.rs#src/dummy.rs#' api/Cargo.toml
RUN sed -i 's#src/lib.rs#src/dummy.rs#' domain/Cargo.toml
RUN mkdir domain/src && echo "" > domain/src/dummy.rs
RUN mkdir api/src && echo "fn main() {}" > api/src/dummy.rs
RUN mkdir api-tests/src && echo "" > api-tests/src/lib.rs
RUN cargo build --release --package prevant

RUN sed -i 's#src/dummy.rs#src/main.rs#' api/Cargo.toml && rm api/src/dummy.rs
RUN sed -i 's#src/dummy.rs#src/lib.rs#' domain/Cargo.toml && rm domain/src/dummy.rs
COPY domain/src /usr/src/domain/src
COPY api/migrations /usr/src/api/migrations
COPY api/src /usr/src/api/src
RUN cargo build --release --package prevant


# Compose application directory
FROM scratch AS directory-composer
COPY --from=backend-builder /usr/src/target/release/prevant /app/prevant
COPY api/res/Rocket.toml api/res/config.toml /app/
COPY api/res/openapi.yml /app/res/
COPY --from=frontend-builder /usr/src/frontend/dist/index.html /usr/src/frontend/dist/favicon.svg /usr/src/frontend/dist/logo.svg /app/frontend/
COPY --from=frontend-builder /usr/src/frontend/dist/assets /app/frontend/assets


# Build whole application
FROM gcr.io/distroless/cc-debian12
LABEL maintainer="marc.schreiber@aixigo.de"

WORKDIR /app
EXPOSE 80
ENV ROCKET_PROFILE=staging RUST_LOG=info
CMD ["./prevant"]

COPY --from=directory-composer /app /app
