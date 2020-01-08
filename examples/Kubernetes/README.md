# PREvant in a Kubernetes Setup

Install a Kubernetes environment, such as [minikube](https://github.com/kubernetes/minikube), or use an existing Kubernetes installation. Also, make sure that [`kubectl`](https://kubernetes.io/docs/tasks/tools/install-kubectl/) is working with your Kubernetes installation. Then, follow the instructions below. 

PREvant and Traefik require to interact with the Kubernetes cluster. Therefore, the example relies on Kubernetes' Role-based access control (RBAC). To install RBAC use the following command to create a cluster role and service account `prevant-ingress-controller` that has the required permissions. 

```bash
kubectl apply -f RBAC-authorization.yml
```

These RBAC are similar to the minimal required permissions of Traefik but they have been extended with the required permission to create deployments, services, and custom resource definitions.

Additionally, it is required to install [IngressRoute Definition](https://docs.traefik.io/v2.0/user-guides/crd-acme/#ingressroute-definition) for Traefik. Install with following command:

```bash
kubectl apply -f ingress-route-definition.yml
```

Then, you can deploy PREvant and Traefik:

```bash
kubectl apply -f PREvant.yml
```

For testing purposes you can forward the PREvant setup with following command.

```bash
kubectl port-forward --address 127.0.0.1 service/traefik 8080:80 18080:8080 -n default
```

Now, PREvant is running at [`http://localhost:8080`](http://localhost:8080).