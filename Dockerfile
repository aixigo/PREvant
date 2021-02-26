# Build Frontend
FROM node:10-alpine as frontend-builder
RUN apk add --no-cache python3 make g++
WORKDIR /usr/src/frontend/
COPY frontend/package.json frontend/package-lock.json frontend/*.config.js /usr/src/frontend/
COPY frontend/public /usr/src/frontend/public/
COPY frontend/src /usr/src/frontend/src/
RUN npm ci && npm run build


# Build Backend
FROM rust:1 as backend-builder
USER root
RUN rustup default nightly-2021-03-15
COPY api/Cargo.toml api/Cargo.lock /usr/src/api/
COPY api/src /usr/src/api/src
WORKDIR /usr/src/api
RUN cargo build --release


# Compose application directory
FROM scratch as directory-composer
COPY --from=backend-builder /usr/src/api/target/release/prevant /app/prevant
COPY api/res/Rocket.toml api/res/config.toml api/res/openapi.yml /app/
COPY --from=frontend-builder /usr/src/frontend/dist/index.html /usr/src/frontend/dist/favicon.svg /app/frontend/
COPY --from=frontend-builder /usr/src/frontend/dist/js /app/frontend/js
COPY --from=frontend-builder /usr/src/frontend/dist/css /app/frontend/css


# Build whole application
FROM gcr.io/distroless/cc
LABEL maintainer="marc.schreiber@aixigo.de"

WORKDIR /app
EXPOSE 80
ENV ROCKET_ENV=staging RUST_LOG=info
CMD ["./prevant"]

COPY --from=directory-composer /app /app
