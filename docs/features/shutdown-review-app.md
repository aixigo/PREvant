<!--
  Detailed spec for the "Shutdown Review App" feature.
-->
# Shutdown Review App

**Endpoint:** DELETE `/apps/{appName}`
**Header Parameter:** `Prefer: respond-async[,wait=<seconds>]`

**Purpose:** Stop and remove all containers associated with the specified review application.

## 1. Overview

This endpoint allows clients to terminate a review application identified by `appName`. Clients can choose synchronous or asynchronous execution via the `Prefer` header. After the operation, remaining container states are returned or can be polled.

## 2. Request

### 2.1 Path Parameter
- `appName` (string, required): Name of the review application to shut down.

### 2.2 Header Parameter: Prefer
Based on [RFC-7240]:
- `Prefer: respond-async` — return immediately with HTTP 202 Accepted.
- `Prefer: respond-async,wait=<seconds>` — wait up to `<seconds>` before responding.

### 2.3 Request Body
None.

## 3. Response

| Status | Description                                                       | Body                                    |
|--------|-------------------------------------------------------------------|-----------------------------------------|
| 200    | Synchronous shutdown completed.                                   | JSON array of `Service` objects         |
| 202    | Shutdown queued for asynchronous processing.                      | *empty* (use `Location` header to poll) |
| 409    | Conflict: application is currently deploying or deleting.         | `ProblemDetails`                        |
| 500    | Internal server error.                                           | `ProblemDetails`                        |

**Location Header** (202): `/api/apps/{appName}/status-changes/{statusId}`

## 4. Data Models

- **Service**: Represents a container/service and its final state (see OpenAPI).
- **AppStatusChangeId**: Identifier for polling async status changes.

## 5. Diagrams

### 5.1 Entity–Relationship Diagram (ERD)

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

### 5.2 Class Diagram

```mermaid
classDiagram
    AppsController --> AppsService
    AppsService --> Infrastructure
    class AppsController {
      +delete_app(appName, RunOptions) : AsyncCompletion<Json<Services>>
    }
    class AppsService {
      +delete_app(appName, statusId) : Future<Result<Services, AppsServiceError>>
    }
    class Infrastructure {
      +delete_app(appName, statusId) : Future<Result<Vec<ServiceEntity>, Error>>
    }
```

### 5.3 Sequence Diagram

```mermaid
sequenceDiagram
    participant Client
    participant Controller as AppsController
    participant Service as AppsService
    participant Infra as Infrastructure
    participant Engine as ContainerEngine

    Client->>Controller: DELETE /apps/foo
    Controller->>Controller: parse RunOptions from Prefer header
    Controller->>Service: delete_app(foo, statusId)
    alt sync or wait completed
        Service->>Infra: delete_app(foo, statusId)
        Infra->>Engine: stop and remove containers
        Engine-->>Infra: success list
        Service-->>Controller: Poll::Ready(Ok(services))
        Controller-->>Client: HTTP 200 with JSON array
    else async pending
        Service-->>Controller: Poll::Pending
        Controller-->>Client: HTTP 202 with Location header
    end
```


*See also:* [List Deployed Review Apps](list-deployed-review-apps.md), [Start or Update Review App](start-or-update-review-app.md)