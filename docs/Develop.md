This file provides some hints and examples how to develop PREvant.

# Backend Development

You can build PREvant's backend API with [`cargo`](https://doc.rust-lang.org/cargo/) in the sub directory `/api`. For example, `cargo run` build and starts the backend so that it will be available at `http://localhost:8000`.

When you than interact with the REST API to deploy service, it is worthwhile to have a look into the [Traefik dashboard](https://doc.traefik.io/traefik/operations/dashboard/#the-dashboard) to double check if PREvant exposes the services as expected.

If you want to use PREvant's frontend during development, head over to the [Frontend Development section](#fe-dev).

Without any CLI options, PREvant will use the Docker API. If you want to develop with against Kubernetes, have a look into the [Kubernetes section](#k8s-dev).

## <a name="k8s-dev"></a>Kubernetes Backend

For developing against a local Kubernetes cluster you can use [k3d].

1. Create a cluster:

   ```bash
   k3d cluster create dash -p "80:80@loadbalancer" -p "443:443@loadbalancer"
   ```

2. Start PREvant with Kubernetes (it will infer the cluster configuration by searching for kube-config file or in-cluster environment variables)

   ```bash
   cargo run -- --runtime-type Kubernetes
   ```

3. Deploy some containers and observe the result [here](http://localhost/master/whoami/):

   ```bash
   curl -X POST -d '[{"serviceName": "whoami", "image": "quay.io/truecharts/whoami:1.8.1"}]' \
      -H "Content-type: application/json" \
      http://localhost:8000/api/apps/master
   ```

4. Check Traefik's dashboard by exposing the port (see command below and detail [here](https://stackoverflow.com/q/68565048/5088458)) and visit [`http://localhost:9000/dashboard`](http://localhost:9000/dashboard).

   ```bash
   kubectl -n kube-system port-forward $(kubectl -n kube-system get pods --selector "app.kubernetes.io/name=traefik" --output name) 9000:9000
   ```

# <a name="fe-dev"></a>Frontend Development

PREvant’s frontend is located in the `/frontend` directory and uses [`npm`](https://www.npmjs.com/) for development and builds. You can either [build the static HTML files](#frontend-static-html-build) or [run the development server](#frontend-development-server). There is also a section on how to [run the frontend tests](#frontend-tests).

## <a name="fe-static-html"></a>Frontend Static HTML Build

To build the static HTML files that can be served by PREvant's backend:

1. Change into the `/frontend` directory:
   ```bash
   cd frontend
   ```
2. Install dependencies:
   ```bash
   npm ci
   ```
3. Build the frontend:
   ```bash
   npm run build
   ```

Afterwards, start the backend (see [Backend Development](#backend-development)). PREvant will then be accessible at:
**http://localhost:8000**

## <a name="fe-dev-server"></a>Frontend Development Server

To run the frontend in development mode:

1. Start the backend as described in [Backend Development](#backend-development).
2. Navigate to the `/frontend` directory:

   ```bash
   cd frontend
   ```

3. Install dependencies and start the dev server:

   ```bash
   npm ci
   npm run serve
   ```

4. Open the following URL in your browser:
   **http://localhost:9001**

## Frontend Tests

We use [Playwright](https://playwright.dev/) for end-to-end testing.

Before running the tests for the first time, you must install the required browsers:

```bash
npx playwright install
```

This only needs to be done once (or whenever Playwright updates its browser requirements).

To run the Playwright tests:

```bash
npm run test:e2e
```

Alternatively, you can run the tests in debug mode (with a UI):

```bash
npm run test:e2e:ui
```

# Integration Testing

To test the image end-2-end, build the Docker image (`docker build --pull -t
aixigo/prevant .`) and then choose testing via
[testcontainers](https://testcontainers.com/) or [k3d].

## Testcontainers for Docker Backend

```sh
export RUST_LOG="info,testcontainers=debug"
cargo test --manifest-path api-tests/Cargo.toml --test docker -- --test-threads=1 --nocapture
```

## K3s for Kubernetes Backend

0. Build the bootstrap image:
   ```sh
   cd examples/Kubernetes
   docker build --pull -t aixigo/httpd-bootstrap-example -f Dockerfile.bootstrap  .
   cd -
   ```
1. Create cluster and import the PREvant image:
   ```sh
   k3d cluster create dash -p "8080:80@loadbalancer" --no-rollback --k3s-arg --disable=metrics-server@server:* --image rancher/k3s:v1.31.7-k3s1
   k3d image import aixigo/prevant -c dash
   k3d image import aixigo/httpd-bootstrap-example -c dash
   ```
2. Deploy PREvant:
   ```sh
   kubectl apply -f examples/Kubernetes/RBAC-authorization.yml
   kubectl apply -f examples/Kubernetes/PREvant.yml
   ```
3. Run Tests:
   ```sh
   cargo test --manifest-path api-tests/Cargo.toml --test k3s
   ```

[k3d]: https://k3d.io
