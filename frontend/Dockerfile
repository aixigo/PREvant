FROM docker.io/library/node:10-alpine as frontend-builder
WORKDIR /usr/src/apps/
COPY . .
RUN npm install && npm run build

FROM docker.io/library/nginx:1.15-alpine
LABEL maintainer="marc.schreiber@aixigo.de"

COPY --from=frontend-builder /usr/src/apps/dist/* /usr/share/nginx/html/dist/
COPY index.html favicon.svg  /usr/share/nginx/html/

