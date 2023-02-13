This file provides some hints and examples how to develop PREvant.

# Backend Development

Change into the directory `/api` and follow the instructions in the subsections.

## Kubernetes Backend

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

# Frontend Development

1. Start the backend as described in [Backend Development](#backend-development).
2. Change into the directory `/frontend`
3. Build and run the frontend in the development mode
   ```bash
   npm ci
   npm run serve
   ```
4. Open the URL `http://localhost:9001` in your browser

