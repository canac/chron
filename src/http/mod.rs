mod http_error;
mod job_info;

use self::http_error::HttpError;
use self::job_info::JobInfo;
use crate::chron_service::ProcessStatus;
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

enum RunStatus {
    Running,
    Completed { success: bool, status_code: i32 },
    Terminated,
}

impl RunStatus {
    // Create a completed run status from a status code
    fn from_status_code(status_code: i32) -> Self {
        RunStatus::Completed {
            success: status_code == 0,
            status_code,
        }
    }
}

struct RunInfo {
    timestamp: DateTime<Local>,
    status: RunStatus,
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
        .map(|run| {
            // If the job is currently running, the run in the database will have a status of None,
            // which is indistinguishable from a run that terminated without a status code. To be
            // able to show that the current run is running in the runs table, we need to use the
            // status from the job's current process, if any, instead of from the database.
            let status = if job.run_id == Some(run.id) {
                match job.status {
                    ProcessStatus::Running { .. } => RunStatus::Running,
                    ProcessStatus::Completed { status_code } => {
                        RunStatus::from_status_code(status_code)
                    }
                    ProcessStatus::Terminated => RunStatus::Terminated,
                }
            } else if let Some(status_code) = run.status_code {
                RunStatus::from_status_code(status_code)
            } else {
                RunStatus::Terminated
            };
            RunInfo {
                timestamp: Local.from_utc_datetime(&run.timestamp),
                status,
            }
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
