FROM docker.io/library/node:10-alpine as frontend-builder
WORKDIR /usr/src/frontend/
COPY package.json package-lock.json postcss.config.js webpack.config.js /usr/src/frontend/
COPY src/ /usr/src/frontend/src/
RUN npm ci && npm run build



FROM docker.io/ekidd/rust-musl-builder as http-backend-builder
USER root
RUN rustup default nightly && rustup target add x86_64-unknown-linux-musl
COPY Cargo.lock Cargo.toml /usr/src/frontend/
COPY src/*.rs /usr/src/frontend/src/
WORKDIR /usr/src/frontend/
RUN cargo build --release --target x86_64-unknown-linux-musl



FROM docker.io/library/alpine
LABEL maintainer="marc.schreiber@aixigo.de"

RUN adduser -D -u 1000 frontend
COPY --chown=frontend --from=http-backend-builder /usr/src/frontend/target/x86_64-unknown-linux-musl/release/frontend /app/frontend
COPY --chown=frontend --from=frontend-builder /usr/src/frontend/target/* /app/static/
COPY --chown=frontend index.html favicon.svg  /app/static/

EXPOSE 8000
WORKDIR /app/
CMD ./frontend
