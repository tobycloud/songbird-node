mod discord;
mod ffmpeg_support;
mod obj_event;
mod router;
mod task_support;
mod types;

use axum::{
    extract::Request,
    http::{self, StatusCode},
    middleware::{from_fn, Next},
    response::Response,
};
use json_comments::StripComments;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::net::SocketAddr;

lazy_static! {
    static ref ROOT_CONFIG: ConfigFile = {
        let file_data = std::fs::read("config.json").unwrap();
        let stripped = StripComments::new(file_data.as_slice());
        let root_config: ConfigFile =
            serde_json::from_reader(stripped).expect("Config Error: Couldn't parse config");
        root_config
    };
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConfigFile {
    pub bind: String,
    pub auth: Value,
}

async fn auth(req: Request, next: Next) -> Result<Response, StatusCode> {
    if req.uri().path() == "/" {
        return Err(StatusCode::OK);
    }
    if ROOT_CONFIG.auth.is_string() {
        let auth_header = req
            .headers()
            .get(http::header::AUTHORIZATION)
            .and_then(|header| header.to_str().ok());
        if auth_header.is_none() {
            return Err(StatusCode::UNAUTHORIZED);
        } else if auth_header.unwrap() != ROOT_CONFIG.auth.as_str().unwrap() {
            return Err(StatusCode::UNAUTHORIZED);
        }
    } else if !ROOT_CONFIG.auth.is_null() && !ROOT_CONFIG.auth.is_string() {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    Ok(next.run(req).await)
}

#[tokio::main]
async fn main() {
    // create the server
    let app = router::get_router().layer(from_fn(auth));
    let server_addr = &ROOT_CONFIG.bind;
    let addr_l: SocketAddr = server_addr.parse().expect("Unable to parse socket address");
    println!("listening on {}", addr_l.to_string());
    let listener = tokio::net::TcpListener::bind(addr_l).await.unwrap();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}
