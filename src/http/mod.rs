mod http_error;
mod job_info;

use self::http_error::HttpError;
use self::job_info::JobInfo;
use actix_web::web::{Data, Path};
use actix_web::HttpResponse;
use actix_web::{get, http::StatusCode, App, HttpServer, Responder, Result};
use askama::Template;
use chrono::{DateTime, Local, TimeZone};
use log::info;
use std::sync::Arc;

type ThreadData = crate::chron_service::ChronServiceLock;

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    jobs: Vec<JobInfo>,
}

#[get("/static/styles.css")]
async fn styles() -> Result<impl Responder> {
    Ok(HttpResponse::build(StatusCode::OK)
        .content_type("text/css; charset=utf-8")
        .body(include_str!("./static/styles.css")))
}

#[get("/")]
async fn index_handler(data: Data<ThreadData>) -> Result<impl Responder> {
    let data_guard = data
        .read()
        .map_err(|_| HttpError::from_status_code(StatusCode::INTERNAL_SERVER_ERROR))?;

    let mut jobs = data_guard
        .get_jobs_iter()
        .map(|(name, job)| JobInfo::from_job(name, job))
        .collect::<Result<Vec<_>>>()?;
    jobs.sort_by(|job1, job2| job1.name.cmp(&job2.name));

    let template = IndexTemplate { jobs };
    Ok(HttpResponse::build(StatusCode::OK)
        .content_type("text/html; charset=utf-8")
        .body(
            template
                .render()
                .map_err(|_| HttpError::from_status_code(StatusCode::INTERNAL_SERVER_ERROR))?,
        ))
}

struct RunInfo {
    timestamp: DateTime<Local>,
    status_code: Option<i32>,
    success: bool,
}

#[derive(Template)]
#[template(path = "job.html")]
struct JobTemplate {
    job: JobInfo,
    runs: Vec<RunInfo>,
}

#[get("/job/{job}")]
async fn job_handler(name: Path<String>, data: Data<ThreadData>) -> Result<impl Responder> {
    let data_guard = data
        .read()
        .map_err(|_| HttpError::from_status_code(StatusCode::INTERNAL_SERVER_ERROR))?;

    let Some(job) = data_guard.get_job(&name) else {
        return Err(HttpError::from_status_code(StatusCode::NOT_FOUND).into());
    };
    let job = JobInfo::from_job(&name, job)?;
    let runs = data_guard
        .get_db()
        .lock()
        .map_err(|_| HttpError::from_status_code(StatusCode::INTERNAL_SERVER_ERROR))?
        .get_last_runs(&name, 20)
        .map_err(|_| HttpError::from_status_code(StatusCode::INTERNAL_SERVER_ERROR))?
        .into_iter()
        .map(|run| RunInfo {
            timestamp: Local.from_utc_datetime(&run.timestamp),
            status_code: run.status_code,
            success: run.status_code == Some(0),
        })
        .collect();
    let template = JobTemplate { job, runs };
    Ok(HttpResponse::build(StatusCode::OK)
        .content_type("text/html; charset=utf-8")
        .body(
            template
                .render()
                .map_err(|_| HttpError::from_status_code(StatusCode::INTERNAL_SERVER_ERROR))?,
        ))
}

#[get("/job/{job}/logs")]
async fn job_logs_handler(name: Path<String>, data: Data<ThreadData>) -> Result<impl Responder> {
    let data_guard = data.read().unwrap();
    let job = data_guard
        .get_job(&name)
        .ok_or_else(|| HttpError::from_status_code(StatusCode::NOT_FOUND))?;
    let log_contents = std::fs::read_to_string(job.log_path.clone())?;
    Ok(log_contents)
}

pub async fn start_server(data: ThreadData, port: u16) -> Result<(), std::io::Error> {
    info!("Starting HTTP server on port {}", port);
    HttpServer::new(move || {
        let app_data = Data::new(Arc::clone(&data));
        App::new()
            .app_data(app_data)
            .service(styles)
            .service(index_handler)
            .service(job_handler)
            .service(job_logs_handler)
    })
    .bind(("localhost", port))?
    .run()
    .await
}
