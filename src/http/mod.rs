mod filters;
mod http_error;
mod job_info;

use self::http_error::HttpError;
use self::job_info::JobInfo;
use crate::chron_service::{ChronService, ProcessStatus};
use crate::database::{Database, Run, RunStatus};
use actix_web::dev::Server;
use actix_web::middleware::DefaultHeaders;
use actix_web::web::{Data, Path};
use actix_web::{
    App, HttpResponse, HttpServer, Responder, Result, get, head, http::StatusCode, post,
};
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

struct RunInfo {
    run: Run,
    timestamp: DateTime<Local>,
    late: Duration,
    status: RunStatus,
    log_file: PathBuf,
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
            let started_at = Local.from_utc_datetime(&run.started_at);
            Ok(RunInfo {
                timestamp: started_at,
                late: started_at.signed_duration_since(Local.from_utc_datetime(&run.scheduled_at)),
                status: run.status()?,
                log_file: job.log_dir.join(format!("{}.log", run.id)),
                run,
            })
        })
        .collect::<anyhow::Result<_>>()
        .map_err(|_| HttpError::from_status_code(StatusCode::INTERNAL_SERVER_ERROR))?;
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

#[head("/job/{job}")]
async fn job_exists_handler(name: Path<String>, data: AppData) -> Result<impl Responder> {
    let chron_guard = data.chron.read().await;
    if chron_guard.get_job(name.as_str()).is_none() {
        return Err(HttpError::from_status_code(StatusCode::NOT_FOUND).into());
    }
    drop(chron_guard);

    Ok(HttpResponse::Ok())
}

#[get("/job/{job}/log_path")]
async fn job_log_path_handler(name: Path<String>, data: AppData) -> Result<impl Responder> {
    let run_id = data
        .db
        .get_last_runs(name.to_owned(), 1)
        .await
        .map_err(|_| HttpError::from_status_code(StatusCode::INTERNAL_SERVER_ERROR))?
        .first()
        .ok_or_else(|| HttpError::from_status_code(StatusCode::NOT_FOUND))?
        .id;

    let log_path = data
        .chron
        .read()
        .await
        .get_job(name.as_str())
        .ok_or_else(|| HttpError::from_status_code(StatusCode::NOT_FOUND))?
        .log_dir
        .join(format!("{run_id}.log"));

    Ok(HttpResponse::Ok()
        .content_type("text/plain; charset=utf-8")
        .body(log_path.to_string_lossy().into_owned()))
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

    if let Some(process) = process {
        let pid = process.pid;
        if process.terminate().await {
            return Ok(HttpResponse::Ok()
                .content_type("text/plain; charset=utf-8")
                .body(pid.to_string()));
        }
    }

    Ok(HttpResponse::NotFound()
        .content_type("text/plain; charset=utf-8")
        .body("Not Running"))
}

/// Create a new chron HTTP server on the provided port. If the port is unavailable, try ascending ports until one can
/// successfully connect.
/// Returns the server and the port it is bound to.
pub fn create_server(
    chron: &Arc<RwLock<ChronService>>,
    db: &Arc<Database>,
    mut port: u16,
) -> Result<(Server, u16), std::io::Error> {
    loop {
        let chron = Arc::clone(chron);
        let db = Arc::clone(db);
        let result = HttpServer::new(move || {
            App::new()
                .app_data(Data::new(AppState {
                    chron: Arc::clone(&chron),
                    db: Arc::clone(&db),
                }))
                .wrap(DefaultHeaders::new().add(("X-Powered-By", "chron")))
                .service(
                    actix_web::web::scope("/api")
                        .service(job_exists_handler)
                        .service(job_log_path_handler)
                        .service(job_terminate_handler),
                )
                .service(styles)
                .service(index_handler)
                .service(job_handler)
                .service(job_logs_handler)
        })
        .disable_signals()
        .bind(("localhost", port));

        match result {
            Ok(server) => {
                info!("Starting HTTP server on port {}", port);
                return Ok((server.run(), port));
            }
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse && port != u16::MAX => {
                let next_port = port.saturating_add(1);
                info!("Port {port} is unavailable, trying port {next_port}...");
                port = next_port;
            }
            Err(err) => return Err(err),
        }
    }
}
