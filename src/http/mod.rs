mod filters;
mod http_error;
mod job_info;

use self::http_error::HttpError;
use self::job_info::JobInfo;
use crate::chron_service::{ChronService, ProcessStatus};
use crate::database::Database;
use actix_web::HttpResponse;
use actix_web::dev::Server;
use actix_web::web::{Data, Path};
use actix_web::{App, HttpServer, Responder, Result, get, http::StatusCode, post};
use askama::Template;
use chrono::{DateTime, Duration, Local, TimeZone};
use futures::future::join_all;
use log::info;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::File;
use tokio::sync::RwLock;
use tokio_util::io::ReaderStream;

struct AppState {
    chron: Arc<RwLock<ChronService>>,
    db: Arc<Database>,
}

type AppData = Data<AppState>;

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
async fn index_handler(data: AppData) -> Result<impl Responder> {
    let mut jobs = join_all(
        data.chron
            .read()
            .await
            .get_jobs_iter()
            .map(|(name, job)| JobInfo::from_job(name, job)),
    )
    .await
    .into_iter()
    .collect::<anyhow::Result<Vec<_>>>()
    .map_err(|_| HttpError::from_status_code(StatusCode::INTERNAL_SERVER_ERROR))?;
    jobs.sort_by(|job1, job2| job1.name.cmp(&job2.name));

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
async fn job_handler(name: Path<String>, data: AppData) -> Result<impl Responder> {
    let data_guard = data.chron.read().await;
    let job = data_guard
        .get_job(&name)
        .ok_or_else(|| HttpError::from_status_code(StatusCode::NOT_FOUND))?;
    let job = JobInfo::from_job(&name, job)
        .await
        .map_err(|_| HttpError::from_status_code(StatusCode::INTERNAL_SERVER_ERROR))?;
    let runs = data
        .db
        .get_last_runs(name.into_inner(), 20)
        .await
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
            // If the job is currently running, ended_at will not be set
            let ended_at = match status {
                RunStatus::Running => Some(Local::now()),
                _ => run
                    .ended_at
                    .map(|timestamp| Local.from_utc_datetime(&timestamp)),
            };
            RunInfo {
                run_id: run.id,
                timestamp: started_at,
                late: started_at.signed_duration_since(Local.from_utc_datetime(&run.scheduled_at)),
                execution_time: ended_at.map(|ended_at| ended_at.signed_duration_since(started_at)),
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
#[allow(clippy::significant_drop_tightening)] // produces false positives
async fn job_logs_handler(path: Path<(String, String)>, data: AppData) -> Result<impl Responder> {
    let (name, run_id) = path.into_inner();
    let log_path = {
        let run_id = if run_id == "latest" {
            // Look up the most recent run id
            data.db
                .get_last_runs(name.clone(), 1)
                .await
                .map_err(|_| HttpError::from_status_code(StatusCode::INTERNAL_SERVER_ERROR))?
                .first()
                .ok_or_else(|| HttpError::from_status_code(StatusCode::NOT_FOUND))?
                .id
        } else {
            run_id
                .parse::<u32>()
                .map_err(|_| HttpError::from_status_code(StatusCode::NOT_FOUND))?
        };
        let data_guard = data.chron.read().await;
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

#[post("/job/{name}/terminate")]
async fn job_terminate_handler(name: Path<String>, data: AppData) -> Result<impl Responder> {
    let data_guard = data.chron.read().await;
    let process = data_guard
        .get_job(&name)
        .ok_or_else(|| HttpError::from_status_code(StatusCode::NOT_FOUND))?
        .running_process
        .write()
        .await
        .take();
    drop(data_guard);

    let terminated = if let Some(process) = process {
        process.terminate().await
    } else {
        false
    };

    let response = if terminated {
        HttpResponse::Ok()
            .content_type("text/plain; charset=utf-8")
            .body("Terminated")
    } else {
        HttpResponse::NotFound()
            .content_type("text/plain; charset=utf-8")
            .body("Not Running")
    };
    Ok(response)
}

pub async fn create_server(
    chron: Arc<RwLock<ChronService>>,
    port: u16,
) -> Result<Server, std::io::Error> {
    info!("Starting HTTP server on port {}", port);

    let db = Arc::clone(&chron.read().await.get_db());
    let server = HttpServer::new(move || {
        App::new()
            .app_data(Data::new(AppState {
                chron: Arc::clone(&chron),
                db: Arc::clone(&db),
            }))
            .service(styles)
            .service(index_handler)
            .service(job_handler)
            .service(job_logs_handler)
            .service(job_terminate_handler)
    })
    .disable_signals()
    .bind(("localhost", port))?
    .run();
    Ok(server)
}
