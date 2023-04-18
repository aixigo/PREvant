This file provides some hints and examples how to develop PREvant.

# Backend Development

You can build PREvant's backend API with [`cargo`](https://doc.rust-lang.org/cargo/) in the sub directory `/api`. For example, `cargo run` build and starts the backend so that it will be available at `http://localhost:8000`. 

If you want to use PREvant's frontend during development, head over to the [Frontend Development section](#fe-dev).

Without any CLI options, PREvant will use the Docker API. If you want to develop with against Kubernetes, have a look into the [Kubernetes section](#k8s-dev).

## <a name="k8s-dev"></a>Kubernetes Backend

For developing against a local Kubernetes cluster you can use [k3d](https://k3d.io).

1. Create a cluster:

   ```bash
   k3d cluster create dash -p "80:80@loadbalancer" -p "443:443@loadbalancer"
   ```

2. Start PREvant with Kubernetes (it will infer the cluster configuration by searching for kube-config file or in-cluster environment variables)

   ```bash
   cargo run -- --runtime.type Kubernetes
   ```

3. Deploy some containers and observe the result [here](http://localhost/master/whoami/):

   ```bash
   curl -X POST -d '[{"serviceName": "whoami", "image": "quay.io/truecharts/whoami"}]' \
      -H "Content-type: application/json" \
      http://localhost:8000/api/apps/master
   ```

# <a name="fe-dev"></a>Frontend Development

You can build PREvant's frontend with [`npm`](https://www.npmjs.com/) in the sub directory `/frontend`. You can [build the static HTML files](#fe-static-html) or [serve the HTML files via the dev server](#fe-dev-server).


## <a name="fe-static-html"></a>Static HTML

To create the static HTML files that can be served by PREvant's backend (see [above](#backend-development), you need to run following commands and start the backend.

```bash
npm ci
npm run build
```

PREvant will be available at `http://localhost:8000`.

## <a name="fe-dev-server"></a>Dev Server

1. Start the backend as described in [Backend Development](#backend-development).
2. Change into the directory `/frontend`
3. Build and run the frontend in the development mode
   ```bash
   npm ci
   npm run serve
   ```
4. Open the URL `http://localhost:9001` in your browser

