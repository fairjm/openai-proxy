use std::{env, net::SocketAddr, time::SystemTime};

use axum::{extract::State, http::HeaderValue, response::Response, routing::any, Router};
use hyper::{client::HttpConnector, Body, Client, Request, StatusCode, Uri};
use hyper_proxy::{Intercept, Proxy, ProxyConnector};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use tracing::{info, log::warn};
use tracing_subscriber::{prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt};

#[derive(Clone, Debug)]
enum ClientEnum {
    Proxy(Client<ProxyConnector<HttpsConnector<HttpConnector>>>),
    Http(Client<HttpsConnector<HttpConnector>>),
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "openai_proxy=trace,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let https = HttpsConnectorBuilder::new()
        .with_native_roots()
        .https_or_http()
        .enable_http1()
        .build();

    let client = if let Ok(proxy) = env::var("HTTP_PROXY")
        .or_else(|_| env::var("http_proxy"))
        .or_else(|_| env::var("HTTPS_PROXY"))
        .or_else(|_| env::var("https_proxy"))
    {
        let proxy = {
            info!("use proxy:{}", proxy);
            let proxy_uri = proxy.parse().unwrap();
            let proxy = Proxy::new(Intercept::All, proxy_uri);
            // proxy.set_authorization(Authorization::basic("", ""));
            let proxy_connector = ProxyConnector::from_proxy(https, proxy).unwrap();
            proxy_connector
        };
        ClientEnum::Proxy(Client::builder().build::<_, hyper::Body>(proxy))
    } else {
        ClientEnum::Http(Client::builder().build::<_, hyper::Body>(https))
    };

    let app = Router::new()
        .route("/*path", any(handler))
        .with_state(client);

    let port = env::var("openai_proxy_port")
        .map(|e| e.parse().unwrap())
        .unwrap_or(4000);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("reverse proxy listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

async fn handler(
    State(client): State<ClientEnum>,
    mut req: Request<Body>,
) -> Result<Response<Body>, ()> {
    let path = req.uri().path();
    if path.starts_with("/openai/") {
        let path = path.replacen("/openai/", "/", 1);
        let query = if let Some(q) = req.uri().query() {
            q
        } else {
            ""
        };
        let uri = format!("https://api.openai.com{}{}", &path, query);
        info!("request to {}", uri);
        req.headers_mut()
            .insert("host", HeaderValue::from_static("api.openai.com"));
        *req.uri_mut() = Uri::try_from(uri.clone()).unwrap();

        let started = SystemTime::now();
        let r = match client {
            ClientEnum::Proxy(client) => client.request(req),
            ClientEnum::Http(client) => client.request(req),
        }
        .await
        .map_err(|e| {
            warn!("{} error:{}", uri, e);
            ()
        });
        info!(
            "request to {}. time: {}ms",
            uri,
            started.elapsed().unwrap().as_millis()
        );
        r
    } else {
        Ok(Response::builder()
            .status(StatusCode::OK)
            .body(Body::empty())
            .unwrap())
    }
}
