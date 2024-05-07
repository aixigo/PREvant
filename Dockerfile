# Build Frontend
FROM node:18-alpine as frontend-builder
WORKDIR /usr/src/frontend/
COPY frontend/package.json frontend/index.html frontend/package-lock.json frontend/*.config.mjs /usr/src/frontend/
COPY frontend/public /usr/src/frontend/public/
COPY frontend/src /usr/src/frontend/src/
RUN npm ci && npm run build


# Build Backend
FROM rust:1-bookworm as backend-builder
COPY api/Cargo.toml api/Cargo.lock /usr/src/api/
WORKDIR /usr/src/api

# Improves build caching, see https://stackoverflow.com/a/58474618/5088458
RUN sed -i 's#src/main.rs#src/dummy.rs#' Cargo.toml
RUN mkdir src && echo "fn main() {}" > src/dummy.rs
RUN cargo build --release

RUN sed -i 's#src/dummy.rs#src/main.rs#' Cargo.toml && rm src/dummy.rs
COPY api/src /usr/src/api/src
RUN cargo build --release


# Compose application directory
FROM scratch as directory-composer
COPY --from=backend-builder /usr/src/api/target/release/prevant /app/prevant
COPY api/res/Rocket.toml api/res/config.toml /app/
COPY api/res/openapi.yml /app/res/
COPY --from=frontend-builder /usr/src/frontend/dist/index.html /usr/src/frontend/dist/favicon.svg /app/frontend/
COPY --from=frontend-builder /usr/src/frontend/dist/assets /app/frontend/assets


# Build whole application
FROM gcr.io/distroless/cc-debian12
LABEL maintainer="marc.schreiber@aixigo.de"

WORKDIR /app
EXPOSE 80
ENV ROCKET_PROFILE=staging RUST_LOG=info
CMD ["./prevant"]

COPY --from=directory-composer /app /app
