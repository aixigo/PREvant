 # Architecture Overview

 This document describes the architecture of the PREvant solution, including its main components, external system integrations, and key diagrams (entity relationship, class, and sequence diagrams).

 ## Components

- **API Service**: A Rust-based HTTP server built with Rocket, handling client requests, business logic, and persistence.
- **Frontend UI**: A Vue.js single-page application providing user interaction for creating, reviewing, and managing preview environments.
- **Infrastructure Layer**: Interfaces for deploying and tearing down preview environments on Docker or Kubernetes, with support for Traefik as a reverse proxy.
- **Examples**: Sample configurations demonstrating Docker Compose and Kubernetes setups.

 ## External Systems

- **Docker Registry**: Stores and retrieves container images.
- **Kubernetes API**: Manages container orchestration, deployments, and networking.
- **Traefik**: Manages HTTP routing and load balancing for preview environments.
- **Git Repository**: Source of application manifests and configuration via webhooks.

 ## Entity Relationship Diagram

 ```mermaid
 erDiagram
     APPLICATION ||--o{ SERVICE : contains
     SERVICE ||--o{ DEPLOYMENT_UNIT : deploys
     APPLICATION ||--o{ TICKET : generates
     APPLICATION ||--o{ WEBHOOK : triggers
     TICKET ||--o{ LOGS_CHUNK : collects
     WEBHOOK ||--o{ REQUEST_INFO : records
 ```

 ## Class Diagram

 ```mermaid
 classDiagram
     class ApiServer {
       +start()
       +routes: Route[]
     }
     class ServiceManager {
       +createApp()
       +shutdownApp()
     }
     class DeploymentUnit {
       +apply()
       +remove()
     }
     class RegistryClient {
       +pullImage()
       +pushImage()
     }
     class KubernetesClient {
       +createDeployment()
       +deleteDeployment()
     }
     ApiServer --> ServiceManager
     ServiceManager --> DeploymentUnit
     DeploymentUnit --> RegistryClient
     DeploymentUnit --> KubernetesClient
 ```

 ## Sequence Diagram: Create Preview Environment

 ```mermaid
 sequenceDiagram
     participant User
     participant Frontend
     participant ApiServer
     participant ServiceManager
     participant Infrastructure

     User->>Frontend: Click "Create Preview"
     Frontend->>ApiServer: POST /apps
     ApiServer->>ServiceManager: createApp()
     ServiceManager->>Infrastructure: deploy(DeploymentUnit)
     Infrastructure->>Registry: pull image
     Infrastructure->>Orchestrator: start containers
     Orchestrator-->>Infrastructure: deployment status
     Infrastructure-->>ServiceManager: success
     ServiceManager-->>ApiServer: app info
     ApiServer-->>Frontend: 201 Created
     Frontend-->>User: Display Preview URL
 ```
