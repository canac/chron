mod http_error;

use self::http_error::HttpError;
use crate::chron_service::JobType;
use actix_web::web::{Data, Json, Path};
use actix_web::{delete, get, http::StatusCode, post, App, HttpServer, Responder, Result};
use log::info;
use serde_json::json;
use std::collections::BTreeMap;
use std::fs::{read_to_string, write};

type ThreadData = crate::chron_service::ChronServiceLock;

#[get("/status")]
async fn status_overview(data: Data<ThreadData>) -> Result<impl Responder> {
    let data_guard = data.read().unwrap();
    let response = data_guard
        .get_jobs_iter()
        .map(|(name, job)| {
            let job_type = match job.job_type {
                JobType::Startup { .. } => "startup",
                JobType::Scheduled { .. } => "scheduled",
            };
            let mut process_guard = job.process.write().unwrap();
            // The job is running if the process is set and try_wait returns None
            let running = match process_guard.as_mut() {
                Some(process) => process.try_wait()?.is_none(),
                None => false,
            };
            Ok((
                name.clone(),
                json!({
                    "type": job_type,
                    "running": running
                }),
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;

    Ok(Json(response))
}

#[get("/status/{name}")]
async fn status(name: Path<String>, data: Data<ThreadData>) -> Result<impl Responder> {
    let name = name.into_inner();

    // Make sure that this job exists before proceeding
    let data_guard = data.read().unwrap();
    let job = data_guard
        .get_job(&name)
        .ok_or_else(|| HttpError::from_status_code(StatusCode::NOT_FOUND))?;

    // Load the last few runs from the database
    let db = data_guard.get_db();
    let runs = db
        .lock()
        .unwrap()
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

    Ok(Json(json!({
        "name": name,
        "runs": runs,
        "next_run": match &job.job_type {
            JobType::Startup {..} => None,
            JobType::Scheduled { scheduled_job, .. } => scheduled_job.read().unwrap().next_run(),
        },
        "pid": job.process.read().unwrap().as_ref().map(std::process::Child::id),
    })))
}

#[get("/log/{name}")]
async fn get_log(name: Path<String>, data: Data<ThreadData>) -> Result<impl Responder> {
    let data_guard = data.read().unwrap();
    let job = data_guard
        .get_job(&name)
        .ok_or_else(|| HttpError::from_status_code(StatusCode::NOT_FOUND))?;
    let log_contents = read_to_string(job.log_path.clone())?;
    Ok(log_contents)
}

#[delete("/log/{name}")]
async fn delete_log(name: Path<String>, data: Data<ThreadData>) -> Result<impl Responder> {
    let name = name.into_inner();
    let data_guard = data.read().unwrap();
    let job = data_guard
        .get_job(&name)
        .ok_or_else(|| HttpError::from_status_code(StatusCode::NOT_FOUND))?;
    let log_path = job.log_path.clone();
    drop(data_guard);
    write(log_path, "")?;
    Ok(format!("Erased log file for {name}"))
}

#[post("/terminate/{name}")]
async fn terminate(name: Path<String>, data: Data<ThreadData>) -> Result<impl Responder> {
    let name = name.into_inner();
    let data_guard = data.read().unwrap();
    let job = data_guard
        .get_job(&name)
        .ok_or_else(|| HttpError::from_status_code(StatusCode::NOT_FOUND))?;
    let message = if let Some(process) = job.process.write().unwrap().as_mut() {
        process.kill()?;
        format!("Terminated job {name}")
    } else {
        format!("Job {name} isn't currently running")
    };
    Ok(message)
}

pub async fn start_server(data: ThreadData, port: u16) -> Result<(), std::io::Error> {
    info!("Starting HTTP server on port {}", port);
    HttpServer::new(move || {
        let app_data = Data::new(data.clone());
        App::new()
            .app_data(app_data)
            .service(status_overview)
            .service(status)
            .service(get_log)
            .service(delete_log)
            .service(terminate)
    })
    .bind(("127.0.0.1", port))?
    .run()
    .await
}
