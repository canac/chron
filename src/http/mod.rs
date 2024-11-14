mod filters;
mod http_error;
mod job_info;

use self::http_error::HttpError;
use self::job_info::JobInfo;
use crate::chron_service::ProcessStatus;
use actix_web::web::{Data, Path};
use actix_web::HttpResponse;
use actix_web::{get, http::StatusCode, App, HttpServer, Responder, Result};
use askama::Template;
use chrono::{DateTime, Duration, Local, TimeZone};
use log::info;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::File;
use tokio_util::io::ReaderStream;

type ThreadData = crate::chron_service::ChronServiceLock;

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    jobs: Vec<JobInfo>,
}

#[get("/static/styles.css")]
async fn styles() -> Result<impl Responder> {
    Ok(HttpResponse::Ok()
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
    drop(data_guard);

    let template = IndexTemplate { jobs };
    Ok(HttpResponse::Ok()
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

struct RunInfo {
    run_id: u32,
    timestamp: DateTime<Local>,
    late: Duration,
    execution_time: Option<Duration>,
    status: RunStatus,
    log_file: PathBuf,
    attempt: String,
}

#[derive(Template)]
#[template(path = "job.html")]
struct JobTemplate {
    job: JobInfo,
    runs: Vec<RunInfo>,
}

#[get("/job/{name}")]
async fn job_handler(name: Path<String>, data: Data<ThreadData>) -> Result<impl Responder> {
    let data_guard = data
        .read()
        .map_err(|_| HttpError::from_status_code(StatusCode::INTERNAL_SERVER_ERROR))?;

    let job = data_guard
        .get_job(&name)
        .ok_or_else(|| HttpError::from_status_code(StatusCode::NOT_FOUND))?;
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
            // able to show that the current run is running in the runs table, we need to look for a
            // run matching the jobs run_id. If one is found, it is considered to be running.
            let status = if job.run_id == Some(run.id) {
                RunStatus::Running
            } else if let Some(status_code) = run.status_code {
                RunStatus::Completed {
                    success: status_code == 0,
                    status_code,
                }
            } else {
                RunStatus::Terminated
            };
            let started_at = Local.from_utc_datetime(&run.started_at);
            RunInfo {
                run_id: run.id,
                timestamp: started_at,
                late: started_at.signed_duration_since(Local.from_utc_datetime(&run.scheduled_at)),
                execution_time: run.ended_at.map(|timestamp| {
                    Local
                        .from_utc_datetime(&timestamp)
                        .signed_duration_since(started_at)
                }),
                status,
                log_file: job.log_dir.join(format!("{}.log", run.id)),
                attempt: format!(
                    "{}{}",
                    run.attempt + 1,
                    run.max_attempts
                        .map_or_else(String::new, |max_attempts| format!(
                            " of {}",
                            max_attempts + 1
                        ))
                ),
            }
        })
        .collect();
    drop(data_guard);

    let template = JobTemplate { job, runs };
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(
            template
                .render()
                .map_err(|_| HttpError::from_status_code(StatusCode::INTERNAL_SERVER_ERROR))?,
        ))
}

#[get("/job/{name}/logs/{run_id}")]
async fn job_logs_handler(
    path: Path<(String, u32)>,
    data: Data<ThreadData>,
) -> Result<impl Responder> {
    let (name, run_id) = path.into_inner();
    let log_path = {
        let data_guard = data.read().unwrap();
        let job = data_guard
            .get_job(&name)
            .ok_or_else(|| HttpError::from_status_code(StatusCode::NOT_FOUND))?;
        job.log_dir.join(format!("{run_id}.log"))
    };

    let file = File::open(log_path)
        .await
        .map_err(|_| HttpError::from_status_code(StatusCode::NOT_FOUND))?;
    Ok(HttpResponse::Ok()
        .content_type("text/plain; charset=utf-8")
        .streaming(ReaderStream::new(file)))
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
