use std::{collections::HashMap, str::FromStr};

use actix_web::{
    error,
    http::{header::HeaderMap, Method, StatusCode},
    Error, HttpRequest, HttpResponse, HttpResponseBuilder,
};
use chrono::{DateTime, Utc};
use r2d2_sqlite::rusqlite::named_params;
use rusqlite::{types::FromSql, Row, ToSql};
use url::Url;

pub type Pool = r2d2::Pool<r2d2_sqlite::SqliteConnectionManager>;

const CREATE_SQL: &str = "
CREATE TABLE IF NOT EXISTS cache (
 method TEXT,
 url TEXT,
 content BLOB,
 headers TEXT,
 status_code INTEGER,
 last_update TEXT DEFAULT CURRENT_TIMESTAMP NOT NULL,
 PRIMARY KEY (method, url)
)";

const UPSERT_SQL: &str = "
INSERT INTO cache (method, url, content, headers, status_code) VALUES (:method, :url, :content, :headers, :status_code)
 ON CONFLICT(method, url) DO UPDATE SET
 content=excluded.content,
 headers=excluded.headers,
 status_code=excluded.status_code,
 last_update=CURRENT_TIMESTAMP";

#[derive(Debug)]
pub struct Entry {
    pub method: Method,
    pub url: Url,
    pub content: Vec<u8>,
    pub headers: HttpHeaders,
    pub status_code: StatusCode,
    pub last_update: DateTime<Utc>,
}

impl Into<HttpResponse> for &Entry {
    fn into(self) -> HttpResponse {
        let mut builder = HttpResponseBuilder::new(self.status_code);
        for (key, values) in &self.headers.0 {
            for value in values {
                builder.append_header((key.to_owned(), value.to_owned()));
            }
        }
        builder.body(self.content.clone())
    }
}

impl TryFrom<&Row<'_>> for Entry {
    type Error = rusqlite::Error;

    fn try_from(row: &Row<'_>) -> Result<Self, Self::Error> {
        let m: String = row.get("method")?;
        Ok(Entry {
            method: Method::from_str(m.as_str()).unwrap(),
            url: row.get("url")?,
            content: row.get("content")?,
            headers: row.get("headers")?,
            status_code: StatusCode::from_u16(row.get("status_code")?).unwrap(),
            last_update: row.get("last_update")?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct CacheSettings {
    pub client_errors: bool,
    pub server_errors: bool,
    pub ttl: u16,
    sql: String,
}

impl CacheSettings {
    pub fn new(client_errors: bool, server_errors: bool, ttl: u16) -> Self {
        let mut sql = String::from("SELECT * FROM cache WHERE method = :method AND url = :url");
        if ttl > 0 {
            sql += format!(
                " AND last_update > datetime(CURRENT_TIMESTAMP, '-{} seconds')",
                ttl
            )
            .as_str();
        }
        sql += " AND (status_code < 400";
        if client_errors {
            sql += " OR status_code BETWEEN 400 AND 499";
        }
        if server_errors {
            sql += " OR status_code BETWEEN 500 AND 599";
        }
        sql += ")";
        CacheSettings {
            client_errors,
            server_errors,
            ttl,
            sql,
        }
    }

    pub fn to_sql(&self) -> &str {
        self.sql.as_str()
    }
}

pub fn create_db(pool: &Pool) -> Result<usize, rusqlite::Error> {
    log::debug!("Creating database");
    let conn = pool.get().unwrap();
    conn.execute(CREATE_SQL, ())
}

#[derive(Debug)]
pub struct HttpHeaders(HashMap<String, Vec<String>>);

impl From<&HeaderMap> for HttpHeaders {
    fn from(headers: &HeaderMap) -> Self {
        let mut m: HashMap<String, Vec<String>> = HashMap::new();
        for k in headers.keys() {
            m.insert(
                k.to_string(),
                headers
                    .get_all(k)
                    .map(|x| x.to_str().unwrap().into())
                    .collect(),
            );
        }
        Self(m)
    }
}

impl FromSql for HttpHeaders {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        value.as_str().and_then(|s| match serde_json::from_str(s) {
            Ok(x) => Ok(Self(x)),
            Err(err) => Err(rusqlite::types::FromSqlError::Other(Box::new(err))),
        })
    }
}

impl ToSql for HttpHeaders {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        match serde_json::to_string(&self.0) {
            Ok(x) => Ok(rusqlite::types::ToSqlOutput::Owned(
                rusqlite::types::Value::Text(x),
            )),
            Err(err) => Err(rusqlite::Error::ToSqlConversionFailure(Box::new(err))),
        }
    }
}

pub async fn execute(
    settings: &CacheSettings,
    pool: &Pool,
    request: &HttpRequest,
    url: &Url,
    client: &awc::Client,
) -> Result<HttpResponse, Error> {
    log::debug!("{:?}", request.uri());
    let method = request.method().to_string();
    let pool = pool.clone();
    let conn = pool.get().map_err(error::ErrorInternalServerError)?;
    let mut stmt = conn.prepare_cached(settings.to_sql()).unwrap();
    let mut entry_iter = stmt
        .query_map(
            named_params! {":method": method, ":url": url.to_string()},
            |row| Entry::try_from(row),
        )
        .map_err(error::ErrorInternalServerError)?;
    match entry_iter.next() {
        Some(x) => {
            log::info!("Serving from cache");
            x
        }
        None => {
            log::info!("No match, proxying");
            let mut client_req = client.request(request.method().to_owned(), url.to_string());
            for header in request.headers() {
                client_req = client_req.insert_header(header);
            }
            client_req = client_req.insert_header(("host", url.host().unwrap().to_string()));
            log::debug!("{} {}", client_req.get_method(), client_req.get_uri());
            let mut res = client_req.send().await.unwrap();
            let content = res.body().limit(core::usize::MAX).await.unwrap(); // TODO limit
            log::debug!("Response: {:?}", res); // <- server http response
            let mut client_response = HttpResponse::build(res.status());
            for (header_name, header_value) in res
                .headers()
                .iter()
                .filter(|(h, _)| !(*h == "connection" || *h == "content-encoding"))
            {
                // TODO factor out header filtering
                client_response.insert_header((header_name.clone(), header_value.clone()));
            }

            let client_response = client_response.finish();
            let entry = Entry {
                method: request.method().into(),
                url: url.clone(),
                content: content.to_vec(), // response.body(),
                headers: HttpHeaders::from(client_response.headers()),
                status_code: client_response.status(),
                last_update: Utc::now(),
            };
            // TODO maybe check with settings if we should save? Or is check only on SELECT?
            log::debug!("Saving to database");
            let mut stmt = conn.prepare_cached(UPSERT_SQL).unwrap();
            stmt.execute(named_params! {
                    ":method": &entry.method.to_string(),
                    ":url": &entry.url,
                    ":content": &entry.content,
                    ":headers": &entry.headers,
                    ":status_code": &entry.status_code.as_str(),
            })
            .unwrap();
            Ok(entry)
        }
    }
    .map(|entry| Ok((&entry).into()))
    .map_err(error::ErrorInternalServerError)?
}
