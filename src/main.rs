use std::sync::Arc;

use actix_web::{
    dev::Payload,
    http::StatusCode,
    middleware,
    web::{self},
    App, Error as AWError, FromRequest, HttpRequest, HttpResponse, HttpServer, ResponseError,
};
use futures_util::future::{err, ok, Ready};
use r2d2_sqlite::{self, SqliteConnectionManager};

mod db;
use db::Pool;
use serde::{Deserialize, Serialize};

static PATH_RE: &lazy_regex::Lazy<lazy_regex::Regex> =
    lazy_regex::regex!(r"^/?([a-z][a-z0-9+\-.]*:)/+");

#[derive(Debug)]
struct ShakyUrl(url::Url);

#[derive(Debug)]
enum ShakyUrlError {
    String(String),
    ParseError(url::ParseError),
}

impl std::fmt::Display for ShakyUrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShakyUrlError::String(x) => f.write_str(x.as_str()),
            ShakyUrlError::ParseError(err) => f.write_fmt(format_args!("{}", err)),
        }
    }
}

impl ResponseError for ShakyUrlError {
    fn status_code(&self) -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

impl FromRequest for ShakyUrl {
    type Error = ShakyUrlError;
    type Future = Ready<Result<Self, Self::Error>>;

    fn from_request(req: &HttpRequest, _: &mut Payload) -> Self::Future {
        let uri = format!(
            "{}{}",
            req.match_info().query("url_no_query"),
            if req.query_string() == "" {
                String::new()
            } else {
                format!("?{}", req.query_string())
            },
        ); // req.uri().to_string();
        log::debug!("Extracted url from request {}", uri);
        // Strip first slash and denormalize url
        // BOTT-INT clients normalize slashes in url path (https://example.com turns to https:/example.com)
        let uri = PATH_RE.replace(uri.as_str(), "${1}//").to_string();
        match url::Url::parse(&uri.as_str()) {
            Ok(x) => {
                let scheme = x.scheme();
                if !(scheme == "https" || scheme == "http") {
                    err(ShakyUrlError::String(format!("Unknown scheme: {scheme}")))
                } else {
                    ok(Self(x))
                }
            }
            Err(e) => err(ShakyUrlError::ParseError(e)),
        }
    }
}

impl TryFrom<&str> for ShakyUrl {
    type Error = String;

    fn try_from(uri: &str) -> Result<Self, Self::Error> {
        let uri = PATH_RE.replace(uri, "${1}//").to_string();
        match url::Url::parse(&uri.as_str()) {
            Ok(x) => {
                let scheme = x.scheme();
                if !(scheme == "https" || scheme == "http") {
                    Err(format!("Unknown scheme: {scheme}"))
                } else {
                    Ok(Self(x))
                }
            }
            Err(err) => Err(format!("Invalid url {uri} Original error: {err}")),
        }
    }
}

async fn cache(
    data: web::Data<(db::CacheSettings, Pool)>,
    client: web::Data<awc::Client>,
    url: ShakyUrl,
    req: HttpRequest,
) -> Result<HttpResponse, AWError> {
    if req.method() == &actix_web::http::Method::OPTIONS {
        log::info!("Ignoring {} request", req.method());
        let mut res = HttpResponse::Ok();
        res.append_header(("access-control-allow-origin", "*"));
        res.append_header(("access-control-allow-headers", "*"));
        return Ok(res.finish());
    }
    let settings = &data.0;
    let db = &data.1;
    let result = db::execute(&settings, &db, &req, &url.0, &client).await?;
    log::debug!("{result:?}");
    log::debug!("{:?}", req.match_info());
    log::debug!("ShakyUrl: {:?}", url);

    Ok(result)
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum ListOrString {
    ListV(Vec<String>),
    StringV(String),
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("debug"));

    // connect to SQLite DB
    let manager = SqliteConnectionManager::file("cache.db"); // TODO
    let pool = Pool::new(manager).unwrap();
    db::create_db(&pool).unwrap();
    let settings = db::CacheSettings {
        client_errors: true,
        server_errors: false,
        ttl: 0,
    };
    log::info!("starting HTTP proxy server at http://localhost:8080/proxy/");
    let client_tls_config = Arc::new(rustls_config());
    // start HTTP server
    HttpServer::new(move || {
        let client = awc::Client::builder()
            // Wikipedia requires a User-Agent header to make requests
            .disable_timeout()
            .add_default_header(("user-agent", "awc-example/1.0"))
            // a "connector" wraps the stream into an encrypted connection
            .connector(awc::Connector::new().rustls(Arc::clone(&client_tls_config)))
            .finish();
        App::new()
            // store db pool as Data object
            .app_data(web::Data::new((settings.clone(), pool.clone())))
            .app_data(web::Data::new(client))
            .wrap(middleware::Logger::default())
            .service(web::resource("/proxy/{url_no_query:https?:/.*}").route(web::to(cache)))
            .default_service(web::to(not_found))
    })
    .bind(("127.0.0.1", 8080))? // TODO
    .worker_max_blocking_threads(1) // TODO
    .workers(1) // TODO
    .run()
    .await
}

async fn not_found() -> Result<HttpResponse, AWError> {
    Ok(HttpResponse::build(StatusCode::NOT_FOUND)
        .content_type("application/json")
        .body(r#"{"errors": [{"status": "404"}]}"#))
}

/// Create simple rustls client config from root certificates.
fn rustls_config() -> rustls::ClientConfig {
    let mut root_store = rustls::RootCertStore::empty();
    root_store.add_server_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.0.iter().map(|ta| {
        rustls::OwnedTrustAnchor::from_subject_spki_name_constraints(
            ta.subject,
            ta.spki,
            ta.name_constraints,
        )
    }));

    rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(root_store)
        .with_no_client_auth()
}
