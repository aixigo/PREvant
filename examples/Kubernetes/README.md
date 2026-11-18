# PREvant in a Kubernetes Setup

PREvant relies in a Kubernetes cluster on [Traefik's Kubernetes
IngressRoute](https://doc.traefik.io/traefik/providers/kubernetes-crd/).
Therefore, make sure that you have a Traefik service installed on your
Kubernetes cluster. For example, [k3d](https://k3d.io) managed clusters come
with Traefik preinstalled.

PREvant needs to interact with the Kubernetes API and requires a ServiceAccount
that has the permission to manipulate the Kubernetes state. Therefore, apply
the following command to create the role-based access control bindings in
Kubernetes.

```bash
kubectl apply -f RBAC-authorization.yml
```

Depending on the Traefik version, apply either for Traefik v2 or v3:

```bash
# For Traefik v2
kubectl apply -f ingress-traefik-v2.yaml
# For Traefik v3
kubectl apply -f ingress-traefik-v3.yaml
```

Then, you can deploy PREvant with ServiceAccount and Deployment that exposes
PREvant under the URL path `/` in your Kubernetes cluster.

```bash
kubectl apply -f PREvant.yml
```

Check your [Traefik](https://doc.traefik.io/traefik/operations/dashboard/) for troubleshooting issues.

