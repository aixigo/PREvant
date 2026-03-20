use futures::future::BoxFuture;
use http_body_util::{combinators::BoxBody, Full};
use hyper::{
    body::{Bytes, Incoming},
    server::conn::http1,
    service::service_fn,
    Request, Response,
};
use hyper_util::rt::TokioIo;
use prometheus_client::{encoding::text::encode, registry::Registry};
use rocket::{
    fairing::{Fairing, Info, Kind},
    Build, Orbit, Rocket,
};
use std::{
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, RwLock},
};
use tokio::{
    net::TcpListener,
    pin,
    signal::unix::{signal, SignalKind},
};

pub struct PrometheusMetrics {}

impl PrometheusMetrics {
    pub fn fairing() -> Self {
        Self {}
    }
}

pub type PrometheusRegistry = Arc<RwLock<Registry>>;

#[rocket::async_trait]
impl Fairing for PrometheusMetrics {
    fn info(&self) -> Info {
        Info {
            name: "apps",
            kind: Kind::Ignite | Kind::Liftoff,
        }
    }

    async fn on_ignite(&self, rocket: Rocket<Build>) -> rocket::fairing::Result {
        let registry = Registry::default();
        Ok(rocket.manage::<PrometheusRegistry>(Arc::new(RwLock::new(registry))))
    }

    async fn on_liftoff(&self, rocket: &Rocket<Orbit>) {
        let registry = rocket.state::<PrometheusRegistry>().unwrap().clone();

        let metrics_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 9100);
        log::info!("Starting metrics server on {metrics_addr}");

        // TODO: create a on_ignite so that it could fail
        let tcp_listener = TcpListener::bind(metrics_addr).await.unwrap();
        tokio::spawn(async move {
            start_metrics_server(tcp_listener, registry).await;
        });
    }
}

async fn start_metrics_server(tcp_listener: TcpListener, registry: PrometheusRegistry) {
    let server = http1::Builder::new();
    while let Ok((stream, _)) = tcp_listener.accept().await {
        let mut shutdown_stream = signal(SignalKind::terminate()).unwrap();
        let io = TokioIo::new(stream);
        let server_clone = server.clone();
        let registry_clone = registry.clone();
        tokio::task::spawn(async move {
            let conn = server_clone.serve_connection(io, service_fn(make_handler(registry_clone)));
            pin!(conn);
            tokio::select! {
                _ = conn.as_mut() => {}
                _ = shutdown_stream.recv() => {
                    conn.as_mut().graceful_shutdown();
                }
            }
        });
    }
}

fn full(bytes: Bytes) -> BoxBody<Bytes, std::io::Error> {
    use http_body_util::BodyExt;
    Full::new(bytes).map_err(|never| match never {}).boxed()
}

fn make_handler(
    registry: PrometheusRegistry,
) -> impl Fn(Request<Incoming>) -> BoxFuture<'static, io::Result<Response<BoxBody<Bytes, std::io::Error>>>>
{
    move |_req: Request<Incoming>| {
        let reg = registry.clone();

        Box::pin(async move {
            let mut buf = String::new();

            let reg = reg.read().unwrap();
            encode(&mut buf, &reg)
                .map_err(std::io::Error::other)
                .map(|_| {
                    let body = full(Bytes::from(buf));
                    Response::builder()
                        .header(
                            hyper::header::CONTENT_TYPE,
                            "application/openmetrics-text; version=1.0.0; charset=utf-8",
                        )
                        .body(body)
                        .unwrap()
                })
        })
    }
}
