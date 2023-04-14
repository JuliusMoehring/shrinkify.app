use rand::Rng;
use redis;
use rocket::http::Status;
use rocket::response::Redirect;
use rocket::serde::{json::Json, Deserialize, Serialize};
use rocket::{get, launch, post, routes};
use std::env;
use std::net::Ipv4Addr;

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
            println!("{:?}", redirect);

            match redirect.status {
                301 => return Ok(Redirect::moved(redirect.target)),
                302 => return Ok(Redirect::found(redirect.target)),
                303 => return Ok(Redirect::to(redirect.target)),
                307 => return Ok(Redirect::temporary(redirect.target)),
                308 => return Ok(Redirect::permanent(redirect.target)),
                _ => return Ok(Redirect::to(redirect.target)),
            }
        }
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
    status: usize,
}

#[post("/", data = "<create_shrink>")]
pub(crate) async fn create(create_shrink: Json<CreateShrinkRequest>) -> Status {
    if let Ok(mut redis_connection) = create_redis_connection() {
        let create_shrink_result: Result<usize, redis::RedisError> = redis::cmd("HSET")
            .arg(&create_shrink.origin)
            .arg(HSET_TARGET)
            .arg(&create_shrink.target)
            .arg(HSET_STATUS)
            .arg(&create_shrink.status)
            .query::<usize>(&mut redis_connection);

        if create_shrink_result.is_ok() {
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
        .mount("/", routes![index])
        .mount("/api", routes![redirect])
        .mount(
            "/api/shrink",
            routes![generate_origin, validate_origin, create],
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
