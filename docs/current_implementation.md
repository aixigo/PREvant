<!--
  current_implementation.md: Consolidated implementation details (Behavior) for all features.
-->
# Current Implementation Details

This document gathers the implementation-specific behavior sections for each API feature as currently realized in the codebase.

## List Deployed Review Apps

1. Rocket dispatches `GET /api/apps/` to `apps()` handler in `AppsController`.
2. Rocket's request guards inject:
   - `State<Arc<Apps>>` (`AppsService` instance)
   - `RequestInfo` (client request context)
   - `State<HostMetaCache>` (for URL generation)
3. `apps.fetch_apps().await` calls into the `Infrastructure` layer:
   - `infrastructure.fetch_services()` gathers running container data
   - Returns `HashMap<AppName, Services>` (raw `Service` entries)
4. `HostMetaCache::convert_services_into_services_with_host_meta` maps each `Service` to `ServiceWithHostMeta`:
   - Attaches host metadata and constructs per-service URLs using `RequestInfo`
   - Produces `HashMap<AppName, ServicesWithHostMeta>`
5. Controller wraps the map in `Json` and returns HTTP 200.
6. Any error in service or cache lookup is converted to an `HttpApiError`:
   - Results in HTTP 500 with a `ProblemDetails` JSON payload

## Start or Update Review App

1. Rocket routes `POST /api/apps/{appName}` to `create_app()`.
2. Guards parse:
   - `State<Arc<Apps>>` (AppsService)
   - `CreateAppOptions` (replicateFrom)
   - `RunOptions` (Prefer header)
   - `CreateAppPayload` (body → Vec<ServiceConfig>, Option<Json>)
3. Generate new `AppStatusChangeId`.
4. `AppsService.create_or_update(...)` performs:
   - Parameter validation and guard checks.
   - Delegation to `Infrastructure.deploy_services`.
5. `spawn_with_options` applies sync/async logic:
   - Sync: await completion → Ready
   - Async without wait: immediate Pending
   - Async with wait: wait up to timeout
6. `AsyncCompletion` responder builds:
   - `Poll::Ready` → 200 with services JSON
   - `Poll::Pending` → 202 with Location header
   - Errors map via `From<AppsError>` → appropriate status and ProblemDetails

## Shutdown Review App

1. Rocket routes `DELETE /api/apps/{appName}` to `delete_app()`.
2. Request guard `RunOptions` parses the `Prefer` header for sync/async semantics.
3. A new `AppStatusChangeId` is generated to track the shutdown operation.
4. The handler invokes `AppsService.delete_app(appName, statusId)`, which:
   - Acquires an app guard to prevent parallel operations.
   - Calls `Infrastructure.delete_app(...)` to orchestrate container shutdown.
5. `spawn_with_options` applies the `RunOptions`:
   - **Sync:** awaits the future and returns `Poll::Ready`.
   - **Async no-wait:** returns `Poll::Pending` immediately.
   - **Async with wait:** waits up to timeout, then returns `Ready` or `Pending`.
6. `AsyncCompletion` responder builds the HTTP response:
   - `Poll::Ready(Ok(services))` → HTTP 200 + JSON array of `Service`.
   - `Poll::Pending` → HTTP 202 + `Location` header to poll status.
   - Errors (`AppsError`) map to 409 or 500 via `From<AppsError>`.

## Get Ticket Information

1. Rocket routes `GET /api/apps/tickets` to `tickets()`.
2. Request guards inject:
   - `State<Config>` for Jira settings.
   - `State<Arc<Apps>>` (`AppsService`).
3. If `config.jira_config()` is `None`, return HTTP 204 No Content.
4. Else, call `apps_service.fetch_app_names().await` to list all apps.
5. For each app name, asynchronously invoke `JiraInstance.issue(appName)`:
   - On success: map `Issue` → `TicketInfo` (link, summary, status).
   - On `MissingIssues` or decode errors: logged and skipped.
   - On other `JiraQueryError`: abort with HTTP 500.
6. Return HTTP 200 with JSON map of `String` → `TicketInfo`.