use crate::chron_service::{CommandType, ThreadState};
use crate::http_error::HttpError;
use actix_web::{
    delete, get, http::StatusCode, post, web, web::Bytes, App, HttpServer, Responder, Result,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;

#[get("/status")]
async fn status_overview(data: web::Data<ThreadState>) -> Result<impl Responder> {
    let response = data
        .commands
        .iter()
        .map(|(name, command_mutex)| {
            let command = command_mutex.lock().unwrap();
            (
                name.clone(),
                json!({
                    "type": match command.command_type {
                        CommandType::Startup => "startup",
                        CommandType::Scheduled(_) => "scheduled",
                    },
                    "running": command.process.is_some(),
                }),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();

    Ok(web::Json(response))
}

#[get("/status/{name}")]
async fn status(name: web::Path<String>, data: web::Data<ThreadState>) -> Result<impl Responder> {
    let name = name.into_inner();

    // Make sure that this command exists before proceeding
    let command_mutex = data
        .commands
        .get(&name)
        .ok_or_else(|| HttpError::from_status_code(StatusCode::NOT_FOUND))?;

    // Load the last few runs from the database
    let db = data.db.lock().unwrap();
    let runs = db
        .get_last_runs(&name, 5)
        .unwrap()
        .into_iter()
        .map(|run| {
            json!({
                "timestamp": run.timestamp,
                "status_code": run.status_code,
            })
        })
        .collect::<Vec<_>>();
    drop(db);

    let command = command_mutex.lock().unwrap();
    Ok(web::Json(json!({
        "name": name,
        "runs": runs,
        "next_run": match &command.command_type {
            CommandType::Startup => None,
            CommandType::Scheduled(scheduled_command) => scheduled_command.next_run(),
        },
        "pid": command.process.as_ref().map(|process| process.id()),
    })))
}

#[get("/log/{name}")]
async fn get_log(name: web::Path<String>, data: web::Data<ThreadState>) -> Result<impl Responder> {
    let command_mutex = data
        .commands
        .get(&name.into_inner())
        .ok_or_else(|| HttpError::from_status_code(StatusCode::NOT_FOUND))?;
    let log_path = command_mutex.lock().unwrap().log_path.clone();
    let log_contents = fs::read_to_string(log_path)?;
    Ok(log_contents)
}

#[delete("/log/{name}")]
async fn delete_log(
    name: web::Path<String>,
    data: web::Data<ThreadState>,
) -> Result<impl Responder> {
    let name = name.into_inner();
    let command_mutex = data
        .commands
        .get(&name)
        .ok_or_else(|| HttpError::from_status_code(StatusCode::NOT_FOUND))?;
    let log_path = command_mutex.lock().unwrap().log_path.clone();
    fs::write(log_path, "")?;
    Ok(format!("Erased log file for {}", name))
}

#[post("/terminate/{name}")]
async fn terminate(
    name: web::Path<String>,
    data: web::Data<ThreadState>,
) -> Result<impl Responder> {
    let name = name.into_inner();
    let command_mutex = data
        .commands
        .get(&name)
        .ok_or_else(|| HttpError::from_status_code(StatusCode::NOT_FOUND))?;
    let mut command = command_mutex.lock().unwrap();
    let message = match command.process.as_mut() {
        Some(process) => {
            process.kill()?;
            format!("Terminated command {name}")
        }
        None => format!("Command {name} isn't currently running"),
    };
    Ok(message)
}

#[get("/mailbox")]
async fn mailbox_overview(data: web::Data<ThreadState>) -> Result<impl Responder> {
    let db = data.db.lock().unwrap();
    let sizes = db
        .get_mailbox_sizes()
        .map_err(HttpError::from_database_error)?
        .filter(|(name, _)| data.commands.contains_key(name))
        .collect::<BTreeMap<_, _>>();
    Ok(web::Json(sizes))
}

#[get("/mailbox/{name}")]
async fn read_mailbox(
    name: web::Path<String>,
    data: web::Data<ThreadState>,
) -> Result<impl Responder> {
    let db = data.db.lock().unwrap();
    let messages = db
        .get_messages(&name.into_inner())
        .map_err(HttpError::from_database_error)?;
    Ok(web::Json(
        messages
            .into_iter()
            .map(|message| {
                json!({
                    "content": message.content,
                    "timestamp": message.timestamp,
                    "read": message.read,
                })
            })
            .collect::<Vec<_>>(),
    ))
}

#[post("/mailbox/{name}")]
async fn write_message(
    name: web::Path<String>,
    body: Bytes,
    data: web::Data<ThreadState>,
) -> Result<impl Responder> {
    let name = name.into_inner();
    let content = std::str::from_utf8(&body)
        .map_err(|_| HttpError::from_status_code(StatusCode::BAD_REQUEST))?;

    let db = data.db.lock().unwrap();
    db.add_message(&name, content)
        .map_err(HttpError::from_database_error)?;
    Ok(format!("Added message to {name} mailbox"))
}

#[delete("/mailbox/{name}")]
async fn empty_mailbox(
    name: web::Path<String>,
    data: web::Data<ThreadState>,
) -> Result<impl Responder> {
    let name = name.into_inner();
    let db = data.db.lock().unwrap();
    db.empty_mailbox(&name)
        .map_err(HttpError::from_database_error)?;
    Ok(format!("Emptied mailbox {name}"))
}

pub async fn start_server(data: ThreadState) -> Result<(), std::io::Error> {
    let app_data = data.clone();
    HttpServer::new(move || {
        let app_data = web::Data::new(app_data.clone());
        App::new()
            .app_data(app_data)
            .service(status_overview)
            .service(status)
            .service(get_log)
            .service(delete_log)
            .service(terminate)
            .service(mailbox_overview)
            .service(read_mailbox)
            .service(write_message)
            .service(empty_mailbox)
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}
