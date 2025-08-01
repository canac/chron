mod filters;
mod http_error;

use self::http_error::HttpError;
use crate::chron_service::ChronService;
use crate::database::{ClientDatabase, Job, JobStatus, Run, RunStatus};
use actix_web::dev::Server;
use actix_web::middleware::DefaultHeaders;
use actix_web::web::{Data, Path};
use actix_web::{App, HttpResponse, HttpServer, Responder, Result, get, http::StatusCode, post};
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
    host_id: u32,
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

#[get("/host/id")]
async fn host_id_handler(data: AppData) -> Result<impl Responder> {
    Ok(HttpResponse::Ok()
        .content_type("application/octet-stream")
        .body(data.host_id.to_be_bytes().to_vec()))
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

/// Select a port for the HTTP server to listen on
/// The provided port is tried first, but if it is unavailable, other ports are tried until one can successfully connect.
pub async fn select_port(mut port: u16) -> Result<(TcpListener, u16), std::io::Error> {
    loop {
        match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => return Ok((listener, port)),
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse && port != u16::MAX => {
                let next_port = port.saturating_add(1);
                info!("Port {port} is unavailable, trying port {next_port}...");
                port = next_port;
            }
            Err(err) => return Err(err),
        }
    }
}

/// Create a new chron HTTP server on the provided TCP listener
pub fn create_server(
    chron: &Arc<RwLock<ChronService>>,
    db: &Arc<ClientDatabase>,
    host_id: u32,
    listener: TcpListener,
) -> Result<Server, std::io::Error> {
    let port = listener.local_addr()?.port();

    let chron = Arc::clone(chron);
    let db = Arc::clone(db);
    let server = HttpServer::new(move || {
        App::new()
            .app_data(Data::new(AppState {
                chron: Arc::clone(&chron),
                db: Arc::clone(&db),
                host_id,
            }))
            .wrap(DefaultHeaders::new().add(("X-Powered-By", "chron")))
            .service(actix_web::web::scope("/api").service(job_terminate_handler))
            .service(styles)
            .service(host_id_handler)
            .service(index_handler)
            .service(job_handler)
            .service(job_logs_handler)
    })
    .disable_signals()
    .listen(listener.into_std()?)?;

    info!("Starting HTTP server on port {}", port);
    Ok(server.run())
}
