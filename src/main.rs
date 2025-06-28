use axum::{
    extract::Query,
    http::{StatusCode, header, HeaderValue},
    response::{IntoResponse, Response},
    routing::get,
    Router,
    body::Body,
};
use serde::Deserialize;
use reqwest::{Client, header as reqwest_header};
use std::time::Duration;
use tower_http::cors::{CorsLayer, AllowOrigin};

#[derive(Deserialize)]
struct FetchQuery {
    url: String,
    ref_: Option<String>,
}

#[tokio::main]
async fn main() {
    let cors_layer = CorsLayer::new()
        .allow_origin(AllowOrigin::any())
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any);

    let app = Router::new()
        .route("/health", get(health_check))
        .route("/fetch", get(fetch_handler))
        .layer(cors_layer);

    println!("🚀 Listening on http://127.0.0.1:3000");

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn health_check() -> &'static str {
    "Hello via Axum!"
}

async fn fetch_handler(
    Query(params): Query<FetchQuery>,
) -> Response {
    let parsed = match url::Url::parse(&params.url) {
        Ok(u) => u,
        Err(_) => return (
            StatusCode::BAD_REQUEST,
            "Invalid URL".to_string()
        ).into_response(),
    };

    let ref_header = params.ref_.unwrap_or_else(|| parsed.origin().ascii_serialization());

    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .unwrap();

    let mut headers = reqwest_header::HeaderMap::new();
    headers.insert(
        reqwest_header::USER_AGENT,
        HeaderValue::from_static("Mozilla/5.0 (compatible; RustProxy/1.0)"),
    );
    headers.insert(
        reqwest_header::REFERER,
        HeaderValue::from_str(&ref_header).unwrap_or(HeaderValue::from_static("")),
    );
    headers.insert(
        reqwest_header::ACCEPT,
        HeaderValue::from_static("*/*"),
    );

    // .ts segments might need Range
    if parsed.path().ends_with(".ts") {
        headers.insert(
            reqwest_header::RANGE,
            HeaderValue::from_static("bytes=0-"),
        );
    }

    // add Origin header
    if let Some(origin) = parsed.domain() {
        let origin_header = format!("https://{}", origin);
        headers.insert(
            reqwest_header::ORIGIN,
            HeaderValue::from_str(&origin_header).unwrap_or(HeaderValue::from_static("")),
        );
    }

    let result = client
        .get(parsed.clone())
        .headers(headers)
        .send()
        .await;

    match result {
        Ok(res) => {
            let status = res.status();
            let headers_copy = res.headers().clone();

            // helpful debug
            if status == StatusCode::GONE {
                eprintln!("410 Gone, response headers: {:?}", headers_copy);
            }

            let content_type = headers_copy
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("text/plain")
                .to_string();

            let original_cache_control = headers_copy
                .get(header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            let original_cdn_cache_control = headers_copy
                .get("CDN-Cache-Control")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            let (cache_control_header, cdn_cache_control_header, proxied_content_type) =
                if content_type.contains("application/vnd.apple.mpegurl") || parsed.path().ends_with(".m3u8") {
                    let cache_control = original_cache_control
                        .unwrap_or_else(|| "public, max-age=18000, stale-while-revalidate=300".to_string());
                    let cdn_cache = original_cdn_cache_control
                        .unwrap_or_else(|| "max-age=18000".to_string());
                    (cache_control, cdn_cache, "application/vnd.apple.mpegurl".to_string())
                } else {
                    let cache_control = original_cache_control
                        .unwrap_or_else(|| "public, max-age=2592000, stale-while-revalidate=86400".to_string());
                    let cdn_cache = original_cdn_cache_control
                        .unwrap_or_else(|| "max-age=2592000".to_string());
                    let proxied_type = if content_type.contains("video/mp2t") || parsed.path().ends_with(".ts") {
                        "video/mp2t".to_string()
                    } else {
                        content_type.clone()
                    };
                    (cache_control, cdn_cache, proxied_type)
                };

            if content_type.contains("application/vnd.apple.mpegurl") || parsed.path().ends_with(".m3u8") {
                let text = res.text().await.unwrap_or_default();

                let lines = text
                    .lines()
                    .map(|line| {
                        if line.starts_with("#EXT-X-KEY") {
                            if let Some(start) = line.find("URI=\"") {
                                let key_uri_start = start + 5;
                                let key_uri_end = line[key_uri_start..]
                                    .find('"')
                                    .map(|e| e + key_uri_start)
                                    .unwrap_or(line.len());
                                let key_uri = &line[key_uri_start..key_uri_end];
                                if let Ok(resolved) = parsed.join(key_uri) {
                                    let proxied = format!("/fetch?url={}", urlencoding::encode(resolved.as_str()));
                                    return line.replace(key_uri, &proxied);
                                }
                            }
                            return line.to_string();
                        }
                        if line.starts_with("#") || line.trim().is_empty() {
                            return line.to_string();
                        }
                        if let Ok(resolved) = parsed.join(line) {
                            let proxied = format!("/fetch?url={}", urlencoding::encode(resolved.as_str()));
                            return proxied;
                        }
                        line.to_string()
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                return Response::builder()
                    .status(status)
                    .header("content-type", proxied_content_type)
                    .header("cache-control", cache_control_header)
                    .header("CDN-Cache-Control", cdn_cache_control_header)
                    .body(Body::from(lines))
                    .unwrap_or_else(|_| {
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "Body assembly failed".to_string()
                        ).into_response()
                    });
            }

            // for binary .ts or other files
            let body = res.bytes().await.unwrap_or_default();

            Response::builder()
                .status(status)
                .header("content-type", proxied_content_type)
                .header("cache-control", cache_control_header)
                .header("CDN-Cache-Control", cdn_cache_control_header)
                .body(Body::from(body))
                .unwrap_or_else(|_| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Body assembly failed".to_string()
                    ).into_response()
                })
        }
        Err(e) => {
            eprintln!("proxy error: {e:?}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Fetch failed: {e}")
            ).into_response()
        }
    }
}
