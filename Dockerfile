# Build Frontend
FROM node:16-alpine as frontend-builder
RUN apk add --no-cache python3 make g++
WORKDIR /usr/src/frontend/
COPY frontend/package.json frontend/package-lock.json frontend/*.config.js /usr/src/frontend/
COPY frontend/public /usr/src/frontend/public/
COPY frontend/src /usr/src/frontend/src/
RUN npm ci && npm run build


# Build Backend
FROM rust:1 as backend-builder
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
COPY --from=frontend-builder /usr/src/frontend/dist/js /app/frontend/js
COPY --from=frontend-builder /usr/src/frontend/dist/css /app/frontend/css


# Build whole application
FROM gcr.io/distroless/cc
LABEL maintainer="marc.schreiber@aixigo.de"

WORKDIR /app
EXPOSE 80
ENV ROCKET_PROFILE=staging RUST_LOG=info
CMD ["./prevant"]

COPY --from=directory-composer /app /app
