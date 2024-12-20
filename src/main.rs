#[macro_use]
extern crate rocket;

mod conn;

use rocket::fairing::{Fairing, Info, Kind};
use rocket::http::Header;
use rocket::serde::json::Json;
use rocket::State;
use rocket::{Request, Response};
use tokio::sync::Mutex;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

pub struct CORS;

#[rocket::async_trait]
impl Fairing for CORS {
    fn info(&self) -> Info {
        Info {
            name: "Add CORS headers to responses",
            kind: Kind::Response,
        }
    }

    async fn on_response<'r>(&self, _request: &'r Request<'_>, response: &mut Response<'r>) {
        response.set_header(Header::new("Access-Control-Allow-Origin", "*"));
        response.set_header(Header::new(
            "Access-Control-Allow-Methods",
            "POST, GET, PATCH, OPTIONS",
        ));
        response.set_header(Header::new("Access-Control-Allow-Headers", "*"));
        response.set_header(Header::new("Access-Control-Allow-Credentials", "true"));
    }
}

#[post("/sdp", format = "json", data = "<session_description>")]
async fn sdp_endpoint(
    manager: &State<Mutex<conn::AetherWebRTCConnectionManager>>,
    session_description: Json<RTCSessionDescription>,
) -> Json<RTCSessionDescription> {
    Json::from(
        manager
            .lock()
            .await
            .connect(session_description.into_inner())
            .await
            .unwrap(),
    )
}

#[options("/<_..>")]
fn all_options() {}

#[launch]
fn rocket() -> _ {
    let app = rocket::build();
    let mut m = MediaEngine::default();

    m.register_default_codecs()
        .expect("Unable to register default codecs.");

    let mut registry = Registry::new();

    registry = register_default_interceptors(registry, &mut m)
        .expect("Unable to register default interceptors.");

    let api = APIBuilder::new()
        .with_media_engine(m)
        .with_interceptor_registry(registry)
        .build();

    app.manage(Mutex::new(conn::AetherWebRTCConnectionManager::new(api)))
        .mount("/", routes![sdp_endpoint, all_options])
        .attach(CORS)
}
