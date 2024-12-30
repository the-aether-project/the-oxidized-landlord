#[macro_use]
extern crate rocket;
mod conn;

use rocket::fairing::{Fairing, Info, Kind};
use rocket::fs::FileServer;
use rocket::http::Header;
use rocket::serde::json::Json;
use rocket::{Request, Response};
use rocket_dyn_templates::{context, Template};

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

#[post("/negotiate-server", format = "json", data = "<token>")]
async fn server_negotiation_request(mut token: Json<serde_json::Value>) {
    let token = token.take().as_str().unwrap().to_owned();
    // Change this or fetch it dynamically.
    tokio::spawn(conn::ws::start_server_connection("127.0.0.1:7878", token));
}

#[get("/")]
async fn default_landing_page() -> Template {
    let conf = rocket::Config::figment().extract::<rocket::Config>();

    Template::render(
        "sample",
        context! {
            local_port: conf.map(|conf| conf.port).unwrap_or(8000u16)
        },
    )
}

#[options("/<_..>")]
fn all_options() {}

#[launch]
fn rocket() -> _ {
    let app = rocket::build();

    app.mount(
        "/",
        routes![
            default_landing_page,
            all_options,
            server_negotiation_request
        ],
    )
    .mount("/static", FileServer::from("./static"))
    .attach(CORS)
    .attach(Template::fairing())
}
