use std::{env, net::SocketAddr, time::SystemTime};

use axum::{extract::State, http::HeaderValue, response::Response, routing::any, Router};
use hyper::{
    client::{connect::Connect, HttpConnector},
    Body, Client, Request, StatusCode, Uri,
};
use hyper_proxy::{Intercept, Proxy, ProxyConnector};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use libflate::gzip::Decoder;
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

        req = read_body(req).await;

        let started = SystemTime::now();
        let r = match client {
            ClientEnum::Proxy(client) => {
                check(&client);
                client.request(req)
            }
            ClientEnum::Http(client) => {
                check(&client);
                client.request(req)
            }
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
        if let Ok(resp) = r {
            let new_resp = read_response(resp).await;
            Ok(new_resp)
        } else {
            r
        }
    } else {
        Ok(Response::builder()
            .status(StatusCode::OK)
            .body(Body::empty())
            .unwrap())
    }
}

fn check<C>(c: &Client<C>)
where
    C: Connect + Clone + Send + Sync + 'static,
{
    info!("{:?}", c);
}

async fn read_body(mut req: Request<Body>) -> Request<Body> {
    // code from https://stackoverflow.com/questions/75849660/how-to-use-the-body-of-a-hyperrequest-without-consuming-it
    // destructure the request so we can get the body & other parts separately
    let (parts, body) = req.into_parts();
    let body_bytes = hyper::body::to_bytes(body).await.unwrap();
    let body = std::str::from_utf8(&body_bytes).unwrap();

    info!("request body:\n\n{}\n", body);
    // reconstruct the Request from parts and the data in `body_bytes`
    req = Request::from_parts(parts, body_bytes.into());

    return req;
}

async fn read_response(mut resp: Response<Body>) -> Response<Body> {
    // destructure the request so we can get the body & other parts separately
    let (mut parts, body) = resp.into_parts();
    // println!("body: {:?}", body);
    // info!("parts: {:?}", parts);
    let body_bytes = hyper::body::to_bytes(body).await.unwrap();

    use std::io::Read;

    if parts.headers.get("content-encoding").is_some() {
        let mut decoder = Decoder::new(&body_bytes[..]).unwrap();
        let mut decoded_data = Vec::new();
        decoder.read_to_end(&mut decoded_data).unwrap();
        // println!("{:?}", body_bytes);
        let body = String::from_utf8(decoded_data).unwrap();
        info!("response:\n\n{}\n", body);
    } else {
        let body = std::str::from_utf8(&body_bytes).unwrap();
        info!("response:\n\n{}\n", body);
    }
    // now we have all data so we just disable chunk and send all data
    parts.headers.remove("transfer-encoding");
    parts
        .headers
        .insert("Content-Length", HeaderValue::from(body_bytes.len()));
    resp = Response::from_parts(parts, body_bytes.into());

    return resp;
}
