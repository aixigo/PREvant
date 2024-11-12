use crate::{
    apps::Apps,
    http_result::HttpResult,
    models::{AppName, AppNameError, LogChunk},
};
use chrono::DateTime;
use futures::stream::StreamExt;
use http_api_problem::HttpApiProblem;
use rocket::http::hyper::header::{ACCEPT, CONTENT_DISPOSITION, LINK};
use rocket::{
    http::{Accept, ContentType, RawStr, Status},
    request::FromRequest,
    response::stream::{Event, EventStream},
    response::{Responder, Response},
    Request, State,
};
use std::{str::FromStr, sync::Arc};

#[get("/<app_name>/logs/<service_name>?<log_query..>", rank = 1)]
pub(super) async fn logs<'r>(
    _apt: AcceptingPlainText,
    app_name: Result<AppName, AppNameError>,
    service_name: &'r str,
    log_query: LogQuery,
    apps: &State<Arc<Apps>>,
) -> HttpResult<LogsResponse<'r>> {
    let app_name = app_name?;

    let since = match log_query.since {
        None => None,
        Some(since) => match DateTime::parse_from_rfc3339(&since) {
            Ok(since) => Some(since),
            Err(err) => {
                return Err(HttpApiProblem::with_title_and_type(
                    http_api_problem::StatusCode::BAD_REQUEST,
                )
                .detail(format!("{}", err))
                .into());
            }
        },
    };

    let log_chunk = apps
        .get_logs(&app_name, service_name, &since, &log_query.limit)
        .await?;

    Ok(LogsResponse {
        log_chunk,
        app_name,
        service_name,
        limit: log_query.limit,
        as_attachment: log_query.as_attachment,
    })
}

#[get(
    "/<app_name>/logs/<service_name>?<log_query..>",
    format = "text/event-stream",
    rank = 2
)]
pub(super) async fn stream_logs<'r>(
    app_name: Result<AppName, AppNameError>,
    service_name: &'r str,
    log_query: LogQuery,
    apps: &'r State<Arc<Apps>>,
) -> HttpResult<EventStream![Event + 'r]> {
    let app_name = app_name?;
    let since = match &log_query.since {
        None => None,
        Some(since) => match DateTime::parse_from_rfc3339(since) {
            Ok(since) => Some(since),
            Err(err) => {
                return Err(HttpApiProblem::with_title_and_type(
                    http_api_problem::StatusCode::BAD_REQUEST,
                )
                .detail(format!("{}", err))
                .into());
            }
        },
    };

    Ok(EventStream! {
        let mut log_chunk = apps
            .stream_logs(&app_name, service_name, &since, &log_query.limit)
            .await;

        while let Some(result) = log_chunk.as_mut().next().await {
            match result {
                Ok((_, log_line)) => yield Event::data(log_line),
                Err(_e) => {
                    break;
                }
            }
        }
    })
}

pub struct LogsResponse<'a> {
    log_chunk: Option<LogChunk>,
    app_name: AppName,
    service_name: &'a str,
    limit: Option<usize>,
    as_attachment: bool,
}

impl<'r, 'o: 'r> Responder<'r, 'o> for LogsResponse<'r> {
    fn respond_to(self, _request: &'r Request) -> Result<Response<'o>, Status> {
        use std::io::Cursor;
        let log_chunk = match self.log_chunk {
            None => {
                let payload =
                    HttpApiProblem::with_title_and_type(http_api_problem::StatusCode::NOT_FOUND)
                        .json_bytes();
                return Response::build()
                    .status(Status::NotFound)
                    .raw_header("Content-type", "application/problem+json")
                    .sized_body(payload.len(), Cursor::new(payload))
                    .ok();
            }
            Some(log_chunk) => log_chunk,
        };

        let from = *log_chunk.until() + chrono::Duration::milliseconds(1);

        let next_logs_url = match self.limit {
            Some(limit) => format!(
                "/api/apps/{}/logs/{}?limit={}&since={}",
                self.app_name,
                self.service_name,
                limit,
                RawStr::new(&from.to_rfc3339()).percent_encode(),
            ),
            None => format!(
                "/api/apps/{}/logs/{}?since={}",
                self.app_name,
                self.service_name,
                RawStr::new(&from.to_rfc3339()).percent_encode(),
            ),
        };

        let content_disposition_value = if self.as_attachment {
            format!(
                "attachment; filename=\"{}_{}_{}.txt\"",
                self.app_name,
                self.service_name,
                log_chunk.until().format("%Y%m%d_%H%M%S")
            )
        } else {
            String::from("inline")
        };

        let log_lines = log_chunk.log_lines();
        Response::build()
            .header(ContentType::Plain)
            .raw_header(LINK.as_str(), format!("<{}>;rel=next", next_logs_url))
            .raw_header(CONTENT_DISPOSITION.as_str(), content_disposition_value)
            .sized_body(log_lines.len(), Cursor::new(log_lines.clone()))
            .ok()
    }
}

#[derive(FromForm)]
pub(super) struct LogQuery {
    since: Option<String>,
    limit: Option<usize>,
    #[field(name = "asAttachment")]
    as_attachment: bool,
}

pub(super) struct AcceptingPlainText;

#[rocket::async_trait]
impl<'r> FromRequest<'r> for AcceptingPlainText {
    type Error = ();

    async fn from_request(req: &'r Request<'_>) -> rocket::request::Outcome<Self, Self::Error> {
        if let Some(accept) = req
            .headers()
            .get(ACCEPT.as_str())
            .next()
            .and_then(|accept| Accept::from_str(accept).ok())
        {
            for m in accept.iter() {
                if m.media_type().is_event_stream() {
                    return rocket::request::Outcome::Forward(Status::SeeOther);
                }

                if m.media_type().top() == "text" || m.media_type().top() == "*" {
                    return rocket::request::Outcome::Success(AcceptingPlainText);
                }
            }
        }
        rocket::request::Outcome::Forward(Status::SeeOther)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{apps::HostMetaCache, infrastructure::Dummy, models::AppStatusChangeId, sc};
    use rocket::{
        http::{hyper::header::CONTENT_TYPE, Accept, Header},
        local::asynchronous::Client,
    };

    async fn set_up_rocket_with_dummy_infrastructure_and_a_running_app(
        host_meta_cache: HostMetaCache,
    ) -> Result<Client, crate::apps::AppsServiceError> {
        let infrastructure = Box::new(Dummy::new());
        let apps = Arc::new(Apps::new(Default::default(), infrastructure).unwrap());
        let _result = apps
            .create_or_update(
                &AppName::master(),
                &AppStatusChangeId::new(),
                None,
                &vec![sc!("service-a")],
                None,
            )
            .await?;

        let rocket = rocket::build()
            .manage(host_meta_cache)
            .manage(apps)
            .mount("/api/apps", routes![logs, stream_logs]);
        Ok(Client::tracked(rocket).await.expect("valid rocket"))
    }

    #[tokio::test]
    async fn log_weblink_with_no_limit() -> Result<(), crate::apps::AppsServiceError> {
        let (host_meta_cache, mut _host_meta_crawler) = crate::host_meta_crawling();

        let client =
            set_up_rocket_with_dummy_infrastructure_and_a_running_app(host_meta_cache).await?;

        let response = client
            .get("/api/apps/master/logs/service-a")
            .header(Accept::Text)
            .dispatch()
            .await;
        let mut link_header = response.headers().get("Link");
        assert_eq!(
            link_header.next(),
            Some(
                "</api/apps/master/logs/service-a?since=2019-07-18T07:35:00.001%2B00:00>;rel=next"
            )
        );
        Ok(())
    }

    #[tokio::test]
    async fn log_weblink_with_some_limit() -> Result<(), crate::apps::AppsServiceError> {
        let (host_meta_cache, mut _host_meta_crawler) = crate::host_meta_crawling();

        let client =
            set_up_rocket_with_dummy_infrastructure_and_a_running_app(host_meta_cache).await?;

        let response = client
            .get("/api/apps/master/logs/service-a?limit=20000&since=2019-07-22T08:42:47-00:00")
            .header(Accept::Text)
            .dispatch()
            .await;
        let mut link_header = response.headers().get("Link");
        assert_eq!(
                link_header.next(),
                Some("</api/apps/master/logs/service-a?limit=20000&since=2019-07-18T07:35:00.001%2B00:00>;rel=next")
            );
        Ok(())
    }

    #[tokio::test]
    async fn log_content_disposition_for_downloading_as_attachment(
    ) -> Result<(), crate::apps::AppsServiceError> {
        let (host_meta_cache, mut _host_meta_crawler) = crate::host_meta_crawling();

        let client =
            set_up_rocket_with_dummy_infrastructure_and_a_running_app(host_meta_cache).await?;

        let response = client
                .get("/api/apps/master/logs/service-a?limit=20000&since=2019-07-22T08:42:47-00:00&asAttachment=true")
                .header(Accept::Text)
                .dispatch()
                .await;
        let mut content_disposition_header = response.headers().get(CONTENT_DISPOSITION.as_str());
        assert_eq!(
            content_disposition_header.next(),
            Some("attachment; filename=\"master_service-a_20190718_073500.txt\"")
        );
        Ok(())
    }

    #[tokio::test]
    async fn log_content_disposition_for_displaying_as_inline(
    ) -> Result<(), crate::apps::AppsServiceError> {
        let (host_meta_cache, mut _host_meta_crawler) = crate::host_meta_crawling();

        let client =
            set_up_rocket_with_dummy_infrastructure_and_a_running_app(host_meta_cache).await?;

        let response = client
            .get("/api/apps/master/logs/service-a?limit=20000&since=2019-07-22T08:42:47-00:00")
            .header(Accept::Text)
            .dispatch()
            .await;
        let mut content_disposition_header = response.headers().get(CONTENT_DISPOSITION.as_str());
        assert_eq!(content_disposition_header.next(), Some("inline"));

        Ok(())
    }

    #[tokio::test]
    async fn log_content_type_when_accepting_text_star() -> Result<(), crate::apps::AppsServiceError>
    {
        let (host_meta_cache, mut _host_meta_crawler) = crate::host_meta_crawling();

        let client =
            set_up_rocket_with_dummy_infrastructure_and_a_running_app(host_meta_cache).await?;

        let response = client
            .get("/api/apps/master/logs/service-a")
            .header(Header::new("Accept", "text/*"))
            .dispatch()
            .await;

        let mut content_type_header = response.headers().get(CONTENT_TYPE.as_str());
        assert_eq!(
            content_type_header.next(),
            Some("text/plain; charset=utf-8")
        );

        Ok(())
    }

    #[tokio::test]
    async fn respond_with_plain_log_content_type_when_accepting_with_firefox_accept_default_value(
    ) -> Result<(), crate::apps::AppsServiceError> {
        let (host_meta_cache, mut _host_meta_crawler) = crate::host_meta_crawling();

        let client =
            set_up_rocket_with_dummy_infrastructure_and_a_running_app(host_meta_cache).await?;

        let response = client
            .get("/api/apps/master/logs/service-a")
            .header(Header::new("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8"))
            .dispatch()
            .await;

        let mut content_type_header = response.headers().get(CONTENT_TYPE.as_str());
        assert_eq!(
            content_type_header.next(),
            Some("text/plain; charset=utf-8")
        );

        Ok(())
    }

    #[tokio::test]
    async fn log_content_type_when_accepting_text_stream(
    ) -> Result<(), crate::apps::AppsServiceError> {
        let (host_meta_cache, mut _host_meta_crawler) = crate::host_meta_crawling();

        let client =
            set_up_rocket_with_dummy_infrastructure_and_a_running_app(host_meta_cache).await?;

        let response = client
            .get("/api/apps/master/logs/service-a")
            .header(Accept::EventStream)
            .dispatch()
            .await;

        let mut content_type_header = response.headers().get(CONTENT_TYPE.as_str());
        assert_eq!(content_type_header.next(), Some("text/event-stream"));

        Ok(())
    }
}
