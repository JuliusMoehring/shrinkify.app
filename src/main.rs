use chrono::{DateTime, Utc};
use qrcode_generator::QrCodeEcc;
use rand::Rng;
use redis;
use rocket::fairing::{Fairing, Info, Kind};
use rocket::http::Header;
use rocket::http::Status;
use rocket::response::Redirect;
use rocket::response::Responder;
use rocket::serde::{json::Json, Deserialize, Serialize};
use rocket::{get, launch, post, routes};
use rocket::{Request, Response};
use std::env;
use std::net::Ipv4Addr;

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
            "POST, GET, OPTIONS",
        ));
        response.set_header(Header::new("Access-Control-Allow-Headers", "*"));
        response.set_header(Header::new("Access-Control-Allow-Credentials", "true"));
    }
}

const PORT_ENV_VAR: &str = "FUNCTIONS_CUSTOMHANDLER_PORT";
const REDIS_URI_ENV_VAR: &str = "REDIS_URI";

pub(crate) const HSET_TARGET: &str = "target";
pub(crate) const HSET_STATUS: &str = "status";

#[get("/")]
pub(crate) async fn index() -> Status {
    Status::Ok
}

#[derive(Debug)]
struct RedisRedirect {
    target: String,
    status: usize,
}

impl RedisRedirect {
    fn from_vec(vec: &Vec<String>) -> Option<Self> {
        let target_index = get_position(vec, "target")?;
        let status_index = get_position(vec, "status")?;

        let target = vec.get(target_index + 1)?;
        let status = vec.get(status_index + 1)?;

        let status: usize = status.parse().ok()?;

        Some(Self {
            target: String::from(target),
            status,
        })
    }
}

#[get("/<origin>")]
pub(crate) async fn redirect(origin: &str) -> Result<Redirect, Status> {
    if let Ok(mut redis_connection) = create_redis_connection() {
        let redirect_vec = get_by_origin(&mut redis_connection, origin);

        if let Some(redirect) = RedisRedirect::from_vec(&redirect_vec) {
            match redirect.status {
                301 => return Ok(Redirect::moved(redirect.target)),
                302 => return Ok(Redirect::found(redirect.target)),
                303 => return Ok(Redirect::to(redirect.target)),
                307 => return Ok(Redirect::temporary(redirect.target)),
                308 => return Ok(Redirect::permanent(redirect.target)),
                _ => return Ok(Redirect::to(redirect.target)),
            }
        }

        return Err(Status::NotFound);
    }

    Err(Status::InternalServerError)
}

fn get_position(vec: &Vec<String>, element: &str) -> Option<usize> {
    vec.iter().position(|entry| entry == element)
}

#[derive(Deserialize, Debug)]
#[serde(crate = "rocket::serde")]
struct CreateShrinkRequest {
    origin: String,
    target: String,
    #[serde(rename = "statusCode")]
    status_code: usize,
    #[serde(default)]
    #[serde(rename = "expireDate", with = "iso_8601")]
    expire_date: Option<DateTime<Utc>>,
}

#[post("/", data = "<create_shrink>")]
pub(crate) async fn create(create_shrink: Json<CreateShrinkRequest>) -> Status {
    if let Ok(mut redis_connection) = create_redis_connection() {
        let create_shrink_result: Result<usize, redis::RedisError> = redis::cmd("HSET")
            .arg(&create_shrink.origin)
            .arg(HSET_TARGET)
            .arg(&create_shrink.target)
            .arg(HSET_STATUS)
            .arg(&create_shrink.status_code)
            .query::<usize>(&mut redis_connection);

        if create_shrink_result.is_ok() {
            if let Some(date) = create_shrink.expire_date {
                let set_expire_at_result: Result<usize, redis::RedisError> = redis::cmd("EXPIREAT")
                    .arg(&create_shrink.origin)
                    .arg(date.timestamp())
                    .query::<usize>(&mut redis_connection);

                if let Err(..) = set_expire_at_result {
                    return Status::InternalServerError;
                }
            }

            return Status::Created;
        }
    }

    Status::InternalServerError
}

#[derive(Serialize)]
#[serde(crate = "rocket::serde")]
struct GenerateOriginResponse {
    origin: String,
}

#[get("/generate-origin")]
pub(crate) async fn generate_origin() -> Result<Json<GenerateOriginResponse>, Status> {
    if let Ok(mut redis_connection) = create_redis_connection() {
        let origin = generate_unique_origin(&mut redis_connection);

        return Ok(Json(GenerateOriginResponse { origin }));
    }

    Err(Status::InternalServerError)
}

#[derive(Deserialize, Debug)]
#[serde(crate = "rocket::serde")]
struct ValidateOriginRequest {
    origin: String,
}

#[post("/validate-origin", data = "<validate_origin_request>")]
pub(crate) fn validate_origin(validate_origin_request: Json<ValidateOriginRequest>) -> Status {
    if let Ok(mut redis_connection) = create_redis_connection() {
        if check_if_path_exists(&mut redis_connection, &validate_origin_request.origin) {
            return Status::Conflict;
        }

        return Status::Ok;
    }

    Status::InternalServerError
}

#[derive(Deserialize, Debug)]
#[serde(crate = "rocket::serde")]
struct GenerateQRCodeRequest {
    shrink: String,
}

#[derive(Responder)]
#[response(content_type = "image/svg+xml")]
pub(crate) struct SVG(pub(crate) String);

#[post("/generate-qr-code", data = "<generate_qr_code_request>")]
pub(crate) fn generate_qr_code(
    generate_qr_code_request: Json<GenerateQRCodeRequest>,
) -> Result<SVG, Status> {
    let result = qrcode_generator::to_svg_to_string(
        &generate_qr_code_request.shrink,
        QrCodeEcc::Low,
        1024,
        None::<&str>,
    );

    match result {
        Ok(svg) => Ok(SVG(svg)),
        Err(..) => Err(Status::InternalServerError),
    }
}

#[launch]
fn rocket() -> _ {
    let port: u16 = match env::var(PORT_ENV_VAR) {
        Ok(val) => val.parse().expect("Custom Handler port is not a number!"),
        Err(_) => 3000,
    };

    let figment = rocket::Config::figment()
        .merge(("profile", Ipv4Addr::LOCALHOST))
        .merge(("port", port));

    rocket::custom(figment)
        .attach(CORS)
        .mount("/", routes![index])
        .mount("/api", routes![redirect])
        .mount(
            "/api/shrink",
            routes![generate_origin, validate_origin, create, generate_qr_code],
        )
}

fn create_redis_connection() -> Result<redis::Connection, String> {
    let redis_uri: String = env::var(REDIS_URI_ENV_VAR).expect("REDIS_URI not set");

    if let Ok(client) = redis::Client::open(redis_uri) {
        if let Ok(connection) = client.get_connection() {
            return Ok(connection);
        }

        return Err("Could not connect to database".to_string());
    }

    Err("Invalid connection URL".to_string())
}

fn generate_random_path(path_length: Option<usize>) -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    const DEFAULT_PATH_LENGTH: usize = 8;

    let path_length = path_length.unwrap_or(DEFAULT_PATH_LENGTH);

    let mut rng = rand::thread_rng();

    let random_path: String = (0..path_length)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect();

    random_path
}

fn generate_unique_origin(redis_connection: &mut redis::Connection) -> String {
    let mut is_path_unique = false;
    let mut path = generate_random_path(None);

    while !is_path_unique {
        if check_if_path_exists(redis_connection, &path) {
            path = generate_random_path(None)
        } else {
            is_path_unique = true
        }
    }

    path
}

fn get_by_origin(redis_connection: &mut redis::Connection, origin: &str) -> Vec<String> {
    let redis_origin: Vec<String> = redis::cmd("HGETALL")
        .arg(&origin)
        .query(redis_connection)
        .expect(format!("failed to execute HGETALL for {}", &origin).as_str());

    redis_origin
}

fn check_if_path_exists(redis_connection: &mut redis::Connection, origin: &str) -> bool {
    let redis_origin = get_by_origin(redis_connection, origin);

    redis_origin.len() != 0
}

mod iso_8601 {
    use chrono::{DateTime, TimeZone, Utc};
    use serde::{self, Deserialize, Deserializer};

    const FORMAT: &'static str = "%Y-%m-%dT%H:%M:%S%.3fZ";

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<DateTime<Utc>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: Option<String> = Option::deserialize(deserializer)?;

        if let Some(s) = s {
            return Ok(Some(
                Utc.datetime_from_str(&s, FORMAT)
                    .map_err(serde::de::Error::custom)?,
            ));
        }

        Ok(None)
    }
}
