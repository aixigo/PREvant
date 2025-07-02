<!--
  Detailed spec for the "List Deployed Review Apps" feature.
-->
# List Deployed Review Apps

Endpoint: GET `/apps/`

Purpose: Retrieve the current status of all review applications deployed on the PREvant instance.

## 1. Overview

This endpoint returns a mapping of review application names to their service details. It enables:
- Monitoring the state and URL of each service in a review app.
- Displaying a live dashboard of active preview environments.
- Automating alerts and cleanup based on service status.

## 2. Request

No parameters.

```
GET /apps/ HTTP/1.1
Host: api.prevant.example.com
Accept: application/json
```

## 3. Response

### 200 OK

JSON object where each key is an application name and each value is a Service object:

```json
{
  "my-app": {
    "name": "db",
    "type": "instance",
    "state": "running",
    "version": "1.0.0",
    "url": "http://prevant.example.com/my-app/db"
  },
  "another-app": {
    "name": "wordpress",
    "type": "replica",
    "state": "pending",
    "version": "5.7",
    "url": "http://prevant.example.com/another-app/blog"
  }
}
```

### 500 Internal Server Error

```json
{
  "type": "about:blank",
  "title": "Internal Server Error",
  "status": 500,
  "detail": "Unexpected error while retrieving apps"
}
```

## 4. Data Model

Service schema (see openapi):

```yaml
Service:
  type: object
  properties:
    name: string
    type: [instance, replica, app-companion, service-companion]
    state: string
    version: string
    url: string (url)
```

## 5. Diagrams

### Entity–Relationship Diagram (ERD)

```mermaid
erDiagram
    REVIEW_APP ||--o{ SERVICE : contains
    REVIEW_APP {
      string name PK
    }
    SERVICE {
      string name
      string type
      string state
      string version
      string url
      string appName FK
    }
```

### Class Diagram

```mermaid
classDiagram
    class AppsController {
      +getAllApps() : Map<String, Service>
    }
    class AppsService {
      +listReviewApps() : Map<String, Service>
    }
    class ServiceRepository {
      +findAllByApp(): List<ServiceEntity>
    }
    class ServiceEntity {
      -appName: String
      -name: String
      -type: ServiceType
      -state: ServiceState
      -version: Version
      -url: URL
    }
    AppsController --> AppsService
    AppsService --> ServiceRepository
    ServiceRepository --> ServiceEntity
```

### Sequence Diagram

```mermaid
sequenceDiagram
    participant Client
    participant Controller as AppsController
    participant Service as AppsService
    participant Repo as ServiceRepository
    participant DB as DataStore

    Client->>Controller: GET /apps/
    Controller->>Service: listReviewApps()
    Service->>Repo: findAllByApp()
    Repo->>DB: query services
    DB-->>Repo: list of ServiceEntity
    Repo-->>Service: List<ServiceEntity>
    Service-->>Controller: map to Service models
    Controller-->>Client: HTTP 200 with JSON
```


*See also:* Shutdown Review App, Start or Update Review App