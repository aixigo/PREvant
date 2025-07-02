<!--
  features.md: Overview of API features derived from OpenAPI specification.
-->
# Features

This document provides an overview of the API features. Each section includes a brief description and a link to a detailed feature specification in the `docs/features` directory.

## List Deployed Review Apps

Endpoint: `GET /apps/`
Returns a JSON object mapping each review application name to its current service status. Typical responses:
- `200` with an object of `Service` schemas
- `500` on server error

Link: [features/list-deployed-review-apps.md](features/list-deployed-review-apps.md)

## Get Ticket Information

Endpoint: `GET /apps/tickets/`
Returns ticket metadata for each review app. Useful when integrating with external issue trackers. Typical responses:
- `200` with an object of `Ticket` schemas per app
- `204` if no ticket system is configured
- `500` on server error

Link: [features/get-ticket-information.md](features/get-ticket-information.md)

## Start or Update Review App

Endpoint: `POST /apps/{appName}`
Creates or updates a review app named `appName`. Accepts a JSON payload of service configurations (as an array or under `services`). Optional query parameters:
- `replicateFrom` (default `master`) to clone another app
- `preferAsync` to run asynchronously
Typical responses:
- `200` with an array of `Service` statuses when deployed synchronously
- `202` when queued (check `Location` header for task URL)
- `409` if the app is already deploying
- `500` on server error

Link: [features/start-or-update-review-app.md](features/start-or-update-review-app.md)

## Shutdown Review App

Endpoint: `DELETE /apps/{appName}`
Stops and removes all containers for the given `appName`. Supports optional `preferAsync` flag. Typical responses:
- `200` with an array of remaining `Service` objects
- `202` when shutdown is queued (check `Location` header)
- `500` on server error

Link: [features/shutdown-review-app.md](features/shutdown-review-app.md)
