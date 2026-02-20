mod filters;
mod http_error;

use self::http_error::HttpError;
use crate::chron_service::ChronService;
use crate::database::{ClientDatabase, Job, JobStatus, Run, RunStatus};
use actix_web::dev::Server;
use actix_web::middleware::DefaultHeaders;
use actix_web::web::{Data, Path};
use actix_web::{App, HttpResponse, HttpServer, Responder, Result, get, http::StatusCode};
use askama::Template;
use chrono::{DateTime, Duration, Local, TimeZone};
use log::info;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::File;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio_util::io::ReaderStream;

struct AppState {
    chron: Arc<RwLock<ChronService>>,
    db: Arc<ClientDatabase>,
}

type AppData = Data<AppState>;

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    jobs: Vec<Job>,
}

#[get("/static/styles.css")]
async fn styles() -> Result<impl Responder> {
    Ok(HttpResponse::Ok()
        .content_type("text/css; charset=utf-8")
        .body(include_str!("./static/styles.css")))
}

#[get("/")]
async fn index_handler(data: AppData) -> Result<impl Responder> {
    let jobs = data
        .db
        .get_active_jobs()
        .await
        .map_err(|_| HttpError::from_status_code(StatusCode::INTERNAL_SERVER_ERROR))?;

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
    job: Job,
    runs: Vec<RunInfo>,
}

#[get("/job/{name}")]
async fn job_handler(name: Path<String>, data: AppData) -> Result<impl Responder> {
    let data_guard = data.chron.read().await;

    let name = name.into_inner();
    let job = data
        .db
        .get_active_job(name.clone())
        .await
        .map_err(|_| HttpError::from_status_code(StatusCode::INTERNAL_SERVER_ERROR))?
        .ok_or_else(|| HttpError::from_status_code(StatusCode::NOT_FOUND))?;

    let runs = data
        .db
        .get_last_runs(name, 20)
        .await
        .map_err(|_| HttpError::from_status_code(StatusCode::INTERNAL_SERVER_ERROR))?
        .into_iter()
        .map(|run| {
            let started_at = Local.from_utc_datetime(&run.started_at);
            Ok(RunInfo {
                timestamp: started_at,
                late: started_at.signed_duration_since(Local.from_utc_datetime(&run.scheduled_at)),
                status: run.status()?,
                log_file: job.config.log_dir.join(format!("{}.log", run.id)),
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

/// Create a new chron HTTP server on the port
pub async fn create_server(
    chron: &Arc<RwLock<ChronService>>,
    db: &Arc<ClientDatabase>,
    port: u16,
) -> Result<Server, std::io::Error> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let data = Data::new(AppState {
        chron: Arc::clone(chron),
        db: Arc::clone(db),
    });
    let server = HttpServer::new(move || {
        App::new()
            .app_data(data.clone())
            .wrap(DefaultHeaders::new().add(("X-Powered-By", "chron")))
            .service(styles)
            .service(index_handler)
            .service(job_handler)
            .service(job_logs_handler)
    })
    .disable_signals()
    .listen(listener.into_std()?)?;

    info!("Listening on http://localhost:{port}");
    Ok(server.run())
}
